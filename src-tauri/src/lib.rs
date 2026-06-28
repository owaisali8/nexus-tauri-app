mod core;

use core::{
    agent::{
        runtime::{AgentPromptRequest, AgentPromptResponse, RuntimePlan},
        AgentProfile,
    },
    browser::{BrowserRun, BrowserRunRequest},
    llm::{provider_templates, merge_providers, provider_is_ready, ProviderTemplate, StoredProvider},
    memory::{
        AgentSession, FileRecord, MemoryEntry, Note, SessionMessage, WorkspaceFolder,
    },
    sessions::{
        build_assistant_message, build_user_message, create_session, run_chat_turn,
        ChatMessageResponse, CreateSessionRequest, SendChatMessageRequest,
    },
    store::{self, append_chat_turn, AppState, DatabaseInfo},
};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    providers: Vec<core::llm::LlmProvider>,
    provider_templates: Vec<ProviderTemplate>,
    agents: Vec<AgentProfile>,
    folders: Vec<WorkspaceFolder>,
    notes: Vec<Note>,
    file_records: Vec<FileRecord>,
    sessions: Vec<AgentSession>,
    memory_entries: Vec<MemoryEntry>,
    browser_runs: Vec<BrowserRun>,
    rig_marker: String,
}

#[tauri::command]
fn get_app_snapshot(app: AppHandle) -> Result<AppSnapshot, String> {
    let state = load_state(&app)?;

    Ok(AppSnapshot {
        providers: merge_providers(&state.providers),
        provider_templates: provider_templates(),
        agents: state.agents,
        folders: state.folders,
        notes: state.notes,
        file_records: state.file_records,
        sessions: state.sessions,
        memory_entries: state.long_term_memory,
        browser_runs: state.browser_runs,
        rig_marker: core::agent::runtime::rig_marker().to_string(),
    })
}

#[tauri::command]
fn get_database_info(app: AppHandle) -> Result<DatabaseInfo, String> {
    store::database_info(&app_data_dir(&app)?)
}

#[tauri::command]
fn save_provider(app: AppHandle, mut provider: StoredProvider) -> Result<StoredProvider, String> {
    let mut state = load_state(&app)?;

    if let Some(existing) = state.providers.iter().find(|item| item.id == provider.id) {
        let incoming_key = provider.api_key.as_deref().unwrap_or("").trim();
        if incoming_key.is_empty() {
            provider.api_key = existing.api_key.clone();
        }
    }

    validate_provider(&provider)?;

    match state.providers.iter().position(|item| item.id == provider.id) {
        Some(index) => state.providers[index] = provider.clone(),
        None => state.providers.push(provider.clone()),
    }

    save_state(&app, &state)?;
    Ok(provider)
}

#[tauri::command]
fn delete_provider(app: AppHandle, provider_id: String) -> Result<(), String> {
    let mut state = load_state(&app)?;
    state.providers.retain(|provider| provider.id != provider_id);
    save_state(&app, &state)
}

#[tauri::command]
fn save_agent(app: AppHandle, agent: AgentProfile) -> Result<AgentProfile, String> {
    validate_agent(&agent, &load_state(&app)?)?;

    let mut state = load_state(&app)?;
    match state.agents.iter().position(|item| item.id == agent.id) {
        Some(index) => state.agents[index] = agent.clone(),
        None => state.agents.push(agent.clone()),
    }

    save_state(&app, &state)?;
    Ok(agent)
}

#[tauri::command]
fn delete_agent(app: AppHandle, agent_id: String) -> Result<(), String> {
    let mut state = load_state(&app)?;
    state.agents.retain(|agent| agent.id != agent_id);
    state.sessions.retain(|session| session.agent_id != agent_id);
    state.long_term_memory
        .retain(|entry| entry.agent_id != agent_id);
    save_state(&app, &state)
}

#[tauri::command]
fn save_note(app: AppHandle, note: Note) -> Result<Note, String> {
    if note.title.trim().is_empty() {
        return Err("note title is required".to_string());
    }

    let mut state = load_state(&app)?;
    match state.notes.iter().position(|item| item.id == note.id) {
        Some(index) => state.notes[index] = note.clone(),
        None => state.notes.push(note.clone()),
    }

    save_state(&app, &state)?;
    Ok(note)
}

#[tauri::command]
fn save_file_record(app: AppHandle, file: FileRecord) -> Result<FileRecord, String> {
    if file.name.trim().is_empty() || file.path.trim().is_empty() {
        return Err("file name and path are required".to_string());
    }

    let mut state = load_state(&app)?;
    match state.file_records.iter().position(|item| item.id == file.id) {
        Some(index) => state.file_records[index] = file.clone(),
        None => state.file_records.push(file.clone()),
    }

    save_state(&app, &state)?;
    Ok(file)
}

#[tauri::command]
fn save_memory_entry(app: AppHandle, entry: MemoryEntry) -> Result<MemoryEntry, String> {
    if entry.key.trim().is_empty() || entry.value.trim().is_empty() {
        return Err("memory key and value are required".to_string());
    }

    let mut state = load_state(&app)?;
    match state
        .long_term_memory
        .iter()
        .position(|item| item.id == entry.id)
    {
        Some(index) => state.long_term_memory[index] = entry.clone(),
        None => state.long_term_memory.push(entry.clone()),
    }

    save_state(&app, &state)?;
    Ok(entry)
}

#[tauri::command]
fn create_agent_session(
    app: AppHandle,
    request: CreateSessionRequest,
) -> Result<AgentSession, String> {
    let state = load_state(&app)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == request.agent_id)
        .ok_or_else(|| format!("agent not found: {}", request.agent_id))?;

    let session = create_session(agent, request.title);
    let mut next_state = state;
    next_state.sessions.insert(0, session.clone());
    save_state(&app, &next_state)?;
    Ok(session)
}

#[tauri::command]
fn get_session_messages(app: AppHandle, session_id: String) -> Result<Vec<SessionMessage>, String> {
    store::load_session_messages(&app_data_dir(&app)?, &session_id)
}

#[tauri::command]
async fn send_chat_message(
    app: AppHandle,
    request: SendChatMessageRequest,
) -> Result<ChatMessageResponse, String> {
    if request.content.trim().is_empty() {
        return Err("message content is required".to_string());
    }

    let app_data = app_data_dir(&app)?;
    let state = load_state(&app)?;
    let session = state
        .sessions
        .iter()
        .find(|session| session.id == request.session_id)
        .ok_or_else(|| format!("session not found: {}", request.session_id))?
        .clone();

    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == session.agent_id)
        .ok_or_else(|| format!("agent not found: {}", session.agent_id))?
        .clone();

    let runtime_plan = core::agent::runtime::build_runtime_plan(&agent, &state.providers);
    if !runtime_plan.ready {
        return Err(format!(
            "agent runtime is missing configuration: {}",
            runtime_plan.missing_configuration.join(", ")
        ));
    }

    let history = store::load_session_messages(&app_data, &session.id)?;
    let memory = store::load_memory_for_agent(&app_data, &agent.id)?;
    let provider = core::llm::resolve_provider(&agent.provider_id, &state.providers);

    let output = run_chat_turn(
        &agent,
        provider,
        &memory,
        &history,
        &request.content,
    )
    .await?;

    let user_message = build_user_message(&session.id, &request.content);
    let assistant_message = build_assistant_message(&session.id, &output);
    let now = now_unix();

    let mut updated_session = session.clone();
    updated_session.updated_at = now;
    updated_session.title = derive_session_title(&updated_session.title, &request.content);

    append_chat_turn(
        &app_data,
        &updated_session,
        &user_message,
        &assistant_message,
    )?;

    Ok(ChatMessageResponse {
        session_id: session.id,
        user_message,
        assistant_message,
        output,
    })
}

#[tauri::command]
fn prepare_browser_run(app: AppHandle, request: BrowserRunRequest) -> Result<BrowserRun, String> {
    let run = core::browser::prepare_run(request)?;
    let mut state = load_state(&app)?;
    state.browser_runs.insert(0, run.clone());
    state.browser_runs.truncate(25);
    save_state(&app, &state)?;
    Ok(run)
}

#[tauri::command]
fn get_runtime_plan(app: AppHandle, agent_id: String) -> Result<RuntimePlan, String> {
    let state = load_state(&app)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("agent not found: {agent_id}"))?;

    Ok(core::agent::runtime::build_runtime_plan(agent, &state.providers))
}

#[tauri::command]
async fn run_agent_prompt(
    app: AppHandle,
    request: AgentPromptRequest,
) -> Result<AgentPromptResponse, String> {
    let state = load_state(&app)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == request.agent_id)
        .ok_or_else(|| format!("agent not found: {}", request.agent_id))?
        .clone();

    core::agent::runtime::run_agent_prompt(&agent, &state.providers, request).await
}

fn validate_provider(provider: &StoredProvider) -> Result<(), String> {
    if provider.id.trim().is_empty() || provider.name.trim().is_empty() {
        return Err("provider id and name are required".to_string());
    }

    if provider.models.is_empty() {
        return Err("at least one model is required".to_string());
    }

    if !provider_is_ready(provider) && matches!(provider.kind, core::llm::ProviderKind::Cloud) {
        return Err("API key is required for cloud providers".to_string());
    }

    Ok(())
}

fn validate_agent(agent: &AgentProfile, state: &AppState) -> Result<(), String> {
    if agent.id.trim().is_empty() {
        return Err("agent id is required".to_string());
    }

    if agent.name.trim().is_empty() {
        return Err("agent name is required".to_string());
    }

    if agent.system_instructions.trim().is_empty() {
        return Err("system instructions are required".to_string());
    }

    if agent.provider_id.trim().is_empty() || agent.model.trim().is_empty() {
        return Err("provider and model are required".to_string());
    }

    let provider_ready = state
        .providers
        .iter()
        .any(|provider| provider.id == agent.provider_id && provider.is_enabled);

    if !provider_ready {
        return Err("select a configured provider before saving the agent".to_string());
    }

    Ok(())
}

fn derive_session_title(current_title: &str, first_message: &str) -> String {
    if !current_title.starts_with("Chat with") {
        return current_title.to_string();
    }

    let trimmed = first_message.trim();
    if trimmed.is_empty() {
        return current_title.to_string();
    }

    let summary: String = trimmed.chars().take(48).collect();
    if trimmed.chars().count() > 48 {
        format!("{summary}...")
    } else {
        summary
    }
}

fn load_state(app: &AppHandle) -> Result<AppState, String> {
    store::load(&app_data_dir(app)?)
}

fn save_state(app: &AppHandle, state: &AppState) -> Result<(), String> {
    store::save(&app_data_dir(app)?, state)
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("failed to resolve app data directory: {error}"))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            get_database_info,
            save_provider,
            delete_provider,
            save_agent,
            delete_agent,
            save_note,
            save_file_record,
            save_memory_entry,
            create_agent_session,
            get_session_messages,
            send_chat_message,
            prepare_browser_run,
            get_runtime_plan,
            run_agent_prompt
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
