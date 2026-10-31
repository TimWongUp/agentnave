//! Rows beyond one per host: an entry that reads another config than its CLI gets its own, because
//! connecting one would not reach the other. Claude Desktop drops `CLAUDE_CONFIG_DIR` from the
//! login shell's variables, so its Code tab stays on the default folder while the terminal follows
//! the variable. Codex Desktop inherits them, so only a further Codex folder, one per account and
//! chosen per command, needs a row.
use super::{Context, Format, Host, Spec};
use std::{
    collections::{BTreeSet, HashSet},
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

fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// Sorted, so a folder reached by two names keeps the same row from one look to the next.
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    let mut paths: Vec<_> = entries.map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// Codex's other folders, by magpie's `CodexHomes`, where they are made: in home and beside `main`,
/// a `codex` or `.codex` folder, or one folder down in such a folder (`~/.codex-profiles/work`),
/// that holds a sign-in or sessions.
pub(super) fn codex_homes(home: &Path, main: &Path) -> Vec<PathBuf> {
    let holds = |dir: &Path| dir.join("auth.json").is_file() || dir.join("sessions").is_dir();
    let mut seen = HashSet::from([real(main)]);
    let mut found = Vec::new();
    let mut add = |dir: PathBuf| {
        if seen.insert(real(&dir)) {
            found.push(dir);
        }
    };
    let mut roots = vec![home.to_owned()];
    roots.extend(
        main.parent()
            .filter(|beside| real(beside) != real(home))
            .map(Path::to_owned),
    );
    for root in roots {
        for dir in sorted(&root) {
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            let name = name.to_lowercase();
            if !name.strip_prefix('.').unwrap_or(&name).starts_with("codex") || !dir.is_dir() {
                continue;
            }
            if holds(&dir) {
                add(dir);
                continue;
            }
            for inner in sorted(&dir) {
                if holds(&inner) {
                    add(inner);
                }
            }
        }
    }
    found
}

/// Claude Desktop is there if its app is, or, as magpie reads it off macOS and Linux, its folders.
fn claude_desktop_here(context: &Context) -> bool {
    let app = |dir: &PathBuf| dir.join("Claude.app").is_dir();
    if context.applications.iter().any(app) {
        return true;
    }
    if cfg!(windows) {
        return false;
    }
    let config = if cfg!(target_os = "macos") {
        context.home.join("Library/Application Support")
    } else {
        context
            .env
            .get("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| context.home.join(".config"))
    };
    ["Claude", "Claude-3p"]
        .iter()
        .any(|name| config.join(name).is_dir())
}

pub(super) fn rows(context: &Context, hosts: &[Host], codex_homes: &[PathBuf]) -> Vec<Spec> {
    let home = &context.home;
    let mut rows: Vec<Spec> = Vec::new();
    let claude = hosts.iter().find(|host| host.id == "claude");
    let default = home.join(".claude");
    // A variable the app itself was started with reaches Claude Desktop the same way.
    if !context.env.contains_key("CLAUDE_CONFIG_DIR")
        && claude.is_some_and(|host| real(&host.root.path) != real(&default))
        && claude_desktop_here(context)
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
    for dir in codex_homes {
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
