//! The repository each working directory belonged to, kept for directories that are gone: an
//! agent worktree that was removed still belongs to the checkout it came from (see CONTEXT.md,
//! "项目"). Only this app reads and writes the file, under its own config directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::path::{canonical, key};

pub(super) const FILE: &str = "projects.json";

/// The working directory keys and the repository each one resolved to, read on first use.
pub(super) struct Memory {
    file: PathBuf,
    entries: Option<BTreeMap<String, String>>,
}

impl Memory {
    pub(super) fn at(file: PathBuf) -> Self {
        Self {
            file,
            entries: None,
        }
    }

    /// The repository recorded while `dir` still existed.
    pub(super) fn root(&mut self, dir: &Path) -> Option<PathBuf> {
        let root = PathBuf::from(self.load().get(&key(dir))?);
        root.is_absolute().then_some(root)
    }

    /// Record that `dir` belongs to `root`; the file is only written when the value differs.
    pub(super) fn remember(&mut self, dir: &Path, root: &Path) {
        let cwd = key(dir);
        let text = root.to_string_lossy().into_owned();
        if self.load().get(&cwd) == Some(&text) {
            return;
        }
        self.load().insert(cwd, text);
        self.save();
    }

    /// Forget directories that are no workspace's origin any more. No origins means the list
    /// could not be read, so every record is kept.
    pub(super) fn retain(&mut self, origins: &BTreeSet<String>) {
        if origins.is_empty() {
            return;
        }
        let present: BTreeSet<String> = origins
            .iter()
            .filter_map(|origin| canonical(Path::new(origin)).map(|dir| key(&dir)))
            .collect();
        let before = self.load().len();
        self.load().retain(|cwd, _| present.contains(cwd));
        if self.load().len() != before {
            self.save();
        }
    }

    fn load(&mut self) -> &mut BTreeMap<String, String> {
        self.entries.get_or_insert_with(|| read(&self.file))
    }

    fn save(&mut self) {
        let path = self.file.clone();
        let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
        else {
            return;
        };
        let document: Map<String, Value> = self
            .load()
            .iter()
            .map(|(cwd, root)| (cwd.clone(), Value::String(root.clone())))
            .collect();
        let _ = crate::data_files::write_atomic(parent, name, &Value::Object(document));
    }
}

/// The file, or nothing when it cannot be read or does not hold an object of paths.
fn read(file: &Path) -> BTreeMap<String, String> {
    let Ok(content) = fs::read(file) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(document)) = serde_json::from_slice::<Value>(&content) else {
        return BTreeMap::new();
    };
    document
        .into_iter()
        .filter_map(|(cwd, root)| match root {
            Value::String(root) if !cwd.is_empty() && !root.is_empty() => Some((cwd, root)),
            _ => None,
        })
        .collect()
}
