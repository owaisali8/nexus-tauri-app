//! An ADK [`Llm`] backed by our own OpenAI-compatible client.
//!
//! # Why this exists
//!
//! `adk_model::openai::OpenAIClient` routes streamed text through a
//! `ToolCallBuffer` that watches for text-encoded tool-call markers such as
//! `<tool_call>` and `[TOOL_CALLS]`. Its partial-prefix check matches a buffer
//! *ending* in any prefix of those markers — which includes a bare `<` or `[`.
//! Once that happens the buffer stops emitting and accumulates, and its flush
//! runs only after the `turn_complete` event, by which point the agent has
//! stopped listening.
//!
//! The effect is silent data loss: everything after the first `<` or `[` in a
//! reply disappears. Measured against LM Studio (qwen3.5-9b), "explain Rust
//! generics, mention Vec<T>" returned 131 characters containing zero `<`,
//! while the same prompt through this client returned 529 characters
//! containing three. For a workspace whose users write Rust, dropping output
//! at `<` and `[` is not survivable.
//!
//! Using our transport keeps ADK's agent loop, sessions and workflow agents
//! while removing that failure mode.
//!
//! # Limitation
//!
//! This adapter forwards text only. Native `tool_calls` deltas are not parsed
//! yet, so an ADK agent given tools will not receive calls through it. Phase 2
//! must add native tool-call parsing here rather than reaching back for ADK's
//! transport.

use adk_core::{
    Content, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, error::AdkError,
};
use futures::StreamExt;

use std::sync::Arc;

use crate::{
    engine::EngineEvent,
    providers::{ChatMessage, ChatRequest, ChatTransport, ProviderConfig, build_transport},
};

pub struct CompatModel {
    transport: Arc<dyn ChatTransport>,
    model: String,
}

impl CompatModel {
    pub fn new(
        provider: &ProviderConfig,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> crate::Result<Self> {
        Ok(Self {
            transport: build_transport(provider, api_key)?,
            model: model.into(),
        })
    }
}

/// Flatten ADK content into OpenAI wire messages.
///
/// ADK uses `model` for assistant turns; the OpenAI format uses `assistant`.
/// Non-text parts are dropped — this adapter is text-only by design.
fn to_chat_messages(contents: &[Content]) -> Vec<ChatMessage> {
    contents
        .iter()
        .filter_map(|content| {
            let text: String = content
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();

            if text.is_empty() {
                return None;
            }

            Some(match content.role.as_str() {
                "model" | "assistant" => ChatMessage::assistant(text),
                "system" => ChatMessage::system(text),
                _ => ChatMessage::user(text),
            })
        })
        .collect()
}

fn partial_text(text: String) -> LlmResponse {
    LlmResponse {
        content: Some(Content {
            role: "model".to_string(),
            parts: vec![Part::Text { text }],
        }),
        usage_metadata: None,
        finish_reason: None,
        citation_metadata: None,
        partial: true,
        turn_complete: false,
        interrupted: false,
        error_code: None,
        error_message: None,
        provider_metadata: None,
        interaction_id: None,
    }
}

fn final_response(finish_reason: FinishReason) -> LlmResponse {
    LlmResponse {
        content: None,
        usage_metadata: None,
        finish_reason: Some(finish_reason),
        citation_metadata: None,
        partial: false,
        turn_complete: true,
        interrupted: false,
        error_code: None,
        error_message: None,
        provider_metadata: None,
        interaction_id: None,
    }
}

#[async_trait::async_trait]
impl Llm for CompatModel {
    fn name(&self) -> &str {
        &self.model
    }

    async fn generate_content(
        &self,
        req: LlmRequest,
        _stream: bool,
    ) -> Result<LlmResponseStream, AdkError> {
        if !req.tools.is_empty() {
            // Loud rather than silent: a caller expecting tool calls would
            // otherwise get a plausible text answer and no calls at all.
            tracing::warn!(
                tool_count = req.tools.len(),
                "CompatModel does not forward tool calls yet; tools will be ignored"
            );
        }

        let messages = to_chat_messages(&req.contents);
        let temperature = req.config.as_ref().and_then(|config| config.temperature);
        let model = if req.model.is_empty() {
            self.model.clone()
        } else {
            req.model.clone()
        };

        let stream = self
            .transport
            .chat_stream(ChatRequest::new(model, messages).with_temperature(temperature))
            .await
            .map_err(|error| AdkError::model(error.to_string()))?;

        // EngineEvent guarantees exactly one terminal event, so the mapping is
        // one-to-one and needs no end-of-stream flush.
        let mapped = stream.map(|event| match event {
            EngineEvent::Token { text } => Ok(partial_text(text)),
            EngineEvent::Done { .. } => Ok(final_response(FinishReason::Stop)),
            EngineEvent::Error { message } => Err(AdkError::model(message)),
            // Tool and citation events cannot originate from this transport.
            _ => Ok(final_response(FinishReason::Other)),
        });

        Ok(Box::pin(mapped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_role_becomes_assistant() {
        let contents = vec![
            Content {
                role: "user".to_string(),
                parts: vec![Part::Text {
                    text: "hi".to_string(),
                }],
            },
            Content {
                role: "model".to_string(),
                parts: vec![Part::Text {
                    text: "hello".to_string(),
                }],
            },
        ];

        let messages = to_chat_messages(&contents);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[test]
    fn empty_and_non_text_content_is_dropped() {
        let contents = vec![Content {
            role: "user".to_string(),
            parts: vec![Part::InlineData {
                mime_type: "image/png".to_string(),
                data: vec![1, 2, 3],
            }],
        }];
        assert!(to_chat_messages(&contents).is_empty());
    }

    #[test]
    fn multiple_text_parts_are_concatenated() {
        let contents = vec![Content {
            role: "user".to_string(),
            parts: vec![
                Part::Text {
                    text: "Vec<".to_string(),
                },
                Part::Text {
                    text: "T>".to_string(),
                },
            ],
        }];
        let messages = to_chat_messages(&contents);
        assert_eq!(messages[0].content, "Vec<T>");
    }
}
