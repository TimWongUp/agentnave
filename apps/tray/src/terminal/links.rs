//! Which workspaces a terminal's CLI is the boss of (ADR-0103). A boss reaches the service through
//! a relay its CLI starts; the relay's connection file, named by the relay's pid, lists the boss
//! chats it spoke for. A relay inside a terminal's process tree speaks for that terminal, and each
//! chat's workspace is the one it owns in the history database.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// How far up from a relay a terminal's CLI is looked for.
const DEPTH: usize = 64;

/// A live-looking connection receipt for a relay process.
pub(super) struct Relay {
    pub pid: u32,
    pub started_ms: Option<u64>,
    pub bosses: Vec<String>,
    modified: Option<std::time::SystemTime>,
}

/// Matches `hosts/processes.rs`: a PID reused after a relay started is treated as exited,
/// allowing two seconds of clock/resolution slack before declaring mismatch.
pub(super) fn same_process(process: &sysinfo::Process, started_ms: Option<u64>) -> bool {
    let at = process.start_time();
    !(at != 0
        && started_ms
            .is_some_and(|started| started != 0 && at > (started / 1000).saturating_add(2)))
}

/// A fresh snapshot holding only these PIDs, never the whole process table.
pub(super) fn snapshot(pids: &[u32]) -> System {
    let mut system = System::new();
    refresh(&mut system, pids, true);
    system
}

fn refresh(system: &mut System, pids: &[u32], remove_dead: bool) {
    if pids.is_empty() {
        return;
    }
    let pids: Vec<Pid> = pids.iter().copied().map(Pid::from_u32).collect();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        remove_dead,
        ProcessRefreshKind::nothing(),
    );
}

/// A snapshot of each relay and its ancestors up to the terminal that owns it, looked up one
/// generation at a time instead of scanning the whole process table.
fn ancestry(relays: &[Relay], terminals: &[(u64, u32)]) -> System {
    let mut system = System::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut frontier: Vec<u32> = relays
        .iter()
        .map(|relay| relay.pid)
        .filter(|pid| seen.insert(*pid))
        .collect();
    for _ in 0..DEPTH {
        if frontier.is_empty() {
            break;
        }
        refresh(&mut system, &frontier, false);
        let mut parents = Vec::new();
        for pid in frontier {
            if terminals.iter().any(|(_, cli)| *cli == pid) {
                continue;
            }
            if let Some(parent) = system.process(Pid::from_u32(pid)).and_then(|p| p.parent())
                && seen.insert(parent.as_u32())
            {
                parents.push(parent.as_u32());
            }
        }
        frontier = parents;
    }
    system
}

/// For each terminal (id, CLI pid), its workspaces, the one it spoke for last at the end.
pub(super) fn workspaces(root: &Path, terminals: &[(u64, u32)]) -> HashMap<u64, Vec<String>> {
    if terminals.is_empty() || !sysinfo::IS_SUPPORTED_SYSTEM {
        return HashMap::new();
    }
    let relays = relays(&root.join("connections"));
    if relays.is_empty() {
        return HashMap::new();
    }
    let system = ancestry(&relays, terminals);
    let mut chats: HashMap<u64, Vec<String>> = HashMap::new();
    for relay in relays {
        if let Some(process) = system.process(Pid::from_u32(relay.pid))
            && same_process(process, relay.started_ms)
            && let Some(id) = terminal_of(&system, relay.pid, terminals)
        {
            chats.entry(id).or_default().extend(relay.bosses);
        }
    }
    let Ok(Some(connection)) = crate::db::read(root) else {
        return HashMap::new();
    };
    chats
        .into_iter()
        .map(|(id, bosses)| (id, owned(&connection, &bosses)))
        .filter(|(_, workspaces)| !workspaces.is_empty())
        .collect()
}

/// Boss chat keys by live terminal, before translating them into workspaces; the current one last.
pub(super) fn chats(root: &Path, terminals: &[(u64, u32)]) -> HashMap<u64, Vec<String>> {
    if terminals.is_empty() || !sysinfo::IS_SUPPORTED_SYSTEM {
        return HashMap::new();
    }
    let mut active = relays(&root.join("connections"));
    if active.is_empty() {
        return HashMap::new();
    }
    active.sort_by_key(|relay| relay.modified);
    let system = ancestry(&active, terminals);
    let mut found: HashMap<u64, Vec<String>> = HashMap::new();
    for relay in active {
        if let Some(process) = system.process(Pid::from_u32(relay.pid))
            && same_process(process, relay.started_ms)
            && let Some(id) = terminal_of(&system, relay.pid, terminals)
        {
            found.entry(id).or_default().extend(relay.bosses);
        }
    }
    found
}

/// The running relays and the boss chats each one spoke for.
pub(super) fn relays(dir: &Path) -> Vec<Relay> {
    if dir.is_symlink() {
        return Vec::new();
    }
    let read = |path: &Path| -> Option<Relay> {
        let pid = path
            .file_name()?
            .to_str()?
            .split_once('-')?
            .0
            .parse()
            .ok()?;
        let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if !value["ended_ms"].is_null() {
            return None;
        }
        let bosses: Vec<String> = value["bosses"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter(|key| boss_key(key))
            .map(str::to_owned)
            .collect();
        (!bosses.is_empty()).then_some(Relay {
            pid,
            started_ms: value["started_ms"].as_u64(),
            bosses,
            modified: fs::metadata(path).and_then(|m| m.modified()).ok(),
        })
    };
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| read(&path))
        .collect()
}

fn boss_key(key: &str) -> bool {
    key.strip_prefix("s-")
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(super) fn terminal_of(system: &System, relay: u32, terminals: &[(u64, u32)]) -> Option<u64> {
    let mut current = system.process(Pid::from_u32(relay));
    for _ in 0..DEPTH {
        let process = current?;
        let pid = process.pid().as_u32();
        if let Some((id, _)) = terminals.iter().find(|(_, cli)| *cli == pid) {
            return Some(*id);
        }
        current = process.parent().and_then(|parent| system.process(parent));
    }
    None
}

/// Appends workspaces to what is already remembered, keeping each once with the latest last.
pub(super) fn remember(known: &mut Vec<String>, observed: impl IntoIterator<Item = String>) {
    for workspace in observed {
        known.retain(|seen| *seen != workspace);
        known.push(workspace);
    }
}

/// The workspaces these boss chats own, in the order given. A chat saved before workspaces were
/// recorded owns the one named by its own key.
fn owned(connection: &rusqlite::Connection, bosses: &[String]) -> Vec<String> {
    let mut workspaces: Vec<String> = Vec::new();
    for boss in bosses {
        let found: Option<String> = connection
            .query_row(
                "SELECT id FROM workspaces WHERE owner = ?1 \
                 UNION ALL SELECT ?1 WHERE EXISTS \
                 (SELECT 1 FROM conversations WHERE manager_session = ?1) LIMIT 1",
                [boss],
                |row| row.get(0),
            )
            .ok();
        remember(&mut workspaces, found);
    }
    workspaces
}

/// The boss chat of a workspace: its host, key and session record when the history knows them. A
/// chat saved before workspaces were recorded is keyed by the workspace itself.
pub(super) fn chat(root: &Path, workspace: &str) -> Option<(String, String, Option<PathBuf>)> {
    let connection = crate::db::read(root).ok()??;
    connection
        .query_row(
            "SELECT owner_host, owner, owner_file FROM workspaces WHERE id = ?1 \
             UNION ALL SELECT host, manager_session, NULL FROM conversations \
             WHERE manager_session = ?1 LIMIT 1",
            [workspace],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .ok()
        .and_then(|(host, key, file)| Some((host?, key, file.map(PathBuf::from))))
}

/// The account the workspace's boss chat runs as: the `owner_account` the service recorded, None
/// for the default account or before the history knows the column (schema 5).
pub(super) fn owner_account(root: &Path, workspace: &str) -> Option<String> {
    let connection = crate::db::read(root).ok()??;
    let known: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'owner_account')",
            [],
            |row| row.get(0),
        )
        .ok()?;
    if !known {
        return None;
    }
    connection
        .query_row(
            "SELECT owner_account FROM workspaces WHERE id = ?1",
            [workspace],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .filter(|account| !account.is_empty())
}
