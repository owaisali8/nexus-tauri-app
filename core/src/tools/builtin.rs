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

/// Writes a note into the workspace's notes directory.
///
/// Deliberately side-effecting: it creates a file, so every call goes through
/// the approval gate. Also the first tool that proves the gate works.
pub struct WriteNote {
    /// Root the tool may write inside. Every path is resolved against it and
    /// rejected if it escapes.
    root: std::path::PathBuf,
}

impl WriteNote {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a caller-supplied name to a path inside the root.
    ///
    /// Rejects separators and parent segments outright rather than trying to
    /// canonicalize: the file does not exist yet, so canonicalization cannot
    /// be relied on to catch traversal.
    fn resolve(&self, name: &str) -> Result<std::path::PathBuf> {
        let trimmed = name.trim();

        if trimmed.is_empty() {
            return Err(crate::Error::Invalid("a file name is required".to_string()));
        }
        if trimmed.contains(['/', '\\'])
            || trimmed.contains("..")
            || std::path::Path::new(trimmed).is_absolute()
        {
            return Err(crate::Error::Invalid(format!(
                "`{trimmed}` must be a plain file name, without directories or `..`"
            )));
        }

        Ok(self.root.join(trimmed))
    }
}

#[async_trait::async_trait]
impl Tool for WriteNote {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_note".to_string(),
            description: "Save a text note to the user's notes folder. \
                          Requires the user's approval before it runs."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "File name, e.g. meeting.md. No directories."
                    },
                    "content": {
                        "type": "string",
                        "description": "The note's full text."
                    }
                },
                "required": ["name", "content"]
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::SideEffecting
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<serde_json::Value> {
        let name = arguments
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| crate::Error::Invalid("`name` is required".to_string()))?;
        let content = arguments
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| crate::Error::Invalid("`content` is required".to_string()))?;

        let path = self.resolve(name)?;

        std::fs::create_dir_all(&self.root).map_err(|error| {
            crate::Error::Invalid(format!("could not create the notes folder: {error}"))
        })?;
        std::fs::write(&path, content)
            .map_err(|error| crate::Error::Invalid(format!("could not write the note: {error}")))?;

        Ok(serde_json::json!({
            "written": path.display().to_string(),
            "bytes": content.len(),
        }))
    }
}

/// Tools available with no configuration.
///
/// Read-only only. Anything that writes, sends or executes needs a path from
/// the shell, and is registered by [`registry_with_notes`].
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CurrentTime));
    registry
}

/// The default tools plus note writing, rooted at `notes_dir`.
pub fn registry_with_notes(notes_dir: impl Into<std::path::PathBuf>) -> ToolRegistry {
    let mut registry = default_registry();
    registry.register(Arc::new(WriteNote::new(notes_dir)));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{AutoApprove, RunContext, ToolCall};

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

    fn temp_root(label: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("essentio-notes-{label}-{}", std::process::id()));
        path
    }

    #[test]
    fn write_note_is_side_effecting_so_it_is_always_gated() {
        assert_eq!(
            WriteNote::new(temp_root("effect")).effect(),
            Effect::SideEffecting
        );
    }

    #[tokio::test]
    async fn write_note_saves_inside_its_root() {
        let root = temp_root("write");
        let tool = WriteNote::new(&root);

        let output = tool
            .call(serde_json::json!({ "name": "note.md", "content": "hello" }))
            .await
            .unwrap();

        let written = std::path::PathBuf::from(output["written"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello");
        assert!(written.starts_with(&root));

        std::fs::remove_dir_all(&root).ok();
    }

    /// Path traversal is the obvious attack on a model-supplied file name.
    #[tokio::test]
    async fn write_note_refuses_to_escape_its_root() {
        let root = temp_root("escape");
        let tool = WriteNote::new(&root);

        for name in [
            "../outside.md",
            "sub/dir.md",
            "..\\windows.md",
            "/etc/passwd",
            "C:\\Windows\\evil.md",
            "",
            "   ",
        ] {
            let result = tool
                .call(serde_json::json!({ "name": name, "content": "x" }))
                .await;
            assert!(result.is_err(), "`{name}` should have been rejected");
        }

        assert!(
            !root.exists(),
            "a rejected write must not create the notes folder"
        );
    }

    #[tokio::test]
    async fn write_note_reports_missing_arguments() {
        let tool = WriteNote::new(temp_root("args"));
        assert!(tool.call(serde_json::json!({})).await.is_err());
        assert!(
            tool.call(serde_json::json!({ "name": "a.md" }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_denied_write_never_touches_the_disk() {
        let root = temp_root("denied");
        let mut registry = default_registry();
        registry.register(Arc::new(WriteNote::new(&root)));

        let outcome = registry
            .invoke(
                &RunContext::new("s1", "r1"),
                &ToolCall {
                    id: "call-1".to_string(),
                    name: "write_note".to_string(),
                    arguments: serde_json::json!({ "name": "x.md", "content": "x" }),
                },
                &crate::tools::DenyAll,
            )
            .await;

        assert!(!outcome.ok);
        assert!(!root.exists(), "the tool ran despite being denied");
    }

    #[tokio::test]
    async fn default_tools_run_through_the_registry() {
        let registry = default_registry();
        let outcome = registry
            .invoke(
                &RunContext::new("session-1", "run-1"),
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
