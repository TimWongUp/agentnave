//! The service's registry of isolated directories (`worktrees.db`), read-only: the App shows
//! them and never changes them (ADR-0085).

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

const NAME: &str = "worktrees.db";
/// The service's `SCHEMA_VERSION` this build reads; any other version shows nothing.
const VERSION: i64 = 1;

/// One registered isolated directory that is past creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dir {
    pub id: String,
    pub created: String,
    pub workspace: String,
    pub path: String,
    pub branch: String,
    /// Registered as usable; any other state is waiting for boss to deal with it.
    pub ready: bool,
}

/// Every registered directory; a missing, unreadable or newer database shows nothing.
pub(crate) fn read(root: &Path) -> Vec<Dir> {
    query(root).unwrap_or_default()
}

fn query(root: &Path) -> rusqlite::Result<Vec<Dir>> {
    let path = root.join(NAME);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(1))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != VERSION {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT workspace_id, worktree_path, branch_ref, state, id, created_at FROM isolated_dirs \
         WHERE state IN ('ready', 'deleting', 'needs_attention') ORDER BY created_at, id",
    )?;
    statement
        .query_map([], |row| {
            let branch: String = row.get(2)?;
            let state: String = row.get(3)?;
            Ok(Dir {
                workspace: row.get(0)?,
                path: row.get(1)?,
                branch: branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&branch)
                    .to_owned(),
                ready: state == "ready",
                id: row.get(4)?,
                created: row.get(5)?,
            })
        })?
        .collect()
}

/// Whether `cwd` is `dir` or lies below it; symbolic links resolve when both still exist.
fn within(cwd: &str, dir: &str) -> bool {
    let (cwd, dir) = (Path::new(cwd), Path::new(dir));
    if cwd.starts_with(dir) {
        return true;
    }
    match (cwd.canonicalize(), dir.canonicalize()) {
        (Ok(cwd), Ok(dir)) => cwd.starts_with(&dir),
        _ => false,
    }
}

/// The call that asked for a directory, found in a workspace's task views.
#[derive(Clone, Debug)]
pub(crate) struct Builder {
    pub id: String,
    pub path: String,
    pub desk: String,
    pub text: String,
}

/// A workspace's directories for the office, those in use last. Each is in use (a running call
/// works inside it), left over (usable but nobody works in it) or unfinished (the last release
/// did not complete).
pub(crate) fn classify(
    dirs: &[Dir],
    workspace: &str,
    running: &[String],
    builders: &[Builder],
) -> Vec<Value> {
    let mut items: Vec<(bool, Value)> = dirs
        .iter()
        .filter(|dir| dir.workspace == workspace)
        .map(|dir| {
            let using = dir.ready && running.iter().any(|cwd| within(cwd, &dir.path));
            let status = match (dir.ready, using) {
                (_, true) => "using",
                (true, false) => "idle",
                (false, _) => "unfinished",
            };
            let task = builders
                .iter()
                .find(|b| b.id == dir.id || b.path == dir.path)
                .map(|b| json!({"desk": b.desk, "text": b.text}));
            let item = json!({"id": dir.id, "path": dir.path, "branch": dir.branch,
                "status": status, "created": dir.created, "task": task});
            (using, item)
        })
        .collect();
    items.sort_by_key(|(using, _)| *using);
    items.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests;
