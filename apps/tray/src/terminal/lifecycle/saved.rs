//! What each derived session was started with, kept in the App's own config folder so the user can
//! take it up again after the App restarts or its boss is gone (ADR-0124). It holds launch
//! parameters only: no keys, prompts or replies. Written only when a record changes.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::account::Active;
use super::super::{DerivedLaunchConfig, State, Terminals, normalize_cwd};

const FILE: &str = "derived-launches.json";

/// One session's launch, by `(provider, native)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::terminal) struct Saved {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permissions: Value,
    pub account: Option<String>,
    pub account_dir: Option<PathBuf>,
    pub isolation_id: Option<String>,
    pub fallback_mode: Option<String>,
    pub parent_workspace_id: Option<String>,
    pub parent_boss_key: String,
}

impl Saved {
    pub(in crate::terminal) fn config(&self) -> DerivedLaunchConfig {
        DerivedLaunchConfig {
            cwd: normalize_cwd(&self.cwd),
            model: self.model.clone(),
            effort: self.effort.clone(),
            permissions: self.permissions.clone(),
        }
    }

    /// The account it ran as, under the label the account has now.
    pub(in crate::terminal) fn account(&self, root: &Path, provider: &str) -> Option<Active> {
        let id = self.account.clone()?;
        let label = super::super::account::resolve_in(root, provider, Some(&id))
            .ok()
            .flatten()
            .map_or_else(|| id.clone(), |found| found.label);
        Some(Active {
            dir: self.account_dir.clone().unwrap_or_default(),
            id,
            label,
        })
    }
}

/// The records file and what was last read from or written to it.
#[derive(Default)]
pub(in crate::terminal) struct Store {
    /// Set by tests; the build's config folder otherwise, and none in tests that set nothing.
    dir: Option<PathBuf>,
    known: Option<BTreeMap<String, Saved>>,
}

fn key(provider: &str, native: &str) -> String {
    format!("{provider}:{native}")
}

impl Store {
    fn dir(&self) -> Option<PathBuf> {
        self.dir
            .clone()
            .or_else(|| (!cfg!(test)).then(crate::preferences::dir))
    }

    fn known(&mut self) -> &mut BTreeMap<String, Saved> {
        let dir = self.dir();
        self.known
            .get_or_insert_with(|| dir.map(read).unwrap_or_default())
    }

    fn save(&mut self) {
        let Some(dir) = self.dir() else {
            return;
        };
        let sessions = self.known.clone().unwrap_or_default();
        let document = json!({"version": 1, "sessions": sessions});
        if let Err(error) = crate::private_keys::write_private_json(&dir, FILE, &document) {
            eprintln!("{error}");
        }
    }

    fn get(&mut self, provider: &str, native: &str) -> Option<Saved> {
        self.known().get(&key(provider, native)).cloned()
    }

    /// Takes in the launches of the sessions running now; the file is written only on a change.
    fn keep(&mut self, launches: Vec<(String, Saved)>) {
        let known = self.known();
        let mut changed = false;
        for (key, saved) in launches {
            if known.get(&key) != Some(&saved) {
                known.insert(key, saved);
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Forgets the sessions the service history no longer has. Kept when that cannot be told.
    fn prune(&mut self, root: &Path) {
        if self.known().is_empty() {
            return;
        }
        let Ok(Some(connection)) = crate::db::read(root) else {
            return;
        };
        let gone: Vec<String> = self
            .known()
            .keys()
            .filter(|key| {
                let Some((provider, native)) = key.split_once(':') else {
                    return false;
                };
                connection
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM tasks WHERE provider = ?1 AND session_id = ?2)",
                        [provider, native],
                        |row| row.get::<_, bool>(0),
                    )
                    .is_ok_and(|exists| !exists)
            })
            .cloned()
            .collect();
        if gone.is_empty() {
            return;
        }
        let known = self.known();
        for key in gone {
            known.remove(&key);
        }
        self.save();
    }
}

fn read(dir: PathBuf) -> BTreeMap<String, Saved> {
    let content = match fs::read(dir.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            eprintln!("无法读取 {FILE}：{error}");
            return BTreeMap::new();
        }
    };
    serde_json::from_slice::<Value>(&content)
        .ok()
        .and_then(|document| serde_json::from_value(document["sessions"].clone()).ok())
        .unwrap_or_default()
}

impl State {
    /// The launches of derived sessions that have started on their native session with a launch
    /// they were given, not one assumed for want of a record.
    fn launches(&self) -> Vec<(String, Saved)> {
        self.open
            .iter()
            .filter_map(|running| {
                let derived = running.derived.as_ref()?;
                let native = running.native.as_deref().filter(|id| !id.is_empty())?;
                if derived.life.config_assumed || !derived.life.process.ready {
                    return None;
                }
                let config = derived.launch_config.as_ref();
                let saved = Saved {
                    cwd: running.cwd.clone(),
                    model: config.and_then(|c| c.model.clone()),
                    effort: config.and_then(|c| c.effort.clone()),
                    permissions: config.map_or_else(|| json!({}), |c| c.permissions.clone()),
                    account: running.account.as_ref().map(|a| a.id.clone()),
                    account_dir: running.account.as_ref().map(|a| a.dir.clone()),
                    isolation_id: derived.isolation_id.clone(),
                    fallback_mode: derived.life.fallback_mode.clone(),
                    parent_workspace_id: derived.parent_workspace_id.clone(),
                    parent_boss_key: derived.parent_boss_key.clone(),
                };
                Some((key(&running.provider, native), saved))
            })
            .collect()
    }
}

impl Terminals {
    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.saved
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    #[cfg(test)]
    pub(in crate::terminal) fn save_launches_in(&self, dir: &Path) {
        *self.store() = Store {
            dir: Some(dir.to_path_buf()),
            known: None,
        };
    }

    pub(in crate::terminal) fn saved_launch(&self, provider: &str, native: &str) -> Option<Saved> {
        self.store().get(provider, native)
    }

    /// Records the launches of the sessions running now.
    pub(super) fn keep_launches(&self) {
        let launches = self.lock().launches();
        if !launches.is_empty() {
            self.store().keep(launches);
        }
    }

    pub(in crate::terminal) fn prune_saved_launches(&self, root: &Path) {
        self.store().prune(root);
    }
}
