//! Working directories that a host app creates by itself. A directory counts only when both
//! the root and the naming format match, so a folder a person made on purpose stays a project.

use std::fs;
use std::path::{Path, PathBuf};

use super::path::{canonical, comparable};

/// One host's layout: a name is chosen per level below `root`, and anything below that counts.
/// A pattern letter `D` is a digit and `X` a hex digit; the rest is literal. `*` is any name.
pub(super) struct AutoRule {
    pub root: PathBuf,
    pub levels: &'static [&'static [&'static str]],
}

const TIMESTAMP: &str = "DDDD-DD-DD-DD-DD-DD";
const AUTOMATION: &str = "automation-DDDD-DD-DD-DD-DD-DD";
const CLAUDE_SCRATCH: &str = "scratch-DDDD-DD-DD-XXXXXX";
const ZCODE: &str = "DDDDDDDDDDDDD-XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX";
const TRAE: &str = "temp_DDDDDDDD_DD";
const DATE: &str = "DDDD-DD-DD";

fn fits(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    pattern.chars().count() == name.chars().count()
        && pattern
            .chars()
            .zip(name.chars())
            .all(|(rule, ch)| match rule {
                'D' => ch.is_ascii_digit(),
                'X' => ch.is_ascii_hexdigit(),
                literal => literal.eq_ignore_ascii_case(&ch),
            })
}

impl AutoRule {
    /// Whether `path` (already normalized) lies in a directory this rule describes.
    pub(super) fn covers(&self, path: &Path) -> bool {
        let (path, root) = (comparable(path), comparable(&self.root));
        let Ok(rest) = path.strip_prefix(&root) else {
            return false;
        };
        let names: Vec<_> = rest.components().collect();
        names.len() >= self.levels.len()
            && self.levels.iter().zip(names).all(|(patterns, name)| {
                let name = name.as_os_str().to_string_lossy();
                patterns.iter().any(|pattern| fits(pattern, &name))
            })
    }
}

/// Where the host apps keep their settings and data on this machine.
#[derive(Default)]
pub(super) struct HostDirs {
    pub home: PathBuf,
    pub documents: PathBuf,
    /// Claude Desktop's data folders (`Claude`, `Claude-3p`).
    pub claude: Vec<PathBuf>,
    pub codex_workspace: PathBuf,
    pub zcode_base: PathBuf,
    pub workbuddy_workspace: PathBuf,
}

pub(super) fn rules(dirs: &HostDirs) -> Vec<AutoRule> {
    let mut rules = vec![
        AutoRule {
            root: dirs.workbuddy_workspace.clone(),
            levels: &[&[TIMESTAMP, AUTOMATION]],
        },
        AutoRule {
            root: dirs.codex_workspace.clone(),
            levels: &[&[DATE], &["*"]],
        },
        AutoRule {
            root: dirs.zcode_base.join("workspace"),
            levels: &[&[ZCODE, "default"]],
        },
        AutoRule {
            root: dirs.documents.join("deepseek-harness"),
            levels: &[&["default-workspace"]],
        },
    ];
    rules.extend(dirs.claude.iter().map(|app| AutoRule {
        root: app.join("scratch-workspaces"),
        levels: &[&[CLAUDE_SCRATCH]],
    }));
    rules.extend(["Trae-CN", "Trae"].iter().map(|name| AutoRule {
        root: dirs.documents.join(name),
        levels: &[&[TRAE]],
    }));
    rules
}

fn read_text(path: &Path) -> Option<String> {
    // Settings files are small; a huge one is not read.
    (fs::metadata(path).ok()?.len() <= 1 << 20)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

fn absolute(text: &str) -> Option<PathBuf> {
    let path = PathBuf::from(text.trim());
    canonical(&path).map(|_| path)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The settings files whose change moves a host's directory; the modification times tell
/// whether to read them again.
pub(super) fn settings_files(home: &Path) -> [PathBuf; 2] {
    let codex = env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    [
        home.join(".workbuddy/app/app-config.json"),
        codex.join("config.toml"),
    ]
}

pub(super) fn detect() -> HostDirs {
    let home = dirs::home_dir().unwrap_or_default();
    let documents = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
    let [workbuddy_settings, codex_settings] = settings_files(&home);
    let workbuddy_workspace = read_text(&workbuddy_settings)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| absolute(value["defaultWorkspacePath"].as_str()?))
        .unwrap_or_else(|| home.join("WorkBuddy"));
    let codex_workspace = read_text(&codex_settings)
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| absolute(table.get("projectless_workspace_root")?.as_str()?))
        .unwrap_or_else(|| documents.join("Codex"));
    let claude = if cfg!(windows) {
        [
            dirs::data_dir().map(|dir| dir.join("Claude")),
            dirs::data_local_dir().map(|dir| dir.join("Claude-3p")),
        ]
        .into_iter()
        .flatten()
        .collect()
    } else {
        let base = if cfg!(target_os = "macos") {
            dirs::data_dir()
        } else {
            dirs::config_dir()
        };
        base.into_iter()
            .flat_map(|dir| [dir.join("Claude"), dir.join("Claude-3p")])
            .collect()
    };
    HostDirs {
        zcode_base: env_dir("ZCODE_DATA_BASE_DIR").unwrap_or_else(|| home.join(".zcode")),
        home,
        documents,
        claude,
        codex_workspace,
        workbuddy_workspace,
    }
}
