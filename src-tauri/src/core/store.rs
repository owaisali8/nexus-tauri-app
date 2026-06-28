use crate::core::{
    agent::{default_agents, AgentProfile},
    browser::BrowserRun,
    memory::{default_folders, default_notes, Note, WorkspaceFolder},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const STORE_FILE: &str = "essentio-state.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub agents: Vec<AgentProfile>,
    pub folders: Vec<WorkspaceFolder>,
    pub notes: Vec<Note>,
    pub browser_runs: Vec<BrowserRun>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            agents: default_agents(),
            folders: default_folders(),
            notes: default_notes(),
            browser_runs: Vec::new(),
        }
    }
}

pub fn load(app_data_dir: &Path) -> Result<AppState, String> {
    let store_path = store_path(app_data_dir);
    if !store_path.exists() {
        let state = AppState::default();
        save(app_data_dir, &state)?;
        return Ok(state);
    }

    let raw = fs::read_to_string(&store_path)
        .map_err(|error| format!("failed to read {}: {error}", store_path.display()))?;

    serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", store_path.display()))
}

pub fn save(app_data_dir: &Path, state: &AppState) -> Result<(), String> {
    fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("failed to create {}: {error}", app_data_dir.display()))?;

    let store_path = store_path(app_data_dir);
    let raw = serde_json::to_string_pretty(state)
        .map_err(|error| format!("failed to serialize state: {error}"))?;

    fs::write(&store_path, raw)
        .map_err(|error| format!("failed to write {}: {error}", store_path.display()))
}

pub fn store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(STORE_FILE)
}
