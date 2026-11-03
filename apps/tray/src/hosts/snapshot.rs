use super::*;

impl Hosts {
    pub(super) fn detect(&self, host: &Host) -> (bool, &'static str, Option<PathBuf>) {
        if self.context.find(host.binary).is_some() {
            return (true, "path", None);
        }
        // A CLI that ships inside a desktop app is not on PATH; the same rules the member list
        // uses resolve it (Codex and DeepSeek Harness both arrive this way on Windows).
        if crate::detect::locate(host.binary).is_some() {
            return (true, "path", None);
        }
        let app = match host.boss {
            "claude" => Some("Claude.app"),
            "workbuddy" => Some("WorkBuddy.app"),
            "antigravity" => Some("Antigravity.app"),
            "deepseek" => Some("DeepSeek Harness.app"),
            "zcode" => Some("ZCode.app"),
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
        // Windows desktop apps install a program directory, or bundle their CLI inside one;
        // neither is on PATH.
        #[cfg(windows)]
        if let Some(path) = self.bundle(host.id) {
            return (true, "app", Some(path));
        }
        (false, "config", None)
    }
    /// The desktop apps install a program directory. Only the ones known to ship an executable
    /// are listed; adding a guess here would report a host as installed when it is not.
    #[cfg(windows)]
    pub(super) fn bundle(&self, id: &str) -> Option<PathBuf> {
        let relative = match id {
            "deepseek" => "DeepSeek Harness/DeepSeek Harness.exe",
            // WorkBuddy installs wherever the user chose (D:\Workbuddy here), and only its
            // uninstall entry records that.
            "workbuddy" => return crate::detect::uninstall_icon("WorkBuddy"),
            _ => return None,
        };
        self.context
            .applications
            .iter()
            .map(|dir| dir.join(relative))
            .find(|path| path.is_file())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    pub(super) fn bundle(&self, _id: &str) -> Option<PathBuf> {
        None
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
        let skill = host
            .skill
            .as_ref()
            .map(|p| skill::state(p, &self.context.home, skill::MANAGER))
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
            json!({"state":"not_connected"})
        } else if let Some(token) = self.reload_token(host) {
            self.reload_state(host, &token)
        } else {
            json!({"state":"unknown"})
        };
        json!({"id":host.id,"boss":host.boss,"mark":host.id.split('@').next(),"label":host.label,"installed":installed,"detect_source":source,"app_path":app_path,"status":status,
            "entry":kind,"command":command,"command_state":command_state,"has_host_env":has_env,"config_path":host.config,"root":{"path":host.root.path,"source":host.root.source},
            "layers":layers,"shadowed_by":shadowed,"tested":matches!(host.boss, "codex" | "claude" | "codebuddy" | "workbuddy" | "grok" | "antigravity" | "pi" | "deepseek" | "hermes"),"skill":skill,"skills":skills,"reload":reload,
            "reload_hint":reload_hint(host.id),"launcher":self.launcher_note(host),"attention":attention,"last_action":self.actions.get(host.id),"first_seen":self.settings[host.id]["first_seen"].as_str().unwrap_or(&self.detected_at)})
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
        let version = crate::update::active_tools(&self.context.app)
            .or_else(|| self.tool_dirs().map(|dirs| dirs.tools.join("agentnave")))
            .as_deref()
            .and_then(crate::update::installed_version);
        json!({"installed":self.launcher().is_some(),"version":version,"command":crate::update::service_command()})
    }
}
