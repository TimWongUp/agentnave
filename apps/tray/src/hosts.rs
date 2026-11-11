//! App-owned host integration: scoped optimistic edits, no host CLI writes.
mod actions;
mod request;
pub(crate) use request::{orchestration_request, request, skill_request, sync_orchestration};
mod config;
mod installed;
#[cfg(all(test, windows, feature = "windows-live-tests"))]
mod live_tests;
mod orchestration;
mod processes;
mod refresh;
mod registry;
mod reload;
mod reopen;
mod retention;
mod roots;
mod skill;
mod snapshot;
mod split;
#[cfg(test)]
mod tests;
mod timeouts;
mod transaction;

use crate::detect::executable;
use crate::tool_dirs::ToolDirs;
use config::{Document, Format};
use orchestration::{host_env, set_host_env};
use roots::Root;
pub(crate) use roots::Shell;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Format of stored reload records; v4 could confirm another row's connection.
const RELOAD_FORMAT: u64 = 5;
use crate::config_paths::{error_parts, message, safe_path};
fn effective_env<'a>(
    context: &'a Context,
    shell: Option<&'a Shell>,
    var: &str,
) -> Option<(&'a str, &'static str)> {
    roots::env_value(&context.env, var)
        .filter(|p| Path::new(p).is_absolute())
        .map(|v| (v.as_str(), "env"))
        .or_else(|| {
            shell
                .and_then(|s| s.as_ref().ok())
                .and_then(|s| s.get(var))
                .map(|v| (v.as_str(), "shell"))
        })
}
pub(crate) fn probe_tools(context: &Context) -> (Shell, ToolDirs) {
    let shell = roots::probe(context);
    let dirs = ToolDirs::resolve(context, &shell);
    (shell, dirs)
}
impl ToolDirs {
    fn resolve(context: &Context, shell: &Shell) -> Self {
        let value = |var| effective_env(context, Some(shell), var);
        let tools = value("UV_TOOL_DIR")
            .map(|(p, _)| PathBuf::from(p))
            .or_else(|| {
                // On Windows uv ignores XDG and only keeps using `uv\data` where it already exists.
                if cfg!(windows) {
                    return context.env.get("APPDATA").map(|p| {
                        let uv = PathBuf::from(p).join("uv");
                        let legacy = uv.join("data");
                        if legacy.exists() { legacy } else { uv }.join("tools")
                    });
                }
                value("XDG_DATA_HOME").map(|(p, _)| PathBuf::from(p).join("uv/tools"))
            })
            .unwrap_or_else(|| context.home.join(".local/share/uv/tools"));
        let bin = value("UV_TOOL_BIN_DIR")
            .or_else(|| value("XDG_BIN_HOME"))
            .map(|(p, _)| PathBuf::from(p))
            .or_else(|| {
                value("XDG_DATA_HOME").map(|(p, _)| {
                    let data = Path::new(p);
                    data.parent().unwrap_or(data).join("bin")
                })
            })
            .unwrap_or_else(|| context.home.join(".local/bin"));
        let mut network = BTreeMap::new();
        for &var in roots::NETWORK_VARIABLES {
            let found = roots::env_value(&context.env, var)
                .filter(|v| !v.is_empty() && !v.contains('\n'))
                .or_else(|| {
                    shell
                        .as_ref()
                        .ok()
                        .and_then(|s| s.get(var))
                        .filter(|v| !v.is_empty() && !v.contains('\n'))
                });
            if let Some(val) = found {
                network.insert(var.into(), val.clone());
            }
        }
        Self {
            home: context.home.clone(),
            tools,
            bin,
            network,
        }
    }
}
pub struct Context {
    pub home: PathBuf,
    pub app: PathBuf,
    pub project: PathBuf,
    pub env: BTreeMap<String, String>,
    pub search: Vec<PathBuf>,
    pub applications: Vec<PathBuf>,
}

/// Where Hermes keeps `config.yaml`. Its Windows installer uses `%LOCALAPPDATA%\hermes` unless
/// `HERMES_HOME` says otherwise; a redirected home keeps its own `.hermes`, so a caller that
/// points at another profile never reads or writes this machine's real configuration.
fn hermes_default(home: &Path) -> PathBuf {
    #[cfg(windows)]
    if dirs::home_dir().as_deref() == Some(home) {
        let installed = dirs::data_local_dir().map(|local| local.join("hermes"));
        if let Some(dir) = installed.filter(|dir| dir.is_dir()) {
            return dir;
        }
    }
    home.join(".hermes")
}
/// Where an installed desktop app lives: `/Applications` and `~/Applications` on macOS, the
/// per-user program directory and the shared ones on Windows.
#[cfg(target_os = "macos")]
fn applications(home: &Path) -> Vec<PathBuf> {
    vec![PathBuf::from("/Applications"), home.join("Applications")]
}
#[cfg(windows)]
fn applications(_home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        dirs.push(local.join("Programs"));
    }
    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(path) = std::env::var_os(name) {
            dirs.push(PathBuf::from(path));
        }
    }
    dirs
}
#[cfg(not(any(target_os = "macos", windows)))]
fn applications(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

impl Context {
    pub fn live(app: PathBuf) -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let mut search: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        search.extend([
            home.join(".local/bin"),
            home.join(".grok/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".bun/bin"),
            home.join(".npm-global/bin"),
            home.join(".volta/bin"),
            home.join("AppData/Local/devin/cli/bin"),
        ]);
        Self {
            applications: applications(&home),
            project: std::env::current_dir().unwrap_or_else(|_| home.clone()),
            home,
            app,
            env,
            search,
        }
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        let names = if cfg!(windows) {
            vec![
                format!("{name}.exe"),
                format!("{name}.cmd"),
                format!("{name}.bat"),
            ]
        } else {
            vec![name.into()]
        };
        let bundled = if name == "codex" {
            self.applications
                .iter()
                .flat_map(|base| {
                    [
                        base.join("Codex.app/Contents/Resources/codex"),
                        base.join("ChatGPT.app/Contents/Resources/codex-cli/bin/codex"),
                    ]
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        bundled
            .into_iter()
            .chain(
                self.search
                    .iter()
                    .flat_map(|dir| names.iter().map(move |name| dir.join(name))),
            )
            .find(|p| executable(p))
    }
}

/// The optional Skills the 技能 page offers, with each SKILL.md.
pub fn skill_catalog() -> Value {
    skill::catalog()
}

#[derive(Clone)]
struct Host {
    home: PathBuf,
    id: &'static str,
    /// The host its connections run as: `id`, except for a row of `split`.
    boss: &'static str,
    label: &'static str,
    binary: &'static str,
    root: Root,
    config: PathBuf,
    skill: Option<PathBuf>,
    format: Format,
    key: &'static str,
    user_layers: Vec<PathBuf>,
}
impl Host {
    /// Optional Skills sit beside the manager Skill in the same skills folder.
    fn optional(&self, name: &str) -> Option<PathBuf> {
        Some(self.skill.as_ref()?.parent()?.join(name))
    }
    fn read(&self) -> Result<Document, String> {
        self.read_at(&self.config)
    }
    fn read_at(&self, path: &Path) -> Result<Document, String> {
        safe_path(path, &self.home)?;
        let raw = match fs::read_to_string(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return Err("无法读取宿主配置".into()),
        };
        Document::parse(&raw, self.format, self.key)
    }
    fn command<'a>(&self, entry: &'a Value) -> Option<&'a str> {
        entry["command"].as_str()
    }
    fn set_command(&self, entry: &mut Value, path: &Path) {
        entry["command"] = json!(path);
    }
}
type Spec = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    PathBuf,
    &'static str,
    Format,
    &'static str,
);
const IDS: [&str; 11] = [
    "codex",
    "claude",
    "codebuddy",
    "workbuddy",
    "grok",
    "antigravity",
    "pi",
    "deepseek",
    "devin",
    "hermes",
    "zcode",
];

pub struct Hosts {
    pub context: Context,
    settings: Value,
    settings_error: Option<String>,
    shell: Option<Shell>,
    tools: Arc<Mutex<Option<ToolDirs>>>,
    actions: HashMap<String, Value>,
    failures: HashMap<String, Value>,
    failed: HashMap<String, String>,
    manual_roots: HashSet<String>,
    codex_homes: Vec<PathBuf>,
    detected_at: String,
    cached: Value,
    processes: Mutex<processes::Probe>,
}
impl Hosts {
    #[cfg(test)]
    pub fn new(context: Context) -> Self {
        Self::from_probe(context, None, Arc::default())
    }
    pub(crate) fn from_probe(
        context: Context,
        shell: Option<Shell>,
        tools: Arc<Mutex<Option<ToolDirs>>>,
    ) -> Self {
        let loaded = roots::load(&context);
        let settings_error = loaded.as_ref().err().cloned();
        let mut hosts = Self {
            context,
            settings: loaded.unwrap_or_else(|_| json!({})),
            settings_error,
            shell,
            tools,
            actions: HashMap::new(),
            failures: HashMap::new(),
            failed: HashMap::new(),
            manual_roots: HashSet::new(),
            codex_homes: Vec::new(),
            detected_at: timestamp(),
            cached: Value::Null,
            processes: Mutex::default(),
        };
        hosts.codex_homes = hosts.other_codex_homes();
        hosts.cached = hosts.snapshot();
        hosts
    }
    fn other_codex_homes(&self) -> Vec<PathBuf> {
        let main = self.registry().into_iter().find(|host| host.id == "codex");
        main.map_or_else(Vec::new, |host| {
            split::codex_homes(&self.context.home, &host.root.path)
        })
    }
    fn launcher(&self) -> Option<PathBuf> {
        // POSIX hosts use the fixed exec script; Windows keeps the versioned executable.
        if let Some(path) = crate::update::active_launcher(&self.context.app) {
            return Some(path);
        }
        let name = "agentnave-mcp";
        let tools = self.tool_dirs()?.tools;
        if !crate::update::unified_service(&tools) {
            return None;
        }
        #[cfg(windows)]
        let candidate = tools.join("agentnave/Scripts").join(format!("{name}.exe"));
        #[cfg(not(windows))]
        let candidate = tools.join("agentnave/bin").join(name);
        executable(&candidate).then_some(candidate)
    }
    fn own_launcher(&self, command: &str) -> bool {
        let path = PathBuf::from(command.replace('\\', "/"));
        let name = if cfg!(windows) {
            "agentnave-mcp.exe"
        } else {
            "agentnave-mcp"
        };
        let service = self.context.app.join("service");
        if same_path(command, &service.join("bin").join(name)) {
            return true;
        }
        if let Some(version) = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            && same_path(command, &service.join(version).join("bin").join(name))
        {
            return true;
        }
        self.tool_dirs().is_some_and(|dirs| {
            let bin = if cfg!(windows) { "Scripts" } else { "bin" };
            same_path(command, &dirs.tools.join("agentnave").join(bin).join(name))
        })
    }
    /// Reuse the last probe, including a failed probe's explicit/default fallback. Before the
    /// first probe there is no confirmed snapshot: installation must not guess another root.
    pub(crate) fn tool_dirs(&self) -> Option<ToolDirs> {
        self.shell
            .as_ref()
            .map(|shell| ToolDirs::resolve(&self.context, shell))
    }
    /// What a connected host's entry points at, when a versioned install expects another
    /// launcher. Rewriting someone else's hand-edited entry is worse than saying so.
    fn launcher_note(&self, host: &Host) -> Value {
        let Some(expected) = crate::update::active_launcher(&self.context.app) else {
            return Value::Null;
        };
        let current = host.user_layers.iter().find_map(|path| {
            let document = host.read_at(path).ok()?;
            host.command(&document.entry?).map(str::to_owned)
        });
        let Some(current) = current else {
            return Value::Null;
        };
        let stale = !same_path(&current, &expected);
        json!({"stale":stale,"expected":expected.to_string_lossy(),"current":current})
    }
    /// Where this machine's Claude and Codex keep their sessions, every row's folder included.
    pub(crate) fn boss_homes(&self) -> crate::data::boss_app::Homes {
        let mut homes = crate::data::boss_app::Homes::desktop_data();
        for host in self.registry() {
            match host.boss {
                "claude" => homes.claude.push(host.root.path),
                "codex" => homes.codex.push(host.root.path),
                _ => {}
            }
        }
        homes
    }
    /// The boss apps installed here with their resolved roots, for the global rules list.
    pub fn rule_apps(&self) -> Vec<crate::rules::App> {
        self.registry()
            .into_iter()
            .filter(|host| self.detect(host).0)
            .map(|host| crate::rules::App {
                key: host.id,
                id: host.boss,
                label: host.label,
                root: host.root.path,
            })
            .collect()
    }
    /// The installed boss apps that are connected, which alone get the orchestration block.
    pub fn connected_rule_apps(&self) -> Vec<crate::rules::App> {
        self.registry()
            .into_iter()
            .filter(|host| self.state(host)["status"] == "connected")
            .map(|host| crate::rules::App {
                key: host.id,
                id: host.boss,
                label: host.label,
                root: host.root.path,
            })
            .collect()
    }
    /// Write the orchestration block into the connected apps' rules and take it out of the rest,
    /// backing up each file first. Returns the files that could not be written.
    pub fn sync_rules(&self, document: &Value, also: &[PathBuf]) -> Vec<Value> {
        let (app, home) = (&self.context.app, &self.context.home);
        // Named by the folder under home, so `AGENTS.md` copies of different apps stay apart.
        let backup = |path: &Path, bytes: &[u8]| {
            let folder = path.parent().and_then(|dir| dir.strip_prefix(home).ok());
            let folder = folder.map(|dir| dir.to_string_lossy().replace(['/', '\\'], "_"));
            let id = folder.as_deref().unwrap_or("rules").trim_start_matches('.');
            backup_bytes(app, RULES_BACKUP, id, path, bytes, home)
                .map_err(|error| message(&error).to_owned())
        };
        crate::rules::sync(
            &self.rule_apps(),
            &self.connected_rule_apps(),
            home,
            document,
            also,
            &backup,
            self.service_behind(),
        )
    }
    /// Whether the service is older than this App. The block tells bosses to use tool arguments
    /// of this App's release, which an older service rejects, so it waits for the upgrade; the
    /// upgrade refreshes the hosts, and that syncs the rules again.
    fn service_behind(&self) -> bool {
        let env = crate::update::active_tools(&self.context.app)
            .or_else(|| self.tool_dirs().map(|dirs| dirs.tools.join("agentnave")));
        env.as_deref()
            .and_then(crate::update::installed_version)
            .is_some_and(|installed| {
                crate::update::parse(&installed) < crate::update::parse(VERSION)
            })
    }
    /// The rules files the installed apps could read now, kept across a root change so the
    /// files left behind lose their block too.
    pub fn rule_paths(&self) -> Vec<PathBuf> {
        crate::rules::existing(&self.rule_apps(), &self.context.home)
    }
}
fn attention(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message})
}
/// Hosts that ask the user to trust an MCP entry again after its config changes.
fn trust_hint(id: &str) -> &'static str {
    match id {
        "workbuddy" => {
            "；WorkBuddy 会把改动后的条目停用，请到“自定义连接器 › MCP 服务管理”点“信任”"
        }
        _ => "",
    }
}
fn reload_hint(id: &str) -> &'static str {
    match id.split('@').next().unwrap_or(id) {
        "codex" => "重启 Codex（桌面版需完整退出）",
        "claude" => "重启 Claude Code",
        "claude-desktop" => "重启 Claude 桌面",
        "codebuddy" => "重启 CodeBuddy Code",
        "workbuddy" => "完整退出并重启 WorkBuddy",
        "grok" => "重启 Grok CLI",
        "antigravity" => "重启 Antigravity（桌面版需完整退出）",
        "pi" => "重启 Pi",
        "deepseek" => "重启 DeepSeek Harness（桌面版需完整退出）",
        "devin" => "重启 Devin CLI 或桌面端",
        "hermes" => "重启 Hermes 网关",
        "zcode" => "在 ZCode 新建会话或重启 ZCode",
        _ => "重启宿主应用",
    }
}
/// ZCode falls back to `~/.agents/mcp.json` only while its own user config names no MCP server,
/// so a first entry there would silently hide every other server kept in the fallback. An
/// `agentnave` server there is the one being connected, so it alone does not block.
fn zcode_fallback(home: &Path, before: &Document) -> Option<PathBuf> {
    if before.rest["mcp"]["servers"]
        .as_object()
        .is_some_and(|servers| !servers.is_empty())
    {
        return None;
    }
    let path = home.join(".agents/mcp.json");
    let fallback =
        Document::parse(&fs::read_to_string(&path).ok()?, Format::Json, "mcpServers").ok()?;
    fallback.rest["mcpServers"]
        .as_object()
        .is_some_and(|servers| !servers.is_empty())
        .then_some(path)
}
/// Compare two launcher paths the way the platform does: Windows paths differ in case and
/// separator spelling but name the same file, so a case-only mismatch must not read as "stale".
fn same_path(left: &str, right: &Path) -> bool {
    let normalize = |value: &str| {
        let value = value.replace('\\', "/");
        let value = value.strip_prefix("//?/").unwrap_or(&value).to_owned();
        let value = if cfg!(windows) {
            value.to_lowercase()
        } else {
            value
        };
        // `\\?\UNC\srv\share` names the same place as `\\srv\share`; lower-casing comes first so
        // the prefix matches whatever case the caller used.
        value
            .strip_prefix("unc/")
            .map_or(value.clone(), |rest| format!("//{rest}"))
    };
    normalize(left) == normalize(&right.to_string_lossy())
}
fn managed_entry(host: &Host, entry: &Value) -> bool {
    host.command(entry)
        .is_some_and(|command| entry_kind(command) == "agentnave")
}
fn error_attention(error: &str, fallback: &str) -> Value {
    let (code, text) = error_parts(error, fallback);
    attention(code, text)
}
/// Apply only the requested delta to a freshly read entry, preserving concurrent unknown fields.
fn patch(latest: &mut Value, before: Option<&Value>, desired: &Value) {
    if before == Some(desired) {
        return;
    }
    if let Some(wanted) = desired.as_object() {
        if !latest.is_object() {
            *latest = json!({});
        }
        if let Some(old) = before.and_then(Value::as_object) {
            for key in old.keys() {
                if !wanted.contains_key(key) {
                    latest.as_object_mut().unwrap().remove(key);
                }
            }
        }
        for (key, value) in wanted {
            let old = before.and_then(|v| v.get(key));
            if old != Some(value) {
                if latest.get(key).is_none() {
                    latest[key] = Value::Null;
                }
                patch(&mut latest[key], old, value);
            }
        }
    } else if let (Some(old), Some(wanted), Some(current)) = (
        before.and_then(Value::as_array),
        desired.as_array(),
        latest.as_array_mut(),
    ) {
        if old.len() == wanted.len() && current.len() >= wanted.len() {
            for (i, value) in wanted.iter().enumerate() {
                if old[i] != *value {
                    current[i] = value.clone();
                }
            }
        } else {
            *latest = desired.clone();
        }
    } else {
        *latest = desired.clone();
    }
}
fn entry_kind(command: &str) -> &'static str {
    let name = Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim_end_matches(".exe");
    match name {
        "agentnave-mcp" => "agentnave",
        _ => "other",
    }
}
fn timestamp() -> String {
    crate::update::rfc3339(SystemTime::now())
}
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("配置路径无父目录")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建配置目录")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法创建临时配置")?;
    if let Ok(meta) = fs::metadata(path) {
        tmp.as_file()
            .set_permissions(meta.permissions())
            .map_err(|_| "无法保留文件权限")?;
    }
    tmp.write_all(bytes)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|_| "无法写入临时配置")?;
    tmp.persist(path).map_err(|_| "无法原子替换配置")?;
    Ok(())
}
/// Host config, Skill and global rules backups are kept apart, so one kind never evicts another.
/// Files written before the split carry no kind and count as config backups.
const CONFIG_BACKUP: &str = "config";
const SKILL_BACKUP: &str = "skill";
const RULES_BACKUP: &str = "rules";
const KEEP_BACKUPS: usize = 30;
fn backup_paths(app: &Path, kind: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(app.join("backups"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && !p.is_symlink())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let rest = name.get(21..).unwrap_or_default();
            let own = [SKILL_BACKUP, RULES_BACKUP]
                .into_iter()
                .find(|other| rest.starts_with(&format!("{other}-")))
                .unwrap_or(CONFIG_BACKUP);
            own == kind
        })
        .collect();
    paths.sort();
    paths
}
/// Backs up one Skill file; host config writes go through `backup_bytes` directly.
fn backup(app: &Path, id: &str, path: &Path, home: &Path) -> Result<(), String> {
    safe_path(path, home)?;
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(path).map_err(|_| "write_failed:无法读取备份源")?;
    backup_bytes(app, SKILL_BACKUP, id, path, &bytes, home)
}
fn backup_bytes(
    app: &Path,
    kind: &str,
    id: &str,
    path: &Path,
    bytes: &[u8],
    home: &Path,
) -> Result<(), String> {
    safe_path(path, home)?;
    let dir = app.join("backups");
    safe_path(&dir, home)?;
    fs::create_dir_all(&dir).map_err(|_| "无法创建备份目录")?;
    let id = id.replace(['/', '\\', ':'], "_");
    let prefix = format!(
        "{:020}-{kind}-{id}-{}-",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut file = tempfile::Builder::new()
        .prefix(&prefix)
        .tempfile_in(&dir)
        .map_err(|_| "无法创建备份")?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "无法保存备份")?;
    file.keep().map_err(|_| "无法保留备份")?;
    let paths = backup_paths(app, kind);
    for old in paths.iter().take(paths.len().saturating_sub(KEEP_BACKUPS)) {
        fs::remove_file(old).map_err(|_| "无法清理旧备份")?;
    }
    Ok(())
}
