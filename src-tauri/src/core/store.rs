use crate::core::{
    agent::{default_agents, AgentProfile},
    browser::{BrowserRun, BrowserRunStatus},
    memory::{default_folders, default_notes, Note, WorkspaceFolder},
};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const DB_FILE: &str = "essentio.sqlite3";
const LEGACY_STORE_FILE: &str = "essentio-state.json";
const INITIAL_MIGRATION: &str = include_str!("../../db/migrations/0001_initial.sql");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub agents: Vec<AgentProfile>,
    pub folders: Vec<WorkspaceFolder>,
    pub notes: Vec<Note>,
    pub browser_runs: Vec<BrowserRun>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub path: String,
    pub agent_count: usize,
    pub note_count: usize,
    pub browser_run_count: usize,
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
    let mut connection = open(app_data_dir)?;
    migrate(&connection)?;
    import_legacy_json_if_needed(app_data_dir, &mut connection)?;
    seed_if_empty(&mut connection)?;

    Ok(AppState {
        agents: load_agents(&connection)?,
        folders: load_folders(&connection)?,
        notes: load_notes(&connection)?,
        browser_runs: load_browser_runs(&connection)?,
    })
}

pub fn save(app_data_dir: &Path, state: &AppState) -> Result<(), String> {
    let mut connection = open(app_data_dir)?;
    migrate(&connection)?;

    let tx = connection.transaction().map_err(db_error)?;
    replace_all(&tx, state)?;
    tx.commit().map_err(db_error)
}

pub fn database_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(DB_FILE)
}

pub fn database_info(app_data_dir: &Path) -> Result<DatabaseInfo, String> {
    let mut connection = open(app_data_dir)?;
    migrate(&connection)?;
    import_legacy_json_if_needed(app_data_dir, &mut connection)?;
    seed_if_empty(&mut connection)?;

    Ok(DatabaseInfo {
        path: database_path(app_data_dir).display().to_string(),
        agent_count: count(&connection, "agents")?,
        note_count: count(&connection, "notes")?,
        browser_run_count: count(&connection, "browser_runs")?,
    })
}

fn open(app_data_dir: &Path) -> Result<Connection, String> {
    fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("failed to create {}: {error}", app_data_dir.display()))?;

    let connection = Connection::open(database_path(app_data_dir)).map_err(db_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(db_error)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(INITIAL_MIGRATION)
        .map_err(db_error)?;

    let applied: Option<i64> = connection
        .query_row(
            "SELECT version FROM schema_migrations WHERE version = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;

    if applied.is_none() {
        connection
            .execute("INSERT INTO schema_migrations (version) VALUES (1)", [])
            .map_err(db_error)?;
    }

    Ok(())
}

fn import_legacy_json_if_needed(
    app_data_dir: &Path,
    connection: &mut Connection,
) -> Result<(), String> {
    if !is_empty(connection)? {
        return Ok(());
    }

    let legacy_path = app_data_dir.join(LEGACY_STORE_FILE);
    if !legacy_path.exists() {
        return Ok(());
    }

    let raw = fs::read_to_string(&legacy_path)
        .map_err(|error| format!("failed to read {}: {error}", legacy_path.display()))?;
    let state: AppState = serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", legacy_path.display()))?;

    let tx = connection.transaction().map_err(db_error)?;
    replace_all(&tx, &state)?;
    tx.commit().map_err(db_error)
}

fn seed_if_empty(connection: &mut Connection) -> Result<(), String> {
    if !is_empty(connection)? {
        return Ok(());
    }

    let tx = connection.transaction().map_err(db_error)?;
    replace_all(&tx, &AppState::default())?;
    tx.commit().map_err(db_error)
}

fn is_empty(connection: &Connection) -> Result<bool, String> {
    let agent_count = count(connection, "agents")?;
    Ok(agent_count == 0)
}

fn count(connection: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(db_error)?;
    Ok(count.max(0) as usize)
}

fn replace_all(tx: &Transaction<'_>, state: &AppState) -> Result<(), String> {
    tx.execute("DELETE FROM browser_runs", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM notes", []).map_err(db_error)?;
    tx.execute("DELETE FROM agents", []).map_err(db_error)?;
    tx.execute("DELETE FROM folders", []).map_err(db_error)?;

    for folder in &state.folders {
        tx.execute(
            "INSERT INTO folders (id, name, items) VALUES (?1, ?2, ?3)",
            params![folder.id, folder.name, folder.items as i64],
        )
        .map_err(db_error)?;
    }

    for agent in &state.agents {
        tx.execute(
            "INSERT INTO agents (
                id, name, description, system_instructions, provider_id, model,
                tools_json, mcps_json, skills_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                agent.id,
                agent.name,
                agent.description,
                agent.system_instructions,
                agent.provider_id,
                agent.model,
                to_json(&agent.tools)?,
                to_json(&agent.mcps)?,
                to_json(&agent.skills)?,
            ],
        )
        .map_err(db_error)?;
    }

    for note in &state.notes {
        tx.execute(
            "INSERT INTO notes (id, folder_id, title, body, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                note.id,
                note.folder_id,
                note.title,
                note.body,
                note.updated_at as i64,
            ],
        )
        .map_err(db_error)?;
    }

    for run in &state.browser_runs {
        tx.execute(
            "INSERT INTO browser_runs (
                id, agent_id, target_url, objective, resume_file_name, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.id,
                run.agent_id,
                run.target_url,
                run.objective,
                run.resume_file_name,
                status_to_str(&run.status),
            ],
        )
        .map_err(db_error)?;
    }

    Ok(())
}

fn load_agents(connection: &Connection) -> Result<Vec<AgentProfile>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, description, system_instructions, provider_id, model,
                    tools_json, mcps_json, skills_json
             FROM agents
             ORDER BY rowid ASC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let tools_json: String = row.get(6)?;
            let mcps_json: String = row.get(7)?;
            let skills_json: String = row.get(8)?;

            Ok(AgentProfile {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                system_instructions: row.get(3)?,
                provider_id: row.get(4)?,
                model: row.get(5)?,
                tools: from_json(6, &tools_json)?,
                mcps: from_json(7, &mcps_json)?,
                skills: from_json(8, &skills_json)?,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_folders(connection: &Connection) -> Result<Vec<WorkspaceFolder>, String> {
    let mut statement = connection
        .prepare("SELECT id, name, items FROM folders ORDER BY rowid ASC")
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let items: i64 = row.get(2)?;
            Ok(WorkspaceFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                items: items.max(0) as usize,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_notes(connection: &Connection) -> Result<Vec<Note>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, folder_id, title, body, updated_at FROM notes ORDER BY updated_at DESC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let updated_at: i64 = row.get(4)?;
            Ok(Note {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
                updated_at: updated_at.max(0) as u64,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_browser_runs(connection: &Connection) -> Result<Vec<BrowserRun>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, agent_id, target_url, objective, resume_file_name, status
             FROM browser_runs
             ORDER BY created_at DESC
             LIMIT 25",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let status: String = row.get(5)?;
            Ok(BrowserRun {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                target_url: row.get(2)?,
                objective: row.get(3)?,
                resume_file_name: row.get(4)?,
                status: status_from_str(&status),
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, String> {
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}

fn to_json(values: &[String]) -> Result<String, String> {
    serde_json::to_string(values).map_err(|error| format!("failed to serialize list: {error}"))
}

fn from_json(index: usize, raw: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn status_to_str(status: &BrowserRunStatus) -> &'static str {
    match status {
        BrowserRunStatus::Draft => "draft",
        BrowserRunStatus::Ready => "ready",
        BrowserRunStatus::Blocked => "blocked",
    }
}

fn status_from_str(status: &str) -> BrowserRunStatus {
    match status {
        "ready" => BrowserRunStatus::Ready,
        "blocked" => BrowserRunStatus::Blocked,
        _ => BrowserRunStatus::Draft,
    }
}

fn db_error(error: rusqlite::Error) -> String {
    format!("sqlite error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sqlite_store_seeds_and_roundtrips_state() {
        let app_data_dir = std::env::temp_dir().join(format!(
            "essentio-store-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));

        let mut state = load(&app_data_dir).expect("store should seed default state");
        assert!(!state.agents.is_empty());
        assert!(!state.folders.is_empty());

        state.notes.push(Note {
            id: "note-roundtrip".to_string(),
            folder_id: state.folders[0].id.clone(),
            title: "Roundtrip".to_string(),
            body: "SQLite persistence test".to_string(),
            updated_at: 1,
        });

        save(&app_data_dir, &state).expect("state should save to sqlite");
        let reloaded = load(&app_data_dir).expect("state should reload from sqlite");

        assert!(reloaded.notes.iter().any(|note| note.id == "note-roundtrip"));

        let _ = fs::remove_dir_all(app_data_dir);
    }
}
