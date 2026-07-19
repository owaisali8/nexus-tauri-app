use crate::core::{
    agent::AgentProfile,
    browser::{BrowserRun, BrowserRunStatus},
    llm::StoredProvider,
    memory::{
        default_folders, default_notes, source_from_str, source_to_str, AgentSession, FileRecord,
        MemoryEntry, Note, SessionMessage, WorkspaceFolder, role_from_str, role_to_str,
    },
};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const DB_FILE: &str = "essentio.sqlite3";
const LEGACY_STORE_FILE: &str = "essentio-state.json";

const MIGRATIONS: &[(&str, i64)] = &[
    (
        include_str!("../../db/migrations/0001_initial.sql"),
        1,
    ),
    (
        include_str!("../../db/migrations/0002_sessions_memory_files.sql"),
        2,
    ),
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub providers: Vec<StoredProvider>,
    pub agents: Vec<AgentProfile>,
    pub folders: Vec<WorkspaceFolder>,
    pub notes: Vec<Note>,
    pub file_records: Vec<FileRecord>,
    pub sessions: Vec<AgentSession>,
    pub session_messages: Vec<SessionMessage>,
    pub long_term_memory: Vec<MemoryEntry>,
    pub browser_runs: Vec<BrowserRun>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub path: String,
    pub provider_count: usize,
    pub agent_count: usize,
    pub note_count: usize,
    pub file_count: usize,
    pub session_count: usize,
    pub memory_count: usize,
    pub browser_run_count: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            agents: default_agents(),
            folders: default_folders(),
            notes: default_notes(),
            file_records: Vec::new(),
            sessions: Vec::new(),
            session_messages: Vec::new(),
            long_term_memory: Vec::new(),
            browser_runs: Vec::new(),
        }
    }
}

use crate::core::agent::default_agents;

pub fn load(app_data_dir: &Path) -> Result<AppState, String> {
    let mut connection = open(app_data_dir)?;
    migrate(&connection)?;
    import_legacy_json_if_needed(app_data_dir, &mut connection)?;
    seed_if_empty(&mut connection)?;

    Ok(read_state(&connection)?)
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
        provider_count: count(&connection, "llm_providers")?,
        agent_count: count(&connection, "agents")?,
        note_count: count(&connection, "notes")?,
        file_count: count(&connection, "file_records")?,
        session_count: count(&connection, "agent_sessions")?,
        memory_count: count(&connection, "long_term_memory")?,
        browser_run_count: count(&connection, "browser_runs")?,
    })
}

pub fn load_session_messages(
    app_data_dir: &Path,
    session_id: &str,
) -> Result<Vec<SessionMessage>, String> {
    let connection = open(app_data_dir)?;
    migrate(&connection)?;
    load_session_messages_for(&connection, session_id)
}

pub fn load_memory_for_agent(
    app_data_dir: &Path,
    agent_id: &str,
) -> Result<Vec<MemoryEntry>, String> {
    let connection = open(app_data_dir)?;
    migrate(&connection)?;
    load_memory_entries_for(&connection, agent_id)
}

pub fn append_chat_turn(
    app_data_dir: &Path,
    session: &AgentSession,
    user_message: &SessionMessage,
    assistant_message: &SessionMessage,
) -> Result<(), String> {
    let mut connection = open(app_data_dir)?;
    migrate(&connection)?;

    let tx = connection.transaction().map_err(db_error)?;

    tx.execute(
        "UPDATE agent_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            session.title,
            session.updated_at as i64,
            session.id,
        ],
    )
    .map_err(db_error)?;

    for message in [user_message, assistant_message] {
        tx.execute(
            "INSERT INTO session_messages (id, session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.session_id,
                role_to_str(&message.role),
                message.content,
                message.created_at as i64,
            ],
        )
        .map_err(db_error)?;
    }

    tx.commit().map_err(db_error)
}

fn read_state(connection: &Connection) -> Result<AppState, String> {
    Ok(AppState {
        providers: load_providers(connection)?,
        agents: load_agents(connection)?,
        folders: load_folders(connection)?,
        notes: load_notes(connection)?,
        file_records: load_file_records(connection)?,
        sessions: load_sessions(connection)?,
        session_messages: Vec::new(),
        long_term_memory: load_memory_entries(connection)?,
        browser_runs: load_browser_runs(connection)?,
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
    for (sql, version) in MIGRATIONS {
        connection.execute_batch(sql).map_err(db_error)?;

        let applied: Option<i64> = connection
            .query_row(
                "SELECT version FROM schema_migrations WHERE version = ?1",
                params![version],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;

        if applied.is_none() {
            connection
                .execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    params![version],
                )
                .map_err(db_error)?;
        }
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

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyAppState {
        agents: Vec<AgentProfile>,
        folders: Vec<WorkspaceFolder>,
        notes: Vec<Note>,
        browser_runs: Vec<BrowserRun>,
    }

    let legacy: LegacyAppState = serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", legacy_path.display()))?;

    let state = AppState {
        providers: Vec::new(),
        agents: legacy.agents,
        folders: legacy.folders,
        notes: legacy.notes,
        file_records: Vec::new(),
        sessions: Vec::new(),
        session_messages: Vec::new(),
        long_term_memory: Vec::new(),
        browser_runs: legacy.browser_runs,
    };

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
    let folder_count = count(connection, "folders")?;
    Ok(agent_count == 0 && folder_count == 0)
}

fn count(connection: &Connection, table: &str) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(db_error)?;
    Ok(count.max(0) as usize)
}

fn replace_all(tx: &Transaction<'_>, state: &AppState) -> Result<(), String> {
    tx.execute("DELETE FROM session_messages", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM agent_sessions", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM long_term_memory", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM file_records", []).map_err(db_error)?;
    tx.execute("DELETE FROM browser_runs", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM notes", []).map_err(db_error)?;
    tx.execute("DELETE FROM agents", []).map_err(db_error)?;
    tx.execute("DELETE FROM llm_providers", [])
        .map_err(db_error)?;
    tx.execute("DELETE FROM folders", []).map_err(db_error)?;

    for folder in &state.folders {
        tx.execute(
            "INSERT INTO folders (id, name, items) VALUES (?1, ?2, ?3)",
            params![folder.id, folder.name, folder.items as i64],
        )
        .map_err(db_error)?;
    }

    for provider in &state.providers {
        tx.execute(
            "INSERT INTO llm_providers (
                id, name, kind, api_key, base_url, models_json, is_enabled, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                provider.id,
                provider.name,
                kind_to_str(&provider.kind),
                provider.api_key,
                provider.base_url,
                to_json(&provider.models)?,
                if provider.is_enabled { 1 } else { 0 },
                now_i64(),
            ],
        )
        .map_err(db_error)?;
    }

    for agent in &state.agents {
        tx.execute(
            "INSERT INTO agents (
                id, name, description, system_instructions, provider_id, model,
                tools_json, mcps_json, skills_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                now_i64(),
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

    for file in &state.file_records {
        tx.execute(
            "INSERT INTO file_records (
                id, name, path, mime_type, size_bytes, folder_id, summary, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                file.id,
                file.name,
                file.path,
                file.mime_type,
                file.size_bytes as i64,
                file.folder_id,
                file.summary,
                file.created_at as i64,
                file.updated_at as i64,
            ],
        )
        .map_err(db_error)?;
    }

    for session in &state.sessions {
        tx.execute(
            "INSERT INTO agent_sessions (id, agent_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.id,
                session.agent_id,
                session.title,
                session.created_at as i64,
                session.updated_at as i64,
            ],
        )
        .map_err(db_error)?;
    }

    for message in &state.session_messages {
        tx.execute(
            "INSERT INTO session_messages (id, session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.session_id,
                role_to_str(&message.role),
                message.content,
                message.created_at as i64,
            ],
        )
        .map_err(db_error)?;
    }

    for entry in &state.long_term_memory {
        tx.execute(
            "INSERT INTO long_term_memory (
                id, agent_id, memory_key, value, source, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.id,
                entry.agent_id,
                entry.key,
                entry.value,
                source_to_str(&entry.source),
                entry.created_at as i64,
                entry.updated_at as i64,
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

fn load_providers(connection: &Connection) -> Result<Vec<StoredProvider>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, kind, api_key, base_url, models_json, is_enabled
             FROM llm_providers
             ORDER BY rowid ASC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let kind: String = row.get(2)?;
            let models_json: String = row.get(5)?;
            let is_enabled: i64 = row.get(6)?;

            Ok(StoredProvider {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: kind_from_str(&kind),
                api_key: row.get(3)?,
                base_url: row.get(4)?,
                models: from_json(5, &models_json)?,
                is_enabled: is_enabled != 0,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
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

fn load_file_records(connection: &Connection) -> Result<Vec<FileRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, path, mime_type, size_bytes, folder_id, summary, created_at, updated_at
             FROM file_records
             ORDER BY updated_at DESC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let size_bytes: i64 = row.get(4)?;
            let created_at: i64 = row.get(7)?;
            let updated_at: i64 = row.get(8)?;
            Ok(FileRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                mime_type: row.get(3)?,
                size_bytes: size_bytes.max(0) as u64,
                folder_id: row.get(5)?,
                summary: row.get(6)?,
                created_at: created_at.max(0) as u64,
                updated_at: updated_at.max(0) as u64,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_sessions(connection: &Connection) -> Result<Vec<AgentSession>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, agent_id, title, created_at, updated_at
             FROM agent_sessions
             ORDER BY updated_at DESC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let created_at: i64 = row.get(3)?;
            let updated_at: i64 = row.get(4)?;
            Ok(AgentSession {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                title: row.get(2)?,
                created_at: created_at.max(0) as u64,
                updated_at: updated_at.max(0) as u64,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_session_messages_for(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<SessionMessage>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, session_id, role, content, created_at
             FROM session_messages
             WHERE session_id = ?1
             ORDER BY created_at ASC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map(params![session_id], |row| {
            let role: String = row.get(2)?;
            let created_at: i64 = row.get(4)?;
            Ok(SessionMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: role_from_str(&role),
                content: row.get(3)?,
                created_at: created_at.max(0) as u64,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_memory_entries(connection: &Connection) -> Result<Vec<MemoryEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, agent_id, memory_key, value, source, created_at, updated_at
             FROM long_term_memory
             ORDER BY updated_at DESC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map([], |row| {
            let source: String = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let updated_at: i64 = row.get(6)?;
            Ok(MemoryEntry {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                key: row.get(2)?,
                value: row.get(3)?,
                source: source_from_str(&source),
                created_at: created_at.max(0) as u64,
                updated_at: updated_at.max(0) as u64,
            })
        })
        .map_err(db_error)?;

    collect_rows(rows)
}

fn load_memory_entries_for(
    connection: &Connection,
    agent_id: &str,
) -> Result<Vec<MemoryEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, agent_id, memory_key, value, source, created_at, updated_at
             FROM long_term_memory
             WHERE agent_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(db_error)?;

    let rows = statement
        .query_map(params![agent_id], |row| {
            let source: String = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let updated_at: i64 = row.get(6)?;
            Ok(MemoryEntry {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                key: row.get(2)?,
                value: row.get(3)?,
                source: source_from_str(&source),
                created_at: created_at.max(0) as u64,
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

fn kind_to_str(kind: &crate::core::llm::ProviderKind) -> &'static str {
    match kind {
        crate::core::llm::ProviderKind::Cloud => "cloud",
        crate::core::llm::ProviderKind::Local => "local",
    }
}

fn kind_from_str(kind: &str) -> crate::core::llm::ProviderKind {
    match kind {
        "local" => crate::core::llm::ProviderKind::Local,
        _ => crate::core::llm::ProviderKind::Cloud,
    }
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

fn now_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn db_error(error: rusqlite::Error) -> String {
    format!("sqlite error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        agent::AgentProfile,
        llm::{ProviderKind, StoredProvider},
        memory::Note,
    };
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
        assert!(!state.folders.is_empty());

        state.providers.push(StoredProvider {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            kind: ProviderKind::Cloud,
            api_key: Some("test-key".to_string()),
            base_url: None,
            models: vec!["gpt-4.1-mini".to_string()],
            is_enabled: true,
        });

        state.agents.push(AgentProfile {
            id: "test-agent".to_string(),
            name: "Test Agent".to_string(),
            description: "Roundtrip agent".to_string(),
            system_instructions: "You are helpful.".to_string(),
            provider_id: "openai".to_string(),
            model: "gpt-4.1-mini".to_string(),
            tools: vec![],
            mcps: vec![],
            skills: vec![],
        });

        state.notes.push(Note {
            id: "note-roundtrip".to_string(),
            folder_id: state.folders[0].id.clone(),
            title: "Roundtrip".to_string(),
            body: "SQLite persistence test".to_string(),
            updated_at: 1,
        });

        save(&app_data_dir, &state).expect("state should save to sqlite");
        let reloaded = load(&app_data_dir).expect("state should reload from sqlite");

        assert!(reloaded.providers.iter().any(|provider| provider.id == "openai"));
        assert!(reloaded.agents.iter().any(|agent| agent.id == "test-agent"));
        assert!(reloaded.notes.iter().any(|note| note.id == "note-roundtrip"));

        let _ = fs::remove_dir_all(app_data_dir);
    }
}
