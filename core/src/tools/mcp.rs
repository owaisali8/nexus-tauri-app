//! MCP client: connects to servers and exposes their tools.
//!
//! # Trust
//!
//! An MCP server is a third-party process the user chose to run. Its tools can
//! do anything that process can do, and the server describes its own tools —
//! including any hint about whether a tool is read-only.
//!
//! That hint is self-attestation from untrusted code, so this module ignores
//! it: **every MCP tool is [`Effect::SideEffecting`]** and therefore passes
//! through the approval gate. Honouring `readOnlyHint` would let a server
//! opt itself out of the only check standing between it and the user's
//! machine.
//!
//! The cost is a prompt per call, including for genuinely harmless reads. The
//! fix is per-tool trust the *user* grants, not trust the server claims.

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientInfo, ContentBlock},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    Error, Result,
    tools::{Effect, Tool, ToolRegistry, ToolSpec},
};

/// Separates a server id from a tool name in the exposed tool name.
///
/// Two servers may both offer `search`; the registry is flat, so names are
/// namespaced. Chosen to be legal in the `^[a-zA-Z0-9_-]{1,64}$` pattern
/// providers enforce on function names — a dot or slash would be rejected.
const NAMESPACE_SEPARATOR: &str = "__";

/// How to launch one MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Stable id, also the namespace for this server's tools.
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment for the child process.
    ///
    /// Secrets belong in the OS keychain, not here — this file is plaintext.
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl McpServerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::Invalid("server id is required".to_string()));
        }
        if self.id.contains(NAMESPACE_SEPARATOR) {
            return Err(Error::Invalid(format!(
                "server id must not contain `{NAMESPACE_SEPARATOR}`"
            )));
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::Invalid(
                "server id may only contain letters, digits, `-` and `_`".to_string(),
            ));
        }
        if self.command.trim().is_empty() {
            return Err(Error::Invalid("a command is required".to_string()));
        }
        Ok(())
    }
}

/// Resolve a command name to a full path.
///
/// `Command::new("npx")` fails on Windows: npm installs `npx.cmd` and
/// `npx.ps1` shims rather than an `.exe`, and Rust's PATH search only appends
/// `.exe`. The `which` crate honours `PATHEXT`, so it finds the shim that
/// CreateProcess can actually run. Since most MCP servers are launched with
/// `npx`, without this nearly every server fails on Windows with an unhelpful
/// "program not found".
fn resolve_program(command: &str) -> Result<std::path::PathBuf> {
    // An explicit path is used as given; the user meant that exact file.
    let candidate = std::path::Path::new(command);
    if candidate.is_absolute() || command.contains('/') || command.contains('\\') {
        return Ok(candidate.to_path_buf());
    }

    which::which(command).map_err(|error| {
        Error::Invalid(format!(
            "`{command}` was not found on PATH ({error}). \
             Install it, or give the full path to the executable."
        ))
    })
}

/// Namespaced tool name, e.g. `filesystem__read_file`.
pub fn namespaced(server_id: &str, tool_name: &str) -> String {
    format!("{server_id}{NAMESPACE_SEPARATOR}{tool_name}")
}

/// Split a namespaced name back into server and tool.
pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    name.split_once(NAMESPACE_SEPARATOR)
}

/// A live connection to one MCP server.
pub struct McpConnection {
    id: String,
    /// Behind a mutex because the peer is shared by every tool this server
    /// exposes, and calls may overlap.
    service: Mutex<RunningService<rmcp::RoleClient, ClientInfo>>,
    tools: Vec<ToolSpec>,
}

impl McpConnection {
    /// Spawn the server and complete the MCP handshake.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        config.validate()?;

        let program = resolve_program(&config.command)?;

        let mut command = tokio::process::Command::new(&program);
        command.args(&config.args);
        for (key, value) in &config.env {
            command.env(key, value);
        }

        let transport = TokioChildProcess::new(command).map_err(|error| {
            Error::Transport(format!(
                "could not start MCP server `{}` ({}): {error}",
                config.id,
                program.display()
            ))
        })?;

        let service = ClientInfo::default()
            .serve(transport)
            .await
            .map_err(|error| {
                Error::Transport(format!(
                    "MCP handshake with `{}` failed: {error}",
                    config.id
                ))
            })?;

        let listed = service.list_all_tools().await.map_err(|error| {
            Error::Transport(format!("`{}` tools/list failed: {error}", config.id))
        })?;

        let tools = listed
            .into_iter()
            .map(|tool| ToolSpec {
                name: namespaced(&config.id, &tool.name),
                description: tool
                    .description
                    .map(|text| text.to_string())
                    .unwrap_or_else(|| format!("{} (via {})", tool.name, config.id)),
                parameters: serde_json::Value::Object((*tool.input_schema).clone()),
            })
            .collect();

        Ok(Self {
            id: config.id.clone(),
            service: Mutex::new(service),
            tools,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Invoke a tool by its bare (un-namespaced) name.
    async fn call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let arguments = match arguments {
            serde_json::Value::Object(map) => Some(map),
            // A tool with no parameters may receive null; send no arguments
            // rather than an invalid payload.
            serde_json::Value::Null => None,
            other => {
                return Err(Error::Invalid(format!(
                    "tool arguments must be an object, got {other}"
                )));
            }
        };

        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }

        let service = self.service.lock().await;
        let result = service
            .call_tool(params)
            .await
            .map_err(|error| Error::Transport(format!("`{}` call failed: {error}", self.id)))?;

        // A server reporting failure is a tool error, not a transport error:
        // the model should see it and adapt.
        if result.is_error.unwrap_or(false) {
            return Err(Error::Invalid(flatten_content(&result.content)));
        }

        // Prefer structured output when the server provides it; otherwise
        // hand back the text blocks.
        Ok(result
            .structured_content
            .unwrap_or_else(|| serde_json::json!({ "content": flatten_content(&result.content) })))
    }
}

/// Reduce MCP content blocks to text.
///
/// Images and other binary blocks are named rather than inlined — a base64
/// blob in the transcript would flood the model's context.
fn flatten_content(blocks: &[ContentBlock]) -> String {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => out.push(text.text.clone()),
            ContentBlock::Image(image) => out.push(format!("[image: {}]", image.mime_type)),
            ContentBlock::Audio(audio) => out.push(format!("[audio: {}]", audio.mime_type)),
            ContentBlock::ResourceLink(resource) => {
                out.push(format!("[resource: {}]", resource.uri))
            }
            ContentBlock::Resource(_) => out.push("[embedded resource]".to_string()),
            // ContentBlock is non_exhaustive: a block type added by a later
            // spec revision should degrade, not fail to compile.
            _ => out.push("[unsupported content block]".to_string()),
        }
    }
    out.join("\n")
}

/// One MCP tool, adapted to the [`Tool`] trait.
pub struct McpTool {
    connection: Arc<McpConnection>,
    /// Name as the server knows it, without the namespace prefix.
    bare_name: String,
    spec: ToolSpec,
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    /// Always gated. See the module docs: a server's own claim that a tool is
    /// harmless is not evidence.
    fn effect(&self) -> Effect {
        Effect::SideEffecting
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<serde_json::Value> {
        self.connection.call(&self.bare_name, arguments).await
    }
}

/// Connected MCP servers and the tools they expose.
#[derive(Default)]
pub struct McpManager {
    connections: Vec<Arc<McpConnection>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connect every enabled server, collecting rather than propagating
    /// failures: one broken server must not prevent the others from loading.
    pub async fn connect_all(configs: &[McpServerConfig]) -> (Self, Vec<String>) {
        let mut manager = Self::new();
        let mut failures = Vec::new();

        for config in configs.iter().filter(|config| config.enabled) {
            match McpConnection::connect(config).await {
                Ok(connection) => manager.connections.push(Arc::new(connection)),
                Err(error) => {
                    tracing::warn!(server = %config.id, %error, "MCP server failed to start");
                    failures.push(format!("{}: {error}", config.id));
                }
            }
        }

        (manager, failures)
    }

    pub fn server_ids(&self) -> Vec<String> {
        self.connections
            .iter()
            .map(|connection| connection.id().to_string())
            .collect()
    }

    /// Register every connected server's tools into `registry`.
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        for connection in &self.connections {
            for spec in connection.tools() {
                let Some((_, bare)) = split_namespaced(&spec.name) else {
                    continue;
                };

                registry.register(Arc::new(McpTool {
                    connection: Arc::clone(connection),
                    bare_name: bare.to_string(),
                    spec: spec.clone(),
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.to_string(),
            command: "echo".to_string(),
            args: vec![],
            env: HashMap::new(),
            enabled: true,
        }
    }

    #[test]
    fn names_are_namespaced_by_server() {
        assert_eq!(namespaced("fs", "read_file"), "fs__read_file");
        assert_eq!(split_namespaced("fs__read_file"), Some(("fs", "read_file")));
        assert_eq!(split_namespaced("no_separator_here"), None);
    }

    /// Two servers offering the same tool name must not collide in the flat
    /// registry.
    #[test]
    fn identical_tool_names_stay_distinct_across_servers() {
        assert_ne!(namespaced("a", "search"), namespaced("b", "search"));
    }

    #[test]
    fn ids_that_would_break_namespacing_are_rejected() {
        for bad in ["", "has space", "has__separator", "dot.name", "slash/name"] {
            let mut cfg = config("ok");
            cfg.id = bad.to_string();
            assert!(cfg.validate().is_err(), "`{bad}` should be rejected");
        }
    }

    #[test]
    fn a_command_is_required() {
        let mut cfg = config("fs");
        cfg.command = "  ".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn a_valid_config_passes() {
        assert!(config("filesystem").validate().is_ok());
        assert!(config("my-server_2").validate().is_ok());
    }

    #[test]
    fn text_blocks_are_flattened_in_order() {
        let blocks = vec![ContentBlock::text("first"), ContentBlock::text("second")];
        assert_eq!(flatten_content(&blocks), "first\nsecond");
    }

    /// Regression: `npx` is a `.cmd`/`.ps1` shim on Windows, and Rust's PATH
    /// search only appends `.exe`. Without PATHEXT-aware resolution nearly
    /// every MCP server fails to launch.
    #[test]
    fn a_shim_on_path_resolves_to_a_runnable_file() {
        // Pick something guaranteed present on each platform.
        #[cfg(windows)]
        let program = "cmd";
        #[cfg(not(windows))]
        let program = "sh";

        let resolved = resolve_program(program).unwrap();
        assert!(
            resolved.is_absolute(),
            "expected an absolute path, got {}",
            resolved.display()
        );
    }

    #[test]
    fn a_missing_program_says_so_plainly() {
        let error = resolve_program("definitely-not-real-xyz")
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found on PATH"), "got {error}");
    }

    #[test]
    fn an_explicit_path_is_left_alone() {
        // A user-supplied path must not be re-resolved against PATH.
        #[cfg(windows)]
        let path = r"C:\tools\my-server.exe";
        #[cfg(not(windows))]
        let path = "/usr/local/bin/my-server";

        assert_eq!(resolve_program(path).unwrap(), std::path::Path::new(path));
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_reports_which_one() {
        let mut cfg = config("broken");
        cfg.command = "definitely-not-a-real-command-xyz".to_string();

        let (manager, failures) = McpManager::connect_all(&[cfg]).await;
        assert!(manager.server_ids().is_empty());
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("broken"), "got {}", failures[0]);
    }

    #[tokio::test]
    async fn disabled_servers_are_not_started() {
        let mut cfg = config("off");
        cfg.enabled = false;
        cfg.command = "definitely-not-a-real-command-xyz".to_string();

        let (_, failures) = McpManager::connect_all(&[cfg]).await;
        assert!(failures.is_empty(), "a disabled server must not be started");
    }
}
