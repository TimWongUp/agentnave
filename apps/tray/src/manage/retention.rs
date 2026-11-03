//! Shared eligibility for manual age cleanup and the read-only storage preview.
use super::*;
use std::collections::HashSet;

pub(super) fn eligible(
    connection: &Connection,
    ids: &[String],
    cutoff: i64,
) -> Result<bool, String> {
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
                        row.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((created, body, pending, running)) = row else {
            return Ok(false);
        };
        if pending || running {
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

/// Probe existing locks only: a preview never creates lock files or changes the database.
fn probe(root: &Path, ids: &[String]) -> Result<Option<Vec<File>>, String> {
    let mut locks = Vec::new();
    for id in ids {
        if !crate::data::valid_id(id) {
            return Ok(None);
        }
        let path = crate::db::lock_path(root, id);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
            Ok(meta) if !meta.is_file() => return Ok(None),
            _ => {}
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        match lock.try_lock() {
            Ok(()) => locks.push(lock),
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Some(locks))
}

pub(super) fn count(root: &Path, cutoff: i64) -> Result<usize, String> {
    let Some(mut connection) = crate::db::read(root)? else {
        return Ok(0);
    };
    let anchors: Vec<String> = connection
        .prepare("SELECT id FROM conversations ORDER BY id")
        .and_then(|mut query| query.query_map([], |row| row.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut count = 0;
    for anchor in anchors {
        if seen.contains(&anchor) {
            continue;
        }
        let ids = workspace_ids(&connection, &anchor)?;
        seen.extend(ids.iter().cloned());
        if ids.is_empty() {
            continue;
        }
        let Some(_locks) = probe(root, &ids)? else {
            continue;
        };
        let transaction = connection.transaction().map_err(|e| e.to_string())?;
        if workspace_ids(&transaction, &anchor)? == ids && eligible(&transaction, &ids, cutoff)? {
            count += 1;
        }
    }
    Ok(count)
}
