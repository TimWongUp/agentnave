//! The places that decide how a directory is grouped, kept apart so tests can inject them.

use std::path::PathBuf;
use std::time::SystemTime;

use super::auto::{self, AutoRule, HostDirs};
use super::path::{canonical, too_broad, within};

/// Normalized places on this machine. An empty field means the platform does not have it.
#[derive(Default)]
pub(super) struct Env {
    pub home: Option<PathBuf>,
    /// Temporary directories and the AgentNave data root: nothing in them is a project.
    pub scratch: Vec<PathBuf>,
    /// The home, desktop and downloads folders: only themselves are not a project.
    pub bare: Vec<PathBuf>,
    pub auto: Vec<AutoRule>,
}

/// What the places were read from; a change means they are read again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Stamp {
    files: Vec<Option<SystemTime>>,
    vars: Vec<Option<std::ffi::OsString>>,
}

fn normalized(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for path in paths {
        for form in [Some(path.clone()), canonical(&path)].into_iter().flatten() {
            if form.is_absolute() && !found.contains(&form) {
                found.push(form);
            }
        }
    }
    found
}

impl Env {
    /// The raw places, normalized: temporary candidates that are too broad (a volume root or
    /// the home folder) are dropped.
    pub(super) fn new(
        dirs: HostDirs,
        temp: Vec<PathBuf>,
        data_root: PathBuf,
        bare: Vec<PathBuf>,
    ) -> Self {
        let home = canonical(&dirs.home);
        let scratch = normalized(temp.into_iter().chain([data_root]))
            .into_iter()
            .filter(|path| !too_broad(path, home.as_deref()))
            .collect();
        Self {
            bare: normalized(bare.into_iter().chain([dirs.home.clone()])),
            auto: auto::rules(&dirs)
                .into_iter()
                .filter_map(|mut rule| {
                    rule.root = canonical(&rule.root)?;
                    Some(rule)
                })
                .collect(),
            scratch,
            home,
        }
    }

    pub(super) fn detect() -> Self {
        let mut temp = vec![std::env::temp_dir()];
        temp.extend(["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp"].map(PathBuf::from));
        for (name, tail) in [("LOCALAPPDATA", "Temp"), ("SystemRoot", "Temp")] {
            temp.extend(std::env::var_os(name).map(|dir| PathBuf::from(dir).join(tail)));
        }
        let bare = [dirs::desktop_dir(), dirs::download_dir()]
            .into_iter()
            .flatten()
            .collect();
        Self::new(auto::detect(), temp, crate::data_files::data_root(), bare)
    }

    pub(super) fn is_scratch(&self, path: &std::path::Path) -> bool {
        self.scratch.iter().any(|root| within(path, root))
    }

    pub(super) fn is_bare(&self, path: &std::path::Path) -> bool {
        self.bare.iter().any(|place| super::path::same(path, place))
    }

    pub(super) fn is_auto(&self, path: &std::path::Path) -> bool {
        self.auto.iter().any(|rule| rule.covers(path))
    }
}

pub(super) fn stamp() -> Stamp {
    let home = dirs::home_dir().unwrap_or_default();
    Stamp {
        files: auto::settings_files(&home)
            .iter()
            .map(|file| {
                std::fs::metadata(file)
                    .and_then(|meta| meta.modified())
                    .ok()
            })
            .collect(),
        vars: ["CODEX_HOME", "ZCODE_DATA_BASE_DIR", "AGENTNAVE_DATA_DIR"]
            .iter()
            .map(std::env::var_os)
            .collect(),
    }
}
