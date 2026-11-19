//! Which conversations the user renamed in the App, kept in the App's own config directory
//! (`renamed-titles.json`) because the service's database does not tell a name the user gave from
//! the one the boss dispatched with. An agent shows a renamed title before any name its CLI gives.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{Value, json};

pub(crate) const FILE: &str = "renamed-titles.json";
static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn list(dir: &Path) -> BTreeSet<String> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read(dir)
}

fn read(dir: &Path) -> BTreeSet<String> {
    fs::read(dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|document| {
            document["conversations"].as_array().map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Records that the user renamed this conversation in the App.
pub(crate) fn mark(dir: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut ids = read(dir);
    if !ids.insert(id.to_owned()) {
        return Ok(());
    }
    let document = json!({"version": 1, "conversations": ids});
    crate::data_files::write_atomic(dir, FILE, &document)
}
