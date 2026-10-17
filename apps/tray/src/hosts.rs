//! App-owned host integration: scoped optimistic edits, no host CLI writes.
mod config;
mod roots;
mod skill;
#[cfg(test)]
mod tests;
mod transaction;
use config::{Document, Format};
use roots::{Root, Shell};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use transaction::safe_path;
const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) use transaction::safe_path as check_path;
pub struct Context {
    pub home: PathBuf,
    pub app: PathBuf,
    pub project: PathBuf,
    pub env: BTreeMap<String, String>,
    pub search: Vec<PathBuf>,
    pub applications: Vec<PathBuf>,
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
        ]);
        Self {
            applications: vec![PathBuf::from("/Applications"), home.join("Applications")],
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

#[derive(Clone)]
struct Host {
    home: PathBuf,
    id: &'static str,
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
    fn env_key(&self) -> &str {
        if self.id == "opencode" {
            "environment"
        } else {
            "env"
        }
    }
    fn command<'a>(&self, entry: &'a Value) -> Option<&'a str> {
        if self.id == "opencode" {
            entry["command"][0].as_str()
        } else {
            entry["command"].as_str()
        }
    }
    fn set_command(&self, entry: &mut Value, path: &Path) {
        if self.id == "opencode" {
            if entry["command"].as_array().is_none_or(|a| a.is_empty()) {
                entry["command"] = json!([path]);
            } else {
                entry["command"][0] = json!(path);
            }
        } else {
            entry["command"] = json!(path);
        }
    }
}
const IDS: [&str; 10] = [
    "codex",
    "claude",
    "codebuddy",
    "workbuddy",
    "grok",
    "antigravity",
    "pi",
    "gemini",
    "opencode",
    "hermes",
];

pub struct Hosts {
    pub context: Context,
    settings: Value,
    settings_error: Option<String>,
    shell: Option<Shell>,
    actions: HashMap<String, Value>,
    failures: HashMap<String, Value>,
    failed: HashMap<String, String>,
    manual_roots: HashSet<String>,
    detected_at: String,
    cached: Value,
}
impl Hosts {
    pub fn new(context: Context) -> Self {
        let loaded = roots::load(&context);
        let settings_error = loaded.as_ref().err().cloned();
        let mut hosts = Self {
            context,
            settings: loaded.unwrap_or_else(|_| json!({})),
            settings_error,
            shell: None,
            actions: HashMap::new(),
            failures: HashMap::new(),
            failed: HashMap::new(),
            manual_roots: HashSet::new(),
            detected_at: timestamp(),
            cached: Value::Null,
        };
        hosts.cached = hosts.snapshot();
        hosts
    }
    fn env_value(&self, var: &str) -> Option<(&str, &'static str)> {
        self.context
            .env
            .get(var)
            .filter(|p| Path::new(p).is_absolute())
            .map(|v| (v.as_str(), "env"))
            .or_else(|| {
                self.shell
                    .as_ref()
                    .and_then(|s| s.as_ref().ok())
                    .and_then(|s| s.get(var))
                    .map(|v| (v.as_str(), "shell"))
            })
    }
    fn launcher(&self, entry: &str) -> Option<PathBuf> {
        let name = if entry == "core" {
            "agentnave-mcp"
        } else {
            "agentnave-studio-mcp"
        };
        let tools = self
            .env_value("UV_TOOL_DIR")
            .map(|(p, _)| PathBuf::from(p))
            .or_else(|| {
                self.env_value("XDG_DATA_HOME")
                    .map(|(p, _)| PathBuf::from(p).join("uv/tools"))
            })
            .unwrap_or_else(|| self.context.home.join(".local/share/uv/tools"));
        #[cfg(windows)]
        let candidate = tools.join("agentnave/Scripts").join(format!("{name}.exe"));
        #[cfg(not(windows))]
        let candidate = tools.join("agentnave/bin").join(name);
        executable(&candidate).then_some(candidate)
    }
    fn registry(&self) -> Vec<Host> {
        let h = &self.context.home;
        let xdg = self
            .env_value("XDG_CONFIG_HOME")
            .map(|(p, _)| PathBuf::from(p))
            .unwrap_or_else(|| h.join(".config"));
        let specs = [
            (
                "codex",
                "Codex",
                "codex",
                Some("CODEX_HOME"),
                h.join(".codex"),
                "config.toml",
                Format::Toml,
                "mcp_servers",
            ),
            (
                "claude",
                "Claude Code",
                "claude",
                Some("CLAUDE_CONFIG_DIR"),
                h.join(".claude"),
                ".claude.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "codebuddy",
                "CodeBuddy Code",
                "codebuddy",
                Some("CODEBUDDY_CONFIG_DIR"),
                h.join(".codebuddy"),
                ".mcp.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "workbuddy",
                "WorkBuddy 桌面",
                "workbuddy",
                Some("CODEBUDDY_CONFIG_DIR"),
                h.join(".workbuddy"),
                "mcp.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "grok",
                "Grok CLI",
                "grok",
                Some("GROK_HOME"),
                h.join(".grok"),
                "config.toml",
                Format::Toml,
                "mcp_servers",
            ),
            (
                "antigravity",
                "Antigravity",
                "agy",
                None,
                h.join(".gemini/config"),
                "mcp_config.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "pi",
                "Pi",
                "pi",
                Some("PI_CODING_AGENT_DIR"),
                h.join(".pi/agent"),
                "mcp.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "gemini",
                "Gemini CLI",
                "gemini",
                None,
                h.join(".gemini"),
                "settings.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "opencode",
                "OpenCode",
                "opencode",
                Some("OPENCODE_CONFIG_DIR"),
                xdg.join("opencode"),
                "opencode.json",
                Format::Json,
                "mcp",
            ),
            (
                "hermes",
                "Hermes",
                "hermes",
                Some("HERMES_HOME"),
                h.join(".hermes"),
                "config.yaml",
                Format::Yaml,
                "mcp_servers",
            ),
        ];
        specs
            .into_iter()
            .map(|(id, label, binary, var, default, file, format, key)| {
                let mut root = roots::resolve(
                    &self.context,
                    &self.settings,
                    self.shell.as_ref(),
                    id,
                    var,
                    default,
                );
                if id == "opencode"
                    && root.source == "default"
                    && let Some((_, source)) = self.env_value("XDG_CONFIG_HOME")
                {
                    root.source = source;
                    root.confirmed = true;
                }
                if self.manual_roots.contains(id) {
                    root.confirmed = true;
                }
                let mut paths = vec![root.path.join(file)];
                if id == "claude" && root.source == "default" {
                    paths = vec![h.join(".claude.json")];
                }
                if id == "opencode" {
                    paths = config_candidates(&xdg.join("opencode"));
                    if let Some((path, _)) = self.env_value("OPENCODE_CONFIG") {
                        paths.push(path.into());
                    }
                    if let Some((path, _)) = self.env_value("OPENCODE_CONFIG_DIR") {
                        paths.extend(config_candidates(Path::new(path)));
                    }
                    if root.source == "user" {
                        paths.extend(config_candidates(&root.path));
                    }
                }
                let mut unique = HashSet::new();
                paths.retain(|p| unique.insert(p.clone()));
                let config = paths
                    .iter()
                    .rev()
                    .find(|p| {
                        if safe_path(p, &self.context.home).is_err() {
                            return p.exists() || p.is_symlink();
                        }
                        fs::read_to_string(p).is_ok_and(|raw| {
                            match Document::parse(&raw, format, key) {
                                Ok(d) => d.entry.is_some(),
                                Err(_) => true,
                            }
                        })
                    })
                    .unwrap_or_else(|| paths.last().unwrap())
                    .clone();
                Host {
                    home: self.context.home.clone(),
                    id,
                    label,
                    binary,
                    skill: Some(root.path.join("skills/agentnave-manager")),
                    root,
                    config,
                    format,
                    key,
                    user_layers: paths,
                }
            })
            .collect()
    }
    fn detect(&self, host: &Host) -> (bool, &'static str, Option<PathBuf>) {
        if self.context.find(host.binary).is_some() {
            return (true, "path", None);
        }
        let app = match host.id {
            "workbuddy" => Some("WorkBuddy.app"),
            "antigravity" => Some("Antigravity.app"),
            _ => None,
        };
        if let Some(path) = app.and_then(|name| {
            self.context
                .applications
                .iter()
                .map(|p| p.join(name))
                .find(|p| p.is_dir())
        }) {
            return (true, "app", Some(path));
        }
        (false, "config", None)
    }
    fn command_state(&self, host: &Host, entry: &Value) -> &'static str {
        let Some(command) = host.command(entry) else {
            return "missing";
        };
        if let Some(rest) = command.strip_prefix("~/") {
            return if executable(&self.context.home.join(rest)) {
                "present"
            } else {
                "missing"
            };
        }
        let path = Path::new(command);
        if path.is_absolute() {
            return match fs::metadata(path) {
                Ok(_) => {
                    if executable(path) {
                        "present"
                    } else {
                        "missing"
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
                Err(_) => "unknown",
            };
        }
        if path.components().count() != 1 {
            return "unknown";
        }
        let env = entry.get(host.env_key());
        let effective = if env.is_some_and(|e| e.get("PATH").is_some()) {
            env.and_then(|e| e["PATH"].as_str())
        } else {
            self.context.env.get("PATH").map(String::as_str)
        };
        let Some(effective) = effective else {
            return "unknown";
        };
        let mut unknown = false;
        for dir in std::env::split_paths(effective) {
            if !dir.is_absolute() || dir.to_string_lossy().contains('$') {
                unknown = true;
                continue;
            }
            if executable(&dir.join(command)) {
                return "present";
            }
        }
        if unknown { "unknown" } else { "missing" }
    }
    fn layers(&self, host: &Host) -> Vec<Value> {
        let mut layers:Vec<_>=host.user_layers.iter().enumerate().map(|(i,p)|json!({"path":p,"scope":if i==0{"user"}else{"override"},"has_entry":host.read_at(p).is_ok_and(|d|d.entry.is_some())})).collect();
        for dir in self.context.project.ancestors() {
            let relative: &[&str] = match host.id {
                "codex" => &[".codex/config.toml"],
                "claude" | "codebuddy" | "workbuddy" => &[".mcp.json"],
                "grok" => &[".grok/config.toml"],
                "pi" => &[".pi/mcp.json"],
                "gemini" => &[".gemini/settings.json"],
                "opencode" => &[
                    "opencode.json",
                    "opencode.jsonc",
                    ".opencode/opencode.json",
                    ".opencode/opencode.jsonc",
                ],
                _ => &[],
            };
            for relative in relative {
                let p = dir.join(relative);
                if !host.user_layers.contains(&p) && p.exists() {
                    layers.push(json!({"path":p,"scope":"project","has_entry":host.read_at(&p).is_ok_and(|d|d.entry.is_some())}));
                }
            }
            if dir.join(".git").exists() {
                break;
            }
        }
        if host.id == "claude"
            && safe_path(&host.config, &self.context.home).is_ok()
            && let Ok(raw) = fs::read_to_string(&host.config)
            && let Ok(value) = serde_json::from_str::<Value>(&raw)
            && value["projects"][self.context.project.to_string_lossy().as_ref()]["mcpServers"]
                .get("agentnave")
                .is_some()
        {
            layers.push(json!({"path":host.config,"scope":"project","has_entry":true}));
        }
        layers
    }
    fn state(&self, host: &Host) -> Value {
        let (installed, source, app_path) = self.detect(host);
        let document = host.read();
        let entry = document.as_ref().ok().and_then(|d| d.entry.as_ref());
        let command = entry.and_then(|e| host.command(e));
        let kind = command.map(entry_kind);
        let command_state = entry.map(|e| self.command_state(host, e));
        let has_env = entry.is_some_and(|e| e[host.env_key()]["AGENTNAVE_HOST"] == host.id);
        let layers = self.layers(host);
        let has_entry = entry.is_some()
            || layers
                .iter()
                .any(|l| l["has_entry"] == true && l["scope"] != "project");
        let unsafe_path = host
            .user_layers
            .iter()
            .find_map(|p| safe_path(p, &self.context.home).err());
        let attention = if let Some(error) = unsafe_path {
            Some(error_attention(&error, "write_failed"))
        } else if let Some(error) = &self.settings_error {
            Some(error_attention(error, "root_unconfirmed"))
        } else if !host.root.confirmed
            || (self.shell.as_ref().is_some_and(Result::is_err)
                && !self.manual_roots.contains(host.id))
        {
            Some(attention(
                "root_unconfirmed",
                "登录 shell 尚未确认根目录；可手动接入或在高级中指定根目录",
            ))
        } else if let Err(error) = &document {
            Some(error_attention(error, "parse_failed"))
        } else if let Some(failed) = self.failures.get(host.id) {
            Some(failed.clone())
        } else if command_state == Some("missing") {
            Some(attention("command_missing", "条目的命令不存在或不可执行"))
        } else {
            None
        };
        let has_config = host
            .user_layers
            .iter()
            .any(|p| p.exists() || p.is_symlink());
        let status = if attention.is_some() && (installed || has_entry || has_config) {
            "needs_attention"
        } else if has_entry && !installed {
            "orphaned"
        } else if has_entry {
            "connected"
        } else if installed {
            "available"
        } else {
            "absent"
        };
        let shadowed = layers
            .iter()
            .find(|l| l["scope"] == "project" && l["has_entry"] == true)
            .map(|l| l["path"].clone());
        json!({"id":host.id,"label":host.label,"installed":installed,"detect_source":source,"app_path":app_path,"status":status,
            "entry":kind,"command":command,"command_state":command_state,"has_host_env":has_env,"config_path":host.config,"root":{"path":host.root.path,"source":host.root.source},
            "layers":layers,"shadowed_by":shadowed,"tested":false,"skill":host.skill.as_ref().map(|p|skill::state(p,&self.context.home)).unwrap_or_else(||json!({"state":"manual","path":null,"version":null})),
            "attention":attention,"last_action":self.actions.get(host.id),"first_seen":self.settings[host.id]["first_seen"].as_str().unwrap_or(&self.detected_at)})
    }
    pub fn cached_snapshot(&self) -> &Value {
        &self.cached
    }
    pub fn snapshot(&self) -> Value {
        let hosts: Vec<_> = self
            .registry()
            .iter()
            .map(|h| self.state(h))
            .filter(|h| h["status"] != "absent")
            .collect();
        json!({"service":self.service(),"hosts":hosts,"backups":{"dir":self.context.app.join("backups"),"count":backup_paths(&self.context.app).len()}})
    }
    pub fn reveal_backups(&self) -> Result<Value, String> {
        let path = self.context.app.join("backups");
        safe_path(&path, &self.context.home)?;
        fs::create_dir_all(&path).map_err(|_| "无法创建备份目录")?;
        #[cfg(target_os = "macos")]
        let program = "/usr/bin/open";
        #[cfg(target_os = "windows")]
        let program = "explorer.exe";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let program = "xdg-open";
        std::process::Command::new(program)
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| "无法打开备份目录")?;
        Ok(json!({"ok":true,"message":"已打开备份目录"}))
    }
    pub fn service(&self) -> Value {
        let studio = self.launcher("studio");
        let core = self.launcher("core");
        json!({"installed":studio.is_some()&&core.is_some(),"studio":studio.is_some(),"core":core.is_some(),"command":crate::update::service_command()})
    }
    fn signature(&self, host: &Host, entry: &Option<Value>, kind: &str) -> String {
        serde_json::to_string(&(host.id, &host.config, entry, kind)).unwrap()
    }
    pub fn refresh(&mut self) -> Value {
        self.shell = Some(roots::probe(&self.context));
        let mut settings = self.settings.clone();
        for host in self.registry() {
            let state = self.state(&host);
            if state["status"] != "absent" && settings[host.id].get("first_seen").is_none() {
                if !settings[host.id].is_object() {
                    settings[host.id] = json!({});
                }
                settings[host.id]["first_seen"] = json!(&self.detected_at);
            }
            if !self.detect(&host).0
                || !host.root.confirmed
                || self.settings_error.is_some()
                || self.shell.as_ref().is_some_and(Result::is_err)
            {
                continue;
            }
            let Ok(before) = host.read() else {
                continue;
            };
            let signature = self.signature(&host, &before.entry, "takeover");
            if self.failed.get(host.id) == Some(&signature) {
                continue;
            }
            let result: Result<bool, String> = (|| {
                let changed = transaction::edit(
                    transaction::Target {
                        path: &host.config,
                        home: &self.context.home,
                        app: &self.context.app,
                        id: host.id,
                        format: host.format,
                        key: host.key,
                    },
                    |latest| {
                        let Some(desired) = latest.as_mut() else {
                            return Ok(());
                        };
                        if desired.get(host.env_key()).is_none() {
                            desired[host.env_key()] = json!({});
                        }
                        desired[host.env_key()]
                            .as_object_mut()
                            .ok_or("takeover_failed:env 必须为对象")?
                            .insert("AGENTNAVE_HOST".into(), json!(host.id));
                        if self.command_state(&host, desired) == "missing"
                            && let Some(command) = host.command(desired)
                            && matches!(entry_kind(command), "studio" | "core")
                            && let Some(path) = self.launcher(entry_kind(command))
                        {
                            host.set_command(desired, &path);
                        }
                        Ok(())
                    },
                )?;
                let skill_changed = if let Some(path) = &host.skill {
                    let state = skill::state(path, &self.context.home);
                    if state["state"] == "app" && state["version"] != VERSION {
                        skill::install(path, &self.context.app, host.id, &self.context.home)?;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                Ok(changed || skill_changed)
            })();
            match result {
                Ok(true) => self.record(host.id, Ok(()), "已接管条目或更新 Skill"),
                Ok(false) => {}
                Err(error) => {
                    self.failed.insert(host.id.into(), signature);
                    self.record(host.id, Err(error), "");
                }
            }
        }
        if settings != self.settings && self.settings_error.is_none() {
            match roots::save(&self.context, &settings) {
                Ok(()) => self.settings = settings,
                Err(e) => self.settings_error = Some(e),
            }
        }
        self.cached = self.snapshot();
        self.cached.clone()
    }
    pub fn action(&mut self, operation: &str, payload: &Value) -> Result<Value, String> {
        let id = payload["id"]
            .as_str()
            .filter(|id| IDS.contains(id))
            .ok_or("未知宿主 id")?;
        if operation == "root" {
            let path = payload.get("path").ok_or("缺少 path")?;
            if !path.is_null()
                && !path.as_str().is_some_and(|p| {
                    Path::new(p).is_absolute()
                        && !Path::new(p)
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                })
            {
                return Err("根目录必须为绝对路径或 null".into());
            }
            if self.settings_error.is_some() {
                return Err("请先修复 hosts.json".into());
            }
            let mut settings = self.settings.clone();
            if !settings[id].is_object() {
                settings[id] = json!({});
            }
            if path.is_null() {
                settings[id].as_object_mut().unwrap().remove("root");
            } else {
                settings[id]["root"] = path.clone();
            }
            roots::save(&self.context, &settings)?;
            self.settings = settings;
            self.manual_roots.remove(id);
            if !path.is_null() {
                self.manual_roots.insert(id.into());
            }
            self.failed.remove(id);
            self.failures.remove(id);
            self.record(id, Ok(()), "已更新配置根目录");
            let host = self.registry().into_iter().find(|h| h.id == id).unwrap();
            self.cached = self.snapshot();
            return Ok(json!({"host":self.state(&host),"ok":true,"message":"已更新配置根目录"}));
        }
        let host = self.registry().into_iter().find(|h| h.id == id).unwrap();
        let entry = payload
            .get("entry")
            .map(|v| v.as_str().ok_or("entry 必须为 studio 或 core"))
            .transpose()?
            .unwrap_or("studio");
        if operation != "remove" && !matches!(entry, "studio" | "core") {
            return Err("entry 必须为 studio 或 core".into());
        }
        let result: Result<String, String> = (|| {
            if operation == "remove" {
                // Preflight all layers; each subsequent replacement is independently checked.
                for path in &host.user_layers {
                    safe_path(path, &self.context.home)?;
                    host.read_at(path)
                        .map_err(|e| format!("parse_failed:{e}"))?;
                }
                let mut removed = Vec::new();
                for path in &host.user_layers {
                    let mut layer = host.clone();
                    layer.config = path.clone();
                    let before = layer.read().map_err(|e| format!("parse_failed:{e}"))?;
                    if self.write(&layer, &before, None)? {
                        removed.push(path.display().to_string());
                    }
                }
                if let Some(path) = &host.skill {
                    skill::remove(path, &self.context.app, id, &self.context.home)?;
                }
                return Ok(if removed.is_empty() {
                    "没有用户级条目需要移除".into()
                } else {
                    format!("已移除：{}", removed.join("、"))
                });
            }
            if !self.detect(&host).0 {
                return Err("write_failed:宿主未安装".into());
            }
            let before = host.read().map_err(|e| format!("parse_failed:{e}"))?;
            let path = self
                .launcher(entry)
                .ok_or("write_failed:请先安装本机服务")?;
            let mut desired = before.entry.clone();
            match operation {
                "connect" => {
                    if let Some(value) = desired.as_mut() {
                        host.set_command(value, &path);
                        if value.get(host.env_key()).is_none() {
                            value[host.env_key()] = json!({});
                        }
                        value[host.env_key()]
                            .as_object_mut()
                            .ok_or("write_failed:env 必须为对象")?
                            .insert("AGENTNAVE_HOST".into(), json!(id));
                    } else {
                        desired = Some(if id == "opencode" {
                            json!({"type":"local","command":[path],"environment":{"AGENTNAVE_HOST":id},"enabled":true})
                        } else if id == "hermes" {
                            json!({"command":path,"args":[],"env":{"AGENTNAVE_HOST":id},"enabled":true})
                        } else {
                            json!({"command":path,"args":[],"env":{"AGENTNAVE_HOST":id}})
                        });
                    }
                }
                "entry" => {
                    host.set_command(desired.as_mut().ok_or("write_failed:宿主尚未接入")?, &path)
                }
                _ => return Err("未知宿主操作".into()),
            }
            self.write(&host, &before, desired.as_ref())?;
            if operation == "connect"
                && let Some(path) = &host.skill
            {
                skill::install(path, &self.context.app, id, &self.context.home)?;
            }
            Ok(if operation == "entry" {
                "已切换入口".into()
            } else {
                "已接入；外部管理的 Skill 保留".into()
            })
        })();
        let ok = result.is_ok();
        let text = result
            .as_ref()
            .cloned()
            .unwrap_or_else(|e| message(e).into());
        self.failed.remove(id);
        self.record(id, result.map(|_| ()), &text);
        if ok {
            self.manual_roots.insert(id.into());
        }
        let current = self.registry().into_iter().find(|h| h.id == id).unwrap();
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(&current),"ok":ok,"message":text}))
    }
    fn record(&mut self, id: &str, result: Result<(), String>, success: &str) {
        let ok = result.is_ok();
        let text = result.err().unwrap_or_else(|| success.into());
        if ok {
            self.failures.remove(id);
        } else {
            self.failures
                .insert(id.into(), error_attention(&text, "takeover_failed"));
        }
        self.actions.insert(
            id.into(),
            json!({"ok":ok,"message":message(&text),"at":timestamp()}),
        );
    }
    fn write(
        &self,
        host: &Host,
        before: &Document,
        desired: Option<&Value>,
    ) -> Result<bool, String> {
        transaction::edit(
            transaction::Target {
                path: &host.config,
                home: &self.context.home,
                app: &self.context.app,
                id: host.id,
                format: host.format,
                key: host.key,
            },
            |latest| {
                if let Some(desired) = desired {
                    if latest.is_none() && before.entry.is_some() {
                        return Err("conflict:条目已被其他进程移除，请刷新".into());
                    }
                    if latest.is_none() {
                        *latest = Some(json!({}));
                    }
                    patch(latest.as_mut().unwrap(), before.entry.as_ref(), desired);
                } else {
                    *latest = None;
                }
                Ok(())
            },
        )
    }
}
fn attention(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message})
}
fn message(error: &str) -> &str {
    error_parts(error, "takeover_failed").1
}
fn error_attention(error: &str, fallback: &str) -> Value {
    let (code, text) = error_parts(error, fallback);
    attention(code, text)
}
fn error_parts<'a>(error: &'a str, fallback: &'a str) -> (&'a str, &'a str) {
    error
        .split_once(':')
        .filter(|(c, _)| {
            [
                "parse_failed",
                "command_missing",
                "symlink_path",
                "root_unconfirmed",
                "conflict",
                "write_failed",
                "rolled_back",
                "takeover_failed",
            ]
            .contains(c)
        })
        .unwrap_or((fallback, error))
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
fn config_candidates(dir: &Path) -> Vec<PathBuf> {
    vec![dir.join("opencode.json"), dir.join("opencode.jsonc")]
}
fn entry_kind(command: &str) -> &'static str {
    let name = Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim_end_matches(".exe");
    match name {
        "agentnave-studio-mcp" => "studio",
        "agentnave-mcp" => "core",
        _ => "other",
    }
}
fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
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
fn backup_paths(app: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(app.join("backups"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && !p.is_symlink())
        .collect();
    paths.sort();
    paths
}
fn backup(app: &Path, id: &str, path: &Path, home: &Path) -> Result<(), String> {
    safe_path(path, home)?;
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(path).map_err(|_| "write_failed:无法读取备份源")?;
    backup_bytes(app, id, path, &bytes, home)
}
fn backup_bytes(
    app: &Path,
    id: &str,
    path: &Path,
    bytes: &[u8],
    home: &Path,
) -> Result<(), String> {
    safe_path(path, home)?;
    let dir = app.join("backups");
    safe_path(&dir, home)?;
    fs::create_dir_all(&dir).map_err(|_| "无法创建备份目录")?;
    let prefix = format!(
        "{:020}-{id}-{}-",
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
    let paths = backup_paths(app);
    for old in paths.iter().take(paths.len().saturating_sub(30)) {
        fs::remove_file(old).map_err(|_| "无法清理旧备份")?;
    }
    Ok(())
}
