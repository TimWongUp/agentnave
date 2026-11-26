//! Read-only usage scanner for Hermes (`~/.hermes/state.db` and `profiles/*/state.db`, ADR-0129).
//!
//! Reads aggregated per-(session, model) usage from `session_model_usage` joined with `sessions`.
//! Stable `event_key` = `hermes:<session_id>:<model>` so growing active sessions smoothly update
//! the 7-day detail row and hourly bucket deltas without double counting.

use super::{Event, buckets, db, parsers, zcode};
use crate::projects::Project;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

fn secs_or_ms_to_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let ms = if value > 1e11 { value } else { value * 1000.0 };
    Some(ms.round() as i64)
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

struct HermesRow {
    session_id: String,
    model: String,
    api_call_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    first_seen: Option<f64>,
    last_seen: Option<f64>,
    started_at: Option<f64>,
    cwd: String,
    parent_session_id: String,
}

fn build_query(source: &Connection) -> String {
    let sess_cols = table_columns(source, "sessions");
    let has_sessions = !sess_cols.is_empty();
    let cwd_expr = match (
        sess_cols.contains("cwd"),
        sess_cols.contains("git_repo_root"),
    ) {
        (true, true) => "COALESCE(NULLIF(s.cwd, ''), NULLIF(s.git_repo_root, ''), '')",
        (true, false) => "COALESCE(s.cwd, '')",
        (false, true) => "COALESCE(s.git_repo_root, '')",
        (false, false) => "''",
    };
    let parent_expr = if sess_cols.contains("parent_session_id") {
        "COALESCE(s.parent_session_id, '')"
    } else {
        "''"
    };
    let started_expr = if sess_cols.contains("started_at") {
        "s.started_at"
    } else {
        "NULL"
    };
    let join_clause = if has_sessions {
        "LEFT JOIN sessions s ON s.id = smu.session_id"
    } else {
        ""
    };
    format!(
        "SELECT
            smu.session_id,
            smu.model,
            SUM(smu.api_call_count),
            SUM(smu.input_tokens),
            SUM(smu.output_tokens),
            SUM(smu.cache_read_tokens),
            SUM(smu.cache_write_tokens),
            SUM(smu.reasoning_tokens),
            MIN(smu.first_seen),
            MAX(smu.last_seen),
            {started_expr},
            {cwd_expr},
            {parent_expr}
         FROM session_model_usage smu
         {join_clause}
         GROUP BY smu.session_id, smu.model
         HAVING COALESCE(MAX(smu.last_seen), MIN(smu.first_seen), {started_expr}, 0) >= ?1"
    )
}

fn to_event(row: &HermesRow) -> Option<Event> {
    if row.session_id.is_empty() || row.model.is_empty() {
        return None;
    }
    let ts = row
        .last_seen
        .and_then(secs_or_ms_to_ms)
        .or_else(|| row.first_seen.and_then(secs_or_ms_to_ms))
        .or_else(|| row.started_at.and_then(secs_or_ms_to_ms))?;
    let root = if row.parent_session_id.is_empty() {
        row.session_id.as_str()
    } else {
        row.parent_session_id.as_str()
    };
    let mut e = parsers::event(
        "hermes",
        &row.session_id,
        root,
        ts,
        &row.model,
        format!("hermes:{}:{}", row.session_id, row.model),
    );
    e.cwd = row.cwd.clone();
    e.calls = row.api_call_count.max(1);
    e.fresh = row.input_tokens.max(0);
    e.read = row.cache_read_tokens.max(0);
    e.write5 = row.cache_write_tokens.max(0);
    e.output = row
        .output_tokens
        .max(0)
        .saturating_add(row.reasoning_tokens.max(0));
    (e.total() > 0).then_some(e)
}

pub(super) fn scan_with<F>(
    conn: &mut Connection,
    path: &Path,
    busy_timeout: Duration,
    now_ms: i64,
    resolve: &mut F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&Event) -> (Project, &'static str),
{
    let Some((size, mtime)) = zcode::fingerprint(path) else {
        return Ok(());
    };
    let name = path.to_string_lossy();
    let mut cursor = db::cursor(conn, &name)?.unwrap_or_default();
    if cursor.size == size && cursor.mtime == mtime {
        return Ok(());
    }
    let source = zcode::open_with(path, busy_timeout)?;
    if !table_exists(&source, "session_model_usage") {
        return Ok(());
    }
    let newest_raw: f64 = source.query_row(
        "SELECT COALESCE(MAX(COALESCE(last_seen, first_seen, 0)), 0) FROM session_model_usage",
        [],
        |r| r.get(0),
    )?;
    let newest_ms = secs_or_ms_to_ms(newest_raw).unwrap_or(0);
    if newest_ms < cursor.offset {
        cursor = db::Cursor::default();
    }
    let retention_floor = now_ms.saturating_sub(buckets::RETENTION_WINDOW_MS);
    // Small 5s slack handles floating-point rounding and concurrent writes; ingest is idempotent.
    let since_ms = cursor.offset.saturating_sub(5_000).max(retention_floor);
    let since_sec = (since_ms as f64) / 1000.0;
    let sql = build_query(&source);
    let tx = conn.transaction()?;
    let mut watermark = cursor.offset;
    {
        let mut stmt = source.prepare(&sql)?;
        let mut rows = stmt.query_map(params![since_sec], |r| {
            Ok(HermesRow {
                session_id: r.get(0)?,
                model: r.get(1)?,
                api_call_count: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                input_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                output_tokens: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                cache_read_tokens: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                cache_write_tokens: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                reasoning_tokens: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                first_seen: r.get(8)?,
                last_seen: r.get(9)?,
                started_at: r.get(10)?,
                cwd: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
                parent_session_id: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
            })
        })?;
        while let Some(row) = rows.next().transpose()? {
            if let Some(e) = to_event(&row) {
                watermark = watermark.max(e.ts);
                if e.ts >= retention_floor && e.ts <= now_ms.saturating_add(buckets::DAY) {
                    let (project, role_kind) = resolve(&e);
                    db::save_event(&tx, &e, &project, role_kind, now_ms)?;
                }
            }
        }
    }
    cursor.size = size;
    cursor.mtime = mtime;
    cursor.offset = watermark;
    db::save_cursor(&tx, &name, "hermes", &cursor)?;
    tx.commit()?;
    Ok(())
}
