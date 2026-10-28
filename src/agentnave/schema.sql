-- The conversation database (ADR-0026): Python creates it, the tray reads and cleans it.
-- Every statement is idempotent: servers run it on each connection, so tables added later
-- (like workspaces) reach databases created by older versions without a version change.
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('show', 'task')),
    title TEXT NOT NULL,
    created TEXT NOT NULL,
    host TEXT,
    host_source TEXT NOT NULL,
    manager_session TEXT,
    cwd TEXT,
    review INTEGER NOT NULL,
    isolated INTEGER NOT NULL,
    seats TEXT NOT NULL,
    sessions TEXT NOT NULL,
    rev INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS conversations_manager ON conversations (manager_session);
CREATE TABLE IF NOT EXISTS entries (
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (conversation_id, position)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS rounds (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (conversation_id, number)
);
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    provider TEXT NOT NULL,
    session_id TEXT,
    status TEXT NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (conversation_id, position)
);
CREATE INDEX IF NOT EXISTS tasks_session ON tasks (provider, session_id);
CREATE TABLE IF NOT EXISTS views (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations (id) ON DELETE CASCADE,
    rev INTEGER NOT NULL,
    written_ms INTEGER NOT NULL,
    body TEXT NOT NULL
);
-- Boss handover (ADR-0032): which boss writes each workspace (conversations.manager_session)
-- now. A workspace without a row belongs to the boss that created it.
CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    owner_host TEXT,
    owner_file TEXT,
    pending INTEGER NOT NULL DEFAULT 0,
    note TEXT,
    released TEXT,
    previous_host TEXT,
    previous_file TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS workspaces_owner ON workspaces (owner);
