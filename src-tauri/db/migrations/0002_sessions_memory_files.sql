PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS llm_providers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  api_key TEXT,
  base_url TEXT,
  models_json TEXT NOT NULL,
  is_enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS file_records (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  path TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  folder_id TEXT,
  summary TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (folder_id) REFERENCES folders(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS agent_sessions (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS session_messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (session_id) REFERENCES agent_sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS long_term_memory (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL,
  memory_key TEXT NOT NULL,
  value TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'user',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
  UNIQUE(agent_id, memory_key)
);

CREATE INDEX IF NOT EXISTS idx_file_records_folder_id ON file_records(folder_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent_id ON agent_sessions(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_updated_at ON agent_sessions(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_messages_session_id ON session_messages(session_id);
CREATE INDEX IF NOT EXISTS idx_session_messages_created_at ON session_messages(created_at ASC);
CREATE INDEX IF NOT EXISTS idx_long_term_memory_agent_id ON long_term_memory(agent_id);
