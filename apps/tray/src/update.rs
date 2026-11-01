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

mod cleanup;
pub(crate) use cleanup::prune;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Automatic checks run at start and then once a day; the menu can check at any time.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const REPOSITORY: &str = "https://github.com/TimWongUp/agentnave.git";
/// Where the notes of releases older than those in the manifest are.
const RELEASES: &str = "https://github.com/TimWongUp/agentnave/releases";
/// How long the panel keeps saying the service was upgraded.
const UPGRADED_NOTE: Duration = Duration::from_secs(10 * 60);
/// In the app's own data directory, not the AgentNave data root. Written only by apps before the
/// service-first update; a restart into this one still finishes what they promised.
const UPGRADE_MARKER: &str = "upgrade-service-after-restart";
/// The version an update installed, read once by its first start to say so.
const UPDATED_MARKER: &str = "updated-to";
/// How long `POST /app/update/check` waits for a check started elsewhere.
const CHECK_WAIT: Duration = Duration::from_secs(60);
/// Bounds the update check request.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// A found update is used as it is for this long, the same as the automatic checks' interval;
/// after that the update flow first looks for a newer one.
const FOUND_FOR: Duration = EVERY;
/// The updater sets no limit of its own, and the update dialog cannot be closed while it runs.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The dependency index used for the retry that follows a failed direct install. PyPI is slow or
/// unreachable from mainland China, where the first install is otherwise impossible.
const MIRROR_INDEX: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";

/// `uv tool install` fetches from GitHub without a limit of its own.
const SERVICE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Long enough to read the last step before the window goes away.
const RESTART_NOTE: Duration = Duration::from_secs(1);
const EMBEDDED_WHEEL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agentnave.whl"));

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
    /// Which part of an install runs or, after a failure, failed: "download", "service", "app"
    /// or "restart".
    step: Option<&'static str>,
    /// When the running step or service upgrade started.
    started: Option<Instant>,
    /// Bytes of the app update downloaded, and its size when the server tells.
    download: (u64, Option<u64>),
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

    fn enter(&mut self, step: &'static str) {
        self.step = Some(step);
        self.started = Some(Instant::now());
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
    /// upgrade does not conflict with a check, which installs nothing. A found update stays until
    /// the check ends, so a silent one that fails can keep it.
    fn begin(&self) -> Option<Phase> {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Checking | Phase::Installing(_)) {
            return None;
        }
        let before = std::mem::replace(&mut state.phase, Phase::Checking);
        drop(state);
        self.label(&Phase::Checking);
        Some(before)
    }

    /// Decide and claim the requested target in one step; app and service requests never
    /// silently substitute each other. `outdated` is whether the installed service is older.
    fn claim(&self, upgrade: bool, outdated: bool) -> Claim {
        let mut state = self.lock();
        if state.busy() {
            return Claim::Busy;
        }
        if !upgrade && let Some(update) = state.pending.take() {
            state.phase = Phase::Installing(update.version.clone());
            state.enter("download");
            state.download = (0, None);
            drop(state);
            self.label(&Phase::Installing(update.version.clone()));
            return Claim::Install(Box::new(update));
        }
        if upgrade && outdated {
            state.upgrade = Upgrade::Running;
            state.started = Some(Instant::now());
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
        state.started = Some(Instant::now());
        true
    }

    fn phase(&self) -> Phase {
        self.lock().phase.clone()
    }
}

/// An update manifest to use instead of the bundled endpoint. Releases live on GitHub, which is
/// slow or unreachable from mainland China; pointing this at a mirror makes updates usable there.
fn update_endpoint() -> Option<tauri::Url> {
    let value = std::env::var("AGENTNAVE_UPDATE_ENDPOINT").ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    tauri::Url::parse(trimmed).ok()
}

async fn finish_check(app: AppHandle, manual: bool, before: Phase) {
    let mut builder = app.updater_builder().timeout(CHECK_TIMEOUT);
    if let Some(proxy) = updater_proxy(&app) {
        builder = builder.proxy(proxy);
    }
    // `endpoints` consumes the builder, so pick the shape once instead of trying and backing out.
    let builder = match update_endpoint() {
        Some(endpoint) => builder.endpoints(vec![endpoint]),
        None => Ok(builder),
    };
    let found = match builder.and_then(|builder| builder.build()) {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let updates = app.state::<Updates>();
    // A failed silent check still counts as an attempt, so an offline cache is not
    // checked again on every click. Explicit rechecks remain available.
    *updates.checked.lock().unwrap() = Some(SystemTime::now());
    match found {
        Ok(Some(update)) => {
            let current = app.package_info().version.to_string();
            let notes = release_notes(update.body.as_deref(), &current, &update.version);
            updates.set(Phase::Available(update.version.clone()), Some(update));
            let mut state = updates.lock();
            state.notes = notes;
            state.step = None;
        }
        Ok(None) => {
            updates.set(Phase::Latest, None);
            updates.lock().step = None;
        }
        Err(error) if manual => {
            updates.set(Phase::Failed(brief(&error.to_string())), None);
            updates.lock().step = None;
        }
        Err(_) => {
            let found = updates.lock().pending.take();
            let phase = if found.is_some() || matches!(before, Phase::Failed(_) | Phase::Latest) {
                before
            } else {
                Phase::Idle
            };
            updates.set(phase, found);
        }
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

/// Install the cached app update, or only upgrade the service with `?service=1`.
/// The receipt distinguishes a new start (including a fast failure) from an old status.
pub fn apply(app: &AppHandle, service_only: bool) -> Value {
    let outdated = service_for_app(app).is_object();
    let started = match app.state::<Updates>().claim(service_only, outdated) {
        Claim::Install(update) => {
            install(app, *update);
            true
        }
        Claim::Upgrade => {
            let app = app.clone();
            thread::spawn(move || finish_upgrade(&app));
            true
        }
        Claim::Busy | Claim::Nothing => false,
    };
    let mut value = status(app);
    value["started"] = json!(started);
    value
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
    // The service is a separate artifact the hosts start, so it follows the App on every start
    // instead of waiting for a click. A versioned install makes that safe: the running service
    // keeps its own directory, and hosts move over when they next start.
    // A stale marker from an older app still gets consumed here.
    let _marked = take_marker(&dir);
    if service_for_app(app).is_object() {
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
/// uv-installed or already new enough. A failure keeps the update, so a retry needs no new check.
fn install(app: &AppHandle, mut update: Update) {
    let app = app.clone();
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        let version = update.version.clone();
        let outcome = async {
            let bytes = update
                .download(
                    |chunk, total| {
                        let mut state = updates.lock();
                        state.download = (state.download.0 + chunk as u64, total);
                    },
                    || {},
                )
                .await
                .map_err(|error| {
                    format!(
                        "下载失败，App 和本机服务都没有改动：{}",
                        brief(&error.to_string())
                    )
                })?;
            updates.lock().enter("service");
            let service = app.clone();
            let target = version.clone();
            let archive = bytes.clone();
            tauri::async_runtime::spawn_blocking(move || {
                upgrade_service_to(&service, &target, Some(&archive))
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("本机服务升级失败，App 未更新：{error}"))?;
            updates.lock().enter("app");
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
                updates.lock().enter("restart");
                thread::sleep(RESTART_NOTE);
                app.restart();
            }
            Err(error) => updates.set(Phase::Failed(error), Some(update)),
        }
    });
}

/// Bring a uv-installed service up to `version` before the app that needs it is installed.
fn upgrade_service_to(
    app: &AppHandle,
    version: &str,
    archive: Option<&[u8]>,
) -> Result<(), String> {
    let dirs = service_dirs(app).ok_or("宿主目录检测尚未完成，请稍后重试")?;
    let current = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|data| active_version(&data))
        .or_else(|| installed_version(&dirs.tools.join("agentnave")));
    let Some(installed) = current else {
        return Ok(());
    };
    if parse(&installed) >= parse(version) {
        return Ok(());
    }
    let uv = find_uv().ok_or("未找到 uv，请先安装 uv 或在终端运行升级命令")?;
    // The same versioned layout as a manual upgrade, or hosts would keep the old active version.
    let target = version_dirs(app, &dirs, version).ok_or("无法定位应用数据目录")?;
    install_service_version(&uv, &target, version, archive)?;
    cleanup::register(&target.tools.join("agentnave"));
    write_active(app, version, &target)?;
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
    // The manifest carries the last few releases only; an app older than all of them gets a link
    // for the ones in between.
    let gap = entries.iter().all(|entry| {
        entry["version"]
            .as_str()
            .is_none_or(|version| parse(version) > parse(current))
    });
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
    json!({"items": items, "link": gap.then_some(RELEASES)})
}

/// What the panel and settings show about versions: the app update, any outdated local
/// service, and when the last check succeeded.
pub fn status(app: &AppHandle) -> Value {
    let updates = app.try_state::<Updates>();
    let (phase, upgrade, notes, step, download, started, retry, updated) = updates
        .as_ref()
        .map(|updates| {
            let state = updates.lock();
            (
                state.phase.clone(),
                state.upgrade.clone(),
                state.notes.clone(),
                state.step,
                state.download,
                state.started,
                state.pending.as_ref().map(|update| update.version.clone()),
                state
                    .updated
                    .as_ref()
                    .filter(|(_, at)| at.elapsed() < UPGRADED_NOTE)
                    .map(|(version, _)| version.clone()),
            )
        })
        .unwrap_or_default();
    let checked = updates.and_then(|updates| *updates.checked.lock().unwrap());
    let running = matches!(phase, Phase::Installing(_))
        || matches!(upgrade, Upgrade::Running | Upgrade::Installing);
    let update = match phase {
        Phase::Idle => Value::Null,
        Phase::Checking => json!({"state": "checking", "version": null, "error": null}),
        Phase::Latest => json!({"state": "none", "version": null, "error": null}),
        Phase::Available(version) => {
            json!({"state": "available", "version": version, "error": null, "notes": notes})
        }
        Phase::Installing(version) => {
            json!({
                "state": "installing", "version": version, "error": null, "step": step,
                "downloaded": download.0, "total": download.1,
            })
        }
        // A version and a step say the install failed there and can be retried as it is.
        Phase::Failed(error) => {
            json!({"state": "failed", "version": retry, "error": error, "step": step})
        }
    };
    json!({
        "version": VERSION,
        "update": update,
        "service": service_state(service_for_app(app), upgrade),
        "last_checked": checked.map(rfc3339),
        "check_fresh": check_fresh(checked, SystemTime::now()),
        "elapsed": started.filter(|_| running).map(|at| at.elapsed().as_secs()),
        "updated": updated,
    })
}

/// Whether what the last check found can be used without checking again.
fn check_fresh(checked: Option<SystemTime>, now: SystemTime) -> bool {
    // A clock set back reads as just checked, which at worst keeps a found update a little longer.
    checked.is_some_and(|at| now.duration_since(at).unwrap_or_default() < FOUND_FOR)
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
/// Whether this build's service still has to be installed over what is active.
///
/// Re-read under the claim: another path may have installed the same or a newer version while
/// this one was still deciding, and reinstalling a running version would touch live files.
fn should_install(current: Option<&str>, target: &str) -> bool {
    current.is_none_or(|version| parse(version) < parse(target))
}

pub fn upgrade_service(app: &AppHandle) {
    if app.state::<Updates>().claim_upgrade() {
        finish_upgrade(app);
    }
}

fn finish_upgrade(app: &AppHandle) {
    let outcome = (|| {
        // The decision was made outside the claim; make it again now that this path owns the
        // install, so a concurrent upgrade to the same or a newer version is left alone.
        let current = app
            .path()
            .app_data_dir()
            .ok()
            .and_then(|data| active_version(&data));
        if !should_install(current.as_deref(), VERSION) {
            return Ok(());
        }
        let dirs = service_dirs(app).ok_or("宿主目录检测尚未完成，请刷新检测后重试")?;
        let uv = find_uv().ok_or("未找到 uv，请先安装 uv 或复制命令在终端运行")?;
        // Install beside the running service instead of over it; the active manifest then moves
        // the hosts over, which takes effect when each host next starts its server.
        let target = version_dirs(app, &dirs, VERSION).ok_or("无法定位应用数据目录")?;
        install_service_version(&uv, &target, VERSION, None)?;
        cleanup::register(&target.tools.join("agentnave"));
        write_active(app, VERSION, &target)?;
        app.state::<crate::Tray>().reader.refresh_hosts();
        Ok(())
    })();
    app.state::<Updates>().lock().upgrade = match outcome {
        Ok(()) => Upgrade::Done(Instant::now()),
        Err(error) => Upgrade::Failed(error),
    };
}

#[allow(dead_code)]
pub(crate) fn install_service(uv: &Path, dirs: &ToolDirs) -> Result<(), String> {
    install_service_version(uv, dirs, VERSION, None)
}

/// The same directories with uv's dependency index pointed at a reachable mirror.
fn with_mirror(dirs: &ToolDirs) -> ToolDirs {
    let mut mirrored = dirs.clone();
    mirrored
        .network
        .insert("UV_DEFAULT_INDEX".into(), MIRROR_INDEX.into());
    mirrored
}

fn install_service_version(
    uv: &Path,
    dirs: &ToolDirs,
    version: &str,
    archive: Option<&[u8]>,
) -> Result<(), String> {
    let embedded = if version == VERSION {
        extract_wheel(EMBEDDED_WHEEL, VERSION)
    } else {
        archive.and_then(|bytes| wheel_from_archive(bytes, version))
    };
    if let Some(wheel) = embedded {
        let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
        let file = stage
            .path()
            .join(format!("agentnave-{version}-py3-none-any.whl"));
        fs::write(&file, wheel).map_err(|error| error.to_string())?;
        let source = file.to_str().ok_or("临时目录路径无效")?;
        let online = run(
            uv,
            &["tool", "install", "--force", "--python", "3.12", source],
            dirs,
        );
        if online.is_ok() {
            return Ok(());
        }
        // The wheel is embedded but its dependency (`mcp`) is not, so an offline upgrade relies on
        // the wheels the previous install already put in uv's cache. Retry offline for any target
        // directory, not just a re-install of the same version, or a machine without a network
        // could never move to a new version.
        if run(
            uv,
            &[
                "tool",
                "install",
                "--force",
                "--offline",
                "--python",
                "3.12",
                source,
            ],
            dirs,
        )
        .is_ok()
        {
            return Ok(());
        }
        // Dependencies still missing from the cache: the direct index was unreachable, so try a
        // mirror before giving up. This is the only path that needs the network.
        if run(
            uv,
            &["tool", "install", "--force", "--python", "3.12", source],
            &with_mirror(dirs),
        )
        .is_ok()
        {
            return Ok(());
        }
        return Err(format!(
            "{}（首次安装需要联网下载依赖：直连与镜像均失败，可设置 UV_DEFAULT_INDEX 或使用代理）",
            online.unwrap_err()
        ));
    }
    let source = format!("git+{REPOSITORY}@v{version}");
    run(
        uv,
        &["tool", "install", "--force", "--python", "3.12", &source],
        dirs,
    )
}

fn extract_wheel(haystack: &[u8], version: &str) -> Option<Vec<u8>> {
    let mut prefix = Vec::with_capacity(20 + version.len());
    prefix.extend_from_slice(b"\x89ANWHEEL");
    prefix.extend_from_slice(b"\r\n\x1a\n\x00\x00\x00\x01");
    prefix.extend_from_slice(&(version.len() as u32).to_le_bytes());
    prefix.extend_from_slice(version.as_bytes());
    let pos = haystack
        .windows(prefix.len())
        .position(|window| window == prefix.as_slice())?;
    let len_start = pos + prefix.len();
    let len_bytes: [u8; 4] = haystack.get(len_start..len_start + 4)?.try_into().ok()?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    let start = len_start + 4;
    let wheel = haystack.get(start..start.checked_add(len)?)?;
    wheel.starts_with(b"PK\x03\x04").then(|| wheel.to_vec())
}

fn wheel_from_archive(archive: &[u8], version: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decompressed = Vec::new();
    if flate2::read::GzDecoder::new(archive)
        .take(64 * 1024 * 1024)
        .read_to_end(&mut decompressed)
        .is_ok()
        && let Some(wheel) = extract_wheel(&decompressed, version)
    {
        return Some(wheel);
    }
    extract_wheel(archive, version)
}

fn updater_proxy(app: &AppHandle) -> Option<tauri::Url> {
    if let Some(dirs) = service_dirs(app)
        && let Some(url) = proxy_url(&dirs.network)
    {
        return Some(url);
    }
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    proxy_url(&env)
}

fn proxy_url(env: &std::collections::BTreeMap<String, String>) -> Option<tauri::Url> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Some(raw) = env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
            && let Ok(url) = tauri::Url::parse(raw)
        {
            return Some(url);
        }
    }
    None
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
        state.started = Some(Instant::now());
    }
    let app = app.clone();
    thread::spawn(move || finish_upgrade(&app));
    json!({"ok": true, "message": "正在安装本机服务"})
}

fn run(uv: &Path, args: &[&str], dirs: &ToolDirs) -> Result<(), String> {
    let mut command = Command::new(uv);
    command
        .args(args)
        .envs(&dirs.network)
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
    let output = output_within(command, SERVICE_TIMEOUT)?;
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

/// `Command::output` bounded by `limit`, stopping the process tree before returning.
fn output_within(mut command: Command, limit: Duration) -> Result<std::process::Output, String> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let id = child.id();
    let (done, finished) = std::sync::mpsc::channel();
    let waiter = thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = done.send(());
        output
    });
    if finished.recv_timeout(limit).is_err() {
        kill(id);
        let _ = waiter.join();
        return Err(format!(
            "超过 {} 分钟没有完成，请检查网络后重试",
            limit.as_secs() / 60
        ));
    }
    waiter
        .join()
        .map_err(|_| "uv 进程读取失败".to_owned())?
        .map_err(|error| error.to_string())
}

fn kill(id: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(id as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &id.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
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
    // The winget package links uv here. A desktop app started before that PATH entry existed
    // inherits a PATH without it, so the install step would claim uv is missing.
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        candidates.push(local.join("Microsoft/WinGet/Links").join(name));
        candidates.push(local.join("Programs/uv").join(name));
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
    // A versioned install is the truth once one exists; the shared uv tool directory is only
    // the legacy fallback.
    if let Some(version) = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|data| active_version(&data))
    {
        return outdated_note(version);
    }
    service(&service_dirs(app).map(|dirs| dirs.tools))
}

/// The upgrade note for an installed version that is older than this app.
fn outdated_note(installed: String) -> Value {
    if parse(&installed) >= parse(VERSION) {
        return Value::Null;
    }
    let command = format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"");
    json!({"state": "outdated", "version": installed, "error": null, "command": command})
}

/// The launcher the hosts' configs point at once a versioned install exists.
pub(crate) fn active_launcher(app_data: &Path) -> Option<PathBuf> {
    active_entry(app_data).map(|(_, bin)| bin)
}

/// The active version and its launcher, only when both the launcher and the tool environment
/// behind it are still there. A half-removed install must not read as "installed".
fn active_entry(app_data: &Path) -> Option<(String, PathBuf)> {
    let raw = std::fs::read_to_string(app_data.join("service").join("active.json")).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let version = value["version"].as_str()?.to_owned();
    let bin = PathBuf::from(value["bin"].as_str()?);
    let env = app_data
        .join("service")
        .join(&version)
        .join("tools")
        .join("agentnave");
    (bin.is_file() && installed_version(&env).is_some()).then_some((version, bin))
}

/// The version the active manifest records, when its entry still exists.
/// The active version's tool environment, when the versioned install is complete. Callers that
/// report a service version must read this, not the legacy shared directory.
pub(crate) fn active_tools(app_data: &Path) -> Option<PathBuf> {
    active_entry(app_data).map(|(version, _)| {
        app_data
            .join("service")
            .join(version)
            .join("tools")
            .join("agentnave")
    })
}

pub(crate) fn active_version(app_data: &Path) -> Option<String> {
    active_entry(app_data).map(|(version, _)| version)
}

/// This version's own tool and bin directories, so an upgrade never touches the files the
/// running service executes (Windows refuses to remove them; POSIX would unlink them under it).
pub(crate) fn version_dirs(app: &AppHandle, dirs: &ToolDirs, version: &str) -> Option<ToolDirs> {
    let root = app
        .path()
        .app_data_dir()
        .ok()?
        .join("service")
        .join(version);
    Some(ToolDirs {
        home: dirs.home.clone(),
        tools: root.join("tools"),
        bin: root.join("bin"),
        network: dirs.network.clone(),
    })
}

/// Point the hosts at this install once it is complete.
pub(crate) fn write_active(app: &AppHandle, version: &str, dirs: &ToolDirs) -> Result<(), String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("service");
    std::fs::create_dir_all(&base).map_err(|error| error.to_string())?;
    let name = if cfg!(windows) {
        "agentnave-mcp.exe"
    } else {
        "agentnave-mcp"
    };
    let launcher = dirs.bin.join(name);
    let value = json!({"version": version, "bin": launcher.to_string_lossy()});
    write_active_manifest(&base, &value)
}

fn write_active_manifest(base: &Path, value: &Value) -> Result<(), String> {
    // Replace in one step: a truncated manifest would leave the hosts pointing at nothing. The
    // staging name is unique per process, and the swap is retried: on Windows a scanner holding
    // the old file for a moment rejects the replace, which must not fail a finished install.
    let staging = base.join(format!("active.{}.tmp", std::process::id()));
    std::fs::write(&staging, value.to_string()).map_err(|error| error.to_string())?;
    let target = base.join("active.json");
    let mut last = None;
    for attempt in 0..3 {
        match std::fs::rename(&staging, &target) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&staging);
    Err(last.map_or_else(|| "无法写入 active.json".into(), |error| error.to_string()))
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
mod tests;
