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

pub fn default_folders() -> Vec<WorkspaceFolder> {
    vec![
        folder("resumes", "Resumes", 3),
        folder("cover-letters", "Cover Letters", 5),
        folder("job-research", "Job Research", 12),
        folder("agent-memory", "Agent Memory", 8),
    ]
}

pub fn default_notes() -> Vec<Note> {
    vec![Note {
        id: "note-job-search-profile".to_string(),
        folder_id: "job-research".to_string(),
        title: "Job search profile".to_string(),
        body: "Prefer frontend, Rust desktop, and AI tooling roles. Require human approval before submission.".to_string(),
        updated_at: 0,
    }]
}

fn folder(id: &str, name: &str, items: usize) -> WorkspaceFolder {
    WorkspaceFolder {
        id: id.to_string(),
        name: name.to_string(),
        items,
    }
}
