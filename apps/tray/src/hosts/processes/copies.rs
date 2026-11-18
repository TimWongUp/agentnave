//! Copy provenance from one process snapshot; never used for authorization or ownership.
use super::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
struct Node {
    parent: Option<u32>,
    since: u64,
    exe: String,
    args: Vec<String>,
    cwd: Option<String>,
}

#[derive(Debug)]
pub(super) struct Snapshot {
    nodes: HashMap<u32, Node>,
    hosts: HashMap<&'static str, Vec<u32>>,
}

#[derive(Default)]
pub(in crate::hosts) struct Collector {
    other: BTreeMap<&'static str, Copy>,
    cli: BTreeMap<u32, Copy>,
    cli_anon: Vec<Copy>,
}

impl Collector {
    pub fn push(&mut self, copy: Copy) {
        if copy.kind == "cli" {
            if let Some(pid) = copy.pid {
                self.cli.entry(pid).or_insert(copy);
            } else {
                self.cli_anon.push(copy);
            }
        } else if self
            .other
            .get(copy.kind)
            .is_none_or(|old| copy.since < old.since)
        {
            self.other.insert(copy.kind, copy);
        }
    }

    pub fn finish(self) -> Vec<Copy> {
        let mut cli: Vec<Copy> = self.cli.into_values().chain(self.cli_anon).collect();
        cli.sort_by_key(|c| (c.since, c.pid.unwrap_or(0)));
        let mut out: Vec<Copy> = self.other.into_values().collect();
        out.extend(cli);
        out
    }
}

pub(super) fn daemon_helper<'a>(args: impl Iterator<Item = impl AsRef<str> + 'a>) -> bool {
    let args: Vec<_> = args.map(|a| a.as_ref().to_owned()).collect();
    !args
        .iter()
        .any(|a| a == "--managed-daemon" || a.starts_with("--managed-daemon="))
        && args
            .windows(3)
            .any(|a| a[0] == "app-server" && a[1] == "daemon")
}

impl Snapshot {
    pub fn new(system: &System, home: &Path, hosts: HashMap<&'static str, Vec<u32>>) -> Self {
        Self {
            hosts,
            nodes: system
                .processes()
                .iter()
                .map(|(pid, p)| {
                    (
                        pid.as_u32(),
                        Node {
                            parent: p.parent().map(|p| p.as_u32()),
                            since: p.start_time().saturating_mul(1000),
                            exe: p
                                .exe()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            args: p
                                .cmd()
                                .iter()
                                .map(|a| a.to_string_lossy().into_owned())
                                .collect(),
                            cwd: p.cwd().and_then(|cwd| shorten_home(cwd, home)),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn classify(&self, host: &str, pid: u32, relay: bool) -> Option<Copy> {
        let mut pid = if relay {
            // An orphaned relay belongs to no host copy; launchd or init is not a terminal session.
            self.nodes.get(&pid)?.parent.filter(|parent| *parent > 1)?
        } else {
            pid
        };
        let mut seen = HashSet::new();
        let mut chain = Vec::new();
        for _ in 0..64 {
            if !seen.insert(pid) {
                break;
            }
            let Some(node) = self.nodes.get(&pid) else {
                break;
            };
            chain.push((pid, node));
            let Some(parent) = node.parent else {
                break;
            };
            pid = parent;
        }
        let host_pids: HashSet<u32> = self
            .hosts
            .get(host)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        classify(host, &chain, &host_pids)
    }

    pub fn old_copies(&self, host: &str, before: u64) -> Vec<Copy> {
        let mut copies = Collector::default();
        for pid in self.hosts.get(host).into_iter().flatten() {
            if let Some(copy) = self.classify(host, *pid, false)
                && copy.since > 0
                && copy.since <= before
            {
                copies.push(copy);
            }
        }
        copies.finish()
    }
}

fn copy(kind: &'static str, node: &Node, bundle: Option<String>) -> Copy {
    Copy {
        kind,
        since: node.since,
        bundle,
        pid: None,
        cwd: None,
        terminal: None,
    }
}

fn cli_copy(pid: u32, node: &Node, ancestors: &[(u32, &Node)]) -> Copy {
    let terminal = ancestors
        .iter()
        .find_map(|(_, parent)| terminal_name(parent));
    Copy {
        kind: "cli",
        since: node.since,
        bundle: None,
        pid: Some(pid),
        cwd: node.cwd.clone(),
        terminal,
    }
}

fn node_belongs(node: &Node, host: &str) -> bool {
    if super::identity(&node.exe) == Some(host)
        || node
            .args
            .first()
            .is_some_and(|a| super::identity(a) == Some(host))
    {
        return true;
    }
    if !super::is_script_runner(Path::new(&node.exe)) {
        return false;
    }
    node.args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .is_some_and(|a| super::identity(a) == Some(host))
}

fn terminal_name(node: &Node) -> Option<&'static str> {
    let path = if node.exe.is_empty() {
        node.args.first().map(String::as_str).unwrap_or("")
    } else {
        node.exe.as_str()
    };
    if path.is_empty() {
        return None;
    }
    let normalized = path.replace('\\', "/").to_lowercase();
    for (needle, label) in [
        ("/visual studio code - insiders.app/contents/", "VS Code"),
        ("/visual studio code.app/contents/", "VS Code"),
        ("/warp.app/contents/", "Warp"),
        ("/iterm.app/contents/", "iTerm2"),
        ("/terminal.app/contents/", "Terminal"),
        ("/ghostty.app/contents/", "Ghostty"),
        ("/wezterm.app/contents/", "WezTerm"),
        ("/alacritty.app/contents/", "Alacritty"),
        ("/kitty.app/contents/", "kitty"),
        ("/cursor.app/contents/", "Cursor"),
        ("/windsurf.app/contents/", "Windsurf"),
        ("/zed.app/contents/", "Zed"),
        ("/hyper.app/contents/", "Hyper"),
        ("/tabby.app/contents/", "Tabby"),
        ("/rio.app/contents/", "Rio"),
        ("/herdr.app/contents/", "herdr"),
    ] {
        if normalized.contains(needle) {
            return Some(label);
        }
    }
    let file = super::name(&normalized);
    let stem = file.strip_suffix(".exe").unwrap_or(file);
    match stem {
        "herdr" => Some("herdr"),
        "tmux" => Some("tmux"),
        "screen" => Some("screen"),
        "zellij" => Some("zellij"),
        "terminal" => Some("Terminal"),
        "iterm2" => Some("iTerm2"),
        s if s.starts_with("itermserver") => Some("iTerm2"),
        "ghostty" => Some("Ghostty"),
        "wezterm" | "wezterm-gui" | "wezterm-mux-server" => Some("WezTerm"),
        "alacritty" => Some("Alacritty"),
        "kitty" => Some("kitty"),
        "warp-terminal" => Some("Warp"),
        "code" | "code-insiders" => Some("VS Code"),
        "cursor" => Some("Cursor"),
        "windsurf" => Some("Windsurf"),
        "zed" | "zed-editor" => Some("Zed"),
        "gnome-terminal" | "gnome-terminal-server" => Some("GNOME Terminal"),
        "konsole" => Some("Konsole"),
        "foot" => Some("foot"),
        "xterm" => Some("xterm"),
        _ => None,
    }
}

fn classify(host: &str, chain: &[(u32, &Node)], host_pids: &HashSet<u32>) -> Option<Copy> {
    if host == "codex"
        && chain
            .first()
            .is_some_and(|(_, n)| daemon_helper(n.args.iter()))
    {
        return None;
    }
    let mut special = None;
    let mut desktop = None;
    let mut cli_match = None;
    for (index, &(node_pid, node)) in chain.iter().enumerate() {
        let args = &node.args;
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        let kind = match host {
            "claude"
                if pair("daemon", "run")
                    || args.iter().any(|a| a == "bg-spare" || a == "--bg-spare") =>
            {
                Some("daemon")
            }
            "codex"
                if args
                    .iter()
                    .any(|a| a == "--managed-daemon" || a.starts_with("--managed-daemon=")) =>
            {
                Some("daemon")
            }
            "hermes"
                if pair("gateway", "run")
                    || (super::name(&node.exe) == "osascript"
                        && args.iter().any(|a| a.contains("hermes gateway run"))) =>
            {
                Some("gateway")
            }
            _ => None,
        };
        if let Some(kind) = kind {
            special = Some(copy(kind, node, None));
        }
        let node_is_host = host_pids.contains(&node_pid) || node_belongs(node, host);
        if node_is_host {
            // Keep updating along the ancestor walk so the outermost host process represents the session.
            cli_match = Some((index, node_pid, node));
        }
        let paths =
            std::iter::once(node.exe.as_str()).chain(args.iter().take(2).map(String::as_str));
        for path in paths {
            let lower = path.to_ascii_lowercase();
            let belongs = host_pids.contains(&node_pid) || super::identity(path) == Some(host);
            let origin = belongs || (index == 0 && Path::new(path).is_absolute());
            if host == "codex" && origin && lower.contains("/extensions/") {
                desktop = Some(copy("ide", node, None));
            } else if origin && let Some(end) = lower.find(".app/contents/") {
                let kind = if host == "claude" { "desktop" } else { "app" };
                // The first bundle is the outer app, even for nested CodexCLI.app helpers.
                if desktop.as_ref().is_none_or(|c| c.kind != "ide") {
                    desktop = Some(copy(kind, node, Some(path[..end + 4].to_owned())));
                }
            }
        }
    }
    special
        .or(desktop)
        .or_else(|| cli_match.map(|(index, pid, node)| cli_copy(pid, node, &chain[index + 1..])))
        .or_else(|| {
            chain
                .first()
                .map(|&(pid, node)| cli_copy(pid, node, &chain[1..]))
        })
}

#[cfg(test)]
mod tests;
