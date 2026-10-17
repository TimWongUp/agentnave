//! App-owned host integration (ADR-0015). Reads never take over or create files.
mod config;
mod skill;
#[cfg(test)]
mod tests;

use config::{Document, Format};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TIMEOUT: Duration = Duration::from_secs(15);

/// All environment-sensitive paths are injectable; tests never change process-wide HOME.
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

    fn path(&self, var: &str, default: &str) -> PathBuf {
        self.env
            .get(var)
            .filter(|v| !v.is_empty())
            .map(|v| {
                if let Some(rest) = v.strip_prefix("~/") {
                    self.home.join(rest)
                } else {
                    PathBuf::from(v)
                }
            })
            .unwrap_or_else(|| self.home.join(default))
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

    fn launcher(&self, entry: &str) -> Option<PathBuf> {
        let name = if entry == "core" {
            "agentnave-mcp"
        } else {
            "agentnave-studio-mcp"
        };
        let tools = self.path("UV_TOOL_DIR", ".local/share/uv/tools");
        let tools = if !self.env.contains_key("UV_TOOL_DIR") {
            self.env
                .get("XDG_DATA_HOME")
                .map(|p| PathBuf::from(p).join("uv/tools"))
                .unwrap_or(tools)
        } else {
            tools
        };
        #[cfg(windows)]
        let candidate = tools.join("agentnave/Scripts").join(format!("{name}.exe"));
        #[cfg(not(windows))]
        let candidate = tools.join("agentnave/bin").join(name);
        executable(&candidate).then_some(candidate)
    }

    fn registry(&self) -> Vec<Host> {
        let codex = self.path("CODEX_HOME", ".codex");
        let claude = self.path("CLAUDE_CONFIG_DIR", ".claude");
        let buddy = self.path("CODEBUDDY_CONFIG_DIR", ".codebuddy");
        let work = self.path("CODEBUDDY_CONFIG_DIR", ".workbuddy");
        let grok = self.path("GROK_HOME", ".grok");
        let hermes = self.path("HERMES_HOME", ".hermes");
        let pi = self.path("PI_CODING_AGENT_DIR", ".pi/agent");
        let gemini = self.home.join(".gemini");
        let xdg = self.path("XDG_CONFIG_HOME", ".config");
        let opendir = self.path("OPENCODE_CONFIG_DIR", ".config/opencode");
        let opendir = if self.env.contains_key("OPENCODE_CONFIG_DIR") {
            opendir
        } else {
            xdg.join("opencode")
        };
        let mut openfiles = config_candidates(&xdg.join("opencode"));
        if self
            .env
            .get("OPENCODE_CONFIG")
            .is_some_and(|v| !v.is_empty())
        {
            openfiles.push(self.path("OPENCODE_CONFIG", ".config/opencode/opencode.json"));
        }
        if self
            .env
            .get("OPENCODE_CONFIG_DIR")
            .is_some_and(|v| !v.is_empty())
        {
            openfiles.extend(config_candidates(&opendir));
        }
        // Update the effective existing entry. A higher layer without this name inherits it;
        // malformed higher layers must surface an error rather than silently edit a lower one.
        let openfile = openfiles
            .iter()
            .rev()
            .find(|path| {
                fs::read_to_string(path).is_ok_and(|raw| {
                    match Document::parse(&raw, Format::Json, "mcp") {
                        Ok(doc) => doc.entry.is_some(),
                        Err(_) => true,
                    }
                })
            })
            .unwrap_or_else(|| openfiles.last().unwrap())
            .clone();
        vec![
            Host::new(
                "codex",
                "Codex",
                "codex",
                codex.join("config.toml"),
                codex.join("skills"),
                Format::Toml,
                "mcp_servers",
                true,
            ),
            Host::new(
                "claude",
                "Claude Code",
                "claude",
                if self.env.contains_key("CLAUDE_CONFIG_DIR") {
                    claude.join(".claude.json")
                } else {
                    self.home.join(".claude.json")
                },
                claude.join("skills"),
                Format::Json,
                "mcpServers",
                true,
            ),
            Host::new(
                "codebuddy",
                "CodeBuddy Code",
                "codebuddy",
                buddy.join(".mcp.json"),
                buddy.join("skills"),
                Format::Json,
                "mcpServers",
                true,
            ),
            Host::new(
                "workbuddy",
                "WorkBuddy",
                "workbuddy",
                work.join("mcp.json"),
                work.join("skills"),
                Format::Json,
                "mcpServers",
                true,
            ),
            Host::new(
                "grok",
                "Grok CLI",
                "grok",
                grok.join("config.toml"),
                grok.join("skills"),
                Format::Toml,
                "mcp_servers",
                true,
            ),
            Host::new(
                "antigravity",
                "Antigravity",
                "agy",
                gemini.join("config/mcp_config.json"),
                gemini.join("config/skills"),
                Format::Json,
                "mcpServers",
                true,
            ),
            Host::new(
                "pi",
                "Pi",
                "pi",
                pi.join("mcp.json"),
                pi.join("skills"),
                Format::Json,
                "mcpServers",
                false,
            ),
            Host::new(
                "gemini",
                "Gemini CLI",
                "gemini",
                gemini.join("settings.json"),
                gemini.join("skills"),
                Format::Json,
                "mcpServers",
                false,
            ),
            Host::new(
                "opencode",
                "OpenCode",
                "opencode",
                openfile,
                opendir.join("skills"),
                Format::Json,
                "mcp",
                false,
            ),
            Host::new(
                "hermes",
                "Hermes",
                "hermes",
                hermes.join("config.yaml"),
                hermes.join("skills"),
                Format::Yaml,
                "mcp_servers",
                true,
            ),
        ]
    }
}

#[derive(Clone)]
struct Host {
    id: &'static str,
    label: &'static str,
    binary: &'static str,
    config: PathBuf,
    skill: PathBuf,
    format: Format,
    key: &'static str,
    tested: bool,
}
impl Host {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: &'static str,
        label: &'static str,
        binary: &'static str,
        config: PathBuf,
        skills: PathBuf,
        format: Format,
        key: &'static str,
        tested: bool,
    ) -> Self {
        Self {
            id,
            label,
            binary,
            config,
            skill: skills.join("agentnave-manager"),
            format,
            key,
            tested,
        }
    }
    fn cli(&self) -> bool {
        matches!(
            self.id,
            "codex" | "claude" | "codebuddy" | "grok" | "pi" | "gemini"
        )
    }
    fn read(&self) -> Result<Document, String> {
        let raw = match fs::read_to_string(&self.config) {
            Ok(raw) => raw,
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

pub struct Hosts {
    pub context: Context,
    actions: HashMap<String, Value>,
    /// Failed unchanged takeovers are not retried on every refresh.
    failed: HashMap<String, String>,
}

impl Hosts {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            actions: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    fn detect(&self, host: &Host) -> (bool, Option<String>) {
        if let Some(path) = self.context.find(host.binary) {
            return (true, Some(path.display().to_string()));
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
            return (true, Some(path.display().to_string()));
        }
        // Retained configuration directories alone cannot prove a host is still installed.
        (false, None)
    }

    fn state(&self, host: &Host) -> Value {
        let (installed, source) = self.detect(host);
        let document = host.read();
        let error = document.as_ref().err();
        let entry = document.as_ref().ok().and_then(|d| d.entry.as_ref());
        let command = entry.and_then(|e| host.command(e));
        let kind = command.map(entry_kind).unwrap_or("other");
        let has_env = entry.is_some_and(|e| e[host.env_key()]["AGENTNAVE_HOST"] == host.id);
        let broken = entry.is_some() && command.is_none_or(|c| !self.command_exists(c));
        let failed = self.actions.get(host.id).is_some_and(|a| a["ok"] == false);
        let status = if error.is_some() {
            "needs_attention"
        } else if entry.is_some() && !installed {
            "orphaned"
        } else if broken || (entry.is_some() && (!has_env || failed)) {
            "needs_attention"
        } else if entry.is_some() {
            "connected"
        } else if installed {
            "available"
        } else {
            "absent"
        };
        let last = if let Some(error) = error {
            json!({"ok": false, "message": error, "at": null})
        } else {
            self.actions.get(host.id).cloned().unwrap_or(Value::Null)
        };
        json!({"id": host.id, "label": host.label, "installed": installed, "detect_source": source,
            "status": status, "entry": kind, "command": command, "has_host_env": has_env,
            "config_path": host.config, "shadowed_by": self.shadow(host), "tested": host.tested,
            "skill": skill::state(&host.skill), "last_action": last})
    }

    fn command_exists(&self, command: &str) -> bool {
        if let Some(rest) = command.strip_prefix("~/") {
            return executable(&self.context.home.join(rest));
        }
        let path = Path::new(command);
        if path.is_absolute() {
            executable(path)
        } else if path.components().count() == 1 {
            self.context.find(command).is_some()
        } else {
            false
        }
    }

    fn shadow(&self, host: &Host) -> Option<String> {
        for dir in self.context.project.ancestors() {
            let paths: Vec<PathBuf> = match host.id {
                "codex" => vec![dir.join(".codex/config.toml")],
                "claude" | "codebuddy" | "workbuddy" => vec![dir.join(".mcp.json")],
                "grok" => vec![dir.join(".grok/config.toml")],
                "pi" => vec![dir.join(".pi/mcp.json")],
                "gemini" => vec![dir.join(".gemini/settings.json")],
                "opencode" => vec![
                    dir.join("opencode.json"),
                    dir.join("opencode.jsonc"),
                    dir.join(".opencode/opencode.json"),
                    dir.join(".opencode/opencode.jsonc"),
                ],
                _ => vec![],
            };
            for path in paths {
                if path == host.config {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path)
                    && Document::parse(&raw, host.format, host.key).is_ok_and(|d| d.entry.is_some())
                {
                    return Some(path.display().to_string());
                }
            }
            if dir.join(".git").exists() {
                break;
            }
        }
        // Claude also stores local project entries in its user file.
        if host.id == "claude"
            && let Ok(raw) = fs::read_to_string(&host.config)
            && let Ok(value) = serde_json::from_str::<Value>(&raw)
            && value["projects"][self.context.project.to_string_lossy().as_ref()]["mcpServers"]
                .get("agentnave")
                .is_some()
        {
            return Some(format!("{} (project scope)", host.config.display()));
        }
        None
    }

    pub fn snapshot(&self) -> Value {
        let hosts: Vec<_> = self
            .context
            .registry()
            .iter()
            .map(|h| self.state(h))
            .filter(|h| h["status"] != "absent")
            .collect();
        json!({"service": self.service(), "hosts": hosts, "backups": {"dir": self.context.app.join("backups"), "count": backup_paths(&self.context.app).len()}})
    }
    pub fn service(&self) -> Value {
        let studio = self.context.launcher("studio");
        let core = self.context.launcher("core");
        json!({"installed": studio.is_some() || core.is_some(), "studio": studio, "core": core})
    }

    pub fn refresh(&mut self) -> Value {
        for host in self.context.registry() {
            if !self.detect(&host).0 {
                continue;
            }
            let Ok(document) = host.read() else {
                continue;
            };
            let Some(mut desired) = document.entry.clone() else {
                continue;
            };
            let signature = format!(
                "{}:{:?}:{:?}",
                document.raw,
                self.context.launcher("core"),
                self.context.launcher("studio")
            );
            if self.failed.get(host.id) == Some(&signature) {
                continue;
            }
            let managed = skill::state(&host.skill);
            let update_skill = managed["state"] == "app" && managed["version"] != VERSION;
            let result = (|| {
                let env_key = host.env_key();
                if desired.get(env_key).is_none() {
                    desired[env_key] = json!({});
                }
                let env = desired[env_key].as_object_mut().ok_or("env 必须为对象")?;
                env.insert("AGENTNAVE_HOST".into(), json!(host.id));
                if let Some(command) = host.command(&desired)
                    && !self.command_exists(command)
                    && matches!(entry_kind(command), "core" | "studio")
                    && let Some(path) = self.context.launcher(entry_kind(command))
                {
                    host.set_command(&mut desired, &path);
                }
                if document.entry.as_ref() != Some(&desired) {
                    self.write(&host, &document, Some(&desired))?;
                }
                if update_skill {
                    skill::install(&host.skill, &self.context.app, host.id)?;
                }
                Ok(())
            })();
            if result.is_err() {
                self.failed.insert(host.id.into(), signature);
            }
            if document.entry.as_ref() != Some(&desired) || update_skill || result.is_err() {
                self.record(host.id, result, "已接管宿主配置");
            }
        }
        self.snapshot()
    }

    pub fn action(&mut self, operation: &str, payload: &Value) -> Result<Value, String> {
        let id = payload["id"].as_str().ok_or("缺少宿主 id")?;
        let host = self
            .context
            .registry()
            .into_iter()
            .find(|h| h.id == id)
            .ok_or("未知宿主 id")?;
        if payload.get("entry").is_some_and(|v| !v.is_string()) {
            return Err("entry 必须为 studio 或 core".into());
        }
        let entry = payload["entry"].as_str().unwrap_or("studio");
        if operation != "remove" && !matches!(entry, "studio" | "core") {
            return Err("entry 必须为 studio 或 core".into());
        }
        if operation == "entry" && payload.get("entry").is_none() {
            return Err("缺少 entry".into());
        }
        let result = (|| {
            let document = host.read()?;
            if operation != "remove" && !self.detect(&host).0 {
                return Err("宿主未安装".into());
            }
            let mut desired = document.entry.clone();
            match operation {
                "connect" => {
                    let path = self.context.launcher(entry).ok_or("请先安装本机服务")?;
                    if let Some(value) = desired.as_mut() {
                        host.set_command(value, &path);
                        if value.get(host.env_key()).is_none() {
                            value[host.env_key()] = json!({});
                        }
                        value[host.env_key()]
                            .as_object_mut()
                            .ok_or("env 必须为对象")?
                            .insert("AGENTNAVE_HOST".into(), json!(host.id));
                    } else {
                        desired = Some(if host.id == "opencode" {
                            json!({"type": "local", "command": [path], "environment": {"AGENTNAVE_HOST": host.id}, "enabled": true})
                        } else if host.id == "hermes" {
                            json!({"command": path, "args": [], "env": {"AGENTNAVE_HOST": host.id}, "enabled": true})
                        } else {
                            json!({"command": path, "args": [], "env": {"AGENTNAVE_HOST": host.id}})
                        });
                    }
                }
                "remove" => desired = None,
                "entry" => {
                    let path = self.context.launcher(entry).ok_or("请先安装本机服务")?;
                    host.set_command(desired.as_mut().ok_or("宿主尚未接入")?, &path);
                }
                _ => return Err("未知宿主操作".into()),
            }
            if document.entry != desired {
                self.write(&host, &document, desired.as_ref())?;
            }
            if operation == "connect" {
                skill::install(&host.skill, &self.context.app, host.id)?;
            }
            if operation == "remove" {
                skill::remove(&host.skill, &self.context.app, host.id)?;
            }
            Ok(())
        })();
        let ok = result.is_ok();
        let message = result
            .as_ref()
            .err()
            .cloned()
            .unwrap_or_else(|| match operation {
                "remove" => "已移除接入（外部管理的 Skill 保留）".into(),
                "entry" => "已切换入口".into(),
                _ => "已接入（外部管理的 Skill 保留）".into(),
            });
        self.failed.remove(id);
        self.record(id, result, &message);
        let mut state = self.state(&host);
        state["ok"] = json!(ok);
        state["message"] = json!(message);
        Ok(state)
    }

    fn record(&mut self, id: &str, result: Result<(), String>, success: &str) {
        self.actions.insert(id.into(), json!({"ok": result.is_ok(), "message": result.err().unwrap_or_else(|| success.into()), "at": timestamp()}));
    }

    fn write(&self, host: &Host, before: &Document, desired: Option<&Value>) -> Result<(), String> {
        // Never replace a symlink's target, including externally managed config files.
        if host.config.is_symlink() {
            return Err("配置是符号链接，需手动管理".into());
        }
        let original = fs::read(&host.config).ok();
        if original.as_deref().unwrap_or_default() != before.raw.as_bytes() {
            return Err("配置已被其他进程修改，请刷新".into());
        }
        let expected = before.render(desired)?;
        before.verify(&expected, desired)?;
        backup(&self.context.app, host.id, &host.config)?;
        let result = (|| {
            if host.cli() && (desired.is_some() || self.context.find(host.binary).is_some()) {
                self.write_cli(host, before.entry.as_ref(), desired)?;
                let written = host.read()?;
                if written.rest != before.rest {
                    return Err("宿主 CLI 改动了其他配置".into());
                }
                if let Some(desired) = desired {
                    let actual = written.entry.as_ref().ok_or("宿主 CLI 未保存条目")?;
                    if host.command(actual) != host.command(desired)
                        || actual.get(host.env_key()) != desired.get(host.env_key())
                        || actual.get("args").unwrap_or(&json!([]))
                            != desired.get("args").unwrap_or(&json!([]))
                    {
                        return Err("宿主 CLI 保存的条目与请求不一致".into());
                    }
                } else if written.entry.is_some() {
                    return Err("宿主 CLI 未移除条目".into());
                }
                // Native commands may drop flags they cannot express. Restore those original
                // fields through a scoped atomic edit, never accept those lossy defaults.
            }
            atomic(&host.config, expected.as_bytes())?;
            before.verify(
                &fs::read_to_string(&host.config).map_err(|_| "无法重新读取配置")?,
                desired,
            )
        })();
        if let Err(error) = result {
            restore(&host.config, original.as_deref())
                .map_err(|rollback| format!("{error}；回滚失败：{rollback}"))?;
            return Err(error);
        }
        Ok(())
    }

    fn write_cli(
        &self,
        host: &Host,
        old: Option<&Value>,
        desired: Option<&Value>,
    ) -> Result<(), String> {
        let binary = self
            .context
            .find(host.binary)
            .ok_or("找不到宿主 CLI，保留原配置")?;
        // Claude/CodeBuddy reject duplicate names. Remove + add stays in one backed-up transaction.
        if desired.is_none() || (old.is_some() && matches!(host.id, "claude" | "codebuddy")) {
            let mut args = vec!["mcp".into(), "remove".into(), "agentnave".into()];
            if matches!(host.id, "claude" | "codebuddy" | "gemini" | "grok") {
                args.extend(["--scope".into(), "user".into()]);
            }
            self.run_cli(&binary, host, &args)?;
        }
        if let Some(desired) = desired {
            let command = host
                .command(desired)
                .ok_or("条目不是本地 STDIO 命令，需手动处理")?;
            let mut args = vec!["mcp".into(), "add".into(), "agentnave".into()];
            if matches!(host.id, "claude" | "codebuddy" | "gemini" | "grok") {
                args.extend(["--scope".into(), "user".into()]);
            }
            if matches!(host.id, "claude" | "codebuddy" | "grok") {
                args.extend(["--transport".into(), "stdio".into()]);
            }
            if let Some(env) = desired.get(host.env_key()) {
                for (key, value) in env.as_object().ok_or("env 必须为对象")? {
                    args.extend([
                        "--env".into(),
                        format!("{key}={}", value.as_str().ok_or("env 值必须为字符串")?),
                    ]);
                }
            }
            args.extend(["--".into(), command.into()]);
            if let Some(values) = desired.get("args") {
                for value in values.as_array().ok_or("args 必须为数组")? {
                    args.push(value.as_str().ok_or("args 值必须为字符串")?.into());
                }
            }
            self.run_cli(&binary, host, &args)?;
        }
        Ok(())
    }

    fn run_cli(&self, binary: &Path, host: &Host, args: &[String]) -> Result<(), String> {
        let mut command = Command::new(binary);
        command
            .args(args)
            .env_clear()
            .envs(&self.context.env)
            .env("HOME", &self.context.home)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Pin the same effective directory used by the registry, especially CodeBuddy's override.
        let var = match host.id {
            "codex" => Some("CODEX_HOME"),
            "codebuddy" => Some("CODEBUDDY_CONFIG_DIR"),
            "grok" => Some("GROK_HOME"),
            "pi" => Some("PI_CODING_AGENT_DIR"),
            _ => None,
        };
        if let Some(var) = var {
            command.env(var, host.config.parent().unwrap());
        }
        let mut search = vec![binary.parent().unwrap().to_path_buf()];
        search.extend(self.context.search.clone());
        if let Ok(path) = std::env::join_paths(search) {
            command.env("PATH", path);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "宿主 CLI 启动失败")?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err("宿主 CLI 写入失败，已回滚".into())
                    };
                }
                Ok(None) if started.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(20)),
                _ => {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("宿主 CLI 超时或等待失败，已回滚".into());
                }
            }
        }
    }
}

fn config_candidates(dir: &Path) -> Vec<PathBuf> {
    let json = dir.join("opencode.json");
    let jsonc = dir.join("opencode.jsonc");
    if jsonc.exists() {
        vec![json, jsonc]
    } else {
        vec![json]
    }
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
fn restore(path: &Path, original: Option<&[u8]>) -> Result<(), String> {
    if let Some(original) = original {
        atomic(path, original)
    } else if path.exists() {
        fs::remove_file(path).map_err(|_| "无法移除失败写入的配置".into())
    } else {
        Ok(())
    }
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
fn backup(app: &Path, id: &str, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let dir = app.join("backups");
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
    let bytes = fs::read(path).map_err(|_| "无法读取备份源")?;
    file.write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "无法保存备份")?;
    file.keep().map_err(|_| "无法保留备份")?;
    let paths = backup_paths(app);
    for old in paths.iter().take(paths.len().saturating_sub(30)) {
        fs::remove_file(old).map_err(|_| "无法清理旧备份")?;
    }
    Ok(())
}
