//! Translation from ADK events to [`EngineEvent`].
//!
//! Kept separate from the engine so the mapping is unit-testable without a
//! runner, a model, or a network.

use adk_core::{Event, Part};

use crate::engine::EngineEvent;

/// Map one ADK event to zero or more [`EngineEvent`]s.
///
/// A single ADK event may carry several parts — text alongside a tool call —
/// so the result is a vec rather than an option.
pub fn to_engine_events(item: adk_core::Result<Event>) -> Vec<EngineEvent> {
    let event = match item {
        Ok(event) => event,
        Err(error) => {
            return vec![EngineEvent::Error {
                message: error.to_string(),
            }];
        }
    };

    let Some(content) = event.content() else {
        return Vec::new();
    };

    content.parts.iter().filter_map(part_to_event).collect()
}

fn part_to_event(part: &Part) -> Option<EngineEvent> {
    match part {
        Part::Text { text } if !text.is_empty() => Some(EngineEvent::Token { text: text.clone() }),

        Part::FunctionCall { name, args, id, .. } => Some(EngineEvent::ToolCall {
            // Gemini omits call ids; fall back to the tool name so the UI can
            // still pair a call with its result.
            id: id.clone().unwrap_or_else(|| name.clone()),
            name: name.clone(),
            args: args.clone(),
        }),

        Part::FunctionResponse {
            function_response,
            id,
        } => {
            let output = serde_json::to_value(function_response).unwrap_or(serde_json::Value::Null);
            Some(EngineEvent::ToolResult {
                id: id.clone().unwrap_or_default(),
                // ADK does not surface a success flag on the part itself; a
                // response that arrives at all is treated as delivered, and
                // tool-level failures show up inside the payload.
                ok: true,
                output,
            })
        }

        // Thinking traces are deliberately dropped: they are not assistant
        // output and rendering them as tokens would corrupt the transcript.
        Part::Thinking { .. } => None,

        // Empty text, inline data, file data and Gemini server-tool parts have
        // no chat representation yet.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::types::FunctionResponseData;

    #[test]
    fn text_part_becomes_a_token() {
        let event = part_to_event(&Part::Text {
            text: "hello".to_string(),
        });
        assert!(matches!(event, Some(EngineEvent::Token { text }) if text == "hello"));
    }

    #[test]
    fn empty_text_is_dropped() {
        assert!(
            part_to_event(&Part::Text {
                text: String::new()
            })
            .is_none()
        );
    }

    #[test]
    fn thinking_is_dropped() {
        let part = Part::Thinking {
            thinking: "reasoning…".to_string(),
            signature: None,
        };
        assert!(part_to_event(&part).is_none());
    }

    #[test]
    fn function_call_maps_with_id() {
        let part = Part::FunctionCall {
            name: "web_fetch".to_string(),
            args: serde_json::json!({ "url": "https://example.com" }),
            id: Some("call-7".to_string()),
            thought_signature: None,
        };
        match part_to_event(&part) {
            Some(EngineEvent::ToolCall { id, name, .. }) => {
                assert_eq!(id, "call-7");
                assert_eq!(name, "web_fetch");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn function_call_without_id_falls_back_to_name() {
        let part = Part::FunctionCall {
            name: "web_fetch".to_string(),
            args: serde_json::Value::Null,
            id: None,
            thought_signature: None,
        };
        match part_to_event(&part) {
            Some(EngineEvent::ToolCall { id, .. }) => assert_eq!(id, "web_fetch"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn function_response_maps_to_tool_result() {
        let part = Part::FunctionResponse {
            function_response: FunctionResponseData::new(
                "web_fetch",
                serde_json::json!({ "status": 200 }),
            ),
            id: Some("call-7".to_string()),
        };
        match part_to_event(&part) {
            Some(EngineEvent::ToolResult { id, ok, .. }) => {
                assert_eq!(id, "call-7");
                assert!(ok);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
