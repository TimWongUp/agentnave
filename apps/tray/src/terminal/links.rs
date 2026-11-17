//! Which workspaces a terminal's CLI is the boss of (ADR-0101). A boss reaches the service through
//! a relay its CLI starts; the relay's connection file, named by the relay's pid, lists the boss
//! chats it spoke for. A relay inside a terminal's process tree speaks for that terminal, and each
//! chat's workspace is the one it owns in the history database.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// How far up from a relay a terminal's CLI is looked for.
const DEPTH: usize = 64;

/// For each terminal (id, CLI pid), its workspaces, the one it spoke for last at the end.
pub(super) fn workspaces(root: &Path, terminals: &[(u64, u32)]) -> HashMap<u64, Vec<String>> {
    if terminals.is_empty() || !sysinfo::IS_SUPPORTED_SYSTEM {
        return HashMap::new();
    }
    let relays = relays(&root.join("connections"));
    if relays.is_empty() {
        return HashMap::new();
    }
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut chats: HashMap<u64, Vec<String>> = HashMap::new();
    for (relay, bosses) in relays {
        if let Some(id) = terminal_of(&system, relay, terminals) {
            chats.entry(id).or_default().extend(bosses);
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

/// The running relays and the boss chats each one spoke for.
fn relays(dir: &Path) -> Vec<(u32, Vec<String>)> {
    if dir.is_symlink() {
        return Vec::new();
    }
    let read = |path: &Path| -> Option<(u32, Vec<String>)> {
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
        (!bosses.is_empty()).then_some((pid, bosses))
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

fn terminal_of(system: &System, relay: u32, terminals: &[(u64, u32)]) -> Option<u64> {
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
        if let Some(workspace) = found {
            workspaces.retain(|seen| *seen != workspace);
            workspaces.push(workspace);
        }
    }
    workspaces
}
