//! Built-in tools.
//!
//! Everything here declares its [`Effect`] honestly — that declaration is what
//! the approval gate acts on, so an over-permissive one is a security bug, not
//! a UX detail.

use std::sync::Arc;

use crate::{
    Result,
    tools::{Effect, Tool, ToolRegistry, ToolSpec},
};

/// Reports the current local time.
///
/// Trivial, but genuinely useful: models have no clock, and it exercises the
/// full call/result round trip without touching anything.
pub struct CurrentTime;

#[async_trait::async_trait]
impl Tool for CurrentTime {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "current_time".to_string(),
            description: "Get the current date and time on the user's machine. \
                          Use this whenever the answer depends on what time it is now."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    async fn call(&self, _arguments: serde_json::Value) -> Result<serde_json::Value> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);

        Ok(serde_json::json!({
            "unix_seconds": now,
            // RFC-3339-ish without pulling in a date library; the model reads
            // the unix value anyway.
            "note": "unix_seconds is seconds since 1970-01-01T00:00:00Z"
        }))
    }
}

/// The tools available with no configuration.
///
/// Read-only only. Anything that writes, sends or executes is registered
/// explicitly by the shell, so it goes through the approval gate deliberately
/// rather than by default.
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CurrentTime));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{AutoApprove, ToolCall};

    #[tokio::test]
    async fn current_time_returns_a_plausible_timestamp() {
        let output = CurrentTime.call(serde_json::json!({})).await.unwrap();
        let seconds = output["unix_seconds"].as_u64().unwrap();

        // Sometime after 2020 and before 2100 — enough to catch a unit mixup
        // without making the test time-sensitive.
        assert!(seconds > 1_577_836_800, "got {seconds}");
        assert!(seconds < 4_102_444_800, "got {seconds}");
    }

    #[test]
    fn current_time_is_read_only() {
        // If this ever flips, it starts prompting the user for a clock read.
        assert_eq!(CurrentTime.effect(), Effect::ReadOnly);
    }

    #[test]
    fn the_default_registry_contains_only_read_only_tools() {
        let registry = default_registry();
        for spec in registry.specs() {
            let tool = registry.get(&spec.name).unwrap();
            assert_eq!(
                tool.effect(),
                Effect::ReadOnly,
                "{} is registered by default but is not read-only",
                spec.name
            );
        }
    }

    #[tokio::test]
    async fn default_tools_run_through_the_registry() {
        let registry = default_registry();
        let outcome = registry
            .invoke(
                &ToolCall {
                    id: "call-1".to_string(),
                    name: "current_time".to_string(),
                    arguments: serde_json::json!({}),
                },
                &AutoApprove,
            )
            .await;

        assert!(outcome.ok);
        assert!(outcome.output["unix_seconds"].is_number());
    }
}
