//! Tool definitions, execution, and the approval gate.
//!
//! A tool is anything the model can invoke: a built-in like web-fetch, or a
//! function exposed by an MCP server. Tools declare whether invoking them has
//! side effects; those calls are gated behind explicit user approval before
//! they run, never after.

pub mod builtin;

use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::Result;

/// A tool as advertised to the model.
///
/// `parameters` is a JSON Schema object. Providers each wrap it differently,
/// so the transports own that translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A model's request to invoke a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// Provider-assigned id, used to pair the result back to the call.
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The outcome of running a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    pub id: String,
    pub ok: bool,
    pub output: serde_json::Value,
}

impl ToolOutcome {
    pub fn success(id: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            output,
        }
    }

    /// A failed call still returns a result to the model rather than aborting
    /// the run, so it can apologise or try a different approach.
    pub fn failure(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ok: false,
            output: serde_json::json!({ "error": message.into() }),
        }
    }
}

/// Whether a tool's effects are confined to reading.
///
/// This drives the approval gate, so the conservative value is the right
/// default for anything uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Observes without changing anything the user would miss.
    ReadOnly,
    /// Writes, deletes, sends, spends, or executes. Always gated.
    SideEffecting,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// Defaults to [`Effect::SideEffecting`]: a tool that forgets to declare
    /// itself gets gated rather than silently running unattended.
    fn effect(&self) -> Effect {
        Effect::SideEffecting
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<serde_json::Value>;
}

/// Whether a gated call may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approve,
    Deny,
}

/// Asks the user to approve a side-effecting call.
///
/// `core` does not know how approval is obtained; the shell implements this
/// against whatever UI exists.
#[async_trait::async_trait]
pub trait ApprovalGate: Send + Sync {
    async fn request(&self, call: &ToolCall) -> Approval;
}

/// Approves everything without asking.
///
/// For tests and for runs with no side-effecting tools registered. Never wire
/// this to a session where a user is present.
pub struct AutoApprove;

#[async_trait::async_trait]
impl ApprovalGate for AutoApprove {
    async fn request(&self, _call: &ToolCall) -> Approval {
        Approval::Approve
    }
}

/// Denies everything. The safe default when no gate has been supplied.
pub struct DenyAll;

#[async_trait::async_trait]
impl ApprovalGate for DenyAll {
    async fn request(&self, _call: &ToolCall) -> Approval {
        Approval::Deny
    }
}

/// The tools available to a run.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.spec().name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Specs for every registered tool, in a stable order.
    ///
    /// Sorted because a HashMap's iteration order varies between runs, and an
    /// unstable tool list perturbs prompt caching for no reason.
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.tools.values().map(|tool| tool.spec()).collect();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    /// Only the tools named in `ids`, for runs that enable a subset.
    pub fn subset(&self, ids: &[String]) -> Self {
        Self {
            tools: ids
                .iter()
                .filter_map(|id| self.tools.get(id).map(|tool| (id.clone(), tool.clone())))
                .collect(),
        }
    }

    /// Run a call, gating it on approval when the tool has side effects.
    ///
    /// Always returns a [`ToolOutcome`]: a denial or a failure is reported to
    /// the model rather than ending the run.
    pub async fn invoke(&self, call: &ToolCall, gate: &dyn ApprovalGate) -> ToolOutcome {
        let Some(tool) = self.get(&call.name) else {
            return ToolOutcome::failure(&call.id, format!("no such tool: {}", call.name));
        };

        if tool.effect() == Effect::SideEffecting && gate.request(call).await == Approval::Deny {
            return ToolOutcome::failure(&call.id, "the user declined this call");
        }

        match tool.call(call.arguments.clone()).await {
            Ok(output) => ToolOutcome::success(&call.id, output),
            Err(error) => ToolOutcome::failure(&call.id, error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter {
        name: &'static str,
        effect: Effect,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Tool for Counter {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.to_string(),
                description: "test".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            }
        }

        fn effect(&self) -> Effect {
            self.effect
        }

        async fn call(&self, _arguments: serde_json::Value) -> Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    struct Failing;

    #[async_trait::async_trait]
    impl Tool for Failing {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "failing".to_string(),
                description: "always fails".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            }
        }

        fn effect(&self) -> Effect {
            Effect::ReadOnly
        }

        async fn call(&self, _arguments: serde_json::Value) -> Result<serde_json::Value> {
            Err(crate::Error::Invalid("boom".to_string()))
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    fn registry(effect: Effect) -> (ToolRegistry, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Counter {
            name: "counter",
            effect,
            calls: calls.clone(),
        }));
        (registry, calls)
    }

    #[tokio::test]
    async fn read_only_tools_run_without_asking() {
        let (registry, calls) = registry(Effect::ReadOnly);
        // DenyAll would block anything gated; a read-only tool must not be.
        let outcome = registry.invoke(&call("counter"), &DenyAll).await;

        assert!(outcome.ok);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn side_effecting_tools_do_not_run_when_denied() {
        let (registry, calls) = registry(Effect::SideEffecting);
        let outcome = registry.invoke(&call("counter"), &DenyAll).await;

        assert!(!outcome.ok);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a denied tool must not execute"
        );
    }

    #[tokio::test]
    async fn side_effecting_tools_run_once_approved() {
        let (registry, calls) = registry(Effect::SideEffecting);
        let outcome = registry.invoke(&call("counter"), &AutoApprove).await;

        assert!(outcome.ok);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_unknown_tool_reports_back_instead_of_panicking() {
        let (registry, _) = registry(Effect::ReadOnly);
        let outcome = registry.invoke(&call("nope"), &AutoApprove).await;

        assert!(!outcome.ok);
        assert!(outcome.output.to_string().contains("no such tool"));
    }

    #[tokio::test]
    async fn a_failing_tool_returns_an_error_outcome() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Failing));

        let outcome = registry.invoke(&call("failing"), &AutoApprove).await;
        assert!(!outcome.ok);
        assert!(outcome.output.to_string().contains("boom"));
    }

    #[test]
    fn specs_are_ordered_so_the_tool_list_is_stable() {
        let mut registry = ToolRegistry::new();
        for name in ["zebra", "alpha", "middle"] {
            registry.register(Arc::new(Counter {
                name: Box::leak(name.to_string().into_boxed_str()),
                effect: Effect::ReadOnly,
                calls: Arc::new(AtomicUsize::new(0)),
            }));
        }

        let names: Vec<String> = registry.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["alpha", "middle", "zebra"]);
    }

    #[test]
    fn subset_keeps_only_the_named_tools() {
        let (registry, _) = registry(Effect::ReadOnly);
        assert!(
            registry
                .subset(&["counter".to_string()])
                .get("counter")
                .is_some()
        );
        assert!(registry.subset(&["other".to_string()]).is_empty());
    }
}
