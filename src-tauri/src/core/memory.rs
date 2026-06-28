use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFolder {
    pub id: String,
    pub name: String,
    pub items: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub folder_id: String,
    pub title: String,
    pub body: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRecord {
    pub id: String,
    pub name: String,
    pub path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub folder_id: Option<String>,
    pub summary: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub key: String,
    pub value: String,
    pub source: MemorySource,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemorySource {
    User,
    Agent,
    File,
}

pub fn default_folders() -> Vec<WorkspaceFolder> {
    vec![
        folder("resumes", "Resumes", 0),
        folder("cover-letters", "Cover Letters", 0),
        folder("job-research", "Job Research", 0),
        folder("agent-memory", "Agent Memory", 0),
    ]
}

pub fn default_notes() -> Vec<Note> {
    Vec::new()
}

fn folder(id: &str, name: &str, items: usize) -> WorkspaceFolder {
    WorkspaceFolder {
        id: id.to_string(),
        name: name.to_string(),
        items,
    }
}

pub fn role_to_str(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
    }
}

pub fn role_from_str(role: &str) -> MessageRole {
    match role {
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => MessageRole::User,
    }
}

pub fn source_to_str(source: &MemorySource) -> &'static str {
    match source {
        MemorySource::User => "user",
        MemorySource::Agent => "agent",
        MemorySource::File => "file",
    }
}

pub fn source_from_str(source: &str) -> MemorySource {
    match source {
        "agent" => MemorySource::Agent,
        "file" => MemorySource::File,
        _ => MemorySource::User,
    }
}
