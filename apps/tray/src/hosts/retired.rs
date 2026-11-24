//! What AgentNave once wrote into the configuration of a CLI it no longer supports (ADR-0119).
//! The App removes it once at start-up and uninstalling covers it too; only the paths that
//! removal needs are kept, and only what AgentNave wrote goes.
use super::*;

const DEVIN: &str = "devin";

impl Hosts {
    /// Devin's row reduced to what removal touches: `mcp_config.json`, `skills` and `AGENTS.md`
    /// of its configuration folder.
    pub(super) fn retired_devin(&self) -> Host {
        let env = |var| {
            let absolute =
                |value: &String| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
            roots::env_value(&self.context.env, var)
                .and_then(absolute)
                .or_else(|| {
                    let vars = self.shell.as_ref()?.as_ref().ok()?;
                    vars.get(var).and_then(absolute)
                })
        };
        let home = &self.context.home;
        let folder = if cfg!(windows) {
            roots::env_value(&self.context.env, "APPDATA")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join("AppData/Roaming"))
        } else {
            env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"))
        };
        let root = roots::resolve(
            &self.context,
            &self.settings,
            self.shell.as_ref(),
            DEVIN,
            None,
            folder.join(DEVIN),
        );
        let config = root.path.join("mcp_config.json");
        Host {
            home: home.clone(),
            id: DEVIN,
            boss: DEVIN,
            label: "Devin",
            binary: DEVIN,
            skill: Some(root.path.join("skills/agentnave-manager")),
            root,
            config: config.clone(),
            format: Format::Json,
            key: "mcpServers",
            user_layers: vec![config],
        }
    }

    /// Takes AgentNave's MCP entry, Skills and rules block out of Devin's configuration. At
    /// start-up that happens once, recorded in `hosts.json` after it succeeded; uninstalling
    /// always does it. Returns what could not be removed.
    pub(crate) fn remove_retired(&mut self) -> Vec<Value> {
        // A preview build never writes another CLI's configuration or the App's data.
        if crate::PREVIEW {
            return Vec::new();
        }
        let startup = !self.uninstalling;
        if startup && (self.settings_error.is_some() || self.settings[DEVIN]["removed"] == true) {
            return Vec::new();
        }
        let mut host = self.retired_devin();
        // Without a confirmed root the folder may be somewhere else: try again next start.
        if startup && !host.root.confirmed {
            return Vec::new();
        }
        // An entry under that name that is not AgentNave's own stays.
        if host.read().is_ok_and(|document| {
            document
                .entry
                .is_some_and(|entry| !managed_entry(&host, &entry))
        }) {
            host.user_layers.clear();
        }
        let mut failures = Vec::new();
        // Like uninstalling, remove only the App's own unmodified Skill copies.
        let uninstalling = std::mem::replace(&mut self.uninstalling, true);
        let removed = self.remove_host(DEVIN, &host);
        self.uninstalling = uninstalling;
        if let Err(error) = removed {
            failures.push(json!({"host": host.label, "error": message(&error)}));
        }
        let rules = host.root.path.join("AGENTS.md");
        let blocked = crate::rules::sync(
            &[],
            &[],
            &self.context.home,
            &json!({"preset": "off"}),
            &[rules],
            &self.rules_backup(),
            false,
        );
        failures.extend(
            blocked
                .into_iter()
                .map(|failure| json!({"host": failure["path"], "error": failure["error"]})),
        );
        if startup && failures.is_empty() {
            let mut settings = self.settings.clone();
            settings[DEVIN]["removed"] = json!(true);
            match roots::save(&self.context, &settings) {
                Ok(()) => self.settings = settings,
                Err(error) => failures.push(json!({"host": host.label, "error": error})),
            }
        }
        failures
    }
}
