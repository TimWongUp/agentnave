use super::*;

impl Hosts {
    pub(super) fn registry(&self) -> Vec<Host> {
        let h = &self.context.home;
        let devin_config = if cfg!(windows) {
            roots::env_value(&self.context.env, "APPDATA")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| h.join("AppData/Roaming"))
                .join("devin")
        } else {
            roots::env_value(&self.context.env, "XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| h.join(".config"))
                .join("devin")
        };
        let specs = [
            (
                "zcode",
                "ZCode",
                "zcode",
                None,
                h.join(".zcode"),
                "cli/config.json",
                Format::Zcode,
                "servers",
            ),
            (
                "codex",
                match self
                    .context
                    .find("codex")
                    .or_else(|| crate::detect::locate("codex"))
                    .as_deref()
                    .and_then(crate::detect::codex_desktop_app)
                {
                    Some("ChatGPT") => "ChatGPT（Codex）",
                    Some(_) => "Codex",
                    None => "Codex CLI",
                },
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
                // The home-level patch layer applies to the desktop App and every CLI profile.
                "deepseek",
                "DeepSeek Harness",
                "dsh",
                Some("DSH_HOME"),
                h.join(".dsh"),
                "cordis.patch.yml",
                Format::DshPatch,
                "insert",
            ),
            (
                "devin",
                "Devin CLI",
                "devin",
                None,
                devin_config,
                "mcp_config.json",
                Format::Json,
                "mcpServers",
            ),
            (
                "hermes",
                "Hermes",
                "hermes",
                Some("HERMES_HOME"),
                hermes_default(h),
                "config.yaml",
                Format::Yaml,
                "mcp_servers",
            ),
        ];
        let build = |(id, label, binary, var, default, file, format, key): Spec| {
            let boss = split::boss_of(id);
            let mut root = roots::resolve(
                &self.context,
                &self.settings,
                self.shell.as_ref(),
                id,
                var,
                default,
            );
            if self.manual_roots.contains(id) {
                root.confirmed = true;
            }
            let mut paths = vec![root.path.join(file)];
            if boss == "claude" && root.source == "default" {
                paths = vec![h.join(".claude.json")];
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
                boss,
                label,
                binary,
                skill: Some(root.path.join("skills/agentnave-manager")),
                root,
                config,
                format,
                key,
                user_layers: paths,
            }
        };
        let mut hosts: Vec<Host> = specs.into_iter().map(&build).collect();
        let rows = split::rows(&self.context, &hosts, &self.codex_homes);
        if split::claude_desktop_here(&self.context)
            && !rows.iter().any(|row| row.0 == "claude-desktop")
            && let Some(claude) = hosts.iter_mut().find(|host| host.id == "claude")
        {
            claude.label = "Claude Code（终端／桌面端）";
        }
        if self
            .context
            .applications
            .iter()
            .any(|dir| dir.join("Devin.app").is_dir())
            && let Some(devin) = hosts.iter_mut().find(|host| host.id == "devin")
        {
            devin.label = "Devin（CLI／桌面端）";
        }
        hosts.extend(rows.into_iter().map(&build));
        hosts
    }
}
