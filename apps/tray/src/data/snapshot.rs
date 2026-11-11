use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub host: Option<String>,
    pub manager_session: Option<String>,
    pub status: String,
    /// Last activity as Studio recorded it (RFC 3339); empty in older snapshots.
    pub updated: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Overview {
    /// Most recently changed first.
    pub items: Vec<Item>,
    pub unavailable: Vec<(String, String)>,
    /// Structured failures for consumers that need to distinguish recovery from new work.
    pub(super) unreadable: HashSet<String>,
    pub(super) scan_error: Option<String>,
    pub missing: bool,
    pub running: usize,
    pub review: usize,
}

impl Overview {
    pub fn count(&self, status: &str) -> usize {
        match status {
            "running" => self.running,
            "review" => self.review,
            _ => self
                .items
                .iter()
                .filter(|item| item.status == status)
                .count(),
        }
    }
}

pub(crate) struct Snapshot {
    pub item: Item,
    pub view: Arc<Value>,
    pub live: bool,
    pub live_updated: Option<String>,
}

/// Count actual invocations, never conversations or merged desk identities.
pub(crate) fn call_counts(snapshot: &Snapshot) -> (usize, usize) {
    let view = &snapshot.view["director"];
    let tasks = view["tasks"].as_array().into_iter().flatten();
    let seats = view["round"]["seats"].as_array().into_iter().flatten();
    let mut running = 0;
    let mut review = 0;
    for call in tasks.chain(seats) {
        if call["review"] == "pending" || call["status"] == "review" {
            review += 1;
        } else if call["status"] == "running" {
            running += 1;
        }
    }
    (running, review)
}
