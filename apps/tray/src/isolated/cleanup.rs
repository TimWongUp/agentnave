//! Automatic cleanup of isolated directories (ADR-0115 §6). Every rule is off until the user
//! turns it on in Settings. A pass reads the registry, decides per directory and, for the
//! ones to go, re-checks and acts under the repository lock with ADR-0098's lease protocol.
//! Results live in memory only.

mod apply;
mod report;
mod schedule;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::git_pr::gh::GhClient;

use super::store::Record;

pub(crate) use report::{Report, Trigger};
pub(crate) use schedule::{request, start};

/// The idle choices Settings offers, in days.
pub(crate) const IDLE_DAYS: [u32; 4] = [7, 14, 30, 90];
/// How long after its last use a directory is left alone, so a task that just ended can be
/// resumed or its result read (t3code waits one minute for settle scripts; a manual resume
/// takes longer).
const GRACE_MICROS: i64 = 5 * 60 * 1_000_000;
const DAY_MICROS: i64 = 24 * 60 * 60 * 1_000_000;

/// Which rules the user turned on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rules {
    pub merged: bool,
    pub unchanged: bool,
    pub idle_days: Option<u32>,
    pub workspace_deleted: bool,
}

impl Rules {
    /// From the preferences document; unknown or invalid values leave a rule off.
    pub(crate) fn from_document(document: &serde_json::Map<String, Value>) -> Self {
        let section = document.get("isolated_cleanup");
        let flag = |key: &str| {
            section
                .and_then(|s| s.get(key))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let idle_days = section
            .and_then(|s| s.get("idle_days"))
            .and_then(Value::as_u64)
            .and_then(|days| u32::try_from(days).ok())
            .filter(|days| IDLE_DAYS.contains(days));
        Self {
            merged: flag("merged"),
            unchanged: flag("unchanged"),
            idle_days,
            workspace_deleted: flag("workspace_deleted"),
        }
    }

    pub(crate) fn any(&self) -> bool {
        self.merged || self.unchanged || self.idle_days.is_some() || self.workspace_deleted
    }
}

/// What the tray knows about the work around the registry; read once per pass.
#[derive(Debug, Default)]
pub(crate) struct Inputs {
    /// Working directories of running tasks and meeting seats.
    pub running: Vec<PathBuf>,
    /// The newest update of a conversation that ran a task in a directory, by isolation id,
    /// in microseconds since the epoch.
    pub last_task: HashMap<String, i64>,
    /// Ids of the office workspaces that still exist; `None` when the history could not be
    /// read, so that nothing is judged deleted on a failed read.
    pub workspaces: Option<HashSet<String>>,
    /// Some saved conversation could not be read, so `last_task` may miss a recent task.
    pub history_unreadable: bool,
}

/// Everything one pass needs from the outside.
pub(crate) struct Env<'a> {
    pub root: &'a Path,
    pub gh: &'a GhClient,
    /// Microseconds since the epoch.
    pub now: i64,
    pub rules: Rules,
    pub inputs: &'a Inputs,
    /// An open terminal in the directory or a CLI working there, from the terminal registry.
    pub terminal_busy: &'a dyn Fn(&Record) -> bool,
}

impl Env<'_> {
    /// The newest sign of use: the registry's last update, the last task conversation, or the
    /// worktree's last commit or checkout (its HEAD reflog), which also covers a derived
    /// terminal. An unreadable registry time counts as now, which keeps the directory.
    fn last_used(&self, record: &Record) -> i64 {
        let registered = crate::time_format::timestamp(&record.updated_at).unwrap_or(self.now);
        let task = self.inputs.last_task.get(&record.id).copied().unwrap_or(0);
        registered.max(task).max(reflog_time(record).unwrap_or(0))
    }

    fn busy(&self, record: &Record) -> bool {
        let path = Path::new(&record.worktree_path);
        self.inputs
            .running
            .iter()
            .any(|cwd| super::leases::is_same_or_subpath(cwd, path))
            || (self.terminal_busy)(record)
    }
}

/// When the worktree's HEAD last moved, in microseconds since the epoch.
fn reflog_time(record: &Record) -> Option<i64> {
    let git_dir = record.worktree_git_dir.as_deref()?;
    let modified = std::fs::metadata(Path::new(git_dir).join("logs").join("HEAD"))
        .ok()?
        .modified()
        .ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_micros()).ok()
}

/// One pass over the registry. The directories to go are removed or collapsed; every other
/// one is listed with why it stays.
pub(crate) fn run(env: &Env, trigger: Trigger) -> Report {
    let store = super::store::WorktreeStore::new(env.root);
    let (records, error) = match store.all_records() {
        Ok(records) => (records, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    // With every rule off nothing is looked at, so nothing is reported either.
    let entries = records
        .iter()
        .filter(|_| env.rules.any())
        .filter(|record| {
            matches!(
                record.state.as_str(),
                "ready" | "deleting" | "needs_attention"
            )
        })
        .filter_map(|record| apply::clean_one(env, record))
        .collect();
    Report {
        at: env.now,
        trigger,
        entries,
        error,
    }
}
