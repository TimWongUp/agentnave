//! Rows beyond one per host: an entry that reads another config than its CLI gets its own, because
//! connecting one would not reach the other. Claude Desktop drops `CLAUDE_CONFIG_DIR` from the
//! login shell's variables, so its Code tab stays on the default folder while the terminal follows
//! the variable. Codex Desktop inherits them, so only a further Codex folder, one per account and
//! chosen per command, needs a row.
use super::{Context, Format, Host, Spec};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

const CLAUDE_DESKTOP: &str = "claude-desktop";

/// The host a row's connections run as; its service knows no other names.
pub(super) fn boss_of(id: &'static str) -> &'static str {
    match id {
        CLAUDE_DESKTOP => "claude",
        id => id.split_once('@').map_or(id, |(boss, _)| boss),
    }
}

/// Further rows are named `<boss>@<folder>`, found while the app runs.
fn interned(text: String) -> &'static str {
    static SEEN: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(known) = seen.get(text.as_str()) {
        return known;
    }
    let text: &'static str = Box::leak(text.into_boxed_str());
    seen.insert(text);
    text
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    real(a) == real(b)
}

/// Every Codex folder under home, where they are made: a `codex` or `.codex` folder, or one
/// folder down in such a folder (`~/.codex-profiles/work`), that holds a sign-in or sessions.
pub(super) fn codex_homes(home: &Path) -> Vec<PathBuf> {
    let holds = |dir: &Path| dir.join("auth.json").is_file() || dir.join("sessions").is_dir();
    let mut found = Vec::new();
    for entry in fs::read_dir(home).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let dir = entry.path();
        if !name.trim_start_matches('.').starts_with("codex") || !dir.is_dir() {
            continue;
        }
        if holds(&dir) {
            found.push(dir);
            continue;
        }
        for inner in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if holds(&inner.path()) {
                found.push(inner.path());
            }
        }
    }
    found.sort();
    found
}

pub(super) fn rows(context: &Context, hosts: &[Host], codex_homes: &[PathBuf]) -> Vec<Spec> {
    let home = &context.home;
    let mut rows: Vec<Spec> = Vec::new();
    let claude = hosts.iter().find(|host| host.id == "claude");
    let default = home.join(".claude");
    // A variable the app itself was started with reaches Claude Desktop the same way.
    if !context.env.contains_key("CLAUDE_CONFIG_DIR")
        && claude.is_some_and(|host| !same_dir(&host.root.path, &default))
        && context
            .applications
            .iter()
            .any(|dir| dir.join("Claude.app").is_dir())
    {
        rows.push((
            CLAUDE_DESKTOP,
            "Claude 桌面",
            "claude",
            None,
            default,
            ".claude.json",
            Format::Json,
            "mcpServers",
        ));
    }
    let codex = hosts.iter().find(|host| host.id == "codex");
    for dir in codex_homes {
        if codex.is_some_and(|host| same_dir(&host.root.path, dir)) {
            continue;
        }
        let folder = dir.strip_prefix(home).unwrap_or(dir).display().to_string();
        rows.push((
            interned(format!("codex@{folder}")),
            interned(format!("Codex（{folder}）")),
            "codex",
            None,
            dir.clone(),
            "config.toml",
            Format::Toml,
            "mcp_servers",
        ));
    }
    rows
}
