//! Projects manually added by the user in the office sidebar, stored in the App's own
//! config directory (`manual-projects.json`). Removing one only deletes this record.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::path::comparable;
use super::{OTHER, Project};

pub(crate) const FILE: &str = "manual-projects.json";
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ManualProject {
    pub key: String,
    pub label: String,
    pub path: String,
    pub added_at: u64,
}

impl ManualProject {
    pub(crate) fn value(&self) -> Value {
        json!({
            "key": self.key,
            "label": self.label,
            "path": self.path,
            "added_at": self.added_at,
        })
    }
}

pub(crate) fn values(projects: &[ManualProject]) -> Vec<Value> {
    projects.iter().map(ManualProject::value).collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn same_key(left: &str, right: &str) -> bool {
    comparable(Path::new(left)) == comparable(Path::new(right))
}

fn read_unlocked(dir: &Path) -> Vec<ManualProject> {
    let Ok(content) = fs::read(dir.join(FILE)) else {
        return Vec::new();
    };
    let Ok(document) = serde_json::from_slice::<Value>(&content) else {
        return Vec::new();
    };
    let Some(items) = document.get("projects").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut projects: Vec<ManualProject> = Vec::new();
    for item in items {
        let Some(key) = item
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|k| !k.is_empty() && *k != OTHER)
        else {
            continue;
        };
        let Some(label) = item
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|l| !l.is_empty())
        else {
            continue;
        };
        if projects.iter().any(|existing| same_key(&existing.key, key)) {
            continue;
        }
        let path = item
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_owned();
        let added_at = item.get("added_at").and_then(Value::as_u64).unwrap_or(0);
        projects.push(ManualProject {
            key: key.to_owned(),
            label: label.to_owned(),
            path,
            added_at,
        });
    }
    projects
}

fn write_unlocked(dir: &Path, projects: &[ManualProject]) -> Result<(), String> {
    let document = json!({"version": 1, "projects": values(projects)});
    crate::data_files::write_atomic(dir, FILE, &document)
}

pub(crate) fn list(dir: &Path) -> Vec<ManualProject> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_unlocked(dir)
}

/// Add `project` unless it is already saved manually or present in `existing_keys`.
/// Returns `(entry, added, all_manual)`.
pub(crate) fn add(
    dir: &Path,
    project: Project,
    existing_keys: &BTreeSet<String>,
) -> Result<(ManualProject, bool, Vec<ManualProject>), (u16, String)> {
    if project.key.is_empty() || project.key == OTHER {
        return Err((
            400,
            "这个目录不会单独作为项目（如主目录、桌面、下载、临时目录或 AgentNave 数据目录）"
                .into(),
        ));
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries = read_unlocked(dir);
    if let Some(existing) = entries.iter().find(|p| same_key(&p.key, &project.key)) {
        return Ok((existing.clone(), false, entries));
    }
    if existing_keys.iter().any(|k| same_key(k, &project.key)) {
        let entry = ManualProject {
            key: project.key,
            label: project.label,
            path: project.path,
            added_at: now_ms(),
        };
        return Ok((entry, false, entries));
    }
    let entry = ManualProject {
        key: project.key,
        label: project.label,
        path: project.path,
        added_at: now_ms(),
    };
    entries.push(entry.clone());
    write_unlocked(dir, &entries).map_err(|error| (500, error))?;
    Ok((entry, true, entries))
}

/// Remove the manual project record matching `key`; sessions and files are untouched.
pub(crate) fn remove(dir: &Path, key: &str) -> Result<(bool, Vec<ManualProject>), (u16, String)> {
    let key = key.trim();
    if key.is_empty() || key == OTHER {
        return Err((400, "需要有效的项目路径".into()));
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries = read_unlocked(dir);
    let before = entries.len();
    entries.retain(|p| !same_key(&p.key, key));
    let removed = entries.len() != before;
    if removed {
        write_unlocked(dir, &entries).map_err(|error| (500, error))?;
    }
    Ok((removed, entries))
}
