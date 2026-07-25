//! Tauri shell.
//!
//! Thin wrappers over `essentio_core`: command surface, the streaming bridge,
//! and OS keychain access. Product logic belongs in `core`, not here.

mod approval;
mod secrets;

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use essentio_core::{
    engine::{
        AgentEngine, EngineEvent, EngineKind, RunOptions, SessionId, UserInput, build_engine,
    },
    memory::{Agent, Message, Session, Store},
    providers::{ChatTransport, ModelInfo, ProviderConfig, ProviderKind, build_transport},
    rag::{Document, Retriever, embed::OpenAiCompatEmbedder, tool::SearchDocuments},
    tools::{
        Approval, ToolRegistry, ToolSpec,
        builtin::registry_with_notes,
        mcp::{McpManager, McpServerConfig},
    },
};

use approval::ApprovalRouter;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, ipc::Channel};

const PROVIDERS_FILE: &str = "providers.json";
const MCP_FILE: &str = "mcp.json";

/// Cancellation registry: run id -> abort handle for the pump task.
#[derive(Default)]
struct RunRegistry(Mutex<HashMap<String, tokio::task::AbortHandle>>);

impl RunRegistry {
    fn insert(&self, run_id: String, handle: tokio::task::AbortHandle) {
        if let Ok(mut runs) = self.0.lock() {
            runs.insert(run_id, handle);
        }
    }

    fn remove(&self, run_id: &str) {
        if let Ok(mut runs) = self.0.lock() {
            runs.remove(run_id);
        }
    }

    fn abort(&self, run_id: &str) -> bool {
        let Ok(mut runs) = self.0.lock() else {
            return false;
        };
        match runs.remove(run_id) {
            Some(handle) => {
                handle.abort();
                true
            }
            None => false,
        }
    }
}

struct AppState {
    runs: Arc<RunRegistry>,
    store: Store,
    /// Engines are cached per (kind, provider). Transcripts live in SQLite, so
    /// this is about avoiding rebuild cost — and for ADK, about keeping its
    /// hydrated session cache warm.
    engines: Mutex<HashMap<String, Arc<dyn AgentEngine>>>,
    /// Built-ins plus every connected MCP server's tools.
    ///
    /// Behind a lock because connecting or disconnecting a server rebuilds it
    /// while the app is running.
    tools: Mutex<ToolRegistry>,
    /// Tools available with no MCP server attached; the base for a rebuild.
    builtin_tools: ToolRegistry,
    /// Held so the child processes stay alive for the life of the app.
    mcp: Mutex<Arc<McpManager>>,
    /// Servers that failed to start, surfaced to the UI rather than only logged.
    mcp_failures: Mutex<Vec<String>>,
    /// `None` until an embedding model is chosen; document search is
    /// unavailable rather than silently broken until then.
    retriever: Mutex<Option<Arc<Retriever>>>,
    /// Shared by every engine; prompts are routed by run id, which is why
    /// caching engines across conversations stays safe.
    approvals: Arc<ApprovalRouter>,
}

impl AppState {
    fn new(store: Store, notes_dir: PathBuf) -> Self {
        let builtin_tools = registry_with_notes(notes_dir);
        Self {
            runs: Arc::new(RunRegistry::default()),
            store,
            engines: Mutex::new(HashMap::new()),
            tools: Mutex::new(builtin_tools.clone()),
            builtin_tools,
            mcp: Mutex::new(Arc::new(McpManager::new())),
            mcp_failures: Mutex::new(Vec::new()),
            retriever: Mutex::new(None),
            approvals: Arc::new(ApprovalRouter::new()),
        }
    }

    fn tool_registry(&self) -> Result<ToolRegistry, String> {
        Ok(self
            .tools
            .lock()
            .map_err(|_| "tool registry poisoned".to_string())?
            .clone())
    }

    /// Rebuild the tool registry from built-ins plus whatever is connected.
    ///
    /// Single place so MCP and RAG cannot each clobber the other's tools.
    /// Engines are dropped afterwards because each holds a snapshot of the
    /// registry taken when it was built.
    fn rebuild_tools(&self) -> Result<(), String> {
        let mut registry = self.builtin_tools.clone();

        if let Ok(manager) = self.mcp.lock() {
            manager.register_into(&mut registry);
        }

        if let Ok(retriever) = self.retriever.lock()
            && let Some(retriever) = retriever.as_ref()
        {
            registry.register(Arc::new(SearchDocuments::new(retriever.clone())));
        }

        *self
            .tools
            .lock()
            .map_err(|_| "tool registry poisoned".to_string())? = registry;

        if let Ok(mut engines) = self.engines.lock() {
            engines.clear();
        }

        Ok(())
    }

    /// Reconnect every configured MCP server.
    async fn reload_mcp(&self, configs: &[McpServerConfig]) -> Result<Vec<String>, String> {
        let (manager, failures) = McpManager::connect_all(configs).await;

        {
            let mut slot = self
                .mcp
                .lock()
                .map_err(|_| "mcp lock poisoned".to_string())?;
            // Replacing the manager drops the previous one, terminating the
            // child processes it owned.
            *slot = Arc::new(manager);
        }
        if let Ok(mut slot) = self.mcp_failures.lock() {
            slot.clone_from(&failures);
        }

        self.rebuild_tools()?;
        Ok(failures)
    }

    /// Point the retriever at an embedding model, or clear it.
    ///
    /// Embeddings from different models are not comparable, so changing the
    /// model leaves previously indexed documents unsearchable until they are
    /// added again. Retrieval filters on the model name, so those rows are
    /// inert rather than returning nonsense.
    fn set_embedder(
        &self,
        provider: Option<(&ProviderConfig, Option<String>, &str)>,
    ) -> Result<(), String> {
        let built = match provider {
            Some((config, api_key, model)) => {
                let embedder = OpenAiCompatEmbedder::new(config, api_key, model)
                    .map_err(|error| error.to_string())?;
                Some(Arc::new(Retriever::new(
                    self.store.clone(),
                    Arc::new(embedder),
                )))
            }
            None => None,
        };

        *self
            .retriever
            .lock()
            .map_err(|_| "retriever lock poisoned".to_string())? = built;

        self.rebuild_tools()
    }

    fn engine(
        &self,
        kind: EngineKind,
        provider: &ProviderConfig,
        api_key: Option<String>,
    ) -> Result<Arc<dyn AgentEngine>, String> {
        let cache_key = format!("{kind:?}:{}", provider.id);
        let mut engines = self
            .engines
            .lock()
            .map_err(|_| "engine cache poisoned".to_string())?;

        if let Some(existing) = engines.get(&cache_key) {
            return Ok(existing.clone());
        }

        // Take the registry snapshot outside the entry closure so a poisoned
        // tool lock surfaces as an error rather than a panic.
        let tools = self
            .tools
            .lock()
            .map_err(|_| "tool registry poisoned".to_string())?
            .clone();

        let engine = build_engine(
            kind,
            provider.clone(),
            api_key,
            self.store.clone(),
            tools,
            self.approvals.clone(),
        );
        engines.insert(cache_key, engine.clone());
        Ok(engine)
    }

    /// Drop cached engines for a provider so config edits take effect.
    fn invalidate_engines(&self, provider_id: &str) {
        if let Ok(mut engines) = self.engines.lock() {
            engines.retain(|key, _| !key.ends_with(&format!(":{provider_id}")));
        }
    }
}

/// Provider config plus a write-only secret field.
///
/// `api_key` is accepted from the frontend on save and immediately moved into
/// the keychain. It is never returned by any command.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveProviderRequest {
    #[serde(flatten)]
    config: ProviderConfig,
    #[serde(default)]
    api_key: Option<String>,
}

/// What the frontend sees. Mirrors [`ProviderConfig`] but reports only whether
/// a secret exists, never its value.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderView {
    #[serde(flatten)]
    config: ProviderConfig,
    has_api_key: bool,
    /// Whether this provider's transport forwards tool calls.
    ///
    /// Surfaced so the UI can disable the tools toggle rather than offering
    /// something the backend will discard.
    supports_tools: bool,
}

/// Whether a provider kind can carry tools, without building a transport.
///
/// `build_transport` needs a credential, and the provider list is rendered
/// before any key is resolved.
fn kind_supports_tools(kind: ProviderKind) -> bool {
    match kind {
        // Anthropic encodes tools as content blocks; not mapped yet.
        ProviderKind::Anthropic => false,
        ProviderKind::OpenAi
        | ProviderKind::DeepSeek
        | ProviderKind::OpenAiCompatible
        | ProviderKind::Gemini => true,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunStreamRequest {
    run_id: String,
    session_id: String,
    provider_id: String,
    model: String,
    /// The new user turn only. Prior turns live in the engine, keyed by
    /// `session_id` — the UI does not resend the transcript.
    prompt: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    engine: EngineKind,
    /// Tools this run may use. Empty means none are offered to the model.
    #[serde(default)]
    tool_ids: Vec<String>,
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("could not resolve app data dir: {error}"))?;
    fs::create_dir_all(&dir).map_err(|error| format!("could not create app data dir: {error}"))?;
    Ok(dir)
}

fn load_providers(app: &AppHandle) -> Result<Vec<ProviderConfig>, String> {
    let path = app_data_dir(app)?.join(PROVIDERS_FILE);
    if !path.exists() {
        // First run: LM Studio on its default port is a useful starting point
        // and requires no credentials.
        return Ok(vec![ProviderConfig::lm_studio()]);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_str(&raw).map_err(|error| format!("malformed provider config: {error}"))
}

fn write_providers(app: &AppHandle, providers: &[ProviderConfig]) -> Result<(), String> {
    let path = app_data_dir(app)?.join(PROVIDERS_FILE);
    let raw = serde_json::to_string_pretty(providers)
        .map_err(|error| format!("could not serialize providers: {error}"))?;
    fs::write(&path, raw).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn find_provider(app: &AppHandle, provider_id: &str) -> Result<ProviderConfig, String> {
    load_providers(app)?
        .into_iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("provider not found: {provider_id}"))
}

/// Resolve a provider's secret from the keychain.
fn api_key_for(provider: &ProviderConfig) -> Result<Option<String>, String> {
    match provider.api_key_ref.as_deref() {
        Some(key_ref) => secrets::get(key_ref),
        None => Ok(None),
    }
}

/// Build a transport for a provider, resolving its secret from the keychain.
fn transport_for(provider: &ProviderConfig) -> Result<Arc<dyn ChatTransport>, String> {
    build_transport(provider, api_key_for(provider)?).map_err(|error| error.to_string())
}

#[tauri::command]
fn list_providers(app: AppHandle) -> Result<Vec<ProviderView>, String> {
    let providers = load_providers(&app)?;
    Ok(providers
        .into_iter()
        .map(|config| {
            let has_api_key = config
                .api_key_ref
                .as_deref()
                .and_then(|key_ref| secrets::get(key_ref).ok().flatten())
                .is_some();
            ProviderView {
                supports_tools: kind_supports_tools(config.kind),
                config,
                has_api_key,
            }
        })
        .collect())
}

#[tauri::command]
fn save_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SaveProviderRequest,
) -> Result<ProviderView, String> {
    let SaveProviderRequest {
        mut config,
        api_key,
    } = request;

    let existing = load_providers(&app)?
        .into_iter()
        .find(|provider| provider.id == config.id);

    // Carry the stored key reference forward when the caller omits it.
    // Without this, editing a provider without retyping its key would drop
    // the ref and orphan the keychain entry.
    if config.api_key_ref.is_none()
        && let Some(previous) = existing.as_ref().and_then(|p| p.api_key_ref.clone())
    {
        config.api_key_ref = Some(previous);
    }

    // A non-empty key is written to the keychain and the ref recorded. An
    // absent key leaves any existing secret untouched, so the frontend can
    // save edits without round-tripping the secret.
    let mut has_api_key = config
        .api_key_ref
        .as_deref()
        .and_then(|key_ref| secrets::get(key_ref).ok().flatten())
        .is_some();

    if let Some(secret) = api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let key_ref = secrets::ref_for_provider(&config.id);
        secrets::set(&key_ref, secret)?;
        config.api_key_ref = Some(key_ref);
        has_api_key = true;
    }

    config.validate().map_err(|error| error.to_string())?;

    let mut providers = load_providers(&app)?;
    match providers
        .iter()
        .position(|provider| provider.id == config.id)
    {
        Some(index) => providers[index] = config.clone(),
        None => providers.push(config.clone()),
    }
    write_providers(&app, &providers)?;
    state.invalidate_engines(&config.id);

    Ok(ProviderView {
        supports_tools: kind_supports_tools(config.kind),
        config,
        has_api_key,
    })
}

#[tauri::command]
fn delete_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), String> {
    let providers = load_providers(&app)?;
    if let Some(provider) = providers.iter().find(|item| item.id == provider_id)
        && let Some(key_ref) = provider.api_key_ref.as_deref()
    {
        secrets::delete(key_ref)?;
    }

    let remaining: Vec<ProviderConfig> = providers
        .into_iter()
        .filter(|provider| provider.id != provider_id)
        .collect();
    write_providers(&app, &remaining)?;
    state.invalidate_engines(&provider_id);
    Ok(())
}

/// List a provider's models — doubles as the "Test connection" action.
#[tauri::command]
async fn list_models(app: AppHandle, provider_id: String) -> Result<Vec<ModelInfo>, String> {
    let provider = find_provider(&app, &provider_id)?;
    transport_for(&provider)?
        .list_models()
        .await
        .map_err(|error| error.to_string())
}

/// Stream a run into `channel`. Returns once the run is registered; events
/// arrive asynchronously. Cancel with [`cancel_run`].
#[tauri::command]
async fn run_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunStreamRequest,
    channel: Channel<EngineEvent>,
) -> Result<(), String> {
    let RunStreamRequest {
        run_id,
        session_id,
        provider_id,
        model,
        prompt,
        system_prompt,
        temperature,
        engine: engine_kind,
        tool_ids,
    } = request;

    let provider = find_provider(&app, &provider_id)?;
    let engine = state.engine(engine_kind, &provider, api_key_for(&provider)?)?;

    let mut opts = RunOptions::new(&provider_id, &model).with_run_id(&run_id);
    opts.temperature = temperature;
    opts.system_prompt = system_prompt;
    opts.tool_ids = tool_ids;

    let stream = engine
        .run_stream(session_id.into(), UserInput::text(prompt), opts)
        .await
        .map_err(|error| error.to_string())?;

    let runs = Arc::clone(&state.runs);
    let pump_id = run_id.clone();
    let task = tokio::spawn(async move {
        let mut stream = stream;
        while let Some(event) = stream.next().await {
            // A send failure means the webview dropped the channel; stop
            // pumping rather than draining the rest of the stream.
            if channel.send(event).is_err() {
                break;
            }
        }
        runs.remove(&pump_id);
    });

    state.runs.insert(run_id, task.abort_handle());
    Ok(())
}

/// Abort an in-flight run. `false` means the run had already finished.
#[tauri::command]
fn cancel_run(state: State<'_, AppState>, run_id: String) -> bool {
    // Deny anything the run left waiting first: a pending prompt answered
    // after cancellation would otherwise still execute a tool.
    state.approvals.abandon_run(&run_id);
    state.runs.abort(&run_id)
}

/// Tools available to a run, built-ins plus any connected MCP server.
#[tauri::command]
fn list_tools(state: State<'_, AppState>) -> Result<Vec<ToolSpec>, String> {
    Ok(state.tool_registry()?.specs())
}

/// Settings keys for the embedding model.
const EMBED_PROVIDER_KEY: &str = "embedding.provider_id";
const EMBED_MODEL_KEY: &str = "embedding.model";

/// Which provider and model produce embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingConfig {
    provider_id: String,
    model: String,
}

fn load_embedding_config(state: &AppState) -> Option<EmbeddingConfig> {
    let provider_id = state.store.get_setting(EMBED_PROVIDER_KEY).ok().flatten()?;
    let model = state.store.get_setting(EMBED_MODEL_KEY).ok().flatten()?;
    Some(EmbeddingConfig { provider_id, model })
}

#[tauri::command]
fn get_embedding_config(state: State<'_, AppState>) -> Option<EmbeddingConfig> {
    load_embedding_config(&state)
}

/// Choose the embedding model, or pass `null` to turn document search off.
#[tauri::command]
fn set_embedding_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Option<EmbeddingConfig>,
) -> Result<(), String> {
    let Some(config) = config else {
        state.set_embedder(None)?;
        state
            .store
            .set_setting(EMBED_PROVIDER_KEY, "")
            .map_err(|e| e.to_string())?;
        return Ok(());
    };

    let provider = find_provider(&app, &config.provider_id)?;
    let api_key = api_key_for(&provider)?;
    state.set_embedder(Some((&provider, api_key, &config.model)))?;

    state
        .store
        .set_setting(EMBED_PROVIDER_KEY, &config.provider_id)
        .map_err(|e| e.to_string())?;
    state
        .store
        .set_setting(EMBED_MODEL_KEY, &config.model)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn list_agents(state: State<'_, AppState>) -> Result<Vec<Agent>, String> {
    state.store.list_agents().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_agent(state: State<'_, AppState>, agent: Agent) -> Result<Agent, String> {
    state.store.save_agent(&agent).map_err(|e| e.to_string())
}

/// Delete an agent. Conversations held with it survive, unattached.
#[tauri::command]
fn delete_agent(state: State<'_, AppState>, agent_id: String) -> Result<bool, String> {
    state
        .store
        .delete_agent(&agent_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_documents(state: State<'_, AppState>) -> Result<Vec<Document>, String> {
    state.store.list_documents().map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestRequest {
    title: String,
    /// Identifies the document; re-ingesting the same source replaces it.
    source: String,
    #[serde(default)]
    mime_type: Option<String>,
    text: String,
}

/// Chunk, embed and index a document.
///
/// The frontend reads the file and sends its text, so this handles pasted
/// content and dropped files through one path and never opens a file the user
/// did not choose.
#[tauri::command]
async fn ingest_document(
    state: State<'_, AppState>,
    request: IngestRequest,
) -> Result<usize, String> {
    let retriever = {
        let slot = state
            .retriever
            .lock()
            .map_err(|_| "retriever lock poisoned".to_string())?;
        slot.clone()
    };

    let retriever = retriever.ok_or_else(|| {
        "No embedding model is configured. Choose one in Documents before indexing.".to_string()
    })?;

    if request.text.trim().is_empty() {
        return Err("The document is empty.".to_string());
    }

    retriever
        .ingest(
            &request.title,
            &request.source,
            request.mime_type.as_deref().unwrap_or("text/plain"),
            &request.text,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_document(state: State<'_, AppState>, document_id: String) -> Result<bool, String> {
    state
        .store
        .delete_document(&document_id)
        .map_err(|e| e.to_string())
}

fn load_mcp_servers(app: &AppHandle) -> Result<Vec<McpServerConfig>, String> {
    let path = app_data_dir(app)?.join(MCP_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_str(&raw).map_err(|error| format!("malformed MCP config: {error}"))
}

fn write_mcp_servers(app: &AppHandle, servers: &[McpServerConfig]) -> Result<(), String> {
    let path = app_data_dir(app)?.join(MCP_FILE);
    let raw = serde_json::to_string_pretty(servers)
        .map_err(|error| format!("could not serialize MCP config: {error}"))?;
    fs::write(&path, raw).map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// An MCP server plus its live status.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpServerView {
    #[serde(flatten)]
    config: McpServerConfig,
    connected: bool,
    /// Tool names this server currently contributes.
    tools: Vec<String>,
    /// Why it failed to start, when it did.
    error: Option<String>,
}

#[tauri::command]
fn list_mcp_servers(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<McpServerView>, String> {
    let configs = load_mcp_servers(&app)?;
    let connected = state
        .mcp
        .lock()
        .map_err(|_| "mcp lock poisoned".to_string())?
        .server_ids();
    let failures = state
        .mcp_failures
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    let registry = state.tool_registry()?;

    Ok(configs
        .into_iter()
        .map(|config| {
            let is_connected = connected.iter().any(|id| id == &config.id);
            let tools = registry
                .specs()
                .into_iter()
                .filter(|spec| {
                    essentio_core::tools::mcp::split_namespaced(&spec.name)
                        .is_some_and(|(server, _)| server == config.id)
                })
                .map(|spec| spec.name)
                .collect();
            let error = failures
                .iter()
                .find(|failure| failure.starts_with(&format!("{}:", config.id)))
                .cloned();

            McpServerView {
                config,
                connected: is_connected,
                tools,
                error,
            }
        })
        .collect())
}

/// Add or update a server, then reconnect everything.
///
/// Returns the servers that failed to start, so the caller can show them
/// rather than discovering the absence of tools later.
#[tauri::command]
async fn save_mcp_server(
    app: AppHandle,
    state: State<'_, AppState>,
    server: McpServerConfig,
) -> Result<Vec<String>, String> {
    server.validate().map_err(|error| error.to_string())?;

    let mut servers = load_mcp_servers(&app)?;
    match servers.iter().position(|item| item.id == server.id) {
        Some(index) => servers[index] = server,
        None => servers.push(server),
    }
    write_mcp_servers(&app, &servers)?;

    state.reload_mcp(&servers).await
}

#[tauri::command]
async fn delete_mcp_server(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<String>, String> {
    let servers: Vec<McpServerConfig> = load_mcp_servers(&app)?
        .into_iter()
        .filter(|server| server.id != server_id)
        .collect();
    write_mcp_servers(&app, &servers)?;

    state.reload_mcp(&servers).await
}

/// Reconnect every configured server without changing the config.
#[tauri::command]
async fn reconnect_mcp(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let servers = load_mcp_servers(&app)?;
    state.reload_mcp(&servers).await
}

/// Answer a pending approval prompt.
///
/// `false` means nothing was waiting — usually the run was cancelled or the
/// prompt timed out.
#[tauri::command]
fn respond_to_approval(
    state: State<'_, AppState>,
    run_id: String,
    call_id: String,
    approved: bool,
) -> bool {
    let decision = if approved {
        Approval::Approve
    } else {
        Approval::Deny
    };
    state.approvals.resolve(&run_id, &call_id, decision)
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<Session>, String> {
    state.store.list_sessions().map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    #[serde(default)]
    title: Option<String>,
    provider_id: String,
    model: String,
    #[serde(default)]
    engine: EngineKind,
    /// Agent to hold this conversation with. Absent is plain chat.
    #[serde(default)]
    agent_id: Option<String>,
}

#[tauri::command]
fn create_session(
    state: State<'_, AppState>,
    request: CreateSessionRequest,
) -> Result<Session, String> {
    let title = request
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("New chat");

    state
        .store
        .create_session(
            title,
            &request.provider_id,
            &request.model,
            request.engine,
            request.agent_id.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state
        .store
        .delete_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    state
        .store
        .rename_session(&session_id, title.trim())
        .map_err(|e| e.to_string())
}

/// Drop messages at or after `from_seq` and invalidate engine-side caches.
///
/// Backs both regenerate (truncate the last assistant turn) and
/// edit-and-resend (truncate from the edited turn). Engines are told to forget
/// the session afterwards; skipping that would let ADK replay the removed
/// turns on the next run.
#[tauri::command]
async fn truncate_session(
    state: State<'_, AppState>,
    session_id: String,
    from_seq: i64,
) -> Result<usize, String> {
    let removed = state
        .store
        .truncate_from(&session_id, from_seq)
        .map_err(|e| e.to_string())?;

    // Every cached engine may hold state for this session, so clear them all
    // rather than guessing which one produced the transcript.
    let engines: Vec<Arc<dyn AgentEngine>> = {
        let cache = state
            .engines
            .lock()
            .map_err(|_| "engine cache poisoned".to_string())?;
        cache.values().cloned().collect()
    };

    let key = SessionId::from(session_id);
    for engine in engines {
        engine
            .forget_session(&key)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(removed)
}

#[tauri::command]
fn get_messages(state: State<'_, AppState>, session_id: String) -> Result<Vec<Message>, String> {
    state
        .store
        .load_messages(&session_id)
        .map_err(|e| e.to_string())
}

/// Deliberately not `essentio.sqlite3`.
///
/// The pre-workspace build shipped that filename with its own
/// `schema_migrations` ledger recording versions 1 and 2. Reusing the name
/// meant this schema's migration 1 was treated as already applied and skipped,
/// so `sessions` was never created and every query failed with "no such
/// table". The schemas share no lineage, so they get separate files.
const DB_FILE: &str = "workspace.sqlite3";

/// Send `tracing` output to stderr.
///
/// Without this every `warn!` in `core` is discarded — which is how a
/// "this provider cannot forward tools" warning went unseen while the UI
/// showed tools as enabled.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("essentio_core=info,essentio_app_lib=info,warn"));

    // A second init would fail; ignore it so tests and repeated calls are safe.
    let _ = fmt().with_env_filter(filter).with_target(true).try_init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The store needs the resolved app data dir, so it is built here
            // rather than in a Default impl.
            let data_dir = app_data_dir(app.handle())?;
            let store = Store::open(&data_dir.join(DB_FILE))?;
            // Notes are scoped to the app data dir; the tool refuses to write
            // outside whatever root it is given.
            app.manage(AppState::new(store, data_dir.join("notes")));

            // Connect MCP servers in the background: each one is a process
            // launch and a handshake, and the window should not wait on a
            // server that may be slow or broken.
            // Restore a previously chosen embedding model, so indexed
            // documents stay searchable across restarts.
            {
                let state = app.state::<AppState>();
                if let Some(config) = load_embedding_config(&state) {
                    match find_provider(app.handle(), &config.provider_id).and_then(|provider| {
                        let api_key = api_key_for(&provider)?;
                        state.set_embedder(Some((&provider, api_key, &config.model)))
                    }) {
                        Ok(()) => tracing::info!(model = %config.model, "embedding model restored"),
                        // A provider removed since last run should not stop
                        // the app; document search is simply unavailable.
                        Err(error) => {
                            tracing::warn!(%error, "could not restore the embedding model")
                        }
                    }
                }
            }

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let servers = match load_mcp_servers(&handle) {
                    Ok(servers) => servers,
                    Err(error) => {
                        tracing::error!(%error, "could not read the MCP config");
                        return;
                    }
                };
                if servers.is_empty() {
                    return;
                }

                let state = handle.state::<AppState>();
                match state.reload_mcp(&servers).await {
                    Ok(failures) if failures.is_empty() => {
                        tracing::info!(count = servers.len(), "MCP servers connected");
                    }
                    Ok(failures) => {
                        tracing::warn!(?failures, "some MCP servers failed to start");
                    }
                    Err(error) => tracing::error!(%error, "MCP startup failed"),
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_providers,
            save_provider,
            delete_provider,
            list_models,
            run_stream,
            cancel_run,
            list_sessions,
            create_session,
            delete_session,
            rename_session,
            get_messages,
            truncate_session,
            list_tools,
            respond_to_approval,
            list_mcp_servers,
            save_mcp_server,
            delete_mcp_server,
            reconnect_mcp,
            get_embedding_config,
            set_embedding_config,
            list_documents,
            ingest_document,
            delete_document,
            list_agents,
            save_agent,
            delete_agent
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
