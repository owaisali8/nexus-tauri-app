-- Conversation persistence.
--
-- Sessions belong to a provider/model/engine rather than to an agent profile:
-- the workspace has no agent entity, and pinning the engine per session means
-- a conversation replays through the same implementation that produced it.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
  version    INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS sessions (
  id          TEXT    PRIMARY KEY,
  title       TEXT    NOT NULL,
  provider_id TEXT    NOT NULL,
  model       TEXT    NOT NULL,
  engine      TEXT    NOT NULL DEFAULT 'direct',
  created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS messages (
  id         TEXT    PRIMARY KEY,
  session_id TEXT    NOT NULL,
  role       TEXT    NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
  content    TEXT    NOT NULL,
  -- Monotonic per session. created_at has second resolution, so two messages
  -- in the same turn can share a timestamp; seq is what ordering relies on.
  seq        INTEGER NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
  UNIQUE (session_id, seq)
);

CREATE TABLE IF NOT EXISTS settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_session_seq ON messages(session_id, seq ASC);
