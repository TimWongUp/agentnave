//! What the isolated-directory cleanup needs from the saved conversations (ADR-0115 §6).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::Connection;

use super::Reader;
use crate::isolated::cleanup::Inputs;

/// Workspaces the office still shows: those with a conversation or a handover row. `None`
/// when the history cannot be read, so that a failed read never counts as a deletion.
fn existing_workspaces(connection: &Connection) -> Option<HashSet<String>> {
    let mut statement = connection
        .prepare(
            "SELECT manager_session FROM conversations WHERE manager_session IS NOT NULL \
             UNION SELECT id FROM workspaces",
        )
        .ok()?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?;
    rows.collect::<Result<HashSet<_>, _>>().ok()
}

impl Reader {
    /// One scan of the saved views: where tasks run, when each directory's tasks last
    /// changed, and which workspaces still exist.
    pub(crate) fn isolated_cleanup_inputs(&self) -> Inputs {
        let database = crate::db::read(&self.root);
        let (overview, snapshots) = self.scan_with(&database);
        let mut running: Vec<PathBuf> = Vec::new();
        let mut last_task: HashMap<String, i64> = HashMap::new();
        for snapshot in &snapshots {
            let director = &snapshot.view["director"];
            running.extend(
                crate::stage::running_in(director)
                    .into_iter()
                    .map(PathBuf::from),
            );
            let updated = crate::time_format::timestamp(&snapshot.item.updated).unwrap_or(0);
            for task in director["tasks"].as_array().into_iter().flatten() {
                if let Some(id) = task["isolation_id"].as_str().filter(|id| !id.is_empty()) {
                    let newest = last_task.entry(id.to_owned()).or_insert(0);
                    *newest = (*newest).max(updated);
                }
            }
        }
        let history_unreadable = overview.scan_error.is_some() || !overview.unreadable.is_empty();
        let workspaces = match &database {
            Ok(Some(connection)) if overview.scan_error.is_none() => {
                existing_workspaces(connection)
            }
            _ => None,
        };
        Inputs {
            running,
            last_task,
            workspaces,
            history_unreadable,
        }
    }
}
