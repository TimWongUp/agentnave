//! Rows beyond one per host: an entry that reads another config than its CLI gets its own, because
//! connecting one would not reach the other. Claude Desktop drops `CLAUDE_CONFIG_DIR` from the
//! login shell's variables, so its Code tab stays on the default folder while the terminal follows
//! the variable. Codex Desktop inherits them, so only a further Codex folder, one per account and
//! chosen per command, needs a row. A named Claude account (ADR-0108) has a row of its own too:
//! its folder is the `CLAUDE_CONFIG_DIR` its connection is read from.
use super::{Context, Format, Host, Spec};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

const CLAUDE_DESKTOP: &str = "claude-desktop";
const T3_ANTIGRAVITY_PREFIX: &str = "antigravity@t3-";

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

/// Codex's other folders, where they are made: in home and beside `main`,
/// a `codex` or `.codex` folder, or one folder down in such a folder (`~/.codex-profiles/work`),
/// that holds a sign-in or sessions.
pub(super) fn codex_homes(home: &Path, main: &Path) -> Vec<PathBuf> {
    let holds = |dir: &Path| dir.join("auth.json").is_file() || dir.join("sessions").is_dir();
    let mut candidates = Vec::new();
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
                candidates.push(dir);
                continue;
            }
            for inner in sorted(&dir) {
                if holds(&inner) {
                    candidates.push(inner);
                }
            }
        }
    }
    // Prefer a writable physical path before deduplication can hide it behind an alias.
    candidates.sort_by_key(|dir| {
        dir.ancestors()
            .any(|path| path != home && path.is_symlink())
    });
    let mut seen = HashSet::from([real(main)]);
    candidates.retain(|dir| seen.insert(real(dir)));
    candidates
}

/// Claude Desktop is there if its installation or user data can be found.
fn claude_desktop_here(context: &Context) -> bool {
    if super::installed::desktop(context, "claude").is_some() {
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

/// T3 Code isolates Antigravity under `<T3CODE_HOME>/userdata/providers/antigravity/<profile>/`
/// with its own `config/mcp_config.json` and `antigravity-acp/` data directory.
pub(super) fn t3_antigravity_homes(
    context: &Context,
    shell: Option<&super::Shell>,
    main: &Path,
) -> Vec<PathBuf> {
    let t3_home = super::effective_env(context, shell, "T3CODE_HOME")
        .map(|(dir, _)| PathBuf::from(dir))
        .unwrap_or_else(|| context.home.join(".t3"));
    let root = t3_home.join("userdata/providers/antigravity");
    if root.is_symlink() || !root.is_dir() {
        return Vec::new();
    }
    let mut seen = HashSet::from([real(main)]);
    let mut candidates = Vec::new();
    for profile in sorted(&root) {
        if profile.is_symlink() || !profile.is_dir() {
            continue;
        }
        let Some(name) = profile.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            continue;
        }
        let acp = profile.join("antigravity-acp");
        if acp.is_symlink() || !acp.is_dir() {
            continue;
        }
        let config_dir = profile.join("config");
        if config_dir.is_symlink() {
            continue;
        }
        if seen.insert(real(&config_dir)) {
            candidates.push(config_dir);
        }
    }
    candidates
}

fn t3_antigravity_slugs(dirs: &[PathBuf]) -> Vec<String> {
    let names: Vec<String> = dirs
        .iter()
        .map(|dir| {
            dir.parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let shorts: Vec<&str> = names
        .iter()
        .map(|name| &name[..name.len().min(8)])
        .collect();
    names
        .iter()
        .zip(shorts.iter())
        .map(|(name, short)| {
            if shorts.iter().filter(|other| *other == short).count() > 1 {
                name.clone()
            } else {
                (*short).to_owned()
            }
        })
        .collect()
}

pub(super) fn rows(
    context: &Context,
    hosts: &[Host],
    codex_homes: &[PathBuf],
    t3_antigravity_homes: &[PathBuf],
) -> Vec<Spec> {
    let home = &context.home;
    let mut rows: Vec<Spec> = Vec::new();
    let claude = hosts.iter().find(|host| host.id == "claude");
    let default = home.join(".claude");
    // A variable the app itself was started with reaches Claude Desktop the same way.
    if super::roots::env_value(&context.env, "CLAUDE_CONFIG_DIR").is_none()
        && claude.is_some_and(|host| real(&host.config) != real(&home.join(".claude.json")))
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
    let codex_label = hosts
        .iter()
        .find(|host| host.id == "codex")
        .map_or("Codex CLI", |host| host.label);
    for dir in codex_homes {
        let folder = dir.strip_prefix(home).unwrap_or(dir).display().to_string();
        rows.push((
            interned(format!("codex@{folder}")),
            interned(format!("{codex_label}（{folder}）")),
            "codex",
            None,
            dir.clone(),
            "config.toml",
            Format::Toml,
            "mcp_servers",
        ));
    }
    let agy_label = hosts
        .iter()
        .find(|host| host.id == "antigravity")
        .map_or("Antigravity", |host| host.label);
    let slugs = t3_antigravity_slugs(t3_antigravity_homes);
    for (dir, slug) in t3_antigravity_homes.iter().zip(slugs) {
        let label = if t3_antigravity_homes.len() == 1 {
            interned(format!("{agy_label}（T3 Code）"))
        } else {
            interned(format!("{agy_label}（T3 Code · {slug}）"))
        };
        rows.push((
            interned(format!("{T3_ANTIGRAVITY_PREFIX}{slug}")),
            label,
            "agy",
            None,
            dir.clone(),
            "mcp_config.json",
            Format::Json,
            "mcpServers",
        ));
    }
    let claude_label = claude.map_or("Claude", |host| host.label);
    // An unreadable accounts file adds no row; the accounts page names the problem.
    for account in crate::accounts::read(&context.data).unwrap_or_default() {
        rows.push((
            interned(format!("claude@{}", account.id)),
            interned(format!("{claude_label}（{}）", account.label)),
            "claude",
            None,
            account.dir,
            ".claude.json",
            Format::Json,
            "mcpServers",
        ));
    }
    rows
}

/// Whether the row is a named account's: its folder is chosen by the user, not found by the app.
pub(super) fn is_account_row(id: &str) -> bool {
    id.starts_with("claude@")
}

/// Whether the row is a T3 Code isolated Antigravity profile.
pub(super) fn is_t3_antigravity_row(id: &str) -> bool {
    id.starts_with(T3_ANTIGRAVITY_PREFIX)
}
