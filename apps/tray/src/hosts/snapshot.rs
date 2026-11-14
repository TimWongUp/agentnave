use super::*;

impl Hosts {
    fn clients(&self, host: &Host) -> Vec<Value> {
        let mut clients = Vec::new();
        // An extra Codex home proves a configuration exists, not which installed reader uses it.
        if host.id.starts_with("codex@") {
            return clients;
        }
        let separate_desktop =
            host.id == "claude" && self.registry().iter().any(|row| row.id == "claude-desktop");
        let desktop = if separate_desktop {
            None
        } else {
            installed::desktop(&self.context, host.boss)
        };
        if host.id != "claude-desktop" {
            let cli = if host.boss == "codex" {
                let names: &[&str] = if cfg!(windows) {
                    &["codex.exe", "codex.cmd", "codex.bat"]
                } else {
                    &["codex"]
                };
                self.context
                    .search
                    .iter()
                    .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
                    .find(|path| {
                        executable(path) && crate::detect::codex_desktop_app(path).is_none()
                    })
                    .or_else(crate::detect::standalone_codex)
            } else {
                self.context
                    .find(host.binary)
                    .or_else(|| crate::detect::locate(host.binary))
            };
            // A CLI shipped inside the desktop app (DeepSeek Harness's dsh) is that app.
            let app = desktop.as_ref().and_then(|(_, path)| path.as_deref());
            if let Some(path) = cli
                && !app.is_some_and(|app| inside(&path, app))
            {
                clients.push(json!({"kind":"cli","source":"path","path":path}));
            }
        }
        if let Some((source, path)) = desktop {
            clients.push(json!({"kind":"desktop","source":source,"path":path}));
        }
        clients
    }
    pub(super) fn detect(&self, host: &Host) -> (bool, &'static str, Option<PathBuf>) {
        if host.id == "claude-desktop" {
            if let Some((source, path)) = installed::desktop(&self.context, "claude") {
                return (true, source, path);
            }
            return (false, "config", None);
        }
        let separate_desktop =
            host.id == "claude" && self.registry().iter().any(|row| row.id == "claude-desktop");
        // An extra Codex home is read by the CLI alone; desktop apps keep the default one.
        if !separate_desktop
            && !host.id.starts_with("codex@")
            && let Some((source, path)) = installed::desktop(&self.context, host.boss)
        {
            return (true, source, path);
        }
        if self.context.find(host.binary).is_some() || crate::detect::locate(host.binary).is_some()
        {
            return (true, "path", None);
        }
        (false, "config", None)
    }
    pub(super) fn command_state(&self, host: &Host, entry: &Value) -> &'static str {
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
        let env = entry.get("env");
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
            if executable(&dir.join(command))
                || (cfg!(windows)
                    && ["exe", "cmd", "bat"]
                        .iter()
                        .any(|ext| executable(&dir.join(format!("{command}.{ext}")))))
            {
                return "present";
            }
        }
        if unknown { "unknown" } else { "missing" }
    }
    pub(super) fn layers(&self, host: &Host) -> Vec<Value> {
        let mut layers:Vec<_>=host.user_layers.iter().enumerate().map(|(i,p)|json!({"path":p,"scope":if i==0{"user"}else{"override"},"has_entry":host.read_at(p).is_ok_and(|d|d.entry.is_some())})).collect();
        for dir in self.context.project.ancestors() {
            let relative: &[&str] = match host.boss {
                "codex" => &[".codex/config.toml"],
                "claude" | "codebuddy" | "workbuddy" => &[".mcp.json"],
                "devin" => &[".devin/mcp_config.local.json", ".devin/mcp_config.json"],
                "grok" => &[".grok/config.toml"],
                "pi" => &[".pi/mcp.json"],
                "zcode" => &["zcode.json", ".zcode/config.json"],
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
        if host.boss == "claude"
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
    pub(super) fn state(&self, host: &Host) -> Value {
        let (installed, source, app_path) = self.detect(host);
        let clients = self.clients(host);
        let document = host.read();
        let entry = document.as_ref().ok().and_then(|d| d.entry.as_ref());
        let command = entry.and_then(|e| host.command(e));
        let kind = command.map(entry_kind);
        let command_state = entry.map(|e| self.command_state(host, e));
        let has_env = entry.is_some_and(|e| {
            e["env"]["AGENTNAVE_HOST"] == host.boss && e["env"]["AGENTNAVE_HOST_APP"] == host.id
        });
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
        let meeting = self.main_skill_is_meeting(host).unwrap_or_else(|| {
            host.optional("agentnave-meeting").is_some_and(|p| {
                skill::state(&p, &self.context.home, skill::MEETING)["state"] == "app"
            })
        });
        let path = if meeting {
            host.optional("agentnave-meeting")
        } else {
            host.skill.clone()
        };
        let skill = path
            .as_ref()
            .map(|p| {
                skill::state(
                    p,
                    &self.context.home,
                    if meeting {
                        skill::MEETING
                    } else {
                        skill::MANAGER
                    },
                )
            })
            .unwrap_or_else(|| json!({"state":"manual","path":null,"version":null}));
        let mut skills = serde_json::Map::new();
        for (name, embedded) in skill::OPTIONAL {
            let mut state = host
                .optional(name)
                .map(|p| skill::state(&p, &self.context.home, embedded))
                .unwrap_or_else(|| json!({"state":"manual","path":null,"version":null}));
            state["enabled"] = json!(self.skill_wanted(host.id, name));
            skills.insert((*name).into(), state);
        }
        let reload = if !has_entry || !installed {
            json!({"state":"not_connected","copies":[]})
        } else if let Some(token) = self.reload_token(host) {
            self.reload_state(host, &token)
        } else {
            json!({"state":"unknown","copies":[]})
        };
        json!({"id":host.id,"boss":host.boss,"mark":host.id.split('@').next(),"label":host.label,"installed":installed,"detect_source":source,"app_path":app_path,"clients":clients,"status":status,
            "entry":kind,"command":command,"command_state":command_state,"has_host_env":has_env,"config_path":host.config,"root":{"path":host.root.path,"source":host.root.source},
            "layers":layers,"shadowed_by":shadowed,"tested":matches!(host.boss, "codex" | "claude" | "codebuddy" | "workbuddy" | "grok" | "antigravity" | "pi" | "deepseek" | "devin" | "hermes"),"skill":skill,"skills":skills,"reload":reload,"fault":self.fault_state(host),
            "reload_hint":reload_hint(host.id),"reopen":reopen::instructions(host),"copy_actions":reopen::copy_actions(host),"trust":trust_hint(host.boss).trim_start_matches('；'),"launcher":self.launcher_note(host),"attention":attention,"last_action":self.actions.get(host.id),"first_seen":self.settings[host.id]["first_seen"].as_str().unwrap_or(&self.detected_at)})
    }
    pub fn cached_snapshot(&self) -> &Value {
        &self.cached
    }
    pub fn snapshot(&self) -> Value {
        let states: Vec<_> = self.registry().iter().map(|h| self.state(h)).collect();
        // Supported bosses not found on this machine: the page lists them by name only.
        let mut absent: Vec<Value> = Vec::new();
        for state in states.iter().filter(|h| h["status"] == "absent") {
            let shown = states
                .iter()
                .any(|h| h["status"] != "absent" && h["mark"] == state["mark"]);
            if !shown && !absent.iter().any(|h| h["mark"] == state["mark"]) {
                absent.push(json!({"mark":state["mark"],"label":state["label"]}));
            }
        }
        let hosts: Vec<_> = states
            .into_iter()
            .filter(|h| h["status"] != "absent")
            .collect();
        json!({"service":self.service(),"hosts":hosts,"absent":absent,"backups":{"dir":self.context.app.join("backups"),"config":backup_paths(&self.context.app, CONFIG_BACKUP).len(),"skill":backup_paths(&self.context.app, SKILL_BACKUP).len()}})
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
        let env = crate::update::active_tools(&self.context.app)
            .or_else(|| self.tool_dirs().map(|dirs| dirs.tools.join("agentnave")));
        let version = env.as_deref().and_then(crate::update::installed_version);
        // The service package carries the Skills; an agent connecting itself copies them from here.
        let skills = env
            .as_deref()
            .and_then(crate::update::installed_metadata)
            .and_then(|meta| Some(meta.parent()?.join("agentnave/skills")))
            .filter(|dir| dir.join("agentnave-manager/SKILL.md").is_file());
        let launcher = self.launcher();
        json!({"installed":launcher.is_some(),"launcher":launcher,"skills":skills,"version":version,"command":crate::update::service_command()})
    }
}

/// Whether `path` lies within an app folder, or the folder of an app's executable.
fn inside(path: &Path, app: &Path) -> bool {
    let dir = if app.is_file() {
        app.parent().unwrap_or(app)
    } else {
        app
    };
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    real(path).starts_with(real(dir))
}
