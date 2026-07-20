//! Minimal OpenAI-compatible client.
//!
//! Serves LM Studio, Ollama and vLLM over the same code path. Deliberately
//! independent of any agent framework so that basic chat works without one.

use eventsource_stream::Eventsource;
use futures::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    engine::{EngineEvent, Usage},
    providers::{ChatTransport, LOCAL_PLACEHOLDER_KEY, ModelInfo, ProviderConfig},
};

/// A chat message in OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Must be a nested object, not a flattened field — servers silently
    /// ignore an unrecognised top-level key and report no usage at all.
    stream_options: StreamOptions,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// One SSE `data:` payload from `/chat/completions` with `stream: true`.
#[derive(Debug, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: ChunkDelta,
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

impl From<ChunkUsage> for Usage {
    fn from(value: ChunkUsage) -> Self {
        Self {
            prompt_tokens: value.prompt_tokens,
            completion_tokens: value.completion_tokens,
            total_tokens: value.total_tokens,
        }
    }
}

/// Extract the assistant text delta from one SSE payload.
///
/// Returns `Ok(None)` for chunks that carry no text (role-only openers, usage
/// trailers). Pure and synchronous so it is testable without a server.
fn parse_chunk(data: &str) -> Result<(Option<String>, Option<Usage>)> {
    let chunk: ChatChunk = serde_json::from_str(data)?;
    let text = chunk
        .choices
        .into_iter()
        .find_map(|choice| choice.delta.content)
        .filter(|content| !content.is_empty());
    Ok((text, chunk.usage.map(Usage::from)))
}

/// Turn a transport failure into a message that names the likely cause.
/// Connection-refused against a local server almost always means the server
/// is not running, and saying so is worth more than the raw error.
fn transport_error(base_url: &str, error: &reqwest::Error) -> Error {
    if error.is_connect() {
        Error::Transport(format!(
            "could not reach {base_url} — is the local server running? \
             (LM Studio: Developer tab -> Start Server)"
        ))
    } else if error.is_timeout() {
        Error::Transport(format!("request to {base_url} timed out"))
    } else {
        Error::Transport(error.to_string())
    }
}

pub struct OpenAiCompatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiCompatClient {
    /// Build a client from provider config. `api_key` is the resolved secret
    /// from the keychain, or `None` for local servers that ignore auth.
    pub fn new(config: &ProviderConfig, api_key: Option<String>) -> Result<Self> {
        let base_url = config
            .effective_base_url()
            .ok_or_else(|| Error::ProviderMisconfigured {
                provider_id: config.id.clone(),
                reason: "base_url is required".to_string(),
            })?
            .to_string();

        Ok(Self {
            http: reqwest::Client::builder()
                .build()
                .map_err(|error| Error::Transport(error.to_string()))?,
            base_url,
            api_key: api_key.unwrap_or_else(|| LOCAL_PLACEHOLDER_KEY.to_string()),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

#[async_trait::async_trait]
impl ChatTransport for OpenAiCompatClient {
    /// `GET /models` — powers the model picker and the "Test connection" button.
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = self.url("models");
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|error| transport_error(&self.base_url, &error))?;

        if !response.status().is_success() {
            return Err(Error::Transport(format!(
                "{} returned {}",
                url,
                response.status()
            )));
        }

        let body: ModelsResponse = response
            .json()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(body.data)
    }

    /// `POST /chat/completions` with `stream: true`, mapped to [`EngineEvent`].
    ///
    /// The returned stream always terminates with exactly one
    /// [`EngineEvent::Done`] or [`EngineEvent::Error`].
    async fn chat_stream(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
    ) -> Result<BoxStream<'static, EngineEvent>> {
        let url = self.url("chat/completions");
        let request = ChatRequest {
            model,
            messages: &messages,
            stream: true,
            temperature,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|error| transport_error(&self.base_url, &error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "{url} returned {status}: {}",
                body.trim()
            )));
        }

        let events = response.bytes_stream().eventsource();

        // Usage arrives in a trailing chunk *before* `[DONE]`, so it has to be
        // carried in scan state and attached when the terminal event is built.
        #[derive(Default)]
        struct StreamState {
            finished: bool,
            usage: Option<Usage>,
        }

        let stream = events.scan(StreamState::default(), |state, item| {
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
                Ok(event) => {
                    if event.data.trim() == "[DONE]" {
                        state.finished = true;
                        Some(EngineEvent::Done { usage: state.usage })
                    } else {
                        match parse_chunk(&event.data) {
                            Ok((text, usage)) => {
                                if usage.is_some() {
                                    state.usage = usage;
                                }
                                // Content-free chunks (role opener, usage
                                // trailer) become empty tokens and are filtered
                                // out below, keeping the stream open.
                                Some(EngineEvent::Token {
                                    text: text.unwrap_or_default(),
                                })
                            }
                            Err(error) => {
                                state.finished = true;
                                Some(EngineEvent::Error {
                                    message: format!("malformed stream chunk: {error}"),
                                })
                            }
                        }
                    }
                }
            };

            futures::future::ready(next)
        });

        // Drop the empty placeholders introduced above.
        let stream = stream.filter(|event| {
            futures::future::ready(!matches!(event, EngineEvent::Token { text } if text.is_empty()))
        });

        Ok(stream.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_content_delta() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        let (text, _) = parse_chunk(data).unwrap();
        assert_eq!(text.as_deref(), Some("Hello"));
    }

    #[test]
    fn role_opener_yields_no_text() {
        let data = r#"{"choices":[{"delta":{"role":"assistant"}}]}"#;
        let (text, _) = parse_chunk(data).unwrap();
        assert!(text.is_none());
    }

    #[test]
    fn empty_content_is_treated_as_no_text() {
        let data = r#"{"choices":[{"delta":{"content":""}}]}"#;
        let (text, _) = parse_chunk(data).unwrap();
        assert!(text.is_none());
    }

    #[test]
    fn usage_trailer_is_captured() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let (text, usage) = parse_chunk(data).unwrap();
        assert!(text.is_none());
        let usage = usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn malformed_chunk_is_an_error() {
        assert!(parse_chunk("not json").is_err());
    }

    #[test]
    fn client_builds_from_lm_studio_defaults() {
        let client = OpenAiCompatClient::new(&ProviderConfig::lm_studio(), None).unwrap();
        assert_eq!(
            client.url("chat/completions"),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(client.api_key, LOCAL_PLACEHOLDER_KEY);
    }

    #[test]
    fn client_requires_a_base_url() {
        let mut config = ProviderConfig::lm_studio();
        config.base_url = None;
        assert!(OpenAiCompatClient::new(&config, None).is_err());
    }
}
