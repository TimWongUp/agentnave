//! Eligibility and short-lived candidate probes for the read-only storage preview.
use super::*;
use std::collections::HashSet;

pub(super) fn handover_before(
    connection: &Connection,
    workspace: Option<&str>,
    cutoff: i64,
    require_timestamp: bool,
) -> Result<bool, String> {
    let Some(workspace) = workspace else {
        return Ok(!require_timestamp);
    };
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let mut seen = false;
    let row: Option<(bool, Option<String>)> = connection
        .query_row(
            "SELECT pending, released FROM workspaces WHERE id = ?1",
            [workspace],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((pending, released)) = row {
        if pending {
            return Ok(false);
        }
        if let Some(released) = released {
            let Some(ts) = crate::time_format::timestamp(&released) else {
                return Ok(false);
            };
            if ts.div_euclid(1_000) >= cutoff {
                return Ok(false);
            }
            seen = true;
        }
    }
    if version >= 4 {
        let mut statement = connection
            .prepare("SELECT claimed_at FROM workspace_links WHERE source_workspace = ?1")
            .map_err(|e| e.to_string())?;
        let rows: Vec<String> = statement
            .query_map([workspace], |row| row.get(0))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        for claimed_at in rows {
            let Some(ts) = crate::time_format::timestamp(&claimed_at) else {
                return Ok(false);
            };
            if ts.div_euclid(1_000) >= cutoff {
                return Ok(false);
            }
            seen = true;
        }
    }
    Ok(seen || !require_timestamp)
}

fn eligible(connection: &Connection, ids: &[String], cutoff: i64) -> Result<bool, String> {
    let group = conversation_group(connection, &ids[0]).map_err(|e| e.to_string())?;
    if !handover_before(connection, group.as_deref(), cutoff, false)? {
        return Ok(false);
    }
    for id in ids {
        let row = connection
            .query_row(
                include_str!("../../../../src/agentnave/retention-candidate.sql"),
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((created, body, blocked)) = row else {
            return Ok(false);
        };
        if blocked {
            return Ok(false);
        }
        let created = crate::time_format::timestamp(&created);
        let updated = body
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .and_then(|view| {
                view["summary"]["updated"]
                    .as_str()
                    .and_then(crate::time_format::timestamp)
            });
        let Some((created, updated)) = created.zip(updated) else {
            return Ok(false);
        };
        if created.max(updated).div_euclid(1_000) >= cutoff {
            return Ok(false);
        }
    }
    Ok(true)
}

fn claimed_empty_count(connection: &Connection, cutoff: i64) -> Result<usize, String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version < 2 {
        return Ok(0);
    }
    let origin = if version >= 3 {
        "coalesce(origin, id)"
    } else {
        "id"
    };
    let linked = if version >= 4 {
        " OR EXISTS (SELECT 1 FROM workspace_links WHERE source_workspace = workspaces.id)"
    } else {
        ""
    };
    let ids: Vec<String> = connection
        .prepare(&format!(
            "SELECT id FROM workspaces WHERE pending = 0 AND title IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM conversations WHERE manager_session = workspaces.id) \
             AND (owner != {origin} OR previous_host IS NOT NULL{linked}) ORDER BY id",
        ))
        .and_then(|mut query| query.query_map([], |row| row.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    for id in ids {
        if crate::data_files::valid_id(&id) && handover_before(connection, Some(&id), cutoff, true)?
        {
            count += 1;
        }
    }
    Ok(count)
}

/// Only expired candidates reach this probe. Release each lock immediately, before any
/// database reads or opening another lock: the preview is advisory and never reserves work.
fn available(root: &Path, ids: &[String]) -> Result<bool, String> {
    for id in ids {
        if !crate::data_files::valid_id(id) {
            return Ok(false);
        }
        match crate::db::probe_lock(root, id)? {
            crate::db::LockProbe::Missing => continue,
            crate::db::LockProbe::Unlocked(lock) => drop(lock),
            crate::db::LockProbe::BusyOrUnsafe => return Ok(false),
        }
    }
    Ok(true)
}

pub(super) fn count(root: &Path, cutoff: i64) -> Result<usize, String> {
    let Some(mut connection) = crate::db::read(root)? else {
        return Ok(0);
    };
    let anchors: Vec<rusqlite::Result<String>> = connection
        .prepare("SELECT id FROM conversations ORDER BY id")
        .and_then(|mut query| Ok(query.query_map([], |row| row.get(0))?.collect()))
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut count = 0;
    for anchor in anchors {
        let anchor = match anchor {
            Ok(anchor) => anchor,
            Err(error) => {
                eprintln!("Storage preview skipped unreadable conversation ID: {error}");
                continue;
            }
        };
        if seen.contains(&anchor) {
            continue;
        }
        let candidate = (|| -> Result<bool, String> {
            let ids = workspace_ids(&connection, &anchor)?;
            seen.extend(ids.iter().cloned());
            if ids.is_empty() {
                return Ok(false);
            }
            let eligible = {
                let transaction = connection.transaction().map_err(|e| e.to_string())?;
                workspace_ids(&transaction, &anchor)? == ids
                    && eligible(&transaction, &ids, cutoff)?
            };
            Ok(eligible && available(root, &ids)?)
        })();
        match candidate {
            Ok(true) => count += 1,
            Ok(false) => {}
            Err(error) => eprintln!("Storage preview skipped workspace {anchor}: {error}"),
        }
    }
    match claimed_empty_count(&connection, cutoff) {
        Ok(empty) => count += empty,
        Err(error) => eprintln!("Storage preview skipped claimed empty workspaces: {error}"),
    }
    Ok(count)
}

pub(super) fn workspace_states(connection: &Connection) -> Vec<Value> {
    let Ok(version) = connection.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
    else {
        return Vec::new();
    };
    if version < 2 {
        return Vec::new();
    }
    let origin = if version >= 3 {
        "coalesce(origin, id)"
    } else {
        "id"
    };
    let has_links = if version >= 4 {
        "EXISTS (SELECT 1 FROM workspace_links WHERE source_workspace = workspaces.id)"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT id, title, pending, released, \
         EXISTS (SELECT 1 FROM conversations WHERE manager_session = workspaces.id), \
         (owner != {origin} OR previous_host IS NOT NULL OR {has_links}) \
         FROM workspaces ORDER BY id"
    );
    let Ok(mut stmt) = connection.prepare(&sql) else {
        return Vec::new();
    };
    struct WorkspaceRow {
        id: String,
        title: Option<String>,
        pending: bool,
        released: Option<String>,
        has_conv: bool,
        claimed: bool,
    }
    let rows: Vec<WorkspaceRow> = stmt
        .query_map([], |r| {
            Ok(WorkspaceRow {
                id: r.get(0)?,
                title: r.get(1)?,
                pending: r.get(2)?,
                released: r.get(3)?,
                has_conv: r.get(4)?,
                claimed: r.get(5)?,
            })
        })
        .and_then(Iterator::collect)
        .unwrap_or_default();
    let mut out = Vec::new();
    for row in rows {
        let WorkspaceRow {
            id,
            title,
            pending,
            released,
            has_conv,
            claimed,
        } = row;
        if !crate::data_files::valid_id(&id) {
            continue;
        }
        let mut raw_times = Vec::new();
        if let Some(released) = released {
            raw_times.push(released);
        }
        if version >= 4
            && let Ok(mut q) = connection
                .prepare("SELECT claimed_at FROM workspace_links WHERE source_workspace = ?1")
            && let Ok(found) = q
                .query_map([&id], |r| r.get::<_, String>(0))
                .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        {
            raw_times.extend(found);
        }
        let handover_ms = if raw_times.is_empty() {
            None
        } else {
            let mut latest = i64::MIN;
            let mut valid = true;
            for raw in &raw_times {
                match crate::time_format::timestamp(raw) {
                    Some(ts) => latest = latest.max(ts.div_euclid(1_000)),
                    None => valid = false,
                }
            }
            Some(if valid { latest } else { i64::MAX })
        };
        out.push(json!({
            "id": id,
            "title": title,
            "pending": pending,
            "empty_claimed": !has_conv && !pending && title.is_some() && claimed,
            "handover_ms": handover_ms,
        }));
    }
    out
}
