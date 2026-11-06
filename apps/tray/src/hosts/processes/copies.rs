//! Copy provenance from one process snapshot; never used for authorization or ownership.
use super::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
struct Node {
    parent: Option<u32>,
    since: u64,
    exe: String,
    args: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Snapshot {
    nodes: HashMap<u32, Node>,
    hosts: HashMap<&'static str, Vec<u32>>,
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
    pub fn new(system: &System, hosts: HashMap<&'static str, Vec<u32>>) -> Self {
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
            chain.push(node);
            let Some(parent) = node.parent else {
                break;
            };
            pid = parent;
        }
        classify(host, &chain)
    }

    pub fn old_copies(&self, host: &str, before: u64) -> Vec<Copy> {
        let mut copies = BTreeMap::new();
        for pid in self.hosts.get(host).into_iter().flatten() {
            if let Some(copy) = self.classify(host, *pid, false)
                && copy.since > 0
                && copy.since <= before
            {
                insert(&mut copies, copy);
            }
        }
        copies.into_values().collect()
    }
}

pub(in crate::hosts) fn insert(copies: &mut BTreeMap<&'static str, Copy>, copy: Copy) {
    if copies
        .get(copy.kind)
        .is_none_or(|old| copy.since < old.since)
    {
        copies.insert(copy.kind, copy);
    }
}

fn copy(kind: &'static str, node: &Node, bundle: Option<String>) -> Copy {
    Copy {
        kind,
        since: node.since,
        bundle,
    }
}

fn classify(host: &str, chain: &[&Node]) -> Option<Copy> {
    if host == "codex" && chain.first().is_some_and(|n| daemon_helper(n.args.iter())) {
        return None;
    }
    let mut special = None;
    let mut desktop = None;
    let mut cli = None;
    for (index, node) in chain.iter().enumerate() {
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
        let paths =
            std::iter::once(node.exe.as_str()).chain(args.iter().take(2).map(String::as_str));
        for path in paths {
            let lower = path.to_ascii_lowercase();
            let belongs = super::identity(path) == Some(host);
            let origin = belongs || (index == 0 && Path::new(path).is_absolute());
            if belongs && cli.is_none() {
                cli = Some(copy("cli", node, None));
            }
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
        .or(cli)
        .or_else(|| chain.first().map(|node| copy("cli", node, None)))
}

#[cfg(test)]
mod tests;
