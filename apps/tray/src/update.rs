//! In-app updates of the tray itself, and a one-click upgrade of the local AgentNave service
//! installed by `uv` when it is older than the tray. One confirmed click covers both (ADR-0014,
//! revised): the new app is downloaded, the service is upgraded to its version, and only then is
//! the app installed and restarted; a failed service upgrade leaves the app as it was.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::hosts::ToolDirs;
use serde_json::{Value, json};
use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Automatic checks run at start and then once a day; the menu can check at any time.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const REPOSITORY: &str = "https://github.com/TimWongUp/agentnave.git";
/// How long the panel keeps saying the service was upgraded.
const UPGRADED_NOTE: Duration = Duration::from_secs(10 * 60);
/// In the app's own data directory, not the AgentNave data root. Written only by apps before the
/// service-first update; a restart into this one still finishes what they promised.
const UPGRADE_MARKER: &str = "upgrade-service-after-restart";
/// The version an update installed, read once by its first start to say so.
const UPDATED_MARKER: &str = "updated-to";
/// How long `POST /app/update/check` waits for a check started elsewhere.
const CHECK_WAIT: Duration = Duration::from_secs(60);
/// Bounds the update check request; downloads keep the updater's own behavior.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Latest,
    Available(String),
    Installing(String),
    Failed(String),
}

#[derive(Clone, Default)]
enum Upgrade {
    #[default]
    Idle,
    Running,
    Installing,
    Done(Instant),
    Failed(String),
}

/// The app update and the service upgrade share one lock, so every entry checks and claims
/// both at once: an app install never overlaps a service upgrade.
#[derive(Default)]
struct State {
    phase: Phase,
    pending: Option<Update>,
    upgrade: Upgrade,
    /// What changed in each release between this app and the one found, newest first.
    notes: Value,
    /// Which part of an install runs: "download", "service" or "app".
    step: Option<&'static str>,
    /// The version the last restart updated to, and when this app started.
    updated: Option<(String, Instant)>,
}

impl State {
    fn installing(&self) -> bool {
        matches!(self.phase, Phase::Installing(_))
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Installing(_))
            || matches!(self.upgrade, Upgrade::Running | Upgrade::Installing)
    }
}

/// What a click on "更新并重启" or "全部更新" may start.
enum Claim {
    Install(Box<Update>),
    Upgrade,
    Busy,
    Nothing,
}

pub struct Updates {
    item: MenuItem<Wry>,
    state: Mutex<State>,
    checked: Mutex<Option<SystemTime>>,
}

impl Updates {
    pub fn new(item: MenuItem<Wry>) -> Self {
        Self {
            item,
            state: Mutex::new(State::default()),
            checked: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn label(&self, phase: &Phase) {
        let (text, enabled) = match phase {
            Phase::Idle => ("检查更新…".to_owned(), true),
            Phase::Checking => ("正在检查更新…".to_owned(), false),
            Phase::Latest => (format!("已是最新版本 v{VERSION}"), true),
            Phase::Available(version) => (format!("更新到 v{version}…"), true),
            Phase::Installing(_) => ("正在更新…".to_owned(), false),
            Phase::Failed(_) => ("更新失败，点此重试".to_owned(), true),
        };
        let _ = self.item.set_text(text);
        let _ = self.item.set_enabled(enabled);
    }

    fn set(&self, phase: Phase, pending: Option<Update>) {
        self.label(&phase);
        let mut state = self.lock();
        state.phase = phase;
        state.pending = pending;
    }

    /// Mark a check as started, unless one is already checking or installing. A running service
    /// upgrade does not conflict with a check, which installs nothing.
    fn begin(&self) -> Option<Phase> {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Checking | Phase::Installing(_)) {
            return None;
        }
        let before = std::mem::replace(&mut state.phase, Phase::Checking);
        state.pending = None;
        state.notes = Value::Null;
        drop(state);
        self.label(&Phase::Checking);
        Some(before)
    }

    /// Decide and claim in one step. `upgrade` allows falling back to a service upgrade when no
    /// app update is pending; `outdated` is whether the installed service is older.
    fn claim(&self, upgrade: bool, outdated: bool) -> Claim {
        let mut state = self.lock();
        if state.busy() {
            return Claim::Busy;
        }
        if let Some(update) = state.pending.take() {
            state.phase = Phase::Installing(update.version.clone());
            state.step = Some("download");
            drop(state);
            self.label(&Phase::Installing(update.version.clone()));
            return Claim::Install(Box::new(update));
        }
        if upgrade && outdated {
            state.upgrade = Upgrade::Running;
            return Claim::Upgrade;
        }
        Claim::Nothing
    }

    /// Mark the service upgrade as running, unless it already is or the app is installing.
    fn claim_upgrade(&self) -> bool {
        let mut state = self.lock();
        if state.installing() || matches!(state.upgrade, Upgrade::Running | Upgrade::Installing) {
            return false;
        }
        state.upgrade = Upgrade::Running;
        true
    }

    fn phase(&self) -> Phase {
        self.lock().phase.clone()
    }
}

async fn finish_check(app: AppHandle, manual: bool, before: Phase) {
    let found = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let updates = app.state::<Updates>();
    if found.is_ok() {
        *updates.checked.lock().unwrap() = Some(SystemTime::now());
    }
    match found {
        Ok(Some(update)) => {
            let current = app.package_info().version.to_string();
            let notes = release_notes(update.body.as_deref(), &current, &update.version);
            updates.set(Phase::Available(update.version.clone()), Some(update));
            updates.lock().notes = notes;
        }
        Ok(None) => updates.set(Phase::Latest, None),
        Err(error) if manual => updates.set(Phase::Failed(brief(&error.to_string())), None),
        Err(_) => updates.set(
            // A found update was dropped by `begin`, so it cannot be kept.
            if matches!(before, Phase::Failed(_) | Phase::Latest) {
                before
            } else {
                Phase::Idle
            },
            None,
        ),
    }
}

/// Look for a newer release. Automatic checks stay silent when offline or before any release
/// carries an update manifest; a check from the menu reports the failure.
pub fn check(app: &AppHandle, manual: bool) {
    let Some(before) = app.state::<Updates>().begin() else {
        return;
    };
    tauri::async_runtime::spawn(finish_check(app.clone(), manual, before));
}

/// `POST /app/update/check`: check now and answer with the result. The service version is
/// read again by every status. `manual=false` (`?quiet=1`, the settings page opening) fails
/// silently like the automatic checks.
pub fn check_now(app: &AppHandle, manual: bool) -> Value {
    match app.state::<Updates>().begin() {
        Some(before) => tauri::async_runtime::block_on(finish_check(app.clone(), manual, before)),
        None => {
            let started = Instant::now();
            while app.state::<Updates>().phase() == Phase::Checking
                && started.elapsed() < CHECK_WAIT
            {
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
    status(app)
}

/// `POST /app/update/apply`, sent once the page showed what a fresh check found: install a found
/// app update with its service (see `install`), otherwise upgrade an older service now. While a
/// check, an install or a service upgrade is running it only reports the status.
pub fn apply(app: &AppHandle) -> Value {
    let outdated = service_for_app(app).is_object();
    match app.state::<Updates>().claim(true, outdated) {
        Claim::Install(update) => install(app, *update),
        Claim::Upgrade => {
            let app = app.clone();
            thread::spawn(move || finish_upgrade(&app));
        }
        Claim::Busy | Claim::Nothing => {}
    }
    status(app)
}

/// At start: note the version an update installed, and run the service upgrade an older app's
/// one-click update left for after its restart.
pub fn resume(app: &AppHandle) {
    let Some(dir) = marker_dir(app) else {
        return;
    };
    if let Some(version) = take_updated(&dir) {
        app.state::<Updates>().lock().updated = Some((version, Instant::now()));
    }
    if take_marker(&dir) && service_for_app(app).is_object() {
        upgrade_service(app);
    }
}

fn marker_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

fn write_updated(dir: &Path, version: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(UPDATED_MARKER), version)
}

/// The version the last update installed, once.
fn take_updated(dir: &Path) -> Option<String> {
    let path = dir.join(UPDATED_MARKER);
    let version = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(path);
    Some(version.trim().to_owned()).filter(|version| !version.is_empty())
}

#[cfg(test)]
fn write_marker(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(UPGRADE_MARKER), VERSION)
}

/// Remove the marker; true when it was there.
fn take_marker(dir: &Path) -> bool {
    fs::remove_file(dir.join(UPGRADE_MARKER)).is_ok()
}

fn brief(text: &str) -> String {
    text.chars().take(200).collect()
}

/// Download the new app, upgrade the service to its version, then install and restart. A failed
/// download or service upgrade changes nothing; the service upgrade is skipped when it is not
/// uv-installed or already new enough.
fn install(app: &AppHandle, update: Update) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        let version = update.version.clone();
        let outcome = async {
            let bytes = update
                .download(|_, _| {}, || {})
                .await
                .map_err(|error| format!("下载失败：{}", brief(&error.to_string())))?;
            updates.lock().step = Some("service");
            let service = app.clone();
            let target = version.clone();
            tauri::async_runtime::spawn_blocking(move || upgrade_service_to(&service, &target))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("本机服务升级失败，App 未更新：{error}"))?;
            updates.lock().step = Some("app");
            update
                .install(bytes)
                .map_err(|error| format!("安装失败：{}", brief(&error.to_string())))
        }
        .await;
        match outcome {
            Ok(()) => {
                if let Some(dir) = marker_dir(&app) {
                    let _ = write_updated(&dir, &version);
                }
                app.restart();
            }
            Err(error) => {
                updates.lock().step = None;
                updates.set(Phase::Failed(error), None);
            }
        }
    });
}

/// Bring a uv-installed service up to `version` before the app that needs it is installed.
fn upgrade_service_to(app: &AppHandle, version: &str) -> Result<(), String> {
    let dirs = service_dirs(app).ok_or("宿主目录检测尚未完成，请稍后重试")?;
    let Some(installed) = installed_version(&dirs.tools.join("agentnave")) else {
        return Ok(());
    };
    if parse(&installed) >= parse(version) {
        return Ok(());
    }
    let uv = find_uv().ok_or("未找到 uv，请先安装 uv 或在终端运行升级命令")?;
    install_service_version(&uv, &dirs, version)?;
    app.state::<crate::Tray>().reader.refresh_hosts();
    Ok(())
}

/// Each release's notes from the update manifest, newer than `current` up to `target`, newest
/// first. The manifest carries `[{"version", "notes"}]` as JSON text; an older one only links to
/// its release page, which is kept as `link`.
fn release_notes(body: Option<&str>, current: &str, target: &str) -> Value {
    let body = body.unwrap_or_default().trim();
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(body) else {
        let link = body.starts_with("https://").then_some(body);
        return json!({"items": [], "link": link});
    };
    let mut items: Vec<Value> = entries
        .into_iter()
        .filter(|entry| {
            entry["version"].as_str().is_some_and(|version| {
                parse(version) > parse(current) && parse(version) <= parse(target)
            }) && entry["notes"].is_string()
        })
        .collect();
    items.sort_by(|a, b| {
        parse(b["version"].as_str().unwrap_or_default())
            .cmp(&parse(a["version"].as_str().unwrap_or_default()))
    });
    json!({"items": items, "link": null})
}

/// What the panel and settings show about versions: the app update, any outdated local
/// service, and when the last check succeeded.
pub fn status(app: &AppHandle) -> Value {
    let updates = app.try_state::<Updates>();
    let (phase, upgrade, notes, step, updated) = updates
        .as_ref()
        .map(|updates| {
            let state = updates.lock();
            (
                state.phase.clone(),
                state.upgrade.clone(),
                state.notes.clone(),
                state.step,
                state
                    .updated
                    .as_ref()
                    .filter(|(_, at)| at.elapsed() < UPGRADED_NOTE)
                    .map(|(version, _)| version.clone()),
            )
        })
        .unwrap_or_default();
    let checked = updates.and_then(|updates| *updates.checked.lock().unwrap());
    let update = match phase {
        Phase::Idle => Value::Null,
        Phase::Checking => json!({"state": "checking", "version": null, "error": null}),
        Phase::Latest => json!({"state": "none", "version": null, "error": null}),
        Phase::Available(version) => {
            json!({"state": "available", "version": version, "error": null, "notes": notes})
        }
        Phase::Installing(version) => {
            json!({"state": "installing", "version": version, "error": null, "step": step})
        }
        Phase::Failed(error) => json!({"state": "failed", "version": null, "error": error}),
    };
    json!({
        "version": VERSION,
        "update": update,
        "service": service_state(service_for_app(app), upgrade),
        "last_checked": checked.map(rfc3339),
        "updated": updated,
    })
}

/// The outdated note combined with what a one-click upgrade is doing or did.
fn service_state(mut service: Value, upgrade: Upgrade) -> Value {
    match upgrade {
        Upgrade::Installing => service = json!({"state": "installing", "error": null}),
        Upgrade::Running => {
            let version = service.get("version").cloned().unwrap_or(Value::Null);
            service = json!({"state": "upgrading", "version": version, "error": null});
        }
        Upgrade::Failed(error) => service = json!({"state": "failed", "error": error}),
        Upgrade::Done(at) if service.is_null() && at.elapsed() < UPGRADED_NOTE => {
            service = json!({"state": "done", "version": VERSION, "error": null});
        }
        _ => {}
    }
    service
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn rfc3339(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    // The shared formatter, without its microseconds.
    let iso = crate::stage::iso(seconds as i64 * 1_000_000);
    format!("{}Z", &iso[..19])
}

/// Install this release's service with uv.
pub fn upgrade_service(app: &AppHandle) {
    if app.state::<Updates>().claim_upgrade() {
        finish_upgrade(app);
    }
}

fn finish_upgrade(app: &AppHandle) {
    let outcome = (|| {
        let dirs = service_dirs(app).ok_or("宿主目录检测尚未完成，请刷新检测后重试")?;
        let uv = find_uv().ok_or("未找到 uv，请先安装 uv 或复制命令在终端运行")?;
        install_service(&uv, &dirs)?;
        app.state::<crate::Tray>().reader.refresh_hosts();
        Ok(())
    })();
    app.state::<Updates>().lock().upgrade = match outcome {
        Ok(()) => Upgrade::Done(Instant::now()),
        Err(error) => Upgrade::Failed(error),
    };
}

pub(crate) fn install_service(uv: &Path, dirs: &ToolDirs) -> Result<(), String> {
    install_service_version(uv, dirs, VERSION)
}

fn install_service_version(uv: &Path, dirs: &ToolDirs, version: &str) -> Result<(), String> {
    let source = format!("git+{REPOSITORY}@v{version}");
    run(
        uv,
        &["tool", "install", "--force", "--python", "3.12", &source],
        dirs,
    )
}

pub fn service_command() -> String {
    format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"")
}

/// Claim before spawning, so POST returns immediately and progress is visible on the next GET.
pub fn install_service_now(app: &AppHandle) -> Value {
    let updates = app.state::<Updates>();
    {
        let mut state = updates.lock();
        if state.busy() {
            return json!({"ok": false, "message": "已有更新或安装正在进行"});
        }
        state.upgrade = Upgrade::Installing;
    }
    let app = app.clone();
    thread::spawn(move || finish_upgrade(&app));
    json!({"ok": true, "message": "正在安装本机服务"})
}

fn run(uv: &Path, args: &[&str], dirs: &ToolDirs) -> Result<(), String> {
    let mut command = Command::new(uv);
    command
        .args(args)
        .env("HOME", &dirs.home)
        .env("UV_TOOL_DIR", &dirs.tools)
        .env("UV_TOOL_BIN_DIR", &dirs.bin)
        .stdin(std::process::Stdio::null());
    // Apps started from the Finder get a bare PATH; uv needs git to fetch the release.
    #[cfg(not(windows))]
    if let Some(dir) = uv.parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![
            dir.to_path_buf(),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ];
        dirs.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(dirs) {
            command.env("PATH", joined);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("uv 返回失败");
    Err(reason.chars().take(200).collect())
}

/// uv on PATH, else where its installers put it.
fn find_uv() -> Option<PathBuf> {
    let name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join(name));
        candidates.push(home.join(".cargo").join("bin").join(name));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin").join(name));
    candidates.push(PathBuf::from("/usr/local/bin").join(name));
    candidates.into_iter().find(|path| path.is_file())
}

/// Read exactly the cached roots used by Hosts; GET/status never probes a shell.
fn service_dirs(app: &AppHandle) -> Option<ToolDirs> {
    app.state::<crate::Tray>().reader.tool_dirs()
}

fn service_for_app(app: &AppHandle) -> Value {
    service(&service_dirs(app).map(|dirs| dirs.tools))
}

/// A note with the upgrade command when the installed service is older than this app.
pub(crate) fn service(tools: &Option<PathBuf>) -> Value {
    let Some(tools) = tools else {
        return Value::Null;
    };
    let Some(installed) = installed_version(&tools.join("agentnave")) else {
        return Value::Null;
    };
    if parse(&installed) >= parse(VERSION) {
        return Value::Null;
    }
    let command = format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"");
    json!({"state": "outdated", "version": installed, "error": null, "command": command})
}

/// The version in `agentnave-<version>.dist-info` inside a uv tool environment.
pub(crate) fn installed_version(env: &Path) -> Option<String> {
    installed_metadata(env)?
        .file_name()?
        .to_str()?
        .strip_prefix("agentnave-")?
        .strip_suffix(".dist-info")
        .map(str::to_owned)
}

/// When the installed version arrived: its dist-info directory is created by the install.
pub(crate) fn installed_since(env: &Path) -> Option<std::time::SystemTime> {
    installed_metadata(env)?.metadata().ok()?.modified().ok()
}

/// Recognize the unified runtime by its installed console-script contract, not an old core binary.
pub(crate) fn unified_service(tools: &Path) -> bool {
    installed_metadata(&tools.join("agentnave"))
        .and_then(|path| std::fs::read_to_string(path.join("entry_points.txt")).ok())
        .is_some_and(|text| {
            text.lines()
                .any(|line| line.trim() == "agentnave-mcp = agentnave.relay:main")
                && !text
                    .lines()
                    .any(|line| line.trim_start().starts_with("agentnave-studio-mcp"))
        })
}

fn installed_metadata(env: &Path) -> Option<PathBuf> {
    let mut queue = vec![(env.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("agentnave-") && name.ends_with(".dist-info") {
                return Some(entry.path());
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
pub fn copy_service_command(app: &AppHandle) {
    let Some(command) = service_for_app(app)["command"].as_str().map(str::to_owned) else {
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
    fn notes_an_older_service() {
        let tools = tempfile::tempdir().unwrap();
        let site = tools.path().join("agentnave/lib/python3.12/site-packages");
        std::fs::create_dir_all(site.join("agentnave-0.1.0.dist-info")).unwrap();
        std::fs::create_dir_all(site.join("mcp-2.1.1.dist-info")).unwrap();
        let tools = Some(tools.path().to_path_buf());

        let note = service(&tools);
        assert_eq!(note["version"], "0.1.0");
        assert!(
            note["command"]
                .as_str()
                .unwrap()
                .ends_with(&format!("@v{VERSION}\""))
        );

        std::fs::rename(
            site.join("agentnave-0.1.0.dist-info"),
            site.join(format!("agentnave-{VERSION}.dist-info")),
        )
        .unwrap();
        assert!(service(&tools).is_null());
    }

    #[test]
    fn service_state_follows_the_upgrade() {
        let outdated =
            json!({"state": "outdated", "version": "0.1.0", "error": null, "command": "uv"});
        assert_eq!(service_state(outdated.clone(), Upgrade::Idle), outdated);
        assert_eq!(
            service_state(outdated.clone(), Upgrade::Running),
            json!({"state": "upgrading", "version": "0.1.0", "error": null})
        );
        let failed = service_state(outdated, Upgrade::Failed("uv 返回失败".into()));
        assert_eq!(
            (failed["state"].as_str(), failed["error"].as_str()),
            (Some("failed"), Some("uv 返回失败"))
        );
        assert_eq!(
            service_state(Value::Null, Upgrade::Done(Instant::now()))["state"],
            "done"
        );
        assert_eq!(
            service_state(Value::Null, Upgrade::Failed("x".into())),
            json!({"state":"failed", "error":"x"})
        );
        assert_eq!(
            service_state(Value::Null, Upgrade::Installing)["state"],
            "installing"
        );
    }

    #[test]
    fn restart_marker_is_written_once_and_cleared() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path().join("app-data");
        assert!(!take_marker(&dir));
        write_marker(&dir).unwrap();
        assert!(dir.join(UPGRADE_MARKER).is_file());
        assert!(take_marker(&dir));
        assert!(!take_marker(&dir));
    }

    #[test]
    fn notes_keep_the_releases_after_this_app_newest_first() {
        let body = json!([
            {"version": "0.46.1", "notes": "a"},
            {"version": "0.48.0", "notes": "c"},
            {"version": "0.47.0", "notes": "b"},
            {"version": "0.49.0", "notes": "d"},
            {"version": "0.47.5"},
        ])
        .to_string();
        let notes = release_notes(Some(&body), "0.46.1", "0.48.0");
        let versions: Vec<_> = notes["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["version"].as_str().unwrap())
            .collect();
        assert_eq!(versions, ["0.48.0", "0.47.0"]);
        let old = "https://github.com/TimWongUp/agentnave/releases/tag/v0.47.0";
        assert_eq!(
            release_notes(Some(old), "0.46.1", "0.47.0"),
            json!({"items": [], "link": old})
        );
        assert_eq!(
            release_notes(None, "0.46.1", "0.47.0"),
            json!({"items": [], "link": null})
        );
    }

    #[test]
    fn the_updated_version_is_read_once() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path().join("app-data");
        assert_eq!(take_updated(&dir), None);
        write_updated(&dir, "0.48.0").unwrap();
        assert_eq!(take_updated(&dir).as_deref(), Some("0.48.0"));
        assert_eq!(take_updated(&dir), None);
    }

    #[test]
    fn last_checked_is_utc_rfc3339() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_152_000);
        assert_eq!(rfc3339(at), "2026-10-16T12:00:00Z");
        assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339(leap), "2000-02-29T00:00:00Z");
    }
}
