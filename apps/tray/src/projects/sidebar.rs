//! Sidebar preferences for the office chat list (`sidebar-state.json` under the App's own
//! config directory): project aliases, pinned projects, pinned workspace groups, and project order.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{Value, json};

use super::OTHER;
use super::path::comparable;

pub(crate) const FILE: &str = "sidebar-state.json";
const VERSION: u64 = 1;
const MAX_ALIAS_CHARS: usize = 80;
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SidebarState {
    pub project_aliases: BTreeMap<String, String>,
    pub pinned_projects: Vec<String>,
    pub pinned_groups: Vec<String>,
    pub project_order: Vec<String>,
}

impl SidebarState {
    pub(crate) fn value(&self) -> Value {
        json!({
            "project_aliases": self.project_aliases,
            "pinned_projects": self.pinned_projects,
            "pinned_groups": self.pinned_groups,
            "project_order": self.project_order,
        })
    }
}

fn same_key(left: &str, right: &str) -> bool {
    comparable(Path::new(left)) == comparable(Path::new(right))
}

fn valid_project_key(raw: &str) -> Option<&str> {
    let key = raw.trim();
    (!key.is_empty() && key != OTHER).then_some(key)
}

fn normalize_alias(raw: &str) -> String {
    let single_line: String = raw
        .chars()
        .map(|c| if matches!(c, '\r' | '\n') { ' ' } else { c })
        .collect();
    single_line
        .trim()
        .chars()
        .take(MAX_ALIAS_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn read_project_keys(value: Option<&Value>) -> Vec<String> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = Vec::new();
    for item in items {
        let Some(key) = item.as_str().and_then(valid_project_key) else {
            continue;
        };
        if !keys.iter().any(|existing| same_key(existing, key)) {
            keys.push(key.to_owned());
        }
    }
    keys
}

fn read_group_ids(value: Option<&Value>) -> Vec<String> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = Vec::new();
    for item in items {
        let Some(id) = item.as_str().map(str::trim).filter(|id| !id.is_empty()) else {
            continue;
        };
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

fn read_aliases(value: Option<&Value>) -> BTreeMap<String, String> {
    let Some(entries) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    for (raw_key, raw_alias) in entries {
        let Some(key) = valid_project_key(raw_key) else {
            continue;
        };
        let Some(alias) = raw_alias
            .as_str()
            .map(normalize_alias)
            .filter(|a| !a.is_empty())
        else {
            continue;
        };
        if !aliases.keys().any(|existing| same_key(existing, key)) {
            aliases.insert(key.to_owned(), alias);
        }
    }
    aliases
}

fn read_unlocked(dir: &Path) -> SidebarState {
    let Ok(content) = fs::read(dir.join(FILE)) else {
        return SidebarState::default();
    };
    let Ok(document) = serde_json::from_slice::<Value>(&content) else {
        return SidebarState::default();
    };
    if !document.is_object()
        || document
            .get("version")
            .is_some_and(|v| v.as_u64() != Some(VERSION))
    {
        return SidebarState::default();
    }
    SidebarState {
        project_aliases: read_aliases(document.get("project_aliases")),
        pinned_projects: read_project_keys(document.get("pinned_projects")),
        pinned_groups: read_group_ids(document.get("pinned_groups")),
        project_order: read_project_keys(document.get("project_order")),
    }
}

fn write_unlocked(dir: &Path, state: &SidebarState) -> Result<(), String> {
    let document = json!({
        "version": VERSION,
        "project_aliases": state.project_aliases,
        "pinned_projects": state.pinned_projects,
        "pinned_groups": state.pinned_groups,
        "project_order": state.project_order,
    });
    crate::data_files::write_atomic(dir, FILE, &document)
}

pub(crate) fn load(dir: &Path) -> SidebarState {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_unlocked(dir)
}

fn mutate(
    dir: &Path,
    update: impl FnOnce(&mut SidebarState) -> bool,
) -> Result<SidebarState, (u16, String)> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut state = read_unlocked(dir);
    if update(&mut state) {
        write_unlocked(dir, &state).map_err(|error| (500, error))?;
    }
    Ok(state)
}

pub(crate) fn rename(dir: &Path, key: &str, alias: &str) -> Result<SidebarState, (u16, String)> {
    let Some(key) = valid_project_key(key) else {
        return Err((400, "需要有效的项目路径".into()));
    };
    let alias = normalize_alias(alias);
    mutate(dir, |state| {
        let before = state.project_aliases.clone();
        state.project_aliases.retain(|k, _| !same_key(k, key));
        if !alias.is_empty() {
            state.project_aliases.insert(key.to_owned(), alias);
        }
        state.project_aliases != before
    })
}

pub(crate) fn pin_project(
    dir: &Path,
    key: &str,
    pinned: bool,
) -> Result<SidebarState, (u16, String)> {
    let Some(key) = valid_project_key(key) else {
        return Err((400, "需要有效的项目路径".into()));
    };
    mutate(dir, |state| {
        let before = state.pinned_projects.clone();
        if pinned {
            if !state.pinned_projects.iter().any(|k| same_key(k, key)) {
                state.pinned_projects.insert(0, key.to_owned());
            }
        } else {
            state.pinned_projects.retain(|k| !same_key(k, key));
        }
        state.pinned_projects != before
    })
}

pub(crate) fn set_order(dir: &Path, order: &[&str]) -> Result<SidebarState, (u16, String)> {
    let mut next: Vec<String> = Vec::new();
    for raw in order {
        let Some(key) = valid_project_key(raw) else {
            continue;
        };
        if !next.iter().any(|existing| same_key(existing, key)) {
            next.push(key.to_owned());
        }
    }
    mutate(dir, |state| {
        if state.project_order == next {
            return false;
        }
        state.project_order = next;
        true
    })
}

pub(crate) fn pin_group(
    dir: &Path,
    group: &str,
    pinned: bool,
) -> Result<SidebarState, (u16, String)> {
    let group = group.trim();
    if group.is_empty() {
        return Err((400, "需要有效的工作区 ID".into()));
    }
    mutate(dir, |state| {
        let before = state.pinned_groups.clone();
        if pinned {
            if !state.pinned_groups.iter().any(|g| g == group) {
                state.pinned_groups.insert(0, group.to_owned());
            }
        } else {
            state.pinned_groups.retain(|g| g != group);
        }
        state.pinned_groups != before
    })
}
