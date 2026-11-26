use super::{Event, buckets, now};
use crate::projects::Project;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) fn path(root: &Path, preview: bool) -> PathBuf {
    if preview {
        crate::preferences::dir().join("usage.db")
    } else {
        root.join("usage.db")
    }
}

pub(super) fn open(root: &Path) -> rusqlite::Result<Connection> {
    open_at(&path(root, crate::PREVIEW))
}

pub(super) fn open_at(path: &Path) -> rusqlite::Result<Connection> {
    open_at_time(path, now())
}

pub(super) fn open_at_time(path: &Path, now_ms: i64) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    let mut db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS source_cursors(
            source_path TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            mtime_ns INTEGER NOT NULL,
            byte_offset INTEGER NOT NULL,
            head_hash_4k TEXT NOT NULL,
            parser_state TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS usage_events(
            event_key TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            session_id TEXT NOT NULL,
            root_session_id TEXT NOT NULL,
            thread_hash TEXT NOT NULL,
            ts_ms INTEGER NOT NULL,
            model TEXT NOT NULL,
            fresh_input INTEGER NOT NULL,
            cache_read INTEGER NOT NULL,
            cache_write_5m INTEGER NOT NULL,
            cache_write_1h INTEGER NOT NULL,
            output INTEGER NOT NULL,
            cache_unknown INTEGER NOT NULL,
            cwd TEXT NOT NULL DEFAULT '',
            project_key TEXT NOT NULL DEFAULT 'other',
            role_kind TEXT NOT NULL DEFAULT 'native',
            cost_usd REAL,
            unpriced_tokens INTEGER NOT NULL DEFAULT 0,
            calls INTEGER NOT NULL DEFAULT 1
        );
        CREATE INDEX IF NOT EXISTS usage_ts ON usage_events(ts_ms);
        CREATE INDEX IF NOT EXISTS usage_thread ON usage_events(thread_hash);
        CREATE INDEX IF NOT EXISTS usage_session ON usage_events(session_id);
        CREATE TABLE IF NOT EXISTS usage_hourly_buckets (
            hour_ts_utc INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            project_key TEXT NOT NULL,
            role_kind TEXT NOT NULL,
            calls INTEGER NOT NULL,
            fresh_input INTEGER NOT NULL,
            cache_read INTEGER NOT NULL,
            cache_write_5m INTEGER NOT NULL,
            cache_write_1h INTEGER NOT NULL,
            output INTEGER NOT NULL,
            cache_known_prompt INTEGER NOT NULL,
            cache_known_read INTEGER NOT NULL,
            cost_usd REAL NOT NULL,
            unpriced_tokens INTEGER NOT NULL,
            unpriced_calls INTEGER NOT NULL,
            PRIMARY KEY (hour_ts_utc, provider, model, project_key, role_kind)
        );
        CREATE INDEX IF NOT EXISTS usage_hourly_ts ON usage_hourly_buckets(hour_ts_utc);
        CREATE TABLE IF NOT EXISTS usage_dedupe (
            event_hash BLOB PRIMARY KEY,
            ts_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS usage_dedupe_ts ON usage_dedupe(ts_ms);
        CREATE TABLE IF NOT EXISTS usage_projects (
            project_key TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            path TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );",
    )?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 3 {
        migrate_to_v3(&mut db, version, now_ms)?;
    }
    Ok(db)
}

fn migrate_to_v3(db: &mut Connection, version: i64, now_ms: i64) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    if version < 1 {
        tx.execute(
            "DELETE FROM usage_events WHERE fresh_input = 0 AND cache_read = 0
             AND cache_write_5m = 0 AND cache_write_1h = 0 AND output = 0",
            [],
        )?;
    }
    let cols: HashSet<String> = {
        let mut stmt = tx.prepare("PRAGMA table_info(usage_events)")?;
        stmt.query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (col, ddl) in [
        (
            "cwd",
            "ALTER TABLE usage_events ADD COLUMN cwd TEXT NOT NULL DEFAULT ''",
        ),
        (
            "project_key",
            "ALTER TABLE usage_events ADD COLUMN project_key TEXT NOT NULL DEFAULT 'other'",
        ),
        (
            "role_kind",
            "ALTER TABLE usage_events ADD COLUMN role_kind TEXT NOT NULL DEFAULT 'native'",
        ),
        (
            "cost_usd",
            "ALTER TABLE usage_events ADD COLUMN cost_usd REAL",
        ),
        (
            "unpriced_tokens",
            "ALTER TABLE usage_events ADD COLUMN unpriced_tokens INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "calls",
            "ALTER TABLE usage_events ADD COLUMN calls INTEGER NOT NULL DEFAULT 1",
        ),
    ] {
        if !cols.contains(col) {
            tx.execute(ddl, [])?;
        }
    }
    let has_cursors: i64 = tx.query_row("SELECT COUNT(*) FROM source_cursors", [], |r| r.get(0))?;
    if has_cursors > 0 {
        // Legacy v1/v2 cursors predate per-event `cwd` project attribution and the WorkBuddy
        // provider split; reset once on upgrade so the next scan cleanly re-indexes 92 days.
        tx.execute_batch(
            "DELETE FROM source_cursors;
             DELETE FROM usage_events;
             DELETE FROM usage_hourly_buckets;
             DELETE FROM usage_dedupe;
             DELETE FROM usage_projects;",
        )?;
    } else if version < 2 {
        buckets::backfill_v1_events(&tx, now_ms)?;
    }
    tx.pragma_update(None, "user_version", 3)?;
    tx.commit()?;
    let _ = db.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;");
    Ok(())
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
    db.query_row(
        "SELECT file_size, mtime_ns, byte_offset, head_hash_4k, parser_state FROM source_cursors WHERE source_path=?1",
        [path],
        |r| {
            Ok(Cursor {
                size: r.get(0)?,
                mtime: r.get(1)?,
                offset: r.get(2)?,
                head: r.get(3)?,
                state: r.get(4)?,
            })
        },
    )
    .optional()
}

pub(super) fn save_cursor(
    db: &Connection,
    path: &str,
    provider: &str,
    c: &Cursor,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO source_cursors(source_path, provider, file_size, mtime_ns, byte_offset, head_hash_4k, parser_state) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![path, provider, c.size, c.mtime, c.offset, c.head, c.state],
    )?;
    Ok(())
}

/// Ingest an event with resolved project & role attribution into the three-layer store.
pub(super) fn save_event(
    db: &Connection,
    e: &Event,
    project: &Project,
    role_kind: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    buckets::ingest(db, e, project, role_kind, now_ms)
}

/// Convenience helper for tests using arbitrary event timestamps (treated as within the 7d window).
#[cfg(test)]
pub(super) fn save(db: &Connection, e: &Event) -> rusqlite::Result<()> {
    buckets::ingest(db, e, &Project::other(), "native", e.ts)
}

pub(super) fn prune(db: &mut Connection, now_ms: i64) -> rusqlite::Result<()> {
    buckets::prune(db, now_ms)?;
    let _ = db.execute_batch("PRAGMA incremental_vacuum(256);");
    Ok(())
}

pub(super) fn events(db: &Connection, from: i64, to: i64) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = db.prepare(
        "SELECT event_key, provider, session_id, root_session_id, thread_hash,
                ts_ms, model, fresh_input, cache_read, cache_write_5m, cache_write_1h,
                output, cache_unknown, cwd, calls
         FROM usage_events
         WHERE ts_ms >= ?1 AND ts_ms < ?2
         ORDER BY ts_ms, event_key",
    )?;
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
            cwd: r.get(13)?,
            calls: r.get(14)?,
        })
    })?
    .collect()
}
