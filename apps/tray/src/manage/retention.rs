//! Eligibility and short-lived candidate probes for the read-only storage preview.
use super::*;
use std::collections::HashSet;

fn eligible(connection: &Connection, ids: &[String], cutoff: i64) -> Result<bool, String> {
    if connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM workspaces JOIN conversations ON manager_session = workspaces.id \
             WHERE conversations.id = ?1 AND pending = 1)",
            [&ids[0]], |row| row.get::<_, bool>(0),
        ).map_err(|e| e.to_string())? {
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
        let created = crate::stage::timestamp(&created);
        let updated = body
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .and_then(|view| {
                view["summary"]["updated"]
                    .as_str()
                    .and_then(crate::stage::timestamp)
            });
        let Some((created, updated)) = created.zip(updated) else {
            return Ok(false);
        };
        if created.max(updated) / 1_000 >= cutoff {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Only expired candidates reach this probe. Release each lock immediately, before any
/// database reads or opening another lock: the preview is advisory and never reserves work.
fn available(root: &Path, ids: &[String]) -> Result<bool, String> {
    for id in ids {
        if !crate::data::valid_id(id) {
            return Ok(false);
        }
        let path = crate::db::lock_path(root, id);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
            Ok(meta) if !meta.is_file() => return Ok(false),
            _ => {}
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        match lock.try_lock() {
            Ok(()) => drop(lock),
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(error) => return Err(error.to_string()),
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
    Ok(count)
}
