use super::Event;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub(super) fn path(root: &Path, preview: bool) -> PathBuf {
    if preview {
        crate::preferences::dir().join("usage.db")
    } else {
        root.join("usage.db")
    }
}

pub(super) fn open(root: &Path) -> rusqlite::Result<Connection> {
    let path = path(root, crate::PREVIEW);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS source_cursors(source_path TEXT PRIMARY KEY, provider TEXT NOT NULL, file_size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, byte_offset INTEGER NOT NULL, head_hash_4k TEXT NOT NULL, parser_state TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS usage_events(event_key TEXT PRIMARY KEY, provider TEXT NOT NULL, session_id TEXT NOT NULL, root_session_id TEXT NOT NULL, thread_hash TEXT NOT NULL, ts_ms INTEGER NOT NULL, model TEXT NOT NULL, fresh_input INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_write_5m INTEGER NOT NULL, cache_write_1h INTEGER NOT NULL, output INTEGER NOT NULL, cache_unknown INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS usage_ts ON usage_events(ts_ms);
        CREATE INDEX IF NOT EXISTS usage_thread ON usage_events(thread_hash);
        CREATE INDEX IF NOT EXISTS usage_session ON usage_events(session_id);")?;
    Ok(db)
}

#[derive(Default)]
pub(super) struct Cursor {
    pub size: i64,
    pub mtime: i64,
    pub offset: i64,
    pub head: String,
    pub state: String,
}

pub(super) fn cursor(db: &Connection, path: &str) -> rusqlite::Result<Option<Cursor>> {
    db.query_row("SELECT file_size, mtime_ns, byte_offset, head_hash_4k, parser_state FROM source_cursors WHERE source_path=?1", [path], |r| Ok(Cursor { size:r.get(0)?, mtime:r.get(1)?, offset:r.get(2)?, head:r.get(3)?, state:r.get(4)? })).optional()
}

pub(super) fn save_cursor(
    db: &Connection,
    path: &str,
    provider: &str,
    c: &Cursor,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO source_cursors VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![path, provider, c.size, c.mtime, c.offset, c.head, c.state],
    )?;
    Ok(())
}

pub(super) fn save(db: &Connection, e: &Event) -> rusqlite::Result<()> {
    let previous: Option<i64> = db
        .query_row(
            "SELECT ts_ms FROM usage_events WHERE event_key=?1",
            [&e.key],
            |r| r.get(0),
        )
        .optional()?;
    if previous.is_some_and(|ts| ts > e.ts) {
        return Ok(());
    }
    db.execute(
        "INSERT OR REPLACE INTO usage_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            e.key, e.provider, e.session, e.root, e.thread, e.ts, e.model, e.fresh, e.read,
            e.write5, e.write1, e.output, e.unknown
        ],
    )?;
    Ok(())
}

pub(super) fn events(db: &Connection, from: i64, to: i64) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = db.prepare("SELECT event_key,provider,session_id,root_session_id,thread_hash,ts_ms,model,fresh_input,cache_read,cache_write_5m,cache_write_1h,output,cache_unknown FROM usage_events WHERE ts_ms>=?1 AND ts_ms<?2 ORDER BY ts_ms,event_key")?;
    stmt.query_map(params![from, to], |r| {
        Ok(Event {
            key: r.get(0)?,
            provider: r.get(1)?,
            session: r.get(2)?,
            root: r.get(3)?,
            thread: r.get(4)?,
            ts: r.get(5)?,
            model: r.get(6)?,
            fresh: r.get(7)?,
            read: r.get(8)?,
            write5: r.get(9)?,
            write1: r.get(10)?,
            output: r.get(11)?,
            unknown: r.get(12)?,
        })
    })?
    .collect()
}
