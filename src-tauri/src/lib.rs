mod core;

use core::{
    agent::{
        runtime::{AgentPromptRequest, AgentPromptResponse, RuntimePlan},
        AgentProfile,
    },
    browser::{BrowserRun, BrowserRunRequest},
    llm::LlmProvider,
    memory::{Note, WorkspaceFolder},
    store::{AppState, DatabaseInfo},
};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    providers: Vec<LlmProvider>,
    agents: Vec<AgentProfile>,
    folders: Vec<WorkspaceFolder>,
    notes: Vec<Note>,
    browser_runs: Vec<BrowserRun>,
    rig_marker: String,
}

#[tauri::command]
fn get_app_snapshot(app: AppHandle) -> Result<AppSnapshot, String> {
    let state = load_state(&app)?;

    Ok(AppSnapshot {
        providers: core::llm::default_providers(),
        agents: state.agents,
        folders: state.folders,
        notes: state.notes,
        browser_runs: state.browser_runs,
        rig_marker: core::agent::runtime::rig_marker().to_string(),
    })
}

#[tauri::command]
fn get_database_info(app: AppHandle) -> Result<DatabaseInfo, String> {
    core::store::database_info(&app_data_dir(&app)?)
}

#[tauri::command]
fn save_agent(app: AppHandle, agent: AgentProfile) -> Result<AgentProfile, String> {
    validate_agent(&agent)?;

    let mut state = load_state(&app)?;
    match state.agents.iter().position(|item| item.id == agent.id) {
        Some(index) => state.agents[index] = agent.clone(),
        None => state.agents.push(agent.clone()),
    }

    save_state(&app, &state)?;
    Ok(agent)
}

#[tauri::command]
fn save_note(app: AppHandle, note: Note) -> Result<Note, String> {
    if note.title.trim().is_empty() {
        return Err("note title is required".to_string());
    }

    let mut state = load_state(&app)?;
    match state.notes.iter().position(|item| item.id == note.id) {
        Some(index) => state.notes[index] = note.clone(),
        None => state.notes.push(note.clone()),
    }

    save_state(&app, &state)?;
    Ok(note)
}

#[tauri::command]
fn prepare_browser_run(app: AppHandle, request: BrowserRunRequest) -> Result<BrowserRun, String> {
    let run = core::browser::prepare_run(request)?;
    let mut state = load_state(&app)?;
    state.browser_runs.insert(0, run.clone());
    state.browser_runs.truncate(25);
    save_state(&app, &state)?;
    Ok(run)
}

#[tauri::command]
fn get_runtime_plan(app: AppHandle, agent_id: String) -> Result<RuntimePlan, String> {
    let state = load_state(&app)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("agent not found: {agent_id}"))?;

    Ok(core::agent::runtime::build_runtime_plan(agent))
}

#[tauri::command]
async fn run_agent_prompt(
    app: AppHandle,
    request: AgentPromptRequest,
) -> Result<AgentPromptResponse, String> {
    let state = load_state(&app)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == request.agent_id)
        .ok_or_else(|| format!("agent not found: {}", request.agent_id))?;

    core::agent::runtime::run_agent_prompt(agent, request).await
}

fn validate_agent(agent: &AgentProfile) -> Result<(), String> {
    if agent.id.trim().is_empty() {
        return Err("agent id is required".to_string());
    }

    if agent.name.trim().is_empty() {
        return Err("agent name is required".to_string());
    }

    if agent.system_instructions.trim().is_empty() {
        return Err("system instructions are required".to_string());
    }

    if agent.provider_id.trim().is_empty() || agent.model.trim().is_empty() {
        return Err("provider and model are required".to_string());
    }

    Ok(())
}

fn load_state(app: &AppHandle) -> Result<AppState, String> {
    core::store::load(&app_data_dir(app)?)
}

fn save_state(app: &AppHandle, state: &AppState) -> Result<(), String> {
    core::store::save(&app_data_dir(app)?, state)
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("failed to resolve app data directory: {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            get_database_info,
            save_agent,
            save_note,
            prepare_browser_run,
            get_runtime_plan,
            run_agent_prompt
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
