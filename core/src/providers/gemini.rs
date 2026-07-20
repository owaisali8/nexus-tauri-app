//! Google Gemini `generateContent` transport.
//!
//! Differs from the OpenAI wire format in several ways: messages are
//! `contents` with `parts`, the assistant role is `model` rather than
//! `assistant`, the system prompt is a separate `systemInstruction`, and the
//! model name is part of the URL path rather than the body.

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

#[derive(Debug, Serialize)]
struct GenerateRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiPart {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    models: Vec<GeminiModel>,
}

#[derive(Debug, Deserialize)]
struct GeminiModel {
    /// Fully qualified, e.g. `models/gemini-2.0-flash`.
    name: String,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

/// What one SSE payload means to us.
#[derive(Debug, PartialEq)]
pub(crate) enum Chunk {
    Text(String),
    Usage(Usage),
    /// Generation stopped for a reason the user should see, e.g. a safety
    /// filter. Without this a blocked reply would look like an empty answer.
    Blocked(String),
    Ignore,
}

/// Strip the `models/` prefix Gemini uses in resource names.
pub(crate) fn short_model_name(name: &str) -> &str {
    name.strip_prefix("models/").unwrap_or(name)
}

/// Interpret one `data:` payload from a streamed generateContent response.
pub(crate) fn parse_chunk(data: &str) -> Result<Chunk> {
    let value: serde_json::Value = serde_json::from_str(data)?;

    // A prompt rejected before generation reports here rather than in a
    // candidate.
    if let Some(reason) = value
        .get("promptFeedback")
        .and_then(|f| f.get("blockReason"))
        .and_then(|r| r.as_str())
    {
        return Ok(Chunk::Blocked(format!("prompt blocked: {reason}")));
    }

    let candidate = value.get("candidates").and_then(|c| c.get(0));

    if let Some(candidate) = candidate {
        let text: String = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        if !text.is_empty() {
            return Ok(Chunk::Text(text));
        }

        // STOP and MAX_TOKENS are normal endings; anything else means the
        // reply was cut off for a reason worth reporting.
        if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str())
            && !matches!(reason, "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED")
        {
            return Ok(Chunk::Blocked(format!("generation stopped: {reason}")));
        }
    }

    if let Some(usage) = value.get("usageMetadata") {
        let read = |key: &str| {
            usage
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32
        };
        return Ok(Chunk::Usage(Usage {
            prompt_tokens: read("promptTokenCount"),
            completion_tokens: read("candidatesTokenCount"),
            total_tokens: read("totalTokenCount"),
        }));
    }

    Ok(Chunk::Ignore)
}

/// Convert a transcript to Gemini `contents`.
///
/// Maps `assistant` to `model`, drops empty turns, and merges runs of the
/// same role so the conversation alternates.
pub(crate) fn to_contents(messages: Vec<ChatMessage>) -> Vec<GeminiContent> {
    let mut out: Vec<GeminiContent> = Vec::new();

    for message in messages {
        if message.content.trim().is_empty() {
            continue;
        }

        let role = if message.role == "assistant" {
            "model"
        } else {
            "user"
        };

        match out.last_mut() {
            Some(previous) if previous.role.as_deref() == Some(role) => {
                if let Some(part) = previous.parts.last_mut() {
                    part.text.push_str("\n\n");
                    part.text.push_str(&message.content);
                }
            }
            _ => out.push(GeminiContent {
                role: Some(role.to_string()),
                parts: vec![GeminiPart {
                    text: message.content,
                }],
            }),
        }
    }

    out
}

pub struct GeminiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl GeminiClient {
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
            reason: "an API key is required for Gemini".to_string(),
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
            // Gemini also accepts `?key=`, but a secret in a URL ends up in
            // logs and history. The header does not.
            .header("x-goog-api-key", &self.api_key)
    }
}

#[async_trait::async_trait]
impl ChatTransport for GeminiClient {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .request(reqwest::Method::GET, "v1beta/models")
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "Gemini /v1beta/models returned {status}: {}",
                body.trim()
            )));
        }

        let body: ModelsResponse = response
            .json()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(body
            .models
            .into_iter()
            // Embedding and tuning models also appear here; keep the ones
            // that can actually hold a conversation.
            .filter(|model| {
                model.supported_generation_methods.is_empty()
                    || model
                        .supported_generation_methods
                        .iter()
                        .any(|method| method == "generateContent")
            })
            .map(|model| ModelInfo {
                id: short_model_name(&model.name).to_string(),
                owned_by: Some("google".to_string()),
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
        let contents = to_contents(rest);

        if contents.is_empty() {
            return Err(Error::Invalid(
                "a request needs at least one non-empty user message".to_string(),
            ));
        }

        let path = format!(
            "v1beta/models/{}:streamGenerateContent?alt=sse",
            short_model_name(model)
        );

        let response = self
            .request(reqwest::Method::POST, &path)
            .json(&GenerateRequest {
                contents,
                system_instruction: system.map(|text| GeminiContent {
                    // systemInstruction carries no role.
                    role: None,
                    parts: vec![GeminiPart { text }],
                }),
                generation_config: temperature.map(|value| GenerationConfig {
                    temperature: Some(value),
                }),
            })
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "Gemini streamGenerateContent returned {status}: {}",
                body.trim()
            )));
        }

        #[derive(Default)]
        struct State {
            finished: bool,
            usage: Option<Usage>,
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
                        Ok(Chunk::Usage(usage)) => {
                            state.usage = Some(usage);
                            Some(EngineEvent::Token {
                                text: String::new(),
                            })
                        }
                        Ok(Chunk::Blocked(message)) => {
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
            .filter(|event| {
                futures::future::ready(
                    !matches!(event, EngineEvent::Token { text } if text.is_empty()),
                )
            });

        // Gemini closes the stream without an explicit terminal event.
        Ok(crate::engine::ensure_terminal(stream.boxed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderKind;

    fn config() -> ProviderConfig {
        ProviderConfig {
            id: "gemini".to_string(),
            label: "Gemini".to_string(),
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_ref: Some("provider:gemini".to_string()),
            default_model: None,
        }
    }

    #[test]
    fn falls_back_to_the_public_endpoint() {
        assert_eq!(
            config().effective_base_url(),
            Some("https://generativelanguage.googleapis.com")
        );
    }

    #[test]
    fn requires_an_api_key() {
        assert!(GeminiClient::new(&config(), None).is_err());
        assert!(GeminiClient::new(&config(), Some("k".to_string())).is_ok());
    }

    #[test]
    fn resource_names_are_shortened() {
        assert_eq!(
            short_model_name("models/gemini-2.0-flash"),
            "gemini-2.0-flash"
        );
        assert_eq!(short_model_name("gemini-2.0-flash"), "gemini-2.0-flash");
    }

    #[test]
    fn candidate_text_becomes_a_token() {
        let data = r#"{"candidates":[{"content":{"role":"model",
                       "parts":[{"text":"Hello"}]}}]}"#;
        assert_eq!(parse_chunk(data).unwrap(), Chunk::Text("Hello".to_string()));
    }

    #[test]
    fn multiple_parts_are_concatenated() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"a"},{"text":"b"}]}}]}"#;
        assert_eq!(parse_chunk(data).unwrap(), Chunk::Text("ab".to_string()));
    }

    #[test]
    fn usage_metadata_is_captured() {
        let data = r#"{"usageMetadata":{"promptTokenCount":11,
                       "candidatesTokenCount":5,"totalTokenCount":16}}"#;
        assert_eq!(
            parse_chunk(data).unwrap(),
            Chunk::Usage(Usage {
                prompt_tokens: 11,
                completion_tokens: 5,
                total_tokens: 16,
            })
        );
    }

    #[test]
    fn a_normal_stop_is_not_reported_as_blocked() {
        let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
        assert_eq!(parse_chunk(data).unwrap(), Chunk::Ignore);
    }

    #[test]
    fn safety_stops_surface_instead_of_looking_empty() {
        let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}"#;
        assert_eq!(
            parse_chunk(data).unwrap(),
            Chunk::Blocked("generation stopped: SAFETY".to_string())
        );
    }

    #[test]
    fn blocked_prompts_surface() {
        let data = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
        assert_eq!(
            parse_chunk(data).unwrap(),
            Chunk::Blocked("prompt blocked: SAFETY".to_string())
        );
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_chunk("not json").is_err());
    }

    #[test]
    fn assistant_role_maps_to_model() {
        let contents = to_contents(vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
        ]);
        assert_eq!(contents[0].role.as_deref(), Some("user"));
        assert_eq!(contents[1].role.as_deref(), Some("model"));
    }

    #[test]
    fn consecutive_same_role_turns_are_merged() {
        let contents = to_contents(vec![
            ChatMessage::user("first"),
            ChatMessage::user("second"),
        ]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].parts[0].text, "first\n\nsecond");
    }

    #[test]
    fn empty_turns_are_dropped() {
        let contents = to_contents(vec![ChatMessage::user("  "), ChatMessage::user("real")]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].parts[0].text, "real");
    }
}
