//! Three-layer SQLite storage (ADR-0129):
//! 1. `usage_hourly_buckets` (92 days): pre-aggregated UTC hourly totals per
//!    `(hour_ts_utc, provider, model, project_key, role_kind)`.
//! 2. `usage_events` (7 days): recent detail rows for active-session revision deltas and
//!    `/app/usage/workspace`.
//! 3. `usage_dedupe` (92 days): 16-byte truncated SHA-256 of `event_key` preventing double-count
//!    when older sessions are forked/copied after the 7-day detail window expires.
//!
//! Also maintains `usage_projects` persisting `project_key -> (label, path)`.

use super::{Event, pricing};
use crate::projects::{OTHER, Project};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const HOUR: i64 = 3_600_000;
pub(super) const DAY: i64 = 86_400_000;
pub(super) const DETAIL_WINDOW_MS: i64 = 7 * DAY;
pub(super) const RETENTION_WINDOW_MS: i64 = 92 * DAY;

pub(super) fn hour_ts_utc(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(HOUR) * HOUR
}

pub(super) fn event_hash(key: &str) -> [u8; 16] {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

#[derive(Clone, Debug)]
pub(super) struct BucketRow {
    pub hour_ts_utc: i64,
    pub provider: String,
    pub model: String,
    pub project_key: String,
    pub role_kind: String,
    pub calls: i64,
    pub fresh_input: i64,
    pub cache_read: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub output: i64,
    pub cache_known_prompt: i64,
    pub cache_known_read: i64,
    pub cost_usd: f64,
    pub unpriced_tokens: i64,
    pub unpriced_calls: i64,
}

struct Contribution {
    hour_ts_utc: i64,
    provider: String,
    model: String,
    project_key: String,
    role_kind: String,
    calls: i64,
    fresh: i64,
    read: i64,
    write5: i64,
    write1: i64,
    output: i64,
    known_prompt: i64,
    known_read: i64,
    cost_usd: f64,
    unpriced_tokens: i64,
    unpriced_calls: i64,
}

struct StoredEvent {
    provider: String,
    session: String,
    root: String,
    thread: String,
    ts_ms: i64,
    model: String,
    fresh: i64,
    read: i64,
    write5: i64,
    write1: i64,
    output: i64,
    unknown: bool,
    cwd: String,
    project_key: String,
    role_kind: String,
    cost_usd: Option<f64>,
    unpriced_tokens: i64,
    calls: i64,
}

impl StoredEvent {
    fn to_contribution(&self) -> Contribution {
        let calls = self.calls.max(1);
        let prompt = self
            .fresh
            .saturating_add(self.read)
            .saturating_add(self.write5)
            .saturating_add(self.write1);
        Contribution {
            hour_ts_utc: hour_ts_utc(self.ts_ms),
            provider: self.provider.clone(),
            model: self.model.clone(),
            project_key: self.project_key.clone(),
            role_kind: self.role_kind.clone(),
            calls,
            fresh: self.fresh,
            read: self.read,
            write5: self.write5,
            write1: self.write1,
            output: self.output,
            known_prompt: if self.unknown { 0 } else { prompt },
            known_read: if self.unknown { 0 } else { self.read },
            cost_usd: self.cost_usd.unwrap_or(0.0),
            unpriced_tokens: self.unpriced_tokens,
            unpriced_calls: if self.cost_usd.is_none() { calls } else { 0 },
        }
    }
}

fn contribution_of(
    e: &Event,
    project_key: &str,
    role_kind: &str,
    cost_opt: Option<f64>,
) -> Contribution {
    let calls = e.calls.max(1);
    let unpriced_tokens = if cost_opt.is_none() { e.total() } else { 0 };
    let unpriced_calls = if cost_opt.is_none() { calls } else { 0 };
    Contribution {
        hour_ts_utc: hour_ts_utc(e.ts),
        provider: e.provider.clone(),
        model: e.model.clone(),
        project_key: project_key.to_owned(),
        role_kind: role_kind.to_owned(),
        calls,
        fresh: e.fresh,
        read: e.read,
        write5: e.write5,
        write1: e.write1,
        output: e.output,
        known_prompt: if e.unknown { 0 } else { e.prompt() },
        known_read: if e.unknown { 0 } else { e.read },
        cost_usd: cost_opt.unwrap_or(0.0),
        unpriced_tokens,
        unpriced_calls,
    }
}

fn add_bucket(conn: &Connection, c: &Contribution) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO usage_hourly_buckets (
            hour_ts_utc, provider, model, project_key, role_kind,
            calls, fresh_input, cache_read, cache_write_5m, cache_write_1h, output,
            cache_known_prompt, cache_known_read, cost_usd, unpriced_tokens, unpriced_calls
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(hour_ts_utc, provider, model, project_key, role_kind) DO UPDATE SET
            calls = calls + excluded.calls,
            fresh_input = fresh_input + excluded.fresh_input,
            cache_read = cache_read + excluded.cache_read,
            cache_write_5m = cache_write_5m + excluded.cache_write_5m,
            cache_write_1h = cache_write_1h + excluded.cache_write_1h,
            output = output + excluded.output,
            cache_known_prompt = cache_known_prompt + excluded.cache_known_prompt,
            cache_known_read = cache_known_read + excluded.cache_known_read,
            cost_usd = cost_usd + excluded.cost_usd,
            unpriced_tokens = unpriced_tokens + excluded.unpriced_tokens,
            unpriced_calls = unpriced_calls + excluded.unpriced_calls",
        params![
            c.hour_ts_utc,
            c.provider,
            c.model,
            c.project_key,
            c.role_kind,
            c.calls,
            c.fresh,
            c.read,
            c.write5,
            c.write1,
            c.output,
            c.known_prompt,
            c.known_read,
            c.cost_usd,
            c.unpriced_tokens,
            c.unpriced_calls,
        ],
    )?;
    Ok(())
}

fn subtract_bucket(conn: &Connection, c: &Contribution) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE usage_hourly_buckets SET
            calls = MAX(0, calls - ?6),
            fresh_input = MAX(0, fresh_input - ?7),
            cache_read = MAX(0, cache_read - ?8),
            cache_write_5m = MAX(0, cache_write_5m - ?9),
            cache_write_1h = MAX(0, cache_write_1h - ?10),
            output = MAX(0, output - ?11),
            cache_known_prompt = MAX(0, cache_known_prompt - ?12),
            cache_known_read = MAX(0, cache_known_read - ?13),
            cost_usd = CASE WHEN cost_usd - ?14 < 1e-12 THEN 0.0 ELSE cost_usd - ?14 END,
            unpriced_tokens = MAX(0, unpriced_tokens - ?15),
            unpriced_calls = MAX(0, unpriced_calls - ?16)
        WHERE hour_ts_utc = ?1 AND provider = ?2 AND model = ?3
          AND project_key = ?4 AND role_kind = ?5",
        params![
            c.hour_ts_utc,
            c.provider,
            c.model,
            c.project_key,
            c.role_kind,
            c.calls,
            c.fresh,
            c.read,
            c.write5,
            c.write1,
            c.output,
            c.known_prompt,
            c.known_read,
            c.cost_usd,
            c.unpriced_tokens,
            c.unpriced_calls,
        ],
    )?;
    conn.execute(
        "DELETE FROM usage_hourly_buckets
         WHERE hour_ts_utc = ?1 AND provider = ?2 AND model = ?3
           AND project_key = ?4 AND role_kind = ?5
           AND calls <= 0
           AND fresh_input <= 0 AND cache_read <= 0
           AND cache_write_5m <= 0 AND cache_write_1h <= 0 AND output <= 0",
        params![
            c.hour_ts_utc,
            c.provider,
            c.model,
            c.project_key,
            c.role_kind,
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_project(
    conn: &Connection,
    project: &Project,
    now_ms: i64,
) -> rusqlite::Result<()> {
    if project.key.is_empty() || project.key == OTHER {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO usage_projects (project_key, label, path, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_key) DO UPDATE SET
             label = CASE WHEN excluded.label != '' THEN excluded.label ELSE usage_projects.label END,
             path = CASE WHEN excluded.path != '' THEN excluded.path ELSE usage_projects.path END,
             updated_at = excluded.updated_at",
        params![project.key, project.label, project.path, now_ms],
    )?;
    Ok(())
}

fn load_existing_event(conn: &Connection, key: &str) -> rusqlite::Result<Option<StoredEvent>> {
    conn.query_row(
        "SELECT provider, session_id, root_session_id, thread_hash, ts_ms, model,
                fresh_input, cache_read, cache_write_5m, cache_write_1h, output, cache_unknown,
                cwd, project_key, role_kind, cost_usd, unpriced_tokens, calls
         FROM usage_events WHERE event_key = ?1",
        [key],
        |r| {
            Ok(StoredEvent {
                provider: r.get(0)?,
                session: r.get(1)?,
                root: r.get(2)?,
                thread: r.get(3)?,
                ts_ms: r.get(4)?,
                model: r.get(5)?,
                fresh: r.get(6)?,
                read: r.get(7)?,
                write5: r.get(8)?,
                write1: r.get(9)?,
                output: r.get(10)?,
                unknown: r.get(11)?,
                cwd: r.get(12)?,
                project_key: r.get(13)?,
                role_kind: r.get(14)?,
                cost_usd: r.get(15)?,
                unpriced_tokens: r.get(16)?,
                calls: r.get(17)?,
            })
        },
    )
    .optional()
}

fn has_dedupe(conn: &Connection, hash: &[u8; 16]) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM usage_dedupe WHERE event_hash = ?1",
            [hash.as_slice()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn insert_dedupe(conn: &Connection, hash: &[u8; 16], ts_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO usage_dedupe (event_hash, ts_ms) VALUES (?1, ?2)
         ON CONFLICT(event_hash) DO UPDATE SET ts_ms = MAX(usage_dedupe.ts_ms, excluded.ts_ms)",
        params![hash.as_slice(), ts_ms],
    )?;
    Ok(())
}

fn update_existing_event(
    conn: &Connection,
    old: &StoredEvent,
    e: &Event,
    project: &Project,
    role_kind: &str,
    cost_opt: Option<f64>,
    now_ms: i64,
) -> rusqlite::Result<()> {
    if e.ts < old.ts_ms {
        return Ok(());
    }
    let proj_key = if project.key.is_empty() {
        OTHER
    } else {
        project.key.as_str()
    };
    let eff_session = if old.session.is_empty() {
        e.session.as_str()
    } else {
        old.session.as_str()
    };
    let eff_root = if old.root.is_empty() {
        e.root.as_str()
    } else {
        old.root.as_str()
    };
    let eff_thread = if old.thread.is_empty() {
        e.thread.as_str()
    } else {
        old.thread.as_str()
    };
    let eff_cwd = if old.cwd.is_empty() {
        e.cwd.as_str()
    } else {
        old.cwd.as_str()
    };
    let eff_project = if old.project_key != OTHER && !old.project_key.is_empty() {
        old.project_key.as_str()
    } else {
        proj_key
    };
    let eff_role = if old.role_kind != "native" && !old.role_kind.is_empty() {
        old.role_kind.as_str()
    } else {
        role_kind
    };
    let unpriced_tokens = if cost_opt.is_none() { e.total() } else { 0 };
    let calls = e.calls.max(1);
    subtract_bucket(conn, &old.to_contribution())?;
    conn.execute(
        "UPDATE usage_events SET
            provider = ?2, session_id = ?3, root_session_id = ?4, thread_hash = ?5,
            ts_ms = ?6, model = ?7, fresh_input = ?8, cache_read = ?9,
            cache_write_5m = ?10, cache_write_1h = ?11, output = ?12, cache_unknown = ?13,
            cwd = ?14, project_key = ?15, role_kind = ?16,
            cost_usd = ?17, unpriced_tokens = ?18, calls = ?19
         WHERE event_key = ?1",
        params![
            e.key,
            e.provider,
            eff_session,
            eff_root,
            eff_thread,
            e.ts,
            e.model,
            e.fresh,
            e.read,
            e.write5,
            e.write1,
            e.output,
            e.unknown,
            eff_cwd,
            eff_project,
            eff_role,
            cost_opt,
            unpriced_tokens,
            calls,
        ],
    )?;
    let new_c = contribution_of(e, eff_project, eff_role, cost_opt);
    add_bucket(conn, &new_c)?;
    if eff_project == proj_key {
        upsert_project(conn, project, now_ms)?;
    }
    insert_dedupe(conn, &event_hash(&e.key), e.ts)?;
    Ok(())
}

/// Ingest a single event into the three-layer store with strict deduplication and revision deltas.
pub(super) fn ingest(
    conn: &Connection,
    e: &Event,
    project: &Project,
    role_kind: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    if e.total() <= 0 {
        return Ok(());
    }
    let cost_opt = pricing::cost(e);
    let proj_key = if project.key.is_empty() {
        OTHER
    } else {
        project.key.as_str()
    };
    let hash = event_hash(&e.key);
    let within_detail = e.ts >= now_ms.saturating_sub(DETAIL_WINDOW_MS);
    if within_detail && let Some(old) = load_existing_event(conn, &e.key)? {
        return update_existing_event(conn, &old, e, project, role_kind, cost_opt, now_ms);
    }
    if has_dedupe(conn, &hash)? {
        return Ok(());
    }
    if within_detail {
        let unpriced_tokens = if cost_opt.is_none() { e.total() } else { 0 };
        conn.execute(
            "INSERT INTO usage_events (
                event_key, provider, session_id, root_session_id, thread_hash,
                ts_ms, model, fresh_input, cache_read, cache_write_5m, cache_write_1h, output,
                cache_unknown, cwd, project_key, role_kind, cost_usd, unpriced_tokens, calls
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                e.key,
                e.provider,
                e.session,
                e.root,
                e.thread,
                e.ts,
                e.model,
                e.fresh,
                e.read,
                e.write5,
                e.write1,
                e.output,
                e.unknown,
                e.cwd,
                proj_key,
                role_kind,
                cost_opt,
                unpriced_tokens,
                e.calls.max(1),
            ],
        )?;
    }
    insert_dedupe(conn, &hash, e.ts)?;
    upsert_project(conn, project, now_ms)?;
    let c = contribution_of(e, proj_key, role_kind, cost_opt);
    add_bucket(conn, &c)
}

/// Prune expired rows across all three storage layers in one transaction.
pub(super) fn prune(conn: &mut Connection, now_ms: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let detail_cutoff = now_ms.saturating_sub(DETAIL_WINDOW_MS);
    let retention_cutoff = now_ms.saturating_sub(RETENTION_WINDOW_MS);
    let bucket_cutoff = hour_ts_utc(retention_cutoff);
    tx.execute("DELETE FROM usage_events WHERE ts_ms < ?1", [detail_cutoff])?;
    tx.execute(
        "DELETE FROM usage_hourly_buckets WHERE hour_ts_utc < ?1",
        [bucket_cutoff],
    )?;
    tx.execute(
        "DELETE FROM usage_dedupe WHERE ts_ms < ?1",
        [retention_cutoff],
    )?;
    tx.execute(
        "DELETE FROM usage_projects
         WHERE project_key != ?1
           AND project_key NOT IN (SELECT DISTINCT project_key FROM usage_hourly_buckets)",
        [OTHER],
    )?;
    tx.commit()
}

pub(super) fn query_buckets(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<BucketRow>> {
    let mut stmt = conn.prepare(
        "SELECT hour_ts_utc, provider, model, project_key, role_kind,
                calls, fresh_input, cache_read, cache_write_5m, cache_write_1h, output,
                cache_known_prompt, cache_known_read, cost_usd, unpriced_tokens, unpriced_calls
         FROM usage_hourly_buckets
         WHERE hour_ts_utc >= ?1 AND hour_ts_utc < ?2
         ORDER BY hour_ts_utc, provider, model, project_key, role_kind",
    )?;
    stmt.query_map(params![from_ms, to_ms], |r| {
        Ok(BucketRow {
            hour_ts_utc: r.get(0)?,
            provider: r.get(1)?,
            model: r.get(2)?,
            project_key: r.get(3)?,
            role_kind: r.get(4)?,
            calls: r.get(5)?,
            fresh_input: r.get(6)?,
            cache_read: r.get(7)?,
            cache_write_5m: r.get(8)?,
            cache_write_1h: r.get(9)?,
            output: r.get(10)?,
            cache_known_prompt: r.get(11)?,
            cache_known_read: r.get(12)?,
            cost_usd: r.get(13)?,
            unpriced_tokens: r.get(14)?,
            unpriced_calls: r.get(15)?,
        })
    })?
    .collect()
}

pub(super) fn query_projects(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Project>> {
    let mut stmt =
        conn.prepare("SELECT project_key, label, path FROM usage_projects ORDER BY project_key")?;
    let mut map = BTreeMap::new();
    for row in stmt.query_map([], |r| {
        Ok(Project {
            key: r.get(0)?,
            label: r.get(1)?,
            path: r.get(2)?,
        })
    })? {
        let p = row?;
        map.insert(p.key.clone(), p);
    }
    Ok(map)
}

pub(super) fn min_hour_ts(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MIN(hour_ts_utc) FROM usage_hourly_buckets",
        [],
        |r| r.get(0),
    )
}

/// Backfill `usage_hourly_buckets` and `usage_dedupe` from pre-v2 `usage_events` during migration.
pub(super) fn backfill_v1_events(conn: &Connection, now_ms: i64) -> rusqlite::Result<()> {
    let rows: Vec<Event> = {
        let mut stmt = conn.prepare(
            "SELECT event_key, provider, session_id, root_session_id, thread_hash,
                    ts_ms, model, fresh_input, cache_read, cache_write_5m, cache_write_1h,
                    output, cache_unknown
             FROM usage_events",
        )?;
        stmt.query_map([], |r| {
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
                cwd: String::new(),
                calls: 1,
            })
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let retention_cutoff = now_ms.saturating_sub(RETENTION_WINDOW_MS);
    for e in rows {
        if e.total() <= 0 {
            continue;
        }
        let cost_opt = pricing::cost(&e);
        let unpriced_tokens = if cost_opt.is_none() { e.total() } else { 0 };
        conn.execute(
            "UPDATE usage_events SET cost_usd = ?2, unpriced_tokens = ?3 WHERE event_key = ?1",
            params![e.key, cost_opt, unpriced_tokens],
        )?;
        if e.ts < retention_cutoff {
            continue;
        }
        let hash = event_hash(&e.key);
        if has_dedupe(conn, &hash)? {
            continue;
        }
        insert_dedupe(conn, &hash, e.ts)?;
        let c = contribution_of(&e, OTHER, "native", cost_opt);
        add_bucket(conn, &c)?;
    }
    let detail_cutoff = now_ms.saturating_sub(DETAIL_WINDOW_MS);
    conn.execute("DELETE FROM usage_events WHERE ts_ms < ?1", [detail_cutoff])?;
    Ok(())
}
