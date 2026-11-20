use super::{Event, now, parsers};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DAY: i64 = 86_400_000;

/// ZCode's session database, or the one `config.json` points to.
pub(super) fn database() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_default().join(".zcode/cli");
    super::read_small_json(&base.join("config.json"))
        .and_then(|config| {
            config["storage"]["sessionDbPath"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|path| !path.is_empty())
        .map_or_else(
            || base.join("db/db.sqlite"),
            |path| match path.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                None => PathBuf::from(path),
            },
        )
}

fn is_readonly_or_perm(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::CannotOpen
                    | rusqlite::ErrorCode::ReadOnly
                    | rusqlite::ErrorCode::PermissionDenied,
                ..
            },
            _,
        )
    )
}

/// Opens ZCode's database without ever writing to it, so the app is never blocked.
/// Only permission/read-only errors fall back to an immutable snapshot; lock contention
/// (`SQLITE_BUSY` / `SQLITE_LOCKED`) returns an error so the next scan retries safely.
pub(super) fn open_with(
    path: &Path,
    busy_timeout: std::time::Duration,
) -> rusqlite::Result<Connection> {
    let probe = |db: Connection| {
        db.busy_timeout(busy_timeout)?;
        db.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))?;
        Ok(db)
    };
    match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).and_then(probe) {
        Ok(db) => Ok(db),
        Err(err) if is_readonly_or_perm(&err) => {
            let mut uri = String::from("file:");
            let text = path.to_string_lossy().replace('\\', "/");
            if !text.starts_with('/') {
                uri.push('/');
            }
            for c in text.chars() {
                match c {
                    '%' | '?' | '#' | ' ' => uri.push_str(&format!("%{:02X}", c as u32)),
                    _ => uri.push(c),
                }
            }
            uri.push_str("?immutable=1");
            Connection::open_with_flags(
                uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .and_then(probe)
        }
        Err(err) => Err(err),
    }
}

/// Size and newest mtime of the database and its WAL: a change in either means new rows.
fn fingerprint(path: &Path) -> Option<(i64, i64)> {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let mut size = 0;
    let mut mtime = 0;
    for file in [path.to_path_buf(), PathBuf::from(wal)] {
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        size += meta.len() as i64;
        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        mtime = mtime.max(modified.as_nanos().min(i64::MAX as u128) as i64);
    }
    (size > 0).then_some((size, mtime))
}

fn root(parents: &HashMap<String, String>, session: &str) -> String {
    let mut root = session;
    for _ in 0..64 {
        match parents.get(root) {
            Some(parent) if parent != root => root = parent,
            _ => break,
        }
    }
    root.to_owned()
}

/// One assistant message is one call. The AI SDK counts: `input` includes the cache
/// read and written, `output` includes the reasoning. A copy of a message still in the
/// database (side chat, fork, restored checkpoint) keeps the original's tokens, and the
/// original's session counts them.
fn event(db: &Connection, id: &str, session: &str, root: &str, data: &Value) -> Option<Event> {
    if data["role"] != "assistant" {
        return None;
    }
    let origin = data["metadata"]["forkOrigin"]["messageId"]
        .as_str()
        .unwrap_or("");
    if !origin.is_empty()
        && origin != id
        && db
            .query_row("SELECT 1 FROM message WHERE id=?1", [origin], |_| Ok(()))
            .is_ok()
    {
        return None;
    }
    let model = ["modelId", "modelID"]
        .iter()
        .find_map(|key| data[*key].as_str().filter(|m| !m.is_empty()))
        .or_else(|| data["model"]["id"].as_str().filter(|m| !m.is_empty()))?;
    let time = &data["time"];
    let ts = time["completed"]
        .as_i64()
        .filter(|t| *t > 0)
        .or_else(|| time["created"].as_i64().filter(|t| *t > 0))?;
    let tokens = &data["tokens"];
    let count = |value: &Value| value.as_i64().unwrap_or(0).max(0);
    let mut e = parsers::event("zcode", session, root, ts, model, format!("zcode:{id}"));
    e.read = count(&tokens["cache"]["read"]);
    e.write5 = count(&tokens["cache"]["write"]);
    e.fresh = count(&tokens["input"]).saturating_sub(e.read.saturating_add(e.write5));
    e.output = count(&tokens["output"]);
    (e.total() > 0).then_some(e)
}

/// Reads messages written since the last scan into the usage database.
pub(super) fn scan(conn: &mut Connection, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    scan_with_timeout(conn, path, std::time::Duration::from_secs(2))
}

pub(super) fn scan_with_timeout(
    conn: &mut Connection,
    path: &Path,
    busy_timeout: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((size, mtime)) = fingerprint(path) else {
        return Ok(());
    };
    let name = path.to_string_lossy();
    let mut cursor = super::db::cursor(conn, &name)?.unwrap_or_default();
    if cursor.size == size && cursor.mtime == mtime {
        return Ok(());
    }
    let source = open_with(path, busy_timeout)?;
    let newest: i64 = source.query_row(
        "SELECT COALESCE(MAX(time_updated), 0) FROM message",
        [],
        |r| r.get(0),
    )?;
    if newest < cursor.offset {
        cursor = super::db::Cursor::default();
    }
    let since = cursor.offset.max(now() - 31 * DAY);
    let mut parents = HashMap::new();
    let mut stmt =
        source.prepare("SELECT id, parent_id FROM session WHERE parent_id IS NOT NULL")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, parent) = row?;
        parents.insert(id, parent);
    }
    let tx = conn.transaction()?;
    let mut watermark = cursor.offset;
    {
        let mut stmt = source.prepare(
            "SELECT id, session_id, time_updated, data FROM message WHERE time_updated >= ?1 ORDER BY time_updated",
        )?;
        let mut rows = stmt.query_map(params![since], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        while let Some(row) = rows.next().transpose()? {
            let (id, session, updated, data) = row;
            watermark = watermark.max(updated);
            let Ok(data) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            if let Some(e) = event(&source, &id, &session, &root(&parents, &session), &data)
                && e.ts >= now() - 31 * DAY
                && e.ts <= now() + DAY
            {
                super::db::save(&tx, &e)?;
            }
        }
    }
    cursor.size = size;
    cursor.mtime = mtime;
    cursor.offset = watermark;
    super::db::save_cursor(&tx, &name, "zcode", &cursor)?;
    tx.commit()?;
    Ok(())
}
