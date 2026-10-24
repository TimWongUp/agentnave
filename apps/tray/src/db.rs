//! The conversation database the MCP servers write (ADR-0026). The tray reads its views and,
//! under each conversation's OS lock, deletes or renames conversations.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

const NAME: &str = "agentnave.db";
/// The schema version this tray understands; servers record theirs in `user_version`.
const SCHEMA_VERSION: i64 = 1;
const BUSY: Duration = Duration::from_secs(5);

pub fn path(root: &Path) -> PathBuf {
    root.join(NAME)
}

/// The lock file a server holds while it writes a conversation; never removed.
pub fn lock_path(root: &Path, id: &str) -> PathBuf {
    root.join("locks").join(format!("{id}.lock"))
}

fn open(root: &Path, flags: OpenFlags) -> Result<Option<Connection>, String> {
    let path = path(root);
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    connection.busy_timeout(BUSY).map_err(|e| e.to_string())?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "会话数据库版本为 {version}，此 App 只支持 {SCHEMA_VERSION}；请更新 App 或服务"
        ));
    }
    Ok(Some(connection))
}

/// A read-only connection, or None before any server saved a conversation.
pub fn read(root: &Path) -> Result<Option<Connection>, String> {
    open(root, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

/// A writing connection with cascading deletes, or None when there is no database yet.
pub fn write(root: &Path) -> Result<Option<Connection>, String> {
    let Some(connection) = open(root, OpenFlags::SQLITE_OPEN_READ_WRITE)? else {
        return Ok(None);
    };
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|e| e.to_string())?;
    Ok(Some(connection))
}

/// Build the servers' database for tests, from the same schema file Python uses.
#[cfg(test)]
pub fn create(root: &Path) -> Connection {
    std::fs::create_dir_all(root).unwrap();
    let connection = Connection::open(path(root)).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute_batch(include_str!("../../../src/agentnave/schema.sql"))
        .unwrap();
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection
}

/// Save one conversation and its view for tests: a task or show as a server would.
#[cfg(test)]
pub fn put(root: &Path, id: &str, state: &serde_json::Value, view: &serde_json::Value) {
    let connection = if path(root).is_file() {
        write(root).unwrap().unwrap()
    } else {
        create(root)
    };
    let text = |key: &str| state[key].as_str().map(str::to_owned);
    connection
        .execute(
            "INSERT INTO conversations (id, kind, title, created, host, host_source, \
             manager_session, cwd, review, isolated, seats, sessions, rev) VALUES \
             (?1, ?2, ?3, ?4, NULL, 'unknown', NULL, NULL, 0, 0, '[]', '{}', 1) \
             ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, title = excluded.title, \
             rev = rev + 1",
            rusqlite::params![
                id,
                text("kind").unwrap_or_else(|| "task".to_owned()),
                text("title").unwrap_or_else(|| "测试".to_owned()),
                text("created").unwrap_or_else(|| "2026-10-24T00:00:00+00:00".to_owned()),
            ],
        )
        .unwrap();
    connection
        .execute("DELETE FROM rounds WHERE conversation_id = ?1", [id])
        .unwrap();
    for (number, round) in state["rounds"].as_array().into_iter().flatten().enumerate() {
        connection
            .execute(
                "INSERT INTO rounds (id, conversation_id, number, body) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    format!("{id}-r{number}"),
                    id,
                    number as i64,
                    round.to_string()
                ],
            )
            .unwrap();
    }
    connection
        .execute("DELETE FROM tasks WHERE conversation_id = ?1", [id])
        .unwrap();
    for (position, task) in state["tasks"].as_array().into_iter().flatten().enumerate() {
        connection
            .execute(
                "INSERT INTO tasks (id, conversation_id, position, provider, session_id, \
                 status, body) VALUES (?1, ?2, ?3, 'claude', NULL, ?4, ?5)",
                rusqlite::params![
                    format!("{id}-t{position}"),
                    id,
                    position as i64,
                    task["status"].as_str().unwrap_or("running"),
                    task.to_string(),
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO views (conversation_id, rev, written_ms, body) VALUES (?1, 1, \
             CAST(unixepoch('subsec') * 1000 AS INTEGER), ?2) ON CONFLICT (conversation_id) \
             DO UPDATE SET rev = rev + 1, written_ms = excluded.written_ms, body = excluded.body",
            rusqlite::params![id, view.to_string()],
        )
        .unwrap();
}
