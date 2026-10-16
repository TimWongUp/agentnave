//! In-app updates of the tray itself, and a one-click upgrade of the local AgentNave service
//! installed by `uv` when it is older than the tray. "全部更新" covers both with one click
//! (ADR-0014): the app installs first and upgrades the service after its restart.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

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
/// In the app's own data directory, not the AgentNave data root.
const UPGRADE_MARKER: &str = "upgrade-service-after-restart";
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
    Done(Instant),
    Failed(String),
}

pub struct Updates {
    item: MenuItem<Wry>,
    state: Mutex<(Phase, Option<Update>)>,
    upgrade: Mutex<Upgrade>,
    checked: Mutex<Option<SystemTime>>,
    /// Why the post-restart service upgrade could not be promised for the current install.
    marker_error: Mutex<Option<String>>,
}

impl Updates {
    pub fn new(item: MenuItem<Wry>) -> Self {
        Self {
            item,
            state: Mutex::new((Phase::Idle, None)),
            upgrade: Mutex::new(Upgrade::Idle),
            checked: Mutex::new(None),
            marker_error: Mutex::new(None),
        }
    }

    fn label(&self, phase: &Phase) {
        let (text, enabled) = match phase {
            Phase::Idle => ("检查更新…".to_owned(), true),
            Phase::Checking => ("正在检查更新…".to_owned(), false),
            Phase::Latest => (format!("已是最新版本 v{VERSION}"), true),
            Phase::Available(version) => (format!("更新到 v{version} 并重启"), true),
            Phase::Installing(_) => ("正在下载更新…".to_owned(), false),
            Phase::Failed(_) => ("更新失败，点此重试".to_owned(), true),
        };
        let _ = self.item.set_text(text);
        let _ = self.item.set_enabled(enabled);
    }

    fn set(&self, phase: Phase, pending: Option<Update>) {
        self.label(&phase);
        *self.state.lock().unwrap() = (phase, pending);
    }

    /// Mark a check as started, unless one is already checking or installing.
    fn begin(&self) -> Option<Phase> {
        let mut state = self.state.lock().unwrap();
        if matches!(state.0, Phase::Checking | Phase::Installing(_)) {
            return None;
        }
        let before = std::mem::replace(&mut state.0, Phase::Checking);
        state.1 = None;
        drop(state);
        self.label(&Phase::Checking);
        Some(before)
    }

    /// Take the found update and mark it installing, so concurrent clicks install it once.
    fn start_install(&self) -> Option<Update> {
        let mut state = self.state.lock().unwrap();
        if matches!(state.0, Phase::Checking | Phase::Installing(_)) {
            return None;
        }
        let update = state.1.take()?;
        state.0 = Phase::Installing(update.version.clone());
        drop(state);
        self.label(&Phase::Installing(update.version.clone()));
        Some(update)
    }

    fn busy(&self) -> bool {
        matches!(self.phase(), Phase::Checking | Phase::Installing(_))
            || matches!(*self.upgrade.lock().unwrap(), Upgrade::Running)
    }

    fn phase(&self) -> Phase {
        self.state.lock().unwrap().0.clone()
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
        Ok(Some(update)) => updates.set(Phase::Available(update.version.clone()), Some(update)),
        Ok(None) => updates.set(Phase::Latest, None),
        Err(error) if manual => updates.set(Phase::Failed(brief(&error.to_string())), None),
        Err(_) => updates.set(
            if matches!(before, Phase::Failed(_)) {
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
/// read again by every status.
pub fn check_now(app: &AppHandle) -> Value {
    match app.state::<Updates>().begin() {
        Some(before) => tauri::async_runtime::block_on(finish_check(app.clone(), true, before)),
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

/// `POST /app/update/apply`: update everything with one click: install a found app update
/// (see `act`), otherwise upgrade an older service now. While a check, an install or a service
/// upgrade is running it only reports the status.
pub fn apply(app: &AppHandle) -> Value {
    let updates = app.state::<Updates>();
    if updates.busy() {
        return status(app);
    }
    let pending = updates.state.lock().unwrap().1.is_some();
    if pending {
        act(app);
    } else if service(&tool_dir()).is_object() && claim_upgrade(app) {
        let app = app.clone();
        thread::spawn(move || finish_upgrade(&app));
    }
    status(app)
}

/// At start: run the service upgrade a one-click update left for after its restart.
pub fn resume(app: &AppHandle) {
    if marker_dir(app).is_some_and(|dir| take_marker(&dir)) && service(&tool_dir()).is_object() {
        upgrade_service(app);
    }
}

fn marker_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

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

/// The menu, the panel and "全部更新" share one action: install a found update after recording
/// the post-restart service upgrade, otherwise check. Does nothing while checking or installing.
pub fn act(app: &AppHandle) {
    let updates = app.state::<Updates>();
    if matches!(updates.phase(), Phase::Checking | Phase::Installing(_)) {
        return;
    }
    let Some(update) = updates.start_install() else {
        check(app, true);
        return;
    };
    let recorded = marker_dir(app)
        .ok_or_else(|| "找不到应用数据目录".to_owned())
        .and_then(|dir| write_marker(&dir).map_err(|error| error.to_string()));
    *updates.marker_error.lock().unwrap() = recorded.err().map(|error| {
        brief(&format!(
            "无法记录重启后升级本机服务（{error}），重启后请手动升级"
        ))
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => app.restart(),
            Err(error) => {
                // A later, unrelated restart must not upgrade the service unasked.
                if let Some(dir) = marker_dir(&app) {
                    take_marker(&dir);
                }
                let updates = app.state::<Updates>();
                updates.set(Phase::Failed(brief(&error.to_string())), None);
            }
        }
    });
}

/// What the panel and settings show about versions: the app update, any outdated local
/// service, and when the last check succeeded.
pub fn status(app: &AppHandle) -> Value {
    let updates = app.try_state::<Updates>();
    let phase = updates
        .as_ref()
        .map(|updates| updates.phase())
        .unwrap_or_default();
    let upgrade = updates
        .as_ref()
        .map(|updates| updates.upgrade.lock().unwrap().clone())
        .unwrap_or_default();
    let marker_error = updates
        .as_ref()
        .and_then(|updates| updates.marker_error.lock().unwrap().clone());
    let checked = updates.and_then(|updates| *updates.checked.lock().unwrap());
    let update = match phase {
        Phase::Idle => Value::Null,
        Phase::Checking => json!({"state": "checking", "version": null, "error": null}),
        Phase::Latest => json!({"state": "none", "version": null, "error": null}),
        Phase::Available(version) => {
            json!({"state": "available", "version": version, "error": null})
        }
        Phase::Installing(version) => {
            json!({"state": "installing", "version": version, "error": marker_error})
        }
        Phase::Failed(error) => json!({"state": "failed", "version": null, "error": error}),
    };
    json!({
        "version": VERSION,
        "update": update,
        "service": service_state(service(&tool_dir()), upgrade),
        "last_checked": checked.map(rfc3339),
    })
}

/// The outdated note combined with what a one-click upgrade is doing or did.
fn service_state(mut service: Value, upgrade: Upgrade) -> Value {
    match upgrade {
        Upgrade::Running => {
            let version = service.get("version").cloned().unwrap_or(Value::Null);
            service = json!({"state": "upgrading", "version": version, "error": null});
        }
        Upgrade::Failed(error) if service.is_object() => {
            service["state"] = json!("failed");
            service["error"] = json!(error);
        }
        Upgrade::Done(at) if service.is_null() && at.elapsed() < UPGRADED_NOTE => {
            service = json!({"state": "done", "version": VERSION, "error": null});
        }
        _ => {}
    }
    service
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// Install this release's service with uv, replacing the legacy studio tool if present. Hosts
/// switch on their own: since v0.18.0 each launcher relays to a server it restarts when idle.
pub fn upgrade_service(app: &AppHandle) {
    if claim_upgrade(app) {
        finish_upgrade(app);
    }
}

/// Mark the upgrade as running, unless one already is.
fn claim_upgrade(app: &AppHandle) -> bool {
    let updates = app.state::<Updates>();
    let mut upgrade = updates.upgrade.lock().unwrap();
    if matches!(*upgrade, Upgrade::Running) {
        return false;
    }
    *upgrade = Upgrade::Running;
    true
}

fn finish_upgrade(app: &AppHandle) {
    let outcome = install_service();
    *app.state::<Updates>().upgrade.lock().unwrap() = match outcome {
        Ok(()) => Upgrade::Done(Instant::now()),
        Err(error) => Upgrade::Failed(error),
    };
}

fn install_service() -> Result<(), String> {
    let uv = find_uv().ok_or("未找到 uv，请先安装 uv 或复制命令在终端运行")?;
    if tool_dir().is_some_and(|tools| tools.join("agentnave-studio").is_dir()) {
        run(&uv, &["tool", "uninstall", "agentnave-studio"])?;
    }
    let source = format!("git+{REPOSITORY}@v{VERSION}");
    run(
        &uv,
        &["tool", "install", "--force", "--python", "3.12", &source],
    )
}

fn run(uv: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(uv);
    command.args(args);
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
    json!({"state": "outdated", "version": installed, "error": null, "command": command})
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
        assert!(service_state(Value::Null, Upgrade::Failed("x".into())).is_null());
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
    fn last_checked_is_utc_rfc3339() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_152_000);
        assert_eq!(rfc3339(at), "2026-10-16T12:00:00Z");
        assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339(leap), "2000-02-29T00:00:00Z");
    }
}
