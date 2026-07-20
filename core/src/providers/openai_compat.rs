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
    providers::{ChatRequest, ChatTransport, LOCAL_PLACEHOLDER_KEY, ModelInfo, ProviderConfig},
    tools::{ToolCall, ToolSpec},
};

/// A tool call as it appears on an assistant message in OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    /// A JSON string, not an object — the API sends arguments encoded.
    pub arguments: String,
}

/// A chat message in OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Present on an assistant turn that requested tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    /// Present on a `tool` turn, pairing a result to its call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    fn plain(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::plain("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::plain("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain("assistant", content)
    }

    /// The assistant turn recording which tools the model asked for.
    ///
    /// Required in the follow-up request: omitting it makes the subsequent
    /// `tool` messages reference calls the API has no record of.
    pub fn tool_requests(calls: Vec<WireToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: calls,
            tool_call_id: None,
        }
    }

    /// The result of one tool call, fed back to the model.
    pub fn tool_result(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: output.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Serialize)]
struct CompletionsBody<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Must be a nested object, not a flattened field — servers silently
    /// ignore an unrecognised top-level key and report no usage at all.
    stream_options: StreamOptions,
    /// Omitted entirely when empty: some servers reject an empty array.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// A tool declaration in OpenAI wire format.
#[derive(Debug, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolFunction,
}

#[derive(Debug, Serialize)]
struct WireToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl From<ToolSpec> for WireTool {
    fn from(spec: ToolSpec) -> Self {
        Self {
            kind: "function",
            function: WireToolFunction {
                name: spec.name,
                description: spec.description,
                parameters: spec.parameters,
            },
        }
    }
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
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

/// One fragment of a tool call.
///
/// The API streams these piecemeal: `index` identifies which call a fragment
/// belongs to, `id` and `name` usually arrive once on the first fragment, and
/// `arguments` accumulates across many.
#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Reassembles streamed tool-call fragments.
///
/// Keyed by index and ordered, so calls come out in the order the model asked
/// for them regardless of how the fragments interleave.
#[derive(Debug, Default)]
pub(crate) struct ToolCallAccumulator {
    slots: std::collections::BTreeMap<u32, PartialCall>,
}

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn absorb(&mut self, deltas: &[ToolCallDelta]) {
        for delta in deltas {
            let slot = self.slots.entry(delta.index).or_default();

            if let Some(id) = &delta.id {
                slot.id = id.clone();
            }
            if let Some(function) = &delta.function {
                if let Some(name) = &function.name {
                    slot.name.push_str(name);
                }
                if let Some(arguments) = &function.arguments {
                    slot.arguments.push_str(arguments);
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Finished calls, ready to execute.
    ///
    /// Arguments that fail to parse become an empty object rather than
    /// aborting: the tool will reject them with a message the model can act
    /// on, which beats killing the run.
    fn finish(&mut self) -> Vec<ToolCall> {
        std::mem::take(&mut self.slots)
            .into_values()
            .filter(|partial| !partial.name.is_empty())
            .map(|partial| ToolCall {
                id: if partial.id.is_empty() {
                    partial.name.clone()
                } else {
                    partial.id
                },
                name: partial.name,
                arguments: serde_json::from_str(&partial.arguments)
                    .unwrap_or_else(|_| serde_json::json!({})),
            })
            .collect()
    }
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

/// What one SSE payload contributes.
#[derive(Debug, Default)]
struct ParsedChunk {
    /// Assistant text, absent for role-only openers and usage trailers.
    text: Option<String>,
    usage: Option<Usage>,
    tool_calls: Vec<ToolCallDelta>,
    /// `"tool_calls"` signals the model finished asking for tools.
    finish_reason: Option<String>,
}

/// Interpret one SSE payload. Pure and synchronous, so the wire shapes are
/// testable without a server.
fn parse_chunk(data: &str) -> Result<ParsedChunk> {
    let chunk: ChatChunk = serde_json::from_str(data)?;

    let mut parsed = ParsedChunk {
        usage: chunk.usage.map(Usage::from),
        ..Default::default()
    };

    for choice in chunk.choices {
        if let Some(content) = choice.delta.content
            && !content.is_empty()
        {
            parsed.text = Some(content);
        }
        if !choice.delta.tool_calls.is_empty() {
            parsed.tool_calls = choice.delta.tool_calls;
        }
        if choice.finish_reason.is_some() {
            parsed.finish_reason = choice.finish_reason;
        }
    }

    Ok(parsed)
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
    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<'static, EngineEvent>> {
        let url = self.url("chat/completions");
        let body = CompletionsBody {
            model: &request.model,
            messages: &request.messages,
            stream: true,
            temperature: request.temperature,
            stream_options: StreamOptions {
                include_usage: true,
            },
            tools: request.tools.into_iter().map(WireTool::from).collect(),
        };

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
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

        // Usage arrives in a trailing chunk *before* `[DONE]`, and tool-call
        // fragments accumulate across many chunks, so both live in scan state.
        #[derive(Default)]
        struct StreamState {
            finished: bool,
            usage: Option<Usage>,
            calls: ToolCallAccumulator,
        }

        // scan yields a stream per chunk, flattened below: one chunk can
        // produce several events, since a completion may request multiple
        // tools at once.
        let stream = events
            .scan(StreamState::default(), |state, item| {
                if state.finished {
                    return futures::future::ready(None);
                }

                let events: Vec<EngineEvent> = match item {
                    Err(error) => {
                        state.finished = true;
                        vec![EngineEvent::Error {
                            message: error.to_string(),
                        }]
                    }
                    Ok(event) => {
                        if event.data.trim() == "[DONE]" {
                            state.finished = true;
                            // Any tool calls still buffered belong to this
                            // completion; emit them before terminating.
                            let mut out: Vec<EngineEvent> = state
                                .calls
                                .finish()
                                .into_iter()
                                .map(|call| EngineEvent::ToolCall {
                                    id: call.id,
                                    name: call.name,
                                    args: call.arguments,
                                })
                                .collect();
                            out.push(EngineEvent::Done { usage: state.usage });
                            out
                        } else {
                            match parse_chunk(&event.data) {
                                Ok(parsed) => {
                                    if parsed.usage.is_some() {
                                        state.usage = parsed.usage;
                                    }
                                    state.calls.absorb(&parsed.tool_calls);

                                    let mut out = Vec::new();
                                    if let Some(text) = parsed.text {
                                        out.push(EngineEvent::Token { text });
                                    }

                                    // finish_reason "tool_calls" closes the
                                    // set; flush before any further content.
                                    if parsed.finish_reason.as_deref() == Some("tool_calls")
                                        && !state.calls.is_empty()
                                    {
                                        out.extend(state.calls.finish().into_iter().map(|call| {
                                            EngineEvent::ToolCall {
                                                id: call.id,
                                                name: call.name,
                                                args: call.arguments,
                                            }
                                        }));
                                    }
                                    out
                                }
                                Err(error) => {
                                    state.finished = true;
                                    vec![EngineEvent::Error {
                                        message: format!("malformed stream chunk: {error}"),
                                    }]
                                }
                            }
                        }
                    }
                };

                futures::future::ready(Some(futures::stream::iter(events)))
            })
            .flatten();

        Ok(stream.boxed())
    }

    fn supports_tools(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_content_delta() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(parse_chunk(data).unwrap().text.as_deref(), Some("Hello"));
    }

    #[test]
    fn role_opener_yields_no_text() {
        let data = r#"{"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(parse_chunk(data).unwrap().text.is_none());
    }

    #[test]
    fn empty_content_is_treated_as_no_text() {
        let data = r#"{"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_chunk(data).unwrap().text.is_none());
    }

    #[test]
    fn usage_trailer_is_captured() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let parsed = parse_chunk(data).unwrap();
        assert!(parsed.text.is_none());
        let usage = parsed.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn malformed_chunk_is_an_error() {
        assert!(parse_chunk("not json").is_err());
    }

    /// Arguments arrive split across chunks and must be concatenated in
    /// order, not overwritten.
    #[test]
    fn tool_call_fragments_reassemble_into_one_call() {
        let chunks = [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a",
                "function":{"name":"web_fetch","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,
                "function":{"arguments":"{\"url\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,
                "function":{"arguments":"\"https://example.com\"}"}}]}}]}"#,
        ];

        let mut accumulator = ToolCallAccumulator::default();
        for chunk in chunks {
            accumulator.absorb(&parse_chunk(chunk).unwrap().tool_calls);
        }

        let calls = accumulator.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "web_fetch");
        assert_eq!(calls[0].arguments["url"], "https://example.com");
    }

    /// Two calls in one completion interleave by index; each must stay whole.
    #[test]
    fn parallel_tool_calls_stay_separate_and_ordered() {
        let chunks = [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a",
                "function":{"name":"first","arguments":"{\"x\":1}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b",
                "function":{"name":"second","arguments":"{\"y\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,
                "function":{"arguments":"2}"}}]}}]}"#,
        ];

        let mut accumulator = ToolCallAccumulator::default();
        for chunk in chunks {
            accumulator.absorb(&parse_chunk(chunk).unwrap().tool_calls);
        }

        let calls = accumulator.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "first");
        assert_eq!(calls[0].arguments["x"], 1);
        assert_eq!(calls[1].name, "second");
        assert_eq!(calls[1].arguments["y"], 2);
    }

    /// A truncated argument string must not kill the run — the tool rejects
    /// the empty object with a message the model can act on.
    #[test]
    fn unparseable_arguments_degrade_to_an_empty_object() {
        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a",
            "function":{"name":"broken","arguments":"{not json"}}]}}]}"#;

        let mut accumulator = ToolCallAccumulator::default();
        accumulator.absorb(&parse_chunk(chunk).unwrap().tool_calls);

        let calls = accumulator.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, serde_json::json!({}));
    }

    #[test]
    fn a_call_without_an_id_falls_back_to_its_name() {
        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,
            "function":{"name":"anonymous","arguments":"{}"}}]}}]}"#;

        let mut accumulator = ToolCallAccumulator::default();
        accumulator.absorb(&parse_chunk(chunk).unwrap().tool_calls);
        assert_eq!(accumulator.finish()[0].id, "anonymous");
    }

    #[test]
    fn finish_reason_is_surfaced() {
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        assert_eq!(
            parse_chunk(chunk).unwrap().finish_reason.as_deref(),
            Some("tool_calls")
        );
    }

    #[test]
    fn tools_are_serialized_in_openai_shape() {
        let wire = WireTool::from(ToolSpec {
            name: "web_fetch".to_string(),
            description: "fetch a url".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        });

        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "web_fetch");
        assert_eq!(json["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn plain_messages_omit_tool_fields_entirely() {
        // Servers reject unexpected nulls, so absent means absent.
        let json = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn a_tool_result_message_carries_its_call_id() {
        let json = serde_json::to_value(ChatMessage::tool_result("call_a", "{}")).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_a");
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
