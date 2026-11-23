//! What the last cleanup pass did, kept in memory for the isolated-directory list.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::{Value, json};

use super::Record;

/// Why a pass ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    Auto,
    Manual,
}

/// The rule that let a directory go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    Merged,
    Unchanged,
    Idle,
    WorkspaceDeleted,
}

impl Rule {
    fn name(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Unchanged => "unchanged",
            Self::Idle => "idle",
            Self::WorkspaceDeleted => "workspace_deleted",
        }
    }
}

/// Why a directory stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Being created, deleted or waiting for attention.
    NotReady,
    /// A running task, an open terminal or a CLI is working in it.
    Busy,
    Leased,
    /// Used moments ago.
    Recent,
    Dirty,
    Nested,
    Detached,
    Orphan,
    /// Its state could not be verified; the detail says why.
    Unverified,
    /// The repository lock was busy or unavailable.
    Locked,
    /// It changed while the pass looked at it.
    Changed,
    /// No enabled rule applies.
    NoMatch,
}

impl Keep {
    fn name(self) -> &'static str {
        match self {
            Self::NotReady => "not_ready",
            Self::Busy => "busy",
            Self::Leased => "leased",
            Self::Recent => "recent",
            Self::Dirty => "dirty",
            Self::Nested => "nested",
            Self::Detached => "detached",
            Self::Orphan => "orphan",
            Self::Unverified => "unverified",
            Self::Locked => "locked",
            Self::Changed => "changed",
            Self::NoMatch => "no_match",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Directory, managed branch and registry row are gone.
    Removed(Rule),
    /// Only the directory is gone; the branch and the row stay as `collapsed`.
    Collapsed(Rule),
    Kept(Keep),
    /// The rule applied but the removal failed.
    Failed(Rule),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub id: String,
    pub workspace: String,
    pub branch: String,
    pub outcome: Outcome,
    /// A PR number or the failure text.
    pub detail: Option<String>,
}

impl Entry {
    pub(super) fn new(record: &Record, outcome: Outcome, detail: Option<String>) -> Self {
        Self {
            id: record.id.clone(),
            workspace: record.workspace_id.clone(),
            branch: record.branch().to_owned(),
            outcome,
            detail,
        }
    }

    fn to_json(&self) -> Value {
        let (outcome, rule, reason) = match &self.outcome {
            Outcome::Removed(rule) => ("removed", Some(rule.name()), None),
            Outcome::Collapsed(rule) => ("collapsed", Some(rule.name()), None),
            Outcome::Failed(rule) => ("failed", Some(rule.name()), None),
            Outcome::Kept(keep) => ("kept", None, Some(keep.name())),
        };
        json!({
            "id": self.id,
            "workspace": self.workspace,
            "branch": self.branch,
            "outcome": outcome,
            "rule": rule,
            "reason": reason,
            "detail": self.detail,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Report {
    /// Microseconds since the epoch.
    pub at: i64,
    pub trigger: Trigger,
    pub entries: Vec<Entry>,
    /// Set when the registry could not be read.
    pub error: Option<String>,
}

impl Report {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "at": crate::time_format::iso(self.at),
            "trigger": match self.trigger {
                Trigger::Auto => "auto",
                Trigger::Manual => "manual",
            },
            "error": self.error,
            "entries": self.entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        })
    }
}

static LAST: Mutex<Option<Report>> = Mutex::new(None);
/// Directories removed or collapsed by recent passes, newest first, for the list to show
/// after the directories themselves are gone.
static RECENT: Mutex<VecDeque<(i64, Entry)>> = Mutex::new(VecDeque::new());
const RECENT_LIMIT: usize = 100;

pub(super) fn remember(report: &Report) {
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some(report.clone());
    let mut recent = RECENT.lock().unwrap_or_else(|p| p.into_inner());
    for entry in report
        .entries
        .iter()
        .filter(|entry| matches!(entry.outcome, Outcome::Removed(_) | Outcome::Collapsed(_)))
    {
        recent.push_front((report.at, entry.clone()));
    }
    recent.truncate(RECENT_LIMIT);
}

pub(super) fn last() -> Option<Report> {
    LAST.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub(super) fn recent_json() -> Vec<Value> {
    RECENT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|(at, entry)| {
            let mut value = entry.to_json();
            value["at"] = json!(crate::time_format::iso(*at));
            value
        })
        .collect()
}
