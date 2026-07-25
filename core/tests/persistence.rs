//! Durability: the Phase 1 promise is that conversations survive restart.
//!
//! These use a real file, not `:memory:`, because the thing under test is
//! whether data is still there after the process that wrote it is gone.

use essentio_core::{engine::EngineKind, memory::Store};

/// A temp path that cleans itself up, including the WAL sidecar files.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "essentio-test-{name}-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        Self(path)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.clone().into_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
    }
}

#[test]
fn conversations_survive_reopening_the_database() {
    let db = TempDb::new("restart");

    let session_id = {
        let store = Store::open(&db.0).unwrap();
        let session = store
            .create_session(
                "Persisted chat",
                "lmstudio-local",
                "qwen",
                EngineKind::Adk,
                None,
            )
            .unwrap();

        store
            .append_message(&session.id, "user", "what is a workspace?")
            .unwrap();
        store
            .append_message(&session.id, "assistant", "a set of crates")
            .unwrap();

        session.id
        // store dropped here — stands in for the app closing
    };

    let reopened = Store::open(&db.0).unwrap();

    let sessions = reopened.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "Persisted chat");
    assert_eq!(
        sessions[0].engine,
        EngineKind::Adk,
        "the engine a conversation was produced with must survive"
    );

    let messages = reopened.load_messages(&session_id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "what is a workspace?");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].seq, 1);
}

#[test]
fn migrations_are_safe_to_run_against_an_existing_database() {
    let db = TempDb::new("migrate");

    let id = {
        let store = Store::open(&db.0).unwrap();
        store
            .create_session("keep me", "p", "m", EngineKind::Direct, None)
            .unwrap()
            .id
    };

    // Reopening runs migrate() again; it must not clobber existing rows.
    for _ in 0..3 {
        let store = Store::open(&db.0).unwrap();
        assert!(store.get_session(&id).unwrap().is_some());
    }
}

/// Regression: a database from a different schema lineage can already record
/// version 1 in `schema_migrations`. The migration was then skipped, `sessions`
/// was never created, and the app failed at runtime with "no such table:
/// sessions". Opening such a file must fail immediately with a message that
/// names the problem.
#[test]
fn foreign_migration_ledger_is_rejected_not_silently_skipped() {
    let db = TempDb::new("foreign-ledger");

    // Stand in for the old build: same ledger table, same version numbers,
    // entirely different tables.
    {
        let connection = rusqlite::Connection::open(&db.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 INSERT INTO schema_migrations (version) VALUES (1), (2);
                 CREATE TABLE agent_sessions (id TEXT PRIMARY KEY);
                 CREATE TABLE agents (id TEXT PRIMARY KEY);",
            )
            .unwrap();
    }

    let message = match Store::open(&db.0) {
        Ok(_) => panic!("opening a foreign-lineage database must fail loudly"),
        Err(error) => error.to_string(),
    };

    assert!(
        message.contains("sessions"),
        "error should name the missing table, got: {message}"
    );
    assert!(
        message.contains("agent_sessions"),
        "error should show what the file actually contains, got: {message}"
    );
}

#[test]
fn settings_persist_across_reopen() {
    let db = TempDb::new("settings");

    {
        let store = Store::open(&db.0).unwrap();
        store
            .set_setting("last_provider", "lmstudio-local")
            .unwrap();
    }

    let reopened = Store::open(&db.0).unwrap();
    assert_eq!(
        reopened.get_setting("last_provider").unwrap().as_deref(),
        Some("lmstudio-local")
    );
}
