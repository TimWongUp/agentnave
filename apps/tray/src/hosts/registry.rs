use super::*;

fn active_hermes_profile(root: &Root) -> Option<PathBuf> {
    if root.source == "user"
        || root
            .path
            .parent()?
            .file_name()
            .is_some_and(|name| name == "profiles")
    {
        return None;
    }
    let name = fs::read_to_string(root.path.join("active_profile")).ok()?;
    let name = name.trim().trim_start_matches('\u{feff}');
    if name == "default"
        || name.is_empty()
        || name.len() > 64
        || !name.bytes().next()?.is_ascii_lowercase()
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return None;
    }
    let profile = root.path.join("profiles").join(name);
    (profile.is_dir() && !root.path.join("profiles/.deleted").join(name).exists())
        .then_some(profile)
}

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
                .or_else(|| {
                    self.shell
                        .as_ref()
                        .and_then(|shell| shell.as_ref().ok())
                        .and_then(|vars| vars.get("XDG_CONFIG_HOME"))
                })
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
                "Claude",
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
                Some("WORKBUDDY_CONFIG_DIR"),
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
                "Devin",
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
            if id == "hermes"
                && let Some(profile) = active_hermes_profile(&root)
            {
                root.path = profile;
            }
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
        hosts.extend(rows.into_iter().map(&build));
        hosts
    }
}
