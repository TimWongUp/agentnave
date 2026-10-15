//! In-app updates of the tray itself, and a read-only note when the local AgentNave service
//! installed by `uv` is older than the tray.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Automatic checks run at start and then once a day; the menu can check at any time.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const REPOSITORY: &str = "https://github.com/TimWongUp/agentnave.git";

#[derive(Clone, Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Latest,
    Available(String),
    Installing,
    Failed,
}

pub struct Updates {
    item: MenuItem<Wry>,
    state: Mutex<(Phase, Option<Update>)>,
}

impl Updates {
    pub fn new(item: MenuItem<Wry>) -> Self {
        Self {
            item,
            state: Mutex::new((Phase::Idle, None)),
        }
    }

    fn set(&self, phase: Phase, pending: Option<Update>) {
        let (text, enabled) = match &phase {
            Phase::Idle => ("检查更新…".to_owned(), true),
            Phase::Checking => ("正在检查更新…".to_owned(), false),
            Phase::Latest => (format!("已是最新版本 v{VERSION}"), true),
            Phase::Available(version) => (format!("更新到 v{version} 并重启"), true),
            Phase::Installing => ("正在下载更新…".to_owned(), false),
            Phase::Failed => ("更新失败，点此重试".to_owned(), true),
        };
        let _ = self.item.set_text(text);
        let _ = self.item.set_enabled(enabled);
        *self.state.lock().unwrap() = (phase, pending);
    }

    fn phase(&self) -> Phase {
        self.state.lock().unwrap().0.clone()
    }
}

/// Look for a newer release. Automatic checks stay silent when offline or before any release
/// carries an update manifest; a check from the menu reports the failure.
pub fn check(app: &AppHandle, manual: bool) {
    let updates = app.state::<Updates>();
    let before = updates.phase();
    if matches!(before, Phase::Checking | Phase::Installing) {
        return;
    }
    updates.set(Phase::Checking, None);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let found = match app.updater() {
            Ok(updater) => updater.check().await,
            Err(error) => Err(error),
        };
        let updates = app.state::<Updates>();
        match found {
            Ok(Some(update)) => updates.set(Phase::Available(update.version.clone()), Some(update)),
            Ok(None) => updates.set(Phase::Latest, None),
            Err(_) if manual => updates.set(Phase::Failed, None),
            Err(_) => updates.set(
                if before == Phase::Failed {
                    Phase::Failed
                } else {
                    Phase::Idle
                },
                None,
            ),
        }
    });
}

/// The menu and the panel share one action: install a found update, otherwise check.
pub fn act(app: &AppHandle) {
    let updates = app.state::<Updates>();
    let pending = updates.state.lock().unwrap().1.clone();
    let Some(update) = pending else {
        check(app, true);
        return;
    };
    updates.set(Phase::Installing, None);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => app.restart(),
            Err(_) => app.state::<Updates>().set(Phase::Failed, None),
        }
    });
}

/// What the panel shows about versions: the update state and any outdated local service.
pub fn status(app: &AppHandle) -> Value {
    let phase = app
        .try_state::<Updates>()
        .map(|updates| updates.phase())
        .unwrap_or_default();
    let update = match phase {
        Phase::Available(version) => json!({"state": "available", "version": version}),
        Phase::Installing => json!({"state": "installing"}),
        Phase::Failed => json!({"state": "failed"}),
        _ => Value::Null,
    };
    json!({"version": VERSION, "update": update, "service": service(&tool_dir())})
}

/// The `uv` tool directory, following uv's own defaults.
fn tool_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("UV_TOOL_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    return dirs::data_dir().map(|dir| dir.join("uv").join("data").join("tools"));
    #[cfg(not(windows))]
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .map(|dir| dir.join("uv").join("tools"))
}

/// A note with the upgrade command when the installed service is older than this app.
fn service(tools: &Option<PathBuf>) -> Value {
    let Some(tools) = tools else {
        return Value::Null;
    };
    let Some(installed) = installed_version(&tools.join("agentnave")) else {
        return Value::Null;
    };
    if parse(&installed) >= parse(VERSION) {
        return Value::Null;
    }
    // Releases before the single package installed the studio as its own tool.
    let legacy = tools.join("agentnave-studio").is_dir();
    let install = format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"");
    let command = if legacy {
        format!("uv tool uninstall agentnave-studio && {install}")
    } else {
        install
    };
    json!({"version": installed, "command": command})
}

/// The version in `agentnave-<version>.dist-info` inside a uv tool environment.
fn installed_version(env: &Path) -> Option<String> {
    let mut queue = vec![(env.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(version) = name
                .strip_prefix("agentnave-")
                .and_then(|rest| rest.strip_suffix(".dist-info"))
            {
                return Some(version.to_owned());
            }
            // lib/python3.x/site-packages on POSIX, Lib/site-packages on Windows.
            if depth < 3 && entry.path().is_dir() && !name.starts_with('.') && name != "bin" {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    None
}

fn parse(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// Put the service upgrade command on the clipboard.
pub fn copy_service_command() {
    let Some(command) = service(&tool_dir())["command"].as_str().map(str::to_owned) else {
        return;
    };
    #[cfg(target_os = "macos")]
    let program = "pbcopy";
    #[cfg(windows)]
    let program = "clip";
    #[cfg(any(target_os = "macos", windows))]
    if let Ok(mut child) = std::process::Command::new(program)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(command.as_bytes());
        }
        let _ = child.wait();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_an_older_service_and_the_legacy_studio_tool() {
        let tools = tempfile::tempdir().unwrap();
        let site = tools.path().join("agentnave/lib/python3.12/site-packages");
        std::fs::create_dir_all(site.join("agentnave-0.1.0.dist-info")).unwrap();
        std::fs::create_dir_all(site.join("mcp-2.1.1.dist-info")).unwrap();
        let tools = Some(tools.path().to_path_buf());

        let note = service(&tools);
        assert_eq!(note["version"], "0.1.0");
        assert!(!note["command"].as_str().unwrap().contains("uninstall"));

        std::fs::create_dir(tools.as_ref().unwrap().join("agentnave-studio")).unwrap();
        let command = service(&tools)["command"].as_str().unwrap().to_owned();
        assert!(command.starts_with("uv tool uninstall agentnave-studio && "));
        assert!(command.ends_with(&format!("@v{VERSION}\"")));

        std::fs::rename(
            site.join("agentnave-0.1.0.dist-info"),
            site.join(format!("agentnave-{VERSION}.dist-info")),
        )
        .unwrap();
        assert!(service(&tools).is_null());
    }
}
