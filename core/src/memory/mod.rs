//! SQLite-backed conversation store.
//!
//! Source of truth for sessions and transcripts. Engines read history from
//! here rather than holding it in memory, so conversations survive restart.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, engine::EngineKind, providers::openai_compat::ChatMessage};

/// Ordered migrations. Append only; never edit one that has shipped.
const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../../db/migrations/0001_sessions.sql"))];

/// Tables every migrated database must have.
///
/// Checked after migrations run, because the version ledger alone is not
/// trustworthy: a database from a different schema lineage can record the
/// same version numbers, causing migrations to be skipped and leaving the
/// tables absent. Failing here with a clear message beats failing later with
/// "no such table" from whatever query happens to run first.
const EXPECTED_TABLES: &[&str] = &["sessions", "messages", "settings"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub provider_id: String,
    pub model: String,
    pub engine: EngineKind,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub seq: i64,
    pub created_at: i64,
}

impl Message {
    /// Convert to the wire format the OpenAI-compatible client expects.
    pub fn to_chat_message(&self) -> ChatMessage {
        ChatMessage {
            role: self.role.clone(),
            content: self.content.clone(),
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn db_error(error: rusqlite::Error) -> Error {
    Error::Transport(format!("database error: {error}"))
}

/// Handle to the conversation database.
///
/// Cloning shares the same connection. Writes are serialized behind a mutex,
/// which is ample for a single-user desktop app and avoids a pool.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open (creating if needed) the database at `path` and migrate it.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::Transport(format!("could not create {parent:?}: {error}"))
            })?;
        }

        let connection = Connection::open(path).map_err(db_error)?;
        Self::from_connection(connection)
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(db_error)?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection
            .execute_batch(
                // WAL keeps reads from blocking the streaming writer.
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(db_error)?;

        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.migrate()?;
        store.verify_schema()?;
        Ok(store)
    }

    /// Confirm the migrations actually produced the schema they claim to.
    fn verify_schema(&self) -> Result<()> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .map_err(db_error)?;

            let present: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .map_err(db_error)?
                .collect::<rusqlite::Result<_>>()
                .map_err(db_error)?;

            let missing: Vec<&str> = EXPECTED_TABLES
                .iter()
                .filter(|expected| !present.iter().any(|name| name == *expected))
                .copied()
                .collect();

            if missing.is_empty() {
                return Ok(());
            }

            Err(Error::Transport(format!(
                "database is missing tables [{}] even though migrations report as applied. \
                 This usually means the file belongs to an incompatible schema \
                 (tables present: [{}]). Move or delete it and restart to get a fresh database.",
                missing.join(", "),
                present.join(", ")
            )))
        })
    }

    fn with_connection<T>(&self, action: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| Error::Transport("database lock poisoned".to_string()))?;
        action(&connection)
    }

    fn migrate(&self) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS schema_migrations (
                       version    INTEGER PRIMARY KEY,
                       applied_at INTEGER NOT NULL DEFAULT (unixepoch())
                     );",
                )
                .map_err(db_error)?;

            for (version, sql) in MIGRATIONS {
                let applied: Option<i64> = connection
                    .query_row(
                        "SELECT version FROM schema_migrations WHERE version = ?1",
                        params![version],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_error)?;

                if applied.is_some() {
                    continue;
                }

                connection.execute_batch(sql).map_err(db_error)?;
                connection
                    .execute(
                        "INSERT INTO schema_migrations (version) VALUES (?1)",
                        params![version],
                    )
                    .map_err(db_error)?;
            }

            Ok(())
        })
    }

    pub fn create_session(
        &self,
        title: &str,
        provider_id: &str,
        model: &str,
        engine: EngineKind,
    ) -> Result<Session> {
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            engine,
            created_at: now(),
            updated_at: now(),
        };

        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id, title, provider_id, model, engine, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session.id,
                        session.title,
                        session.provider_id,
                        session.model,
                        engine_to_str(session.engine),
                        session.created_at,
                        session.updated_at,
                    ],
                )
                .map_err(db_error)?;
            Ok(())
        })?;

        Ok(session)
    }

    /// Sessions newest-first, for the conversation list.
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, title, provider_id, model, engine, created_at, updated_at
                     FROM sessions ORDER BY updated_at DESC",
                )
                .map_err(db_error)?;

            let rows = statement
                .query_map([], |row| {
                    Ok(Session {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        provider_id: row.get(2)?,
                        model: row.get(3)?,
                        engine: engine_from_str(&row.get::<_, String>(4)?),
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })
                .map_err(db_error)?;

            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
        })
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, title, provider_id, model, engine, created_at, updated_at
                     FROM sessions WHERE id = ?1",
                    params![session_id],
                    |row| {
                        Ok(Session {
                            id: row.get(0)?,
                            title: row.get(1)?,
                            provider_id: row.get(2)?,
                            model: row.get(3)?,
                            engine: engine_from_str(&row.get::<_, String>(4)?),
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                )
                .optional()
                .map_err(db_error)
        })
    }

    pub fn rename_session(&self, session_id: &str, title: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
                    params![session_id, title, now()],
                )
                .map_err(db_error)?;
            Ok(())
        })
    }

    /// Delete a session. Messages go with it via ON DELETE CASCADE.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
                .map_err(db_error)?;
            Ok(())
        })
    }

    /// Append a message and bump the session's `updated_at`.
    ///
    /// `seq` is assigned inside the same transaction as the insert, so two
    /// concurrent appends cannot land on the same sequence number.
    pub fn append_message(&self, session_id: &str, role: &str, content: &str) -> Result<Message> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now();

        self.with_connection(|connection| {
            let transaction = connection.unchecked_transaction().map_err(db_error)?;

            let seq: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(seq), -1) + 1 FROM messages WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .map_err(db_error)?;

            transaction
                .execute(
                    "INSERT INTO messages (id, session_id, role, content, seq, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, session_id, role, content, seq, created_at],
                )
                .map_err(db_error)?;

            transaction
                .execute(
                    "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                    params![session_id, created_at],
                )
                .map_err(db_error)?;

            transaction.commit().map_err(db_error)?;

            Ok(Message {
                id: id.clone(),
                session_id: session_id.to_string(),
                role: role.to_string(),
                content: content.to_string(),
                seq,
                created_at,
            })
        })
    }

    /// Full transcript in order.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, session_id, role, content, seq, created_at
                     FROM messages WHERE session_id = ?1 ORDER BY seq ASC",
                )
                .map_err(db_error)?;

            let rows = statement
                .query_map(params![session_id], |row| {
                    Ok(Message {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        content: row.get(3)?,
                        seq: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(db_error)?;

            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
        })
    }

    /// Drop every message at or after `from_seq`.
    ///
    /// Backs regenerate (truncate the last assistant turn) and edit-and-resend
    /// (truncate from the edited turn onward). Returns how many were removed.
    pub fn truncate_from(&self, session_id: &str, from_seq: i64) -> Result<usize> {
        self.with_connection(|connection| {
            let removed = connection
                .execute(
                    "DELETE FROM messages WHERE session_id = ?1 AND seq >= ?2",
                    params![session_id, from_seq],
                )
                .map_err(db_error)?;

            connection
                .execute(
                    "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                    params![session_id, now()],
                )
                .map_err(db_error)?;

            Ok(removed)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
                    params![key, value, now()],
                )
                .map_err(db_error)?;
            Ok(())
        })
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_error)
        })
    }
}

fn engine_to_str(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Direct => "direct",
        EngineKind::Adk => "adk",
    }
}

/// Unknown values fall back to `Direct` rather than failing the read — a row
/// written by a newer build should not make the session list unopenable.
fn engine_from_str(value: &str) -> EngineKind {
    match value {
        "adk" => EngineKind::Adk,
        _ => EngineKind::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = store();
        store.migrate().unwrap();
        store.migrate().unwrap();
        assert!(store.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn sessions_round_trip() {
        let store = store();
        let created = store
            .create_session("First chat", "lmstudio-local", "qwen", EngineKind::Adk)
            .unwrap();

        let loaded = store.get_session(&created.id).unwrap().unwrap();
        assert_eq!(loaded.title, "First chat");
        assert_eq!(loaded.engine, EngineKind::Adk);
        assert_eq!(store.list_sessions().unwrap().len(), 1);
    }

    #[test]
    fn messages_keep_insertion_order() {
        let store = store();
        let session = store
            .create_session("s", "p", "m", EngineKind::Direct)
            .unwrap();

        for index in 0..5 {
            store
                .append_message(&session.id, "user", &format!("message {index}"))
                .unwrap();
        }

        let messages = store.load_messages(&session.id).unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].seq, 0);
        assert_eq!(messages[4].seq, 4);
        assert_eq!(messages[4].content, "message 4");
    }

    #[test]
    fn deleting_a_session_cascades_to_messages() {
        let store = store();
        let session = store
            .create_session("s", "p", "m", EngineKind::Direct)
            .unwrap();
        store.append_message(&session.id, "user", "hello").unwrap();

        store.delete_session(&session.id).unwrap();

        assert!(store.get_session(&session.id).unwrap().is_none());
        assert!(store.load_messages(&session.id).unwrap().is_empty());
    }

    #[test]
    fn appending_bumps_session_updated_at() {
        let store = store();
        let session = store
            .create_session("s", "p", "m", EngineKind::Direct)
            .unwrap();
        store.append_message(&session.id, "user", "hello").unwrap();

        let reloaded = store.get_session(&session.id).unwrap().unwrap();
        assert!(reloaded.updated_at >= session.created_at);
    }

    #[test]
    fn truncate_removes_the_tail_and_leaves_earlier_turns() {
        let store = store();
        let session = store
            .create_session("s", "p", "m", EngineKind::Direct)
            .unwrap();

        for index in 0..4 {
            store
                .append_message(&session.id, "user", &format!("m{index}"))
                .unwrap();
        }

        assert_eq!(store.truncate_from(&session.id, 2).unwrap(), 2);

        let remaining = store.load_messages(&session.id).unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[1].content, "m1");
    }

    /// After truncating, the next append must not reuse a freed seq in a way
    /// that collides with the UNIQUE(session_id, seq) constraint.
    #[test]
    fn appending_after_truncate_continues_cleanly() {
        let store = store();
        let session = store
            .create_session("s", "p", "m", EngineKind::Direct)
            .unwrap();

        store.append_message(&session.id, "user", "keep").unwrap();
        store
            .append_message(&session.id, "assistant", "drop")
            .unwrap();
        store.truncate_from(&session.id, 1).unwrap();

        let replacement = store
            .append_message(&session.id, "assistant", "regenerated")
            .unwrap();
        assert_eq!(replacement.seq, 1);

        let messages = store.load_messages(&session.id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "regenerated");
    }

    #[test]
    fn settings_upsert() {
        let store = store();
        store.set_setting("theme", "dark").unwrap();
        store.set_setting("theme", "light").unwrap();
        assert_eq!(
            store.get_setting("theme").unwrap().as_deref(),
            Some("light")
        );
        assert!(store.get_setting("missing").unwrap().is_none());
    }

    #[test]
    fn unknown_engine_value_falls_back_to_direct() {
        assert_eq!(engine_from_str("from-the-future"), EngineKind::Direct);
        assert_eq!(engine_from_str("adk"), EngineKind::Adk);
    }
}
