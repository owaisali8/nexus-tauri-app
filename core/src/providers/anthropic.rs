//! Anthropic Messages API transport.
//!
//! Differs from the OpenAI wire format in ways that are easy to get wrong:
//! the system prompt is a top-level field rather than a message, `max_tokens`
//! is required, and consecutive messages with the same role are rejected.

use eventsource_stream::Eventsource;
use futures::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    engine::{EngineEvent, Usage},
    providers::{
        ChatTransport, ModelInfo, ProviderConfig, openai_compat::ChatMessage, split_system,
    },
};

/// Wire version this client is written against.
const API_VERSION: &str = "2023-06-01";

/// Anthropic requires `max_tokens`; there is no "unlimited" value.
///
/// Chosen to be generous enough that replies are not clipped mid-thought.
/// Worth surfacing as a per-run option once anything needs to tune it.
const DEFAULT_MAX_TOKENS: u32 = 8192;

#[derive(Debug, Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: &'a [ChatMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<AnthropicModel>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
}

/// What one SSE payload means to us.
#[derive(Debug, PartialEq)]
pub(crate) enum Chunk {
    Text(String),
    InputTokens(u32),
    OutputTokens(u32),
    Stop,
    Failed(String),
    /// Events with no bearing on the transcript: pings, block starts/stops.
    Ignore,
}

/// Interpret one `data:` payload from the Messages stream.
///
/// Pure and synchronous so the event shapes can be tested without a network.
pub(crate) fn parse_chunk(data: &str) -> Result<Chunk> {
    let value: serde_json::Value = serde_json::from_str(data)?;

    match value.get("type").and_then(|t| t.as_str()) {
        Some("content_block_delta") => {
            let delta = value.get("delta");
            // Anthropic also emits input_json_delta for tool arguments; only
            // text belongs in the transcript.
            let is_text = delta
                .and_then(|d| d.get("type"))
                .and_then(|t| t.as_str())
                .is_none_or(|t| t == "text_delta");

            if !is_text {
                return Ok(Chunk::Ignore);
            }

            match delta.and_then(|d| d.get("text")).and_then(|t| t.as_str()) {
                Some(text) if !text.is_empty() => Ok(Chunk::Text(text.to_string())),
                _ => Ok(Chunk::Ignore),
            }
        }

        Some("message_start") => Ok(value
            .get("message")
            .and_then(|m| m.get("usage"))
            .and_then(|u| u.get("input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .map_or(Chunk::Ignore, |tokens| Chunk::InputTokens(tokens as u32))),

        Some("message_delta") => Ok(value
            .get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(serde_json::Value::as_u64)
            .map_or(Chunk::Ignore, |tokens| Chunk::OutputTokens(tokens as u32))),

        Some("message_stop") => Ok(Chunk::Stop),

        Some("error") => {
            let message = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            Ok(Chunk::Failed(message.to_string()))
        }

        _ => Ok(Chunk::Ignore),
    }
}

/// Prepare a transcript for the Messages API.
///
/// Drops empty messages and merges runs of the same role, which Anthropic
/// rejects. Truncating a transcript for regenerate can leave two user turns
/// adjacent, so this is not hypothetical.
pub(crate) fn normalize(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();

    for message in messages {
        if message.content.trim().is_empty() {
            continue;
        }

        let role = if message.role == "assistant" {
            "assistant"
        } else {
            "user"
        };

        match out.last_mut() {
            Some(previous) if previous.role == role => {
                previous.content.push_str("\n\n");
                previous.content.push_str(&message.content);
            }
            _ => out.push(ChatMessage {
                role: role.to_string(),
                content: message.content,
            }),
        }
    }

    // The API requires the conversation to open with a user turn.
    if out.first().is_some_and(|first| first.role == "assistant") {
        out.remove(0);
    }

    out
}

pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AnthropicClient {
    pub fn new(config: &ProviderConfig, api_key: Option<String>) -> Result<Self> {
        let base_url = config
            .effective_base_url()
            .ok_or_else(|| Error::ProviderMisconfigured {
                provider_id: config.id.clone(),
                reason: "base_url is required".to_string(),
            })?
            .to_string();

        let api_key = api_key.ok_or_else(|| Error::ProviderMisconfigured {
            provider_id: config.id.clone(),
            reason: "an API key is required for Anthropic".to_string(),
        })?;

        Ok(Self {
            http: reqwest::Client::builder()
                .build()
                .map_err(|error| Error::Transport(error.to_string()))?,
            base_url,
            api_key,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/{}", self.base_url, path))
            // Anthropic authenticates with x-api-key, not a bearer token.
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
    }
}

#[async_trait::async_trait]
impl ChatTransport for AnthropicClient {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .request(reqwest::Method::GET, "v1/models")
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "Anthropic /v1/models returned {status}: {}",
                body.trim()
            )));
        }

        let body: ModelsResponse = response
            .json()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(body
            .data
            .into_iter()
            .map(|model| ModelInfo {
                id: model.id,
                owned_by: Some("anthropic".to_string()),
            })
            .collect())
    }

    async fn chat_stream(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
    ) -> Result<BoxStream<'static, EngineEvent>> {
        let (system, rest) = split_system(messages);
        let normalized = normalize(rest);

        if normalized.is_empty() {
            return Err(Error::Invalid(
                "a request needs at least one non-empty user message".to_string(),
            ));
        }

        let response = self
            .request(reqwest::Method::POST, "v1/messages")
            .json(&MessagesRequest {
                model,
                max_tokens: DEFAULT_MAX_TOKENS,
                messages: &normalized,
                stream: true,
                system,
                temperature,
            })
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "Anthropic /v1/messages returned {status}: {}",
                body.trim()
            )));
        }

        #[derive(Default)]
        struct State {
            finished: bool,
            input_tokens: u32,
            output_tokens: u32,
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .scan(State::default(), |state, item| {
                if state.finished {
                    return futures::future::ready(None);
                }

                let next = match item {
                    Err(error) => {
                        state.finished = true;
                        Some(EngineEvent::Error {
                            message: error.to_string(),
                        })
                    }
                    Ok(event) => match parse_chunk(&event.data) {
                        Ok(Chunk::Text(text)) => Some(EngineEvent::Token { text }),
                        Ok(Chunk::InputTokens(tokens)) => {
                            state.input_tokens = tokens;
                            Some(EngineEvent::Token {
                                text: String::new(),
                            })
                        }
                        Ok(Chunk::OutputTokens(tokens)) => {
                            state.output_tokens = tokens;
                            Some(EngineEvent::Token {
                                text: String::new(),
                            })
                        }
                        Ok(Chunk::Stop) => {
                            state.finished = true;
                            Some(EngineEvent::Done {
                                usage: Some(Usage {
                                    prompt_tokens: state.input_tokens,
                                    completion_tokens: state.output_tokens,
                                    total_tokens: state.input_tokens + state.output_tokens,
                                }),
                            })
                        }
                        Ok(Chunk::Failed(message)) => {
                            state.finished = true;
                            Some(EngineEvent::Error { message })
                        }
                        Ok(Chunk::Ignore) => Some(EngineEvent::Token {
                            text: String::new(),
                        }),
                        Err(error) => {
                            state.finished = true;
                            Some(EngineEvent::Error {
                                message: format!("malformed stream chunk: {error}"),
                            })
                        }
                    },
                };

                futures::future::ready(next)
            })
            // Drop the empty placeholders standing in for ignored events.
            .filter(|event| {
                futures::future::ready(
                    !matches!(event, EngineEvent::Token { text } if text.is_empty()),
                )
            });

        // Anthropic ends with message_stop, but a dropped connection would
        // leave the stream terminal-less.
        Ok(crate::engine::ensure_terminal(stream.boxed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderKind;

    fn config() -> ProviderConfig {
        ProviderConfig {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            api_key_ref: Some("provider:anthropic".to_string()),
            default_model: None,
        }
    }

    #[test]
    fn falls_back_to_the_public_endpoint() {
        assert_eq!(
            config().effective_base_url(),
            Some("https://api.anthropic.com")
        );
    }

    #[test]
    fn requires_an_api_key() {
        assert!(AnthropicClient::new(&config(), None).is_err());
        assert!(AnthropicClient::new(&config(), Some("k".to_string())).is_ok());
    }

    #[test]
    fn text_delta_becomes_a_token() {
        let data = r#"{"type":"content_block_delta","index":0,
                       "delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(parse_chunk(data).unwrap(), Chunk::Text("Hello".to_string()));
    }

    #[test]
    fn tool_argument_deltas_are_ignored() {
        // input_json_delta carries tool arguments, not assistant prose.
        let data = r#"{"type":"content_block_delta","index":0,
                       "delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#;
        assert_eq!(parse_chunk(data).unwrap(), Chunk::Ignore);
    }

    #[test]
    fn usage_is_read_from_both_ends_of_the_stream() {
        let start = r#"{"type":"message_start","message":{"usage":{"input_tokens":42}}}"#;
        assert_eq!(parse_chunk(start).unwrap(), Chunk::InputTokens(42));

        let delta = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},
                        "usage":{"output_tokens":17}}"#;
        assert_eq!(parse_chunk(delta).unwrap(), Chunk::OutputTokens(17));
    }

    #[test]
    fn message_stop_is_terminal() {
        assert_eq!(
            parse_chunk(r#"{"type":"message_stop"}"#).unwrap(),
            Chunk::Stop
        );
    }

    #[test]
    fn api_errors_surface_their_message() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error",
                       "message":"Overloaded"}}"#;
        assert_eq!(
            parse_chunk(data).unwrap(),
            Chunk::Failed("Overloaded".to_string())
        );
    }

    #[test]
    fn pings_and_block_boundaries_are_ignored() {
        for data in [
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ] {
            assert_eq!(parse_chunk(data).unwrap(), Chunk::Ignore, "for {data}");
        }
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_chunk("not json").is_err());
    }

    #[test]
    fn consecutive_same_role_messages_are_merged() {
        let merged = normalize(vec![
            ChatMessage::user("first"),
            ChatMessage::user("second"),
            ChatMessage::assistant("reply"),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].content, "first\n\nsecond");
        assert_eq!(merged[1].role, "assistant");
    }

    #[test]
    fn empty_messages_are_dropped() {
        let merged = normalize(vec![
            ChatMessage::user("keep"),
            ChatMessage::assistant("   "),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].content, "keep");
    }

    #[test]
    fn a_leading_assistant_turn_is_removed() {
        // The API requires the first message to be from the user.
        let merged = normalize(vec![
            ChatMessage::assistant("orphaned"),
            ChatMessage::user("question"),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].role, "user");
    }

    #[test]
    fn system_messages_are_lifted_out() {
        let (system, rest) = split_system(vec![
            ChatMessage::system("be terse"),
            ChatMessage::user("hi"),
        ]);
        assert_eq!(system.as_deref(), Some("be terse"));
        assert_eq!(rest.len(), 1);
    }
}
