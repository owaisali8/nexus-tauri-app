//! The engine seam.
//!
//! Every agent capability in the application streams through [`EngineEvent`].
//! Swapping the underlying agent framework means implementing [`AgentEngine`]
//! against these DTOs and flipping a factory — the UI, persistence, RAG and
//! tool layers are untouched.

pub mod adk;
pub mod direct;

use std::sync::Arc;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::{Result, providers::ProviderConfig};

/// Which engine implementation to run a turn through.
///
/// This is the swap point the architecture is built around: adding an engine
/// means a new variant and a new arm in [`build_engine`], with nothing else in
/// the app touched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// Streamed completions with no agent framework.
    #[default]
    Direct,
    /// ADK-Rust: tool loops, sub-agents, workflow agents.
    Adk,
}

/// Construct an engine for a provider.
///
/// `tools` and `gate` travel together on purpose: a registry without a gate
/// would run side-effecting tools unattended.
pub fn build_engine(
    kind: EngineKind,
    provider: ProviderConfig,
    api_key: Option<String>,
    store: crate::memory::Store,
    tools: crate::tools::ToolRegistry,
    gate: Arc<dyn crate::tools::ApprovalGate>,
) -> Arc<dyn AgentEngine> {
    match kind {
        EngineKind::Direct => {
            Arc::new(direct::DirectEngine::new(provider, api_key, store).with_tools(tools, gate))
        }
        // ADK drives tools through its own agent loop, which the CompatModel
        // adapter does not forward yet — see engine/adk/model.rs.
        EngineKind::Adk => Arc::new(adk::AdkEngine::new(provider, api_key, store)),
    }
}

/// Opaque identifier for a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// A single user turn. `attachments` carries ids of already-ingested files
/// rather than raw bytes, so RAG retrieval stays inside `core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInput {
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<String>,
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }
}

/// What the engine is being asked to do for this run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Chat,
    Research,
    Compare,
}

/// Per-run configuration. Carries ids only — secrets are resolved from the OS
/// keychain at call time and never travel through this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    pub provider_id: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tool_ids: Vec<String>,
    #[serde(default)]
    pub mcp_ids: Vec<String>,
    #[serde(default)]
    pub mode: RunMode,
    /// Identifies this run for cancellation and approval routing.
    ///
    /// Engines are shared across conversations, so an approval prompt has to
    /// name the run it belongs to or it could be answered by the wrong one.
    #[serde(default)]
    pub run_id: String,
}

impl RunOptions {
    pub fn new(provider_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
            temperature: None,
            system_prompt: None,
            tool_ids: Vec::new(),
            mcp_ids: Vec::new(),
            mode: RunMode::default(),
            run_id: String::new(),
        }
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = run_id.into();
        self
    }
}

/// Token accounting for a completed run. Providers that do not report usage
/// leave this as `None` on [`EngineEvent::Done`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// The one event type crossing the engine boundary.
///
/// Serialized as an internally-tagged union so the TypeScript side can
/// discriminate on `type` directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Incremental assistant output. Concatenate in arrival order.
    Token { text: String },
    /// The model requested a tool. Side-effectful tools must be gated behind
    /// user approval before the corresponding [`EngineEvent::ToolResult`].
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// A side-effecting call is waiting on the user.
    ///
    /// The run is blocked until the shell answers. Emitted before anything
    /// runs, never after — approval that arrives post-hoc is not approval.
    ApprovalRequest {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        ok: bool,
        output: serde_json::Value,
    },
    /// A retrieved source backing the current answer.
    Citation {
        source: String,
        #[serde(default)]
        url: Option<String>,
        snippet: String,
    },
    /// Terminal success. No further events follow.
    Done {
        #[serde(default)]
        usage: Option<Usage>,
    },
    /// Terminal failure. No further events follow.
    Error { message: String },
}

impl EngineEvent {
    /// Whether this event ends the stream.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Error { .. })
    }
}

/// Enforce the stream contract: exactly one terminal event, nothing after it.
///
/// Truncates anything following a [`EngineEvent::Done`] or
/// [`EngineEvent::Error`], and appends a `Done` only if the source produced no
/// terminal event of its own. Engines whose backend has no completion signal
/// must route through this — appending `Done` unconditionally would mask a
/// failed run as a successful one.
pub fn ensure_terminal(stream: BoxStream<'static, EngineEvent>) -> BoxStream<'static, EngineEvent> {
    use futures::StreamExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let saw_terminal = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&saw_terminal);

    let body = stream.scan(false, move |finished, event| {
        if *finished {
            return futures::future::ready(None);
        }
        if event.is_terminal() {
            *finished = true;
            seen.store(true, Ordering::SeqCst);
        }
        futures::future::ready(Some(event))
    });

    let tail = futures::stream::iter(std::iter::once(())).filter_map(move |()| {
        let needs_done = !saw_terminal.load(Ordering::SeqCst);
        futures::future::ready(needs_done.then_some(EngineEvent::Done { usage: None }))
    });

    body.chain(tail).boxed()
}

/// A streamed agent run.
///
/// Cancellation is by dropping the returned stream; implementations must abort
/// in-flight work on drop rather than leaking a task.
#[async_trait::async_trait]
pub trait AgentEngine: Send + Sync {
    async fn run_stream(
        &self,
        session_id: SessionId,
        input: UserInput,
        opts: RunOptions,
    ) -> Result<BoxStream<'static, EngineEvent>>;

    /// Discard any cached state for a session.
    ///
    /// Called after the stored transcript is edited or truncated. An engine
    /// that keeps its own copy of the conversation must drop it here, or the
    /// next turn would replay history the user just removed.
    ///
    /// The default is a no-op, which is correct for engines that read the
    /// transcript from the store on every run.
    async fn forget_session(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_event_serializes_with_type_tag() {
        let event = EngineEvent::Token {
            text: "hi".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "token");
        assert_eq!(json["text"], "hi");
    }

    #[test]
    fn tool_call_round_trips() {
        let event = EngineEvent::ToolCall {
            id: "call-1".to_string(),
            name: "web_fetch".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: EngineEvent = serde_json::from_str(&json).unwrap();
        match back {
            EngineEvent::ToolCall { id, name, .. } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "web_fetch");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    use futures::StreamExt;

    async fn drain(stream: BoxStream<'static, EngineEvent>) -> Vec<EngineEvent> {
        stream.collect().await
    }

    #[tokio::test]
    async fn ensure_terminal_appends_done_when_missing() {
        let source = futures::stream::iter(vec![EngineEvent::Token {
            text: "hi".to_string(),
        }])
        .boxed();

        let events = drain(ensure_terminal(source)).await;
        assert_eq!(events.len(), 2);
        assert!(events[1].is_terminal());
    }

    /// Regression: an unconditional `Done` after an `Error` reported a failed
    /// run as successful, which hid a live "session not found" failure.
    #[tokio::test]
    async fn ensure_terminal_does_not_mask_an_error() {
        let source = futures::stream::iter(vec![EngineEvent::Error {
            message: "session not found".to_string(),
        }])
        .boxed();

        let events = drain(ensure_terminal(source)).await;
        assert_eq!(events.len(), 1, "no event may follow a terminal Error");
        assert!(matches!(events[0], EngineEvent::Error { .. }));
    }

    #[tokio::test]
    async fn ensure_terminal_truncates_after_the_first_terminal() {
        let source = futures::stream::iter(vec![
            EngineEvent::Done { usage: None },
            EngineEvent::Token {
                text: "leaked".to_string(),
            },
        ])
        .boxed();

        let events = drain(ensure_terminal(source)).await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EngineEvent::Done { .. }));
    }

    #[test]
    fn only_done_and_error_are_terminal() {
        assert!(EngineEvent::Done { usage: None }.is_terminal());
        assert!(
            EngineEvent::Error {
                message: "boom".to_string()
            }
            .is_terminal()
        );
        assert!(
            !EngineEvent::Token {
                text: String::new()
            }
            .is_terminal()
        );
    }
}
