//! TRAE Desktop and CLI have separate configuration roots and share one provider identity.
use super::*;

#[cfg(target_os = "macos")]
impl Hosts {
    pub(super) fn trae_desktop(&self) -> Host {
        let home = &self.context.home;
        let mut root = roots::resolve(
            &self.context,
            &self.settings,
            self.shell.as_ref(),
            "trae-desktop",
            None,
            home.join("Library/Application Support/Trae CN/User"),
        );
        root.confirmed |= self.manual_roots.contains("trae-desktop");
        let config = root.path.join("mcp.json");
        Host {
            home: home.clone(),
            id: "trae-desktop",
            boss: "trae",
            label: "TRAE CN",
            binary: "Electron",
            config: config.clone(),
            skill: Some(home.join(".trae-cn/skills/agentnave-manager")),
            root,
            format: Format::Json,
            key: "mcpServers",
            user_layers: vec![config],
        }
    }
}

pub(super) fn detect_desktop(context: &Context) -> (bool, &'static str, Option<PathBuf>) {
    let path = context
        .applications
        .iter()
        .map(|dir| dir.join("Trae CN.app"))
        .find(|app| executable(&app.join("Contents/MacOS/Electron")));
    (path.is_some(), "app", path)
}

pub(super) fn rules_root(host: &Host) -> PathBuf {
    if host.id == "trae-desktop" {
        host.home.join(".trae-cn")
    } else {
        host.root.path.clone()
    }
}

pub(super) fn process_id(host: &Host) -> &str {
    if host.boss == "trae" {
        host.id
    } else {
        host.boss
    }
}
