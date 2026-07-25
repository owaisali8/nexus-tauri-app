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
        ChatRequest, ChatTransport, ModelInfo, ProviderConfig, openai_compat::ChatMessage,
        split_system,
    },
    tools::{ToolCall, ToolSpec},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    /// Gemini nests declarations under a `tools` array, unlike OpenAI's flat
    /// list of function objects.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTools>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTools {
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl From<ToolSpec> for FunctionDeclaration {
    fn from(spec: ToolSpec) -> Self {
        Self {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

/// One part of a Gemini message.
///
/// Untagged so each variant serializes as the bare object the API expects,
/// rather than being wrapped in a discriminant.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum GeminiPart {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    FunctionCall {
        function_call: FunctionCallPayload,
    },
    #[serde(rename_all = "camelCase")]
    FunctionResponse {
        function_response: FunctionResponsePayload,
    },
}

impl GeminiPart {
    fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// The text of this part, for tests and for merging adjacent turns.
    fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    fn push_text(&mut self, extra: &str) {
        if let Self::Text { text } = self {
            text.push_str(extra);
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionCallPayload {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionResponsePayload {
    name: String,
    /// Gemini expects an object here, not a bare string.
    response: serde_json::Value,
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
    /// The model asked for one or more tools.
    Calls(Vec<ToolCall>),
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
        let parts = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array());

        // Function calls take precedence: a chunk carrying one may also carry
        // filler text, and dropping the call would strand the run.
        if let Some(parts) = parts {
            let calls: Vec<ToolCall> = parts
                .iter()
                .filter_map(|part| part.get("functionCall"))
                .filter_map(|call| {
                    let name = call.get("name")?.as_str()?.to_string();
                    Some(ToolCall {
                        // Gemini does not assign call ids, so the name is the
                        // only stable handle for pairing a result back.
                        id: name.clone(),
                        name,
                        arguments: call
                            .get("args")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    })
                })
                .collect();

            if !calls.is_empty() {
                return Ok(Chunk::Calls(calls));
            }
        }

        let text: String = parts
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
/// Maps `assistant` to `model`, drops empty turns, merges adjacent text turns
/// of the same role, and translates OpenAI-shaped tool traffic into Gemini's
/// functionCall / functionResponse parts.
pub(crate) fn to_contents(messages: Vec<ChatMessage>) -> Vec<GeminiContent> {
    let mut out: Vec<GeminiContent> = Vec::new();

    for message in messages {
        // An assistant turn requesting tools.
        if !message.tool_calls.is_empty() {
            out.push(GeminiContent {
                role: Some("model".to_string()),
                parts: message
                    .tool_calls
                    .iter()
                    .map(|call| GeminiPart::FunctionCall {
                        function_call: FunctionCallPayload {
                            name: call.function.name.clone(),
                            // Arguments travel as a JSON string in the OpenAI
                            // shape; Gemini wants the decoded object.
                            args: serde_json::from_str(&call.function.arguments)
                                .unwrap_or_else(|_| serde_json::json!({})),
                        },
                    })
                    .collect(),
            });
            continue;
        }

        // A tool result.
        if let Some(call_id) = &message.tool_call_id {
            out.push(GeminiContent {
                // Gemini expects results on a user turn, not a dedicated role.
                role: Some("user".to_string()),
                parts: vec![GeminiPart::FunctionResponse {
                    function_response: FunctionResponsePayload {
                        // Our Gemini calls use the tool name as the id, so
                        // this round-trips correctly.
                        name: call_id.clone(),
                        response: serde_json::from_str(&message.content)
                            .unwrap_or_else(|_| serde_json::json!({ "result": message.content })),
                    },
                }],
            });
            continue;
        }

        if message.content.trim().is_empty() {
            continue;
        }

        let role = if message.role == "assistant" {
            "model"
        } else {
            "user"
        };

        match out.last_mut() {
            // Only merge when the previous turn ends in text; appending to a
            // functionCall turn would corrupt it.
            Some(previous)
                if previous.role.as_deref() == Some(role)
                    && previous.parts.last().is_some_and(|p| p.as_text().is_some()) =>
            {
                if let Some(part) = previous.parts.last_mut() {
                    part.push_text("\n\n");
                    part.push_text(&message.content);
                }
            }
            _ => out.push(GeminiContent {
                role: Some(role.to_string()),
                parts: vec![GeminiPart::text(message.content)],
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

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<'static, EngineEvent>> {
        let declarations: Vec<FunctionDeclaration> = request
            .tools
            .iter()
            .cloned()
            .map(FunctionDeclaration::from)
            .collect();

        let model = request.model.clone();
        let temperature = request.temperature;
        let (system, rest) = split_system(request.messages);
        let contents = to_contents(rest);

        if contents.is_empty() {
            return Err(Error::Invalid(
                "a request needs at least one non-empty user message".to_string(),
            ));
        }

        let path = format!(
            "v1beta/models/{}:streamGenerateContent?alt=sse",
            short_model_name(&model)
        );

        let response = self
            .request(reqwest::Method::POST, &path)
            .json(&GenerateRequest {
                contents,
                system_instruction: system.map(|text| GeminiContent {
                    // systemInstruction carries no role.
                    role: None,
                    parts: vec![GeminiPart::text(text)],
                }),
                generation_config: temperature.map(|value| GenerationConfig {
                    temperature: Some(value),
                }),
                tools: if declarations.is_empty() {
                    Vec::new()
                } else {
                    vec![GeminiTools {
                        function_declarations: declarations,
                    }]
                },
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

        // scan yields a stream per chunk, flattened below: one chunk can carry
        // several function calls, so the mapping is not one-to-one.
        let stream = response
            .bytes_stream()
            .eventsource()
            .scan(State::default(), |state, item| {
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
                    Ok(event) => match parse_chunk(&event.data) {
                        Ok(Chunk::Text(text)) => vec![EngineEvent::Token { text }],
                        Ok(Chunk::Calls(calls)) => calls
                            .into_iter()
                            .map(|call| EngineEvent::ToolCall {
                                id: call.id,
                                name: call.name,
                                args: call.arguments,
                            })
                            .collect(),
                        Ok(Chunk::Usage(usage)) => {
                            state.usage = Some(usage);
                            Vec::new()
                        }
                        Ok(Chunk::Blocked(message)) => {
                            state.finished = true;
                            vec![EngineEvent::Error { message }]
                        }
                        Ok(Chunk::Ignore) => Vec::new(),
                        Err(error) => {
                            state.finished = true;
                            vec![EngineEvent::Error {
                                message: format!("malformed stream chunk: {error}"),
                            }]
                        }
                    },
                };

                futures::future::ready(Some(futures::stream::iter(events)))
            })
            .flatten();

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
        assert_eq!(contents[0].parts[0].as_text(), Some("first\n\nsecond"));
    }

    #[test]
    fn function_calls_are_parsed_from_a_candidate() {
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[
            {"functionCall":{"name":"current_time","args":{}}}]}}]}"#;

        match parse_chunk(data).unwrap() {
            Chunk::Calls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "current_time");
                // Gemini assigns no call id, so the name stands in.
                assert_eq!(calls[0].id, "current_time");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn several_function_calls_in_one_chunk_are_all_returned() {
        let data = r#"{"candidates":[{"content":{"parts":[
            {"functionCall":{"name":"a","args":{"x":1}}},
            {"functionCall":{"name":"b","args":{"y":2}}}]}}]}"#;

        match parse_chunk(data).unwrap() {
            Chunk::Calls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].arguments["x"], 1);
                assert_eq!(calls[1].arguments["y"], 2);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// A chunk mixing prose and a call must not lose the call.
    #[test]
    fn a_call_wins_over_text_in_the_same_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[
            {"text":"let me check"},
            {"functionCall":{"name":"current_time","args":{}}}]}}]}"#;

        assert!(matches!(parse_chunk(data).unwrap(), Chunk::Calls(_)));
    }

    #[test]
    fn tool_declarations_are_nested_under_tools() {
        let request = GenerateRequest {
            contents: vec![],
            system_instruction: None,
            generation_config: None,
            tools: vec![GeminiTools {
                function_declarations: vec![FunctionDeclaration::from(ToolSpec {
                    name: "current_time".to_string(),
                    description: "the time".to_string(),
                    parameters: serde_json::json!({ "type": "object" }),
                })],
            }],
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            json["tools"][0]["functionDeclarations"][0]["name"],
            "current_time"
        );
    }

    #[test]
    fn an_assistant_tool_request_becomes_a_function_call_part() {
        let contents = to_contents(vec![ChatMessage::tool_requests(vec![
            crate::providers::WireToolCall {
                id: "current_time".to_string(),
                kind: "function".to_string(),
                function: crate::providers::WireFunction {
                    name: "current_time".to_string(),
                    arguments: "{\"tz\":\"utc\"}".to_string(),
                },
            },
        ])]);

        let json = serde_json::to_value(&contents).unwrap();
        assert_eq!(json[0]["role"], "model");
        assert_eq!(json[0]["parts"][0]["functionCall"]["name"], "current_time");
        // Arguments arrive as a JSON string and must be decoded for Gemini.
        assert_eq!(json[0]["parts"][0]["functionCall"]["args"]["tz"], "utc");
    }

    #[test]
    fn a_tool_result_becomes_a_function_response_part() {
        let contents = to_contents(vec![ChatMessage::tool_result(
            "current_time",
            r#"{"unix_seconds":123}"#,
        )]);

        let json = serde_json::to_value(&contents).unwrap();
        assert_eq!(json[0]["role"], "user");
        assert_eq!(
            json[0]["parts"][0]["functionResponse"]["name"],
            "current_time"
        );
        assert_eq!(
            json[0]["parts"][0]["functionResponse"]["response"]["unix_seconds"],
            123
        );
    }

    /// Gemini requires an object; a plain-string result must still be valid.
    #[test]
    fn a_non_object_tool_result_is_wrapped() {
        let contents = to_contents(vec![ChatMessage::tool_result("t", "just text")]);
        let json = serde_json::to_value(&contents).unwrap();
        assert_eq!(
            json[0]["parts"][0]["functionResponse"]["response"]["result"],
            "just text"
        );
    }

    /// Merging text into a functionCall turn would corrupt it.
    #[test]
    fn text_is_not_merged_into_a_function_call_turn() {
        let contents = to_contents(vec![
            ChatMessage::tool_requests(vec![crate::providers::WireToolCall {
                id: "t".to_string(),
                kind: "function".to_string(),
                function: crate::providers::WireFunction {
                    name: "t".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            ChatMessage::assistant("and here is the answer"),
        ]);

        assert_eq!(contents.len(), 2, "the turns must stay separate");
    }

    #[test]
    fn empty_turns_are_dropped() {
        let contents = to_contents(vec![ChatMessage::user("  "), ChatMessage::user("real")]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].parts[0].as_text(), Some("real"));
    }
}
