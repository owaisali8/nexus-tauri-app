-- Named agent profiles.
--
-- An agent bundles the settings a conversation would otherwise carry loose:
-- system instructions, provider, model, temperature, and which tools it may
-- use. A session may have no agent, which is plain chat.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS agents (
  id           TEXT    PRIMARY KEY,
  name         TEXT    NOT NULL,
  description  TEXT    NOT NULL DEFAULT '',
  -- The system prompt. Empty is allowed: an agent may exist only to pin a
  -- model and a tool set.
  instructions TEXT    NOT NULL DEFAULT '',
  provider_id  TEXT    NOT NULL,
  model        TEXT    NOT NULL,
  temperature  REAL,
  -- JSON array of tool names this agent may call.
  tool_ids     TEXT    NOT NULL DEFAULT '[]',
  engine       TEXT    NOT NULL DEFAULT 'direct',
  created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at   INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Which agent a conversation belongs to. NULL is plain chat.
--
-- ON DELETE SET NULL rather than CASCADE: deleting an agent must not destroy
-- the conversations held with it.
ALTER TABLE sessions ADD COLUMN agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
