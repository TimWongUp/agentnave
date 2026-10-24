-- The conversation database (ADR-0026): Python creates it, the tray reads and cleans it.
CREATE TABLE conversations (
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
CREATE INDEX conversations_manager ON conversations (manager_session);
CREATE TABLE entries (
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (conversation_id, position)
) WITHOUT ROWID;
CREATE TABLE rounds (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (conversation_id, number)
);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    provider TEXT NOT NULL,
    session_id TEXT,
    status TEXT NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (conversation_id, position)
);
CREATE INDEX tasks_session ON tasks (provider, session_id);
CREATE TABLE views (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations (id) ON DELETE CASCADE,
    rev INTEGER NOT NULL,
    written_ms INTEGER NOT NULL,
    body TEXT NOT NULL
);
