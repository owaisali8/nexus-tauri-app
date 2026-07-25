//! Framework-free [`AgentEngine`] over the provider transports.
//!
//! No agent framework: a completion stream plus a tool loop. It exists so
//! plain chat has a path with minimal moving parts, and so the ADK engine has
//! something to be compared against.

use std::sync::Arc;

use futures::{StreamExt, stream::BoxStream};

use crate::{
    Result,
    engine::{AgentEngine, EngineEvent, RunOptions, SessionId, UserInput},
    memory::Store,
    providers::{
        ChatMessage, ChatRequest, ChatTransport, ProviderConfig, WireFunction, WireToolCall,
        build_transport,
    },
    tools::{ApprovalGate, DenyAll, Effect, RunContext, ToolCall, ToolRegistry},
};

/// Ceiling on tool round-trips within a single turn.
///
/// A model that keeps calling tools without producing an answer would
/// otherwise loop until the user cancels, spending tokens each round.
const MAX_TOOL_ROUNDS: usize = 8;

pub struct DirectEngine {
    provider: ProviderConfig,
    api_key: Option<String>,
    /// Transcripts live in SQLite, so conversations survive restart and the
    /// engine holds no conversation state of its own.
    store: Store,
    tools: ToolRegistry,
    /// Defaults to denying everything: an engine built without an explicit
    /// gate must not run side-effecting tools unattended.
    gate: Arc<dyn ApprovalGate>,
}

impl DirectEngine {
    pub fn new(provider: ProviderConfig, api_key: Option<String>, store: Store) -> Self {
        Self {
            provider,
            api_key,
            store,
            tools: ToolRegistry::new(),
            gate: Arc::new(DenyAll),
        }
    }

    pub fn with_tools(mut self, tools: ToolRegistry, gate: Arc<dyn ApprovalGate>) -> Self {
        self.tools = tools;
        self.gate = gate;
        self
    }

    fn transport(&self) -> Result<Arc<dyn ChatTransport>> {
        build_transport(&self.provider, self.api_key.clone())
    }
}

#[async_trait::async_trait]
impl AgentEngine for DirectEngine {
    async fn run_stream(
        &self,
        session_id: SessionId,
        input: UserInput,
        opts: RunOptions,
    ) -> Result<BoxStream<'static, EngineEvent>> {
        let transport = self.transport()?;

        // Persist the user turn before the request goes out, so a failed or
        // cancelled run still leaves the question in the transcript.
        self.store
            .append_message(&session_id.0, "user", &input.text)?;

        let mut messages = Vec::new();
        if let Some(system_prompt) = opts.system_prompt.as_deref() {
            messages.push(ChatMessage::system(system_prompt));
        }
        messages.extend(
            self.store
                .load_messages(&session_id.0)?
                .iter()
                .map(crate::memory::Message::to_chat_message),
        );

        // Only the tools this run enabled, and only if the transport can
        // actually carry them.
        let tools = if transport.supports_tools() {
            if opts.tool_ids.is_empty() {
                ToolRegistry::new()
            } else {
                self.tools.subset(&opts.tool_ids)
            }
        } else {
            if !opts.tool_ids.is_empty() {
                tracing::warn!(
                    provider = %self.provider.id,
                    "this provider's transport cannot forward tools; the run will proceed without them"
                );
            }
            ToolRegistry::new()
        };

        let gate = Arc::clone(&self.gate);
        let store = self.store.clone();
        let key = session_id.0.clone();
        let model = opts.model.clone();
        let temperature = opts.temperature;
        let context = RunContext::new(&session_id.0, &opts.run_id);

        let stream = async_stream::stream! {
            let mut messages = messages;
            let mut answer = String::new();

            for round in 0..MAX_TOOL_ROUNDS {
                let request = ChatRequest::new(&model, messages.clone())
                    .with_temperature(temperature)
                    .with_tools(tools.specs());

                let mut inner = match transport.chat_stream(request).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        yield EngineEvent::Error { message: error.to_string() };
                        return;
                    }
                };

                let mut calls: Vec<ToolCall> = Vec::new();
                let mut usage = None;
                let mut failed = false;

                while let Some(event) = inner.next().await {
                    match event {
                        EngineEvent::Token { text } => {
                            answer.push_str(&text);
                            yield EngineEvent::Token { text };
                        }
                        EngineEvent::ToolCall { id, name, args } => {
                            calls.push(ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: args.clone(),
                            });
                            yield EngineEvent::ToolCall { id, name, args };
                        }
                        EngineEvent::Done { usage: reported } => {
                            usage = reported;
                            break;
                        }
                        EngineEvent::Error { message } => {
                            failed = true;
                            yield EngineEvent::Error { message };
                            break;
                        }
                        other => yield other,
                    }
                }

                if failed {
                    return;
                }

                // No tools requested: this round produced the answer.
                if calls.is_empty() {
                    if !answer.is_empty()
                        && let Err(error) = store.append_message(&key, "assistant", &answer)
                    {
                        tracing::error!(%error, "failed to persist assistant message");
                    }
                    yield EngineEvent::Done { usage };
                    return;
                }

                // Record what the model asked for. Omitting this makes the
                // tool results below reference calls the API has no record of.
                messages.push(ChatMessage::tool_requests(
                    calls
                        .iter()
                        .map(|call| WireToolCall {
                            id: call.id.clone(),
                            kind: "function".to_string(),
                            function: WireFunction {
                                name: call.name.clone(),
                                arguments: call.arguments.to_string(),
                            },
                        })
                        .collect(),
                ));

                for call in &calls {
                    // Announce the prompt before blocking on it, so the UI can
                    // show a card rather than appearing to hang.
                    if tools.effect_of(&call.name) == Some(Effect::SideEffecting) {
                        yield EngineEvent::ApprovalRequest {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            args: call.arguments.clone(),
                        };
                    }

                    let outcome = tools.invoke(&context, call, gate.as_ref()).await;

                    yield EngineEvent::ToolResult {
                        id: outcome.id.clone(),
                        ok: outcome.ok,
                        output: outcome.output.clone(),
                    };

                    messages.push(ChatMessage::tool_result(
                        &outcome.id,
                        outcome.output.to_string(),
                    ));
                }

                if round + 1 == MAX_TOOL_ROUNDS {
                    yield EngineEvent::Error {
                        message: format!(
                            "stopped after {MAX_TOOL_ROUNDS} rounds of tool calls without a final answer"
                        ),
                    };
                    return;
                }
            }
        };

        Ok(stream.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;

    #[tokio::test]
    async fn user_turn_is_persisted_even_when_the_server_is_unreachable() {
        let store = Store::open_in_memory().unwrap();
        let session = store
            .create_session("s", "lmstudio-local", "any", EngineKind::Direct)
            .unwrap();

        let mut provider = ProviderConfig::lm_studio();
        // Port chosen to be closed so the request fails fast.
        provider.base_url = Some("http://127.0.0.1:1/v1".to_string());
        let engine = DirectEngine::new(provider, None, store.clone());

        let mut stream = engine
            .run_stream(
                session.id.clone().into(),
                UserInput::text("hello"),
                RunOptions::new("lmstudio-local", "any-model"),
            )
            .await
            .expect("the stream itself opens; the failure arrives as an event");

        let events: Vec<EngineEvent> = stream.by_ref().collect().await;
        assert!(
            matches!(events.first(), Some(EngineEvent::Error { .. })),
            "expected a transport error event, got {events:?}"
        );

        let messages = store.load_messages(&session.id).unwrap();
        assert_eq!(messages.len(), 1, "the user turn should still be recorded");
        assert_eq!(messages[0].role, "user");
    }

    /// The default must stay deny, not drift to permissive: an engine built
    /// without a gate would otherwise run side-effecting tools unattended.
    #[tokio::test]
    async fn an_engine_without_an_explicit_gate_denies_by_default() {
        let store = Store::open_in_memory().unwrap();
        let engine = DirectEngine::new(ProviderConfig::lm_studio(), None, store);

        let decision = engine
            .gate
            .request(
                &RunContext::new("session-1", "run-1"),
                &ToolCall {
                    id: "call-1".to_string(),
                    name: "anything".to_string(),
                    arguments: serde_json::json!({}),
                },
            )
            .await;

        assert_eq!(decision, crate::tools::Approval::Deny);
    }
}
