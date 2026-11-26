//! Shared read-only SQLite scanners for ZCode, OpenCode (v1 & v2), and optional Devin CLI.
//!
//! OpenCode v1 (`session`/`message`) and v2 (`session_v2`/`session_message`) token and directory
//! semantics are ported from yetone/magpie `internal/sessions/opencode.go` and
//! `internal/sessions/calls_opencode.go` (MIT).

#[cfg(test)]
use super::now;
use super::{Event, buckets, db, parsers};
use crate::projects::Project;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// ZCode's session database, or the one `config.json` points to.
pub(crate) fn database() -> PathBuf {
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

/// Opens a SQLite database strictly read-only so the owning app is never blocked.
/// Only permission/read-only errors fall back to an immutable snapshot; lock contention
/// (`SQLITE_BUSY` / `SQLITE_LOCKED`) returns an error so the next scan retries safely.
pub(super) fn open_with(path: &Path, busy_timeout: Duration) -> rusqlite::Result<Connection> {
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
pub(super) fn fingerprint(path: &Path) -> Option<(i64, i64)> {
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

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

fn table_columns(conn: &Connection, table: &str) -> HashSet<String> {
    let sql = format!("PRAGMA table_info({table})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return HashSet::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(1))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .collect()
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

fn load_sessions(
    source: &Connection,
    table: &str,
) -> rusqlite::Result<(HashMap<String, String>, HashMap<String, String>)> {
    let mut parents = HashMap::new();
    let mut dirs = HashMap::new();
    if !table_exists(source, table) {
        return Ok((parents, dirs));
    }
    let cols = table_columns(source, table);
    let parent_expr = if cols.contains("parent_id") {
        "COALESCE(parent_id, '')"
    } else {
        "''"
    };
    let dir_expr = if cols.contains("directory") {
        "COALESCE(directory, '')"
    } else {
        "''"
    };
    let sql = format!("SELECT id, {parent_expr}, {dir_expr} FROM {table}");
    let mut stmt = source.prepare(&sql)?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, parent, dir) = row?;
        if !parent.is_empty() {
            parents.insert(id.clone(), parent);
        }
        if !dir.is_empty() {
            dirs.insert(id, dir);
        }
    }
    Ok((parents, dirs))
}

fn count_i64(value: &Value) -> i64 {
    value.as_i64().unwrap_or(0).max(0)
}

/// Parse one assistant message from ZCode or OpenCode v1 `message` table.
/// ZCode (AI SDK): `input` includes cache read/write, `output` includes reasoning.
/// OpenCode: `input` excludes cache, `output` excludes `reasoning` (per Magpie MIT).
fn event_v1(
    db: &Connection,
    provider: &str,
    id: &str,
    session: &str,
    root: &str,
    session_dir: &str,
    data: &Value,
) -> Option<Event> {
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
    let mut e = parsers::event(
        provider,
        session,
        root,
        ts,
        model,
        format!("{provider}:{id}"),
    );
    e.cwd = data["path"]["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(session_dir)
        .to_owned();
    e.read = count_i64(&tokens["cache"]["read"]);
    e.write5 = count_i64(&tokens["cache"]["write"]);
    if provider == "zcode" {
        e.fresh = count_i64(&tokens["input"]).saturating_sub(e.read.saturating_add(e.write5));
        e.output = count_i64(&tokens["output"]);
    } else {
        e.fresh = count_i64(&tokens["input"]);
        e.output = count_i64(&tokens["output"]).saturating_add(count_i64(&tokens["reasoning"]));
    }
    (e.total() > 0).then_some(e)
}

/// Parse one assistant/compaction message from OpenCode v2 `session_message` table.
fn event_v2(id: &str, session: &str, root: &str, session_dir: &str, data: &Value) -> Option<Event> {
    let model = data["model"]["id"]
        .as_str()
        .filter(|m| !m.is_empty())
        .or_else(|| data["modelID"].as_str().filter(|m| !m.is_empty()))
        .or_else(|| data["modelId"].as_str().filter(|m| !m.is_empty()))?;
    let time = &data["time"];
    let ts = time["completed"]
        .as_i64()
        .filter(|t| *t > 0)
        .or_else(|| time["created"].as_i64().filter(|t| *t > 0))?;
    let tokens = &data["tokens"];
    let mut e = parsers::event(
        "opencode",
        session,
        root,
        ts,
        model,
        format!("opencode:{id}"),
    );
    e.cwd = data["path"]["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(session_dir)
        .to_owned();
    e.read = count_i64(&tokens["cache"]["read"]);
    e.write5 = count_i64(&tokens["cache"]["write"]);
    e.fresh = count_i64(&tokens["input"]);
    e.output = count_i64(&tokens["output"]).saturating_add(count_i64(&tokens["reasoning"]));
    (e.total() > 0).then_some(e)
}

/// Reads ZCode messages written since the last scan into the usage database.
#[cfg(test)]
pub(super) fn scan(conn: &mut Connection, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    scan_with_timeout(conn, path, Duration::from_secs(2))
}

#[cfg(test)]
pub(super) fn scan_with_timeout(
    conn: &mut Connection,
    path: &Path,
    busy_timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut fallback = |_: &Event| (Project::other(), "native");
    scan_provider(conn, path, "zcode", busy_timeout, now(), &mut fallback)
}

struct ScanCtx<'a, F> {
    source: &'a Connection,
    tx: &'a Connection,
    provider: &'a str,
    since: i64,
    retention_floor: i64,
    now_ms: i64,
    resolve: &'a mut F,
}

fn scan_v1_messages<F: FnMut(&Event) -> (Project, &'static str)>(
    ctx: &mut ScanCtx<'_, F>,
    parents: &HashMap<String, String>,
    dirs: &HashMap<String, String>,
    watermark: &mut i64,
) -> rusqlite::Result<()> {
    let mut stmt = ctx.source.prepare(
        "SELECT id, session_id, time_updated, data FROM message
         WHERE time_updated >= ?1 ORDER BY time_updated",
    )?;
    let mut rows = stmt.query_map(params![ctx.since], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    while let Some(row) = rows.next().transpose()? {
        let (id, session, updated, data) = row;
        *watermark = (*watermark).max(updated);
        let Ok(data) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let s_dir = dirs.get(&session).map(String::as_str).unwrap_or("");
        if let Some(e) = event_v1(
            ctx.source,
            ctx.provider,
            &id,
            &session,
            &root(parents, &session),
            s_dir,
            &data,
        ) && e.ts >= ctx.retention_floor
            && e.ts <= ctx.now_ms.saturating_add(buckets::DAY)
        {
            let (project, role_kind) = (ctx.resolve)(&e);
            db::save_event(ctx.tx, &e, &project, role_kind, ctx.now_ms)?;
        }
    }
    Ok(())
}

fn scan_v2_messages<F: FnMut(&Event) -> (Project, &'static str)>(
    ctx: &mut ScanCtx<'_, F>,
    parents_v2: &HashMap<String, String>,
    parents_v1: &HashMap<String, String>,
    dirs_v2: &HashMap<String, String>,
    dirs_v1: &HashMap<String, String>,
    watermark: &mut i64,
) -> rusqlite::Result<()> {
    let mut stmt = ctx.source.prepare(
        "SELECT id, session_id, time_updated, data FROM session_message
         WHERE type IN ('assistant', 'compaction') AND time_updated >= ?1
         ORDER BY time_updated",
    )?;
    let mut rows = stmt.query_map(params![ctx.since], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    while let Some(row) = rows.next().transpose()? {
        let (id, session, updated, data) = row;
        *watermark = (*watermark).max(updated);
        let Ok(data) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let s_dir = dirs_v2
            .get(&session)
            .or_else(|| dirs_v1.get(&session))
            .map(String::as_str)
            .unwrap_or("");
        let root_id = if parents_v2.contains_key(&session) {
            root(parents_v2, &session)
        } else {
            root(parents_v1, &session)
        };
        if let Some(e) = event_v2(&id, &session, &root_id, s_dir, &data)
            && e.ts >= ctx.retention_floor
            && e.ts <= ctx.now_ms.saturating_add(buckets::DAY)
        {
            let (project, role_kind) = (ctx.resolve)(&e);
            db::save_event(ctx.tx, &e, &project, role_kind, ctx.now_ms)?;
        }
    }
    Ok(())
}

/// Shared incremental scanner for ZCode (`provider = "zcode"`) and OpenCode (`provider = "opencode"`).
pub(super) fn scan_provider<F>(
    conn: &mut Connection,
    path: &Path,
    provider: &str,
    busy_timeout: Duration,
    now_ms: i64,
    resolve: &mut F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&Event) -> (Project, &'static str),
{
    let Some((size, mtime)) = fingerprint(path) else {
        return Ok(());
    };
    let name = path.to_string_lossy();
    let mut cursor = db::cursor(conn, &name)?.unwrap_or_default();
    if cursor.size == size && cursor.mtime == mtime {
        return Ok(());
    }
    let source = open_with(path, busy_timeout)?;
    let has_v1 = table_exists(&source, "message");
    let has_v2 = provider == "opencode" && table_exists(&source, "session_message");
    if !has_v1 && !has_v2 {
        return Ok(());
    }
    let mut newest = 0i64;
    if has_v1 {
        let v1_max: i64 = source.query_row(
            "SELECT COALESCE(MAX(time_updated), 0) FROM message",
            [],
            |r| r.get(0),
        )?;
        newest = newest.max(v1_max);
    }
    if has_v2 {
        let v2_max: i64 = source.query_row(
            "SELECT COALESCE(MAX(time_updated), 0) FROM session_message",
            [],
            |r| r.get(0),
        )?;
        newest = newest.max(v2_max);
    }
    if newest < cursor.offset {
        cursor = db::Cursor::default();
    }
    let retention_floor = now_ms.saturating_sub(buckets::RETENTION_WINDOW_MS);
    let since = cursor.offset.max(retention_floor);
    let (parents_v1, dirs_v1) = load_sessions(&source, "session")?;
    let (parents_v2, dirs_v2) = if has_v2 {
        load_sessions(&source, "session_v2")?
    } else {
        (HashMap::new(), HashMap::new())
    };
    let tx = conn.transaction()?;
    let mut watermark = cursor.offset;
    let mut ctx = ScanCtx {
        source: &source,
        tx: &tx,
        provider,
        since,
        retention_floor,
        now_ms,
        resolve,
    };
    if has_v1 {
        scan_v1_messages(&mut ctx, &parents_v1, &dirs_v1, &mut watermark)?;
    }
    if has_v2 {
        scan_v2_messages(
            &mut ctx,
            &parents_v2,
            &parents_v1,
            &dirs_v2,
            &dirs_v1,
            &mut watermark,
        )?;
    }
    cursor.size = size;
    cursor.mtime = mtime;
    cursor.offset = watermark;
    db::save_cursor(&tx, &name, provider, &cursor)?;
    tx.commit()?;
    Ok(())
}

/// Optional read-only probe for Devin CLI (`~/.devin/sessions.db`): zero overhead when absent,
/// safely skips databases whose schema has no token columns.
pub(super) fn scan_devin<F>(
    conn: &mut Connection,
    path: &Path,
    now_ms: i64,
    resolve: &mut F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&Event) -> (Project, &'static str),
{
    if !path.is_file() {
        return Ok(());
    }
    let Some((size, mtime)) = fingerprint(path) else {
        return Ok(());
    };
    let name = path.to_string_lossy();
    let mut cursor = db::cursor(conn, &name)?.unwrap_or_default();
    if cursor.size == size && cursor.mtime == mtime {
        return Ok(());
    }
    let source = open_with(path, Duration::from_secs(1))?;
    if !table_exists(&source, "sessions") {
        return Ok(());
    }
    let cols = table_columns(&source, "sessions");
    if !(cols.contains("input_tokens") && cols.contains("output_tokens")) {
        cursor.size = size;
        cursor.mtime = mtime;
        db::save_cursor(conn, &name, "devin", &cursor)?;
        return Ok(());
    }
    let id_col = if cols.contains("session_id") {
        "session_id"
    } else {
        "id"
    };
    let model_expr = if cols.contains("model") {
        "COALESCE(model, 'devin')"
    } else {
        "'devin'"
    };
    let cwd_expr = if cols.contains("cwd") {
        "COALESCE(cwd, '')"
    } else {
        "''"
    };
    let ts_expr = if cols.contains("updated_at") {
        "updated_at"
    } else if cols.contains("created_at") {
        "created_at"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT {id_col}, {model_expr}, {cwd_expr}, {ts_expr}, input_tokens, output_tokens
         FROM sessions WHERE COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0) > 0"
    );
    let retention_floor = now_ms.saturating_sub(buckets::RETENTION_WINDOW_MS);
    let tx = conn.transaction()?;
    if let Ok(mut stmt) = source.prepare(&sql) {
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                r.get::<_, Option<i64>>(5)?.unwrap_or(0),
            ))
        })?;
        for row in rows.flatten() {
            let (id, model, cwd, raw_ts, inp, out) = row;
            let ts = if raw_ts > 100_000_000_000 {
                raw_ts
            } else if raw_ts > 0 {
                raw_ts * 1000
            } else {
                continue;
            };
            if ts < retention_floor || ts > now_ms.saturating_add(buckets::DAY) {
                continue;
            }
            let mut e = parsers::event("devin", &id, &id, ts, &model, format!("devin:{id}"));
            e.cwd = cwd;
            e.fresh = inp.max(0);
            e.output = out.max(0);
            e.unknown = true;
            if e.total() > 0 {
                let (project, role_kind) = resolve(&e);
                db::save_event(&tx, &e, &project, role_kind, now_ms)?;
            }
        }
    }
    cursor.size = size;
    cursor.mtime = mtime;
    db::save_cursor(&tx, &name, "devin", &cursor)?;
    tx.commit()?;
    Ok(())
}
