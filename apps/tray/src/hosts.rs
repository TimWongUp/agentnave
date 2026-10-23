//! App-owned host integration: scoped optimistic edits, no host CLI writes.
mod config;
mod roots;
mod skill;
#[cfg(test)]
mod tests;
mod transaction;
use config::{Document, Format};
use roots::Root;
pub(crate) use roots::Shell;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use transaction::safe_path;
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Format of the stored reload records; older records hashed whole Skill trees.
const RELOAD_FORMAT: u64 = 3;
/// Matches the relay's own report that an update has waited too long.
const STALE_RUNTIME_SECONDS: u64 = 600;
pub(crate) use transaction::safe_path as check_path;
#[derive(Clone)]
pub(crate) struct ToolDirs {
    pub home: PathBuf,
    pub tools: PathBuf,
    pub bin: PathBuf,
}
fn effective_env<'a>(
    context: &'a Context,
    shell: Option<&'a Shell>,
    var: &str,
) -> Option<(&'a str, &'static str)> {
    context
        .env
        .get(var)
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
            .or_else(|| value("XDG_DATA_HOME").map(|(p, _)| PathBuf::from(p).join("uv/tools")))
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
        Self {
            home: context.home.clone(),
            tools,
            bin,
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
    tools: Arc<Mutex<Option<ToolDirs>>>,
    actions: HashMap<String, Value>,
    failures: HashMap<String, Value>,
    failed: HashMap<String, String>,
    manual_roots: HashSet<String>,
    detected_at: String,
    cached: Value,
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
            detected_at: timestamp(),
            cached: Value::Null,
        };
        hosts.cached = hosts.snapshot();
        hosts
    }
    fn env_value(&self, var: &str) -> Option<(&str, &'static str)> {
        effective_env(&self.context, self.shell.as_ref(), var)
    }
    fn launcher(&self) -> Option<PathBuf> {
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
    /// Reuse the last probe, including a failed probe's explicit/default fallback. Before the
    /// first probe there is no confirmed snapshot: installation must not guess another root.
    pub(crate) fn tool_dirs(&self) -> Option<ToolDirs> {
        self.shell
            .as_ref()
            .map(|shell| ToolDirs::resolve(&self.context, shell))
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
        let skill = host
            .skill
            .as_ref()
            .map(|p| skill::state(p, &self.context.home))
            .unwrap_or_else(|| json!({"state":"manual","path":null,"version":null}));
        let reload = if !has_entry || !installed {
            json!({"state":"not_connected"})
        } else if let Some(token) = self.reload_token(host, &skill) {
            self.reload_state(host, &token)
        } else {
            json!({"state":"unknown"})
        };
        json!({"id":host.id,"label":host.label,"installed":installed,"detect_source":source,"app_path":app_path,"status":status,
            "entry":kind,"command":command,"command_state":command_state,"has_host_env":has_env,"config_path":host.config,"root":{"path":host.root.path,"source":host.root.source},
            "layers":layers,"shadowed_by":shadowed,"tested":matches!(host.id, "codex" | "claude" | "codebuddy" | "grok" | "antigravity" | "pi" | "hermes"),"skill":skill,"reload":reload,
            "reload_hint":activation_hint(host.id),"attention":attention,"last_action":self.actions.get(host.id),"first_seen":self.settings[host.id]["first_seen"].as_str().unwrap_or(&self.detected_at)})
    }
    /// Only actual configuration/Skill content matters; release numbers do not.
    fn reload_token(&self, host: &Host, _skill: &Value) -> Option<String> {
        let mut entries = Vec::new();
        for path in &host.user_layers {
            let document = host.read_at(path).ok()?;
            if let Some(entry) = document.entry {
                if !managed_entry(host, &entry) {
                    return None;
                }
                entries.push(json!({"path":path,"entry":entry}));
            }
        }
        if entries.is_empty() {
            return None;
        }
        let contents = self.skill_contents(host); // Unreadable/external Skills are not MCP proof.
        Some(skill::hash(
            &serde_json::to_vec(&json!({"entries":entries,"skill":contents})).ok()?,
        ))
    }
    /// What a host loads from the Skill when a session starts: the SKILL.md front matter. The
    /// body and references are read on use, so changing only them needs no reload.
    fn skill_contents(&self, host: &Host) -> Option<Value> {
        let Some(path) = &host.skill else {
            return Some(Value::Null);
        };
        safe_path(path, &self.context.home).ok()?;
        let file = path.join("SKILL.md");
        if !path.exists() || !file.exists() {
            return Some(Value::Null);
        }
        if file.is_symlink() || !file.is_file() {
            return None;
        }
        let text = fs::read_to_string(file).ok()?;
        Some(json!(skill::hash(skill::front_matter(&text).as_bytes())))
    }
    fn reload_state(&self, host: &Host, token: &str) -> Value {
        // Records from before the front-matter-only token (v3) describe another token format.
        let record = |name: &str| {
            let value = &self.settings[host.id][name];
            if value["v"] == RELOAD_FORMAT {
                value
            } else {
                &Value::Null
            }
        };
        let (confirmed, changed, observed) = (
            record("reload_confirmed"),
            record("reload_changed"),
            record("reload_observed"),
        );
        let manual = confirmed["token"] == token;
        let pending = !manual
            && ((changed["token"] == token)
                || (confirmed["token"].is_string() && confirmed["token"] != token)
                || (observed["token"].is_string() && observed["token"] != token));
        let root = self.connection_root(host);
        let now = SystemTime::now();
        let connections: Vec<Value> = root
            .into_iter()
            .flat_map(|root| {
                fs::read_dir(root.join("connections"))
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_symlink() || path.extension().is_none_or(|ext| ext != "json") {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                if meta.len() > 16_384
                    || now
                        .duration_since(meta.modified().ok()?)
                        .map_or(true, |age| age.as_secs() > 20)
                {
                    return None;
                }
                let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
                (value["version"] == 1 && value["host"] == host.id).then_some(value)
            })
            .collect();
        let env = self.tool_dirs().map(|dirs| dirs.tools.join("agentnave"));
        let installed = env.as_deref().and_then(crate::update::installed_version);
        // A relay swaps to a new install within seconds once idle; much later it is stuck.
        let install_settled = env
            .as_deref()
            .and_then(crate::update::installed_since)
            .and_then(|since| now.duration_since(since).ok())
            .is_some_and(|age| age.as_secs() > STALE_RUNTIME_SECONDS);
        let valid_hash = |v: &Value| {
            v.as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        let mut state = if pending {
            "required"
        } else if manual {
            "confirmed"
        } else {
            "unknown"
        };
        let mut reason = if pending { "config" } else { "no_connection" };
        if !connections.is_empty() {
            if connections.iter().any(|c| {
                valid_hash(&c["runtime_tools_hash"])
                    && valid_hash(&c["host_tools_hash"])
                    && c["runtime_tools_hash"] != c["host_tools_hash"]
            }) {
                state = "required";
                reason = "tools";
            } else if connections.iter().all(|c| {
                valid_hash(&c["runtime_tools_hash"])
                    && c["runtime_tools_hash"] == c["host_tools_hash"]
            }) {
                let newly_connected = changed["token"] == token
                    && changed["at_ms"].as_u64().is_some_and(|at| {
                        connections
                            .iter()
                            .all(|c| c["started_ms"].as_u64().is_some_and(|start| start > at))
                    });
                // Every live connection started after the change. Taken as a reload: a new relay
                // usually means a restarted host or new session, though a bare MCP reconnect in an
                // old chat keeps that chat's earlier Skill front matter (ADR-0022).
                if !pending || newly_connected {
                    reason = "connection";
                    let unreported = |c: &&Value| c["runtime_version"].as_str().is_none();
                    state = if connections
                        .iter()
                        .filter(unreported)
                        .any(|c| c["envelope_aware"] != true)
                    {
                        // Relays before v0.31.0 cannot read the version from 2026-07-28 hosts and
                        // never swap their server for such a host: only reconnecting helps.
                        reason = "relay";
                        "required"
                    } else if installed.is_none() || connections.iter().any(|c| unreported(&c)) {
                        // A current relay has not yet seen the version, e.g. before server/discover.
                        "unknown"
                    } else if connections
                        .iter()
                        .all(|c| c["runtime_version"].as_str() == installed.as_deref())
                    {
                        "current"
                    } else if install_settled {
                        // Still serving an old version long after the install: busy all along,
                        // holding an unread result, or otherwise unable to swap.
                        reason = "stale";
                        "required"
                    } else {
                        "updating"
                    };
                }
            }
        }
        json!({"state":state,"reason":reason,"token":token,"connections":connections.len(),"confirmed_at":confirmed["at"]})
    }
    fn connection_root(&self, host: &Host) -> Option<PathBuf> {
        let entry = host.read().ok()?.entry?;
        let explicit = entry[host.env_key()]["AGENTNAVE_DATA_DIR"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.context.env.get("AGENTNAVE_DATA_DIR").cloned());
        let root = explicit
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.context.home.join(".agentnave"));
        (root.is_absolute() && !root.is_symlink() && !root.join("connections").is_symlink())
            .then_some(root)
    }
    fn note_reload_change(&self, host: &Host, before: &Value, settings: &mut Value) {
        let current = self.reload_token(host, &Value::Null);
        if let Some(token) = current {
            let observed = &self.settings[host.id]["reload_observed"];
            let changed = if observed["v"] == RELOAD_FORMAT && observed["token"].is_string() {
                observed["token"] != token
            } else {
                *before != token
            };
            if !settings[host.id].is_object() {
                settings[host.id] = json!({});
            }
            settings[host.id]["reload_observed"] = json!({"v":RELOAD_FORMAT,"token":token});
            if changed {
                settings[host.id]["reload_changed"] = json!({"v":RELOAD_FORMAT,"token":token,"at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64});
            }
        }
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
        json!({"service":self.service(),"hosts":hosts,"backups":{"dir":self.context.app.join("backups"),"config":backup_paths(&self.context.app, CONFIG_BACKUP).len(),"skill":backup_paths(&self.context.app, SKILL_BACKUP).len()}})
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
        let version = self
            .tool_dirs()
            .and_then(|dirs| crate::update::installed_version(&dirs.tools.join("agentnave")));
        json!({"installed":self.launcher().is_some(),"version":version,"command":crate::update::service_command()})
    }
    pub fn refresh(&mut self) -> Value {
        let (shell, dirs) = probe_tools(&self.context);
        self.shell = Some(shell);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        self.refresh_probed()
    }
    pub(crate) fn refresh_probed(&mut self) -> Value {
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
                || (self.shell.as_ref().is_some_and(Result::is_err)
                    && !self.manual_roots.contains(host.id))
            {
                continue;
            }
            // Preflight every user layer before changing any of them. Project layers stay read-only.
            let layers = host
                .user_layers
                .iter()
                .map(|path| host.read_at(path).map(|document| (path, document.entry)))
                .collect::<Result<Vec<_>, _>>();
            let layers = match layers {
                Ok(layers) => layers,
                Err(error) => {
                    let (code, text) = error_parts(&error, "parse_failed");
                    self.record(host.id, Err(format!("{code}:{text}")), "");
                    continue;
                }
            };
            let signature = serde_json::to_string(&(host.id, &layers, "takeover")).unwrap();
            if self.failed.get(host.id) == Some(&signature) {
                continue;
            }
            let managed_before = layers.iter().any(|(_, entry)| {
                entry
                    .as_ref()
                    .is_some_and(|entry| managed_entry(&host, entry))
            });
            let result: Result<(bool, bool), String> = (|| {
                let mut changed = false;
                for (path, _) in &layers {
                    let mut layer = host.clone();
                    layer.config = (*path).clone();
                    changed |= transaction::edit(
                        transaction::Target {
                            path,
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
                            if let Some(command) = layer.command(desired)
                                && (entry_kind(command) == "legacy"
                                    || (entry_kind(command) == "agentnave"
                                        && self.command_state(&layer, desired) == "missing"))
                                && let Some(path) = self.launcher()
                            {
                                layer.set_command(desired, &path);
                            }
                            Ok(())
                        },
                    )?;
                }
                // Require a new-format entry both before and after the transaction. The preflight
                // check keeps legacy migration from installing a Skill in the same refresh; the
                // post-transaction check prevents a concurrently removed entry from being revived.
                Ok((changed, managed_before && self.has_managed_entry(&host)?))
            })();
            match result {
                Ok((changed, managed_entry)) => match self.ensure_skill(&host, managed_entry) {
                    Ok(skill_changed) if changed || skill_changed => {
                        let text = format!(
                            "已接管条目或更新 Skill{}；{}",
                            trust_hint(host.id),
                            activation_hint(host.id)
                        );
                        self.record(host.id, Ok(()), &text)
                    }
                    Ok(_) => {
                        self.failed.remove(host.id);
                        self.failures.remove(host.id);
                    }
                    Err(error) => {
                        // Retry Skill failures on every refresh; unlike a config conflict, a transient
                        // filesystem problem must not disable automatic repair for this host.
                        self.failed.remove(host.id);
                        self.record(host.id, Err(error), "");
                    }
                },
                Err(error) => {
                    self.failed.insert(host.id.into(), signature);
                    self.record(host.id, Err(error), "");
                }
            }
            self.note_reload_change(&host, &state["reload"]["token"], &mut settings);
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
            let mut settings = self.settings.clone();
            self.note_reload_change(&host, &Value::Null, &mut settings);
            if settings != self.settings {
                roots::save(&self.context, &settings)?;
                self.settings = settings;
            }
            self.cached = self.snapshot();
            return Ok(json!({"host":self.state(&host),"ok":true,"message":"已更新配置根目录"}));
        }
        let host = self.registry().into_iter().find(|h| h.id == id).unwrap();
        if operation == "confirm-reload" {
            if self.settings_error.is_some() {
                return Err("请先修复 hosts.json".into());
            }
            let current = self.state(&host);
            if current["status"] != "connected" {
                return Err("请先修复宿主接入，再确认重启".into());
            }
            let token = current["reload"]["token"]
                .as_str()
                .ok_or("无法确认此宿主，请先修复接入并刷新检测")?;
            if payload["token"].as_str() != Some(token) {
                return Err("服务或接入配置已变化，请刷新并重新启动宿主后再确认".into());
            }
            let mut settings = self.settings.clone();
            if !settings[id].is_object() {
                settings[id] = json!({});
            }
            settings[id]["reload_confirmed"] =
                json!({"v":RELOAD_FORMAT,"token":token,"at":timestamp()});
            roots::save(&self.context, &settings)?;
            self.settings = settings;
            self.cached = self.snapshot();
            return Ok(
                json!({"host":self.state(&host),"ok":true,"message":"已记录你已重启或重连；未自动验证宿主接口"}),
            );
        }
        if !matches!(operation, "connect" | "remove") {
            return Err("未知宿主操作".into());
        }
        let before_reload = self.state(&host)["reload"]["token"].clone();
        let result: Result<String, String> = (|| {
            if operation == "remove" {
                // Preflight all layers; each subsequent replacement is independently checked.
                for path in &host.user_layers {
                    safe_path(path, &self.context.home)?;
                    host.read_at(path)
                        .map_err(|e| format!("parse_failed:{e}"))?;
                }
                let mut removed = Vec::new();
                // Clear before the first mutation: a later partial removal may fail.
                if self.settings[id].get("reload_confirmed").is_some() {
                    let mut settings = self.settings.clone();
                    settings[id]
                        .as_object_mut()
                        .unwrap()
                        .remove("reload_confirmed");
                    roots::save(&self.context, &settings)?;
                    self.settings = settings;
                }
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
                .launcher()
                .ok_or("write_failed:请先安装或更新本机服务")?;
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
                _ => return Err("未知宿主操作".into()),
            }
            self.write(&host, &before, desired.as_ref())?;
            // The MCP entry is already connected even if the following Skill write fails.
            self.manual_roots.insert(id.into());
            let skill_changed = operation == "connect" && self.ensure_skill(&host, true)?;
            let state = host
                .skill
                .as_ref()
                .map(|path| skill::state(path, &self.context.home));
            let done = if skill_changed {
                "已接入并自动安装或更新 Skill"
            } else if state
                .as_ref()
                .is_some_and(|state| state["state"] == "external")
            {
                "已接入；外部管理的 Skill 保留"
            } else {
                "已接入；Skill 已是最新"
            };
            Ok(format!("{done}{}；{}", trust_hint(id), activation_hint(id)))
        })();
        let ok = result.is_ok();
        if operation == "connect" {
            let mut settings = self.settings.clone();
            self.note_reload_change(&host, &before_reload, &mut settings);
            if settings != self.settings && self.settings_error.is_none() {
                roots::save(&self.context, &settings)?;
                self.settings = settings;
            }
        }
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
    fn ensure_skill(&self, host: &Host, install_missing: bool) -> Result<bool, String> {
        let Some(path) = &host.skill else {
            return Ok(false);
        };
        let state = skill::state(path, &self.context.home);
        let missing = state["state"] == "missing";
        let outdated = state["state"] == "app" && state["version"] != VERSION;
        if (missing && install_missing) || outdated {
            skill::install(path, &self.context.app, host.id, &self.context.home)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn has_managed_entry(&self, host: &Host) -> Result<bool, String> {
        for path in &host.user_layers {
            if host
                .read_at(path)?
                .entry
                .as_ref()
                .is_some_and(|entry| managed_entry(host, entry))
            {
                return Ok(true);
            }
        }
        Ok(false)
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
fn activation_hint(id: &str) -> String {
    format!("接入配置或 Skill 简介变更时，{}", reload_hint(id))
}
fn reload_hint(id: &str) -> &'static str {
    match id {
        "codex" => "重启 Codex 应用（完整退出后重新打开）以生效",
        "claude" => "重启 Claude Code 后生效",
        "codebuddy" => "重启 CodeBuddy Code 后生效",
        "workbuddy" => "重启 WorkBuddy 后生效",
        "grok" => "重启 Grok CLI 后生效",
        "antigravity" => "重启 Antigravity 后生效",
        "pi" => "重启 Pi 后生效",
        "gemini" => "重启 Gemini CLI 后生效",
        "opencode" => "重启 OpenCode 后生效",
        "hermes" => "重启 Hermes 网关后生效",
        _ => "重启宿主应用后生效",
    }
}
fn managed_entry(host: &Host, entry: &Value) -> bool {
    host.command(entry)
        .is_some_and(|command| entry_kind(command) == "agentnave")
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
        "agentnave-studio-mcp" => "legacy",
        "agentnave-mcp" => "agentnave",
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
/// Host config files and Skill files are kept apart, so Skill copies never evict config backups.
/// Files written before the split carry no kind and count as config backups.
const CONFIG_BACKUP: &str = "config";
const SKILL_BACKUP: &str = "skill";
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
            let skill = name
                .get(21..)
                .is_some_and(|rest| rest.starts_with(&format!("{SKILL_BACKUP}-")));
            skill == (kind == SKILL_BACKUP)
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
