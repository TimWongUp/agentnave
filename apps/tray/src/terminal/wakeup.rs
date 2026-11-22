//! App presence and background inbox delivery.
#[cfg(unix)]
use super::codex_server::{Client as CodexClient, Delivery};
use super::{Terminals, links, session::Session, session::WakeupOutcome};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Manager};

static STOP: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
const PROVIDERS: &[&str] = &["claude", "grok", "antigravity", "pi", "codex"];
#[cfg(not(unix))]
const PROVIDERS: &[&str] = &["claude", "grok", "antigravity", "pi"];
#[derive(Clone, Deserialize)]
struct Item {
    key: String,
    kind: String,
    role: Option<String>,
    status: Option<String>,
}
#[derive(Deserialize)]
struct Boss {
    waiting: bool,
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Inbox {
    version: u32,
    pid: u32,
    bosses: HashMap<String, Boss>,
}

#[derive(Default)]
struct RawBoss {
    waiting: bool,
    items: Vec<Item>,
}

#[cfg(unix)]
struct QueuedWake {
    terminal_id: u64,
    boss_key: String,
    thread_id: String,
    submission_id: String,
    item_keys: Vec<String>,
}

#[derive(Default)]
pub(super) struct WakeupState {
    pub(super) attempted: HashSet<String>,
    pub(super) seen: HashMap<String, Instant>,
    pub(super) failed: HashSet<String>,
    #[cfg(unix)]
    queued: Vec<QueuedWake>,
}

static COORDINATOR: OnceLock<Mutex<Option<super::spawn_requests::Coordinator>>> = OnceLock::new();

fn coordinator_slot() -> &'static Mutex<Option<super::spawn_requests::Coordinator>> {
    COORDINATOR.get_or_init(|| Mutex::new(None))
}

pub(super) fn on_terminal_closed(invocation_id: &str) {
    let coord = coordinator_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(coord) = coord {
        coord.on_terminal_closed(invocation_id);
    }
}

pub(super) fn start(app: AppHandle) {
    STOP.store(false, Ordering::SeqCst);
    let root = crate::data_files::data_root();
    super::spawn_requests::ensure_spawn_requests_dir(&root);
    let pid = std::process::id();
    let started_at = super::spawn_requests::current_process_started_at();
    let coordinator =
        super::spawn_requests::Coordinator::new(root.clone(), pid, started_at, PROVIDERS);
    if let Err(error) = coordinator.publish() {
        eprintln!("无法登记终端推送：{error}");
        return;
    }
    *coordinator_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(coordinator.clone());
    thread::spawn(move || {
        let mut state = WakeupState::default();
        let terminals = app.state::<Terminals>().inner().clone();
        while !STOP.load(Ordering::SeqCst) {
            coordinator.poll(&terminals);
            scan(&terminals, &root, &mut state);
            thread::sleep(Duration::from_millis(250));
        }
    });
}
pub(super) fn shutdown() {
    STOP.store(true, Ordering::SeqCst);
    *coordinator_slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
    let _ = fs::remove_file(
        crate::data_files::data_root()
            .join("push")
            .join(format!("{}.json", std::process::id())),
    );
}
fn read_inbox(path: &Path, process: &sysinfo::Process) -> Option<Inbox> {
    if fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let millis = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let millis = u64::try_from(millis).ok()?;
    if process.start_time() == 0 || !links::same_process(process, Some(millis)) {
        return None;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn inboxes(root: &Path) -> HashMap<String, RawBoss> {
    let dir = root.join("inbox");
    if dir.is_symlink() {
        return HashMap::new();
    }
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut bosses: HashMap<String, RawBoss> = HashMap::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if path.extension().is_none_or(|s| s != "json") {
            continue;
        }
        let Some(process) = system.process(Pid::from_u32(pid)) else {
            continue;
        };
        let Some(inbox) = read_inbox(&path, process) else {
            continue;
        };
        if inbox.version != 1 || inbox.pid != pid {
            continue;
        }
        for (key, boss) in inbox.bosses {
            let entry = bosses.entry(key).or_default();
            entry.waiting |= boss.waiting;
            entry.items.extend(boss.items);
        }
    }
    bosses
}

fn prune_state(raw: &HashMap<String, RawBoss>, state: &mut WakeupState) {
    let live_keys: HashSet<&str> = raw
        .values()
        .flat_map(|boss| boss.items.iter().map(|i| i.key.as_str()))
        .collect();
    state.attempted.retain(|k| live_keys.contains(k.as_str()));
    state.seen.retain(|k, _| live_keys.contains(k.as_str()));
    state.failed.retain(|k| live_keys.contains(k.as_str()));
    for boss in raw.values().filter(|b| b.waiting) {
        for item in &boss.items {
            state.seen.remove(&item.key);
        }
    }
}

fn fresh_unattempted(root: &Path, boss_key: &str, attempted: &HashSet<String>) -> Vec<Item> {
    let raw = inboxes(root);
    let Some(boss) = raw.get(boss_key) else {
        return Vec::new();
    };
    if boss.waiting {
        return Vec::new();
    }
    let mut unique = HashSet::new();
    boss.items
        .iter()
        .filter(|i| !attempted.contains(&i.key) && unique.insert(i.key.clone()))
        .cloned()
        .collect()
}

fn has_any_live_unwaiting(root: &Path, boss_key: &str, selected: &[Item]) -> bool {
    let raw = inboxes(root);
    let Some(boss) = raw.get(boss_key) else {
        return false;
    };
    if boss.waiting {
        return false;
    }
    let live: HashSet<&str> = boss.items.iter().map(|i| i.key.as_str()).collect();
    selected.iter().any(|i| live.contains(i.key.as_str()))
}

#[cfg(unix)]
fn reconcile_codex_queued(
    terminals: &Terminals,
    links: &HashMap<u64, String>,
    raw: &HashMap<String, RawBoss>,
    state: &mut WakeupState,
) {
    if state.queued.is_empty() {
        return;
    }
    let open_codex: HashMap<u64, Arc<CodexClient>> = {
        let guard = terminals.lock();
        guard
            .open
            .iter()
            .filter_map(|r| Some((r.id, r.codex.clone()?)))
            .collect()
    };
    let mut kept = Vec::new();
    for mut q in std::mem::take(&mut state.queued) {
        let Some(codex) = open_codex.get(&q.terminal_id) else {
            continue;
        };
        if let Some(current_boss) = links.get(&q.terminal_id)
            && current_boss != &q.boss_key
        {
            continue;
        }
        if !links.contains_key(&q.terminal_id) || !codex.is_connected() {
            kept.push(q);
            continue;
        }
        let raw_items = raw.get(&q.boss_key).map_or(&[][..], |b| b.items.as_slice());
        let mut by_key: HashMap<&str, &Item> = HashMap::new();
        for item in raw_items {
            by_key.entry(item.key.as_str()).or_insert(item);
        }
        let remaining: Vec<Item> = q
            .item_keys
            .iter()
            .filter_map(|k| by_key.get(k.as_str()).copied().cloned())
            .collect();
        if remaining.is_empty() {
            let _ = codex.delete_queued(&q.thread_id, &q.submission_id);
        } else if remaining.len() < q.item_keys.len() {
            if codex.update_queued(&q.thread_id, &q.submission_id, &message(&remaining)) {
                q.item_keys = remaining.into_iter().map(|i| i.key).collect();
                kept.push(q);
            }
        } else {
            kept.push(q);
        }
    }
    state.queued = kept;
}

struct Candidate {
    id: u64,
    provider: String,
    session: Arc<Session>,
    boss_key: String,
    items: Vec<Item>,
    #[cfg(unix)]
    codex: Option<Arc<CodexClient>>,
}

fn pending_count(
    items: &[Item],
    attempted: &HashSet<String>,
    seen: &HashMap<String, Instant>,
    failed: &HashSet<String>,
) -> usize {
    items
        .iter()
        .filter(|i| {
            failed.contains(&i.key)
                || (!attempted.contains(&i.key)
                    && seen
                        .get(&i.key)
                        .is_some_and(|t| t.elapsed() >= Duration::from_secs(60)))
        })
        .count()
}

#[cfg(any(unix, test))]
fn event_key(selected: &[Item]) -> String {
    if selected.len() == 1 {
        selected[0].key.clone()
    } else {
        selected
            .iter()
            .map(|i| i.key.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(unix)]
fn deliver_codex(root: &Path, candidate: &Candidate, state: &mut WakeupState) -> bool {
    let Some(codex) = &candidate.codex else {
        return false;
    };
    let mut delivered = Vec::new();
    let outcome = codex.deliver_if_fresh(&candidate.boss_key, || {
        let fresh = fresh_unattempted(root, &candidate.boss_key, &state.attempted);
        if fresh.is_empty() {
            return None;
        }
        let key = event_key(&fresh);
        let msg = message(&fresh);
        delivered = fresh;
        Some((key, msg))
    });
    match outcome {
        Delivery::NotReady => false,
        Delivery::Queued {
            thread_id,
            submission_id,
        } => {
            let item_keys: Vec<String> = delivered.iter().map(|i| i.key.clone()).collect();
            for key in &item_keys {
                state.attempted.insert(key.clone());
                state.failed.remove(key);
            }
            state.queued.push(QueuedWake {
                terminal_id: candidate.id,
                boss_key: candidate.boss_key.clone(),
                thread_id,
                submission_id,
                item_keys,
            });
            true
        }
        Delivery::Started => {
            for item in &delivered {
                state.attempted.insert(item.key.clone());
                state.failed.remove(&item.key);
            }
            true
        }
        Delivery::Failed => {
            for item in &delivered {
                state.attempted.remove(&item.key);
                state.failed.insert(item.key.clone());
            }
            true
        }
    }
}

fn deliver_pty(
    root: &Path,
    boss_key: &str,
    session: &Session,
    attempted: &mut HashSet<String>,
    failed: &mut HashSet<String>,
) -> bool {
    let selected = fresh_unattempted(root, boss_key, attempted);
    if selected.is_empty() {
        return false;
    }
    let started = match session.begin_wakeup(&message(&selected)) {
        Ok(None) => return false,
        result => {
            // A failed write may have written a prefix, so it must not be retried.
            for item in &selected {
                attempted.insert(item.key.clone());
            }
            match result {
                Ok(Some(())) => true,
                Err(_) => false,
                Ok(None) => unreachable!(),
            }
        }
    };
    let outcome = if started {
        thread::sleep(Duration::from_millis(300));
        let still_needed = has_any_live_unwaiting(root, boss_key, &selected);
        session.finish_wakeup(still_needed)
    } else {
        WakeupOutcome::Failed
    };
    update_attempts(&selected, outcome, attempted, failed);
    true
}

pub(super) fn scan(terminals: &Terminals, root: &Path, state: &mut WakeupState) {
    let links = links::bosses(root, &terminals.live_pids());
    let raw = inboxes(root);
    prune_state(&raw, state);
    #[cfg(unix)]
    reconcile_codex_queued(terminals, &links, &raw, state);
    let candidates: Vec<_> = {
        let open = terminals.lock();
        open.open
            .iter()
            .filter(|r| r.derived.is_none() && PROVIDERS.contains(&r.provider.as_str()))
            .filter_map(|r| {
                let boss_key = links.get(&r.id)?.clone();
                let boss = raw.get(&boss_key)?;
                if boss.waiting || boss.items.is_empty() {
                    return None;
                }
                Some(Candidate {
                    id: r.id,
                    provider: r.provider.clone(),
                    session: r.session.clone(),
                    boss_key,
                    items: boss.items.clone(),
                    #[cfg(unix)]
                    codex: r.codex.clone(),
                })
            })
            .collect()
    };
    let mut pending = HashMap::new();
    for candidate in candidates {
        for item in &candidate.items {
            state
                .seen
                .entry(item.key.clone())
                .or_insert_with(Instant::now);
        }
        pending.insert(
            candidate.id,
            pending_count(
                &candidate.items,
                &state.attempted,
                &state.seen,
                &state.failed,
            ),
        );
        if candidate
            .items
            .iter()
            .all(|i| state.attempted.contains(&i.key))
        {
            continue;
        }
        let handled = if candidate.provider == "codex" {
            #[cfg(unix)]
            {
                deliver_codex(root, &candidate, state)
            }
            #[cfg(not(unix))]
            {
                let _ = (&candidate.boss_key, &candidate.provider);
                false
            }
        } else {
            deliver_pty(
                root,
                &candidate.boss_key,
                &candidate.session,
                &mut state.attempted,
                &mut state.failed,
            )
        };
        if handled {
            let latest = inboxes(root);
            let latest_items = latest
                .get(&candidate.boss_key)
                .filter(|b| !b.waiting)
                .map_or(&[][..], |b| b.items.as_slice());
            pending.insert(
                candidate.id,
                pending_count(latest_items, &state.attempted, &state.seen, &state.failed),
            );
        }
    }
    for running in &mut terminals.lock().open {
        running.pending = pending.get(&running.id).copied().unwrap_or(0);
    }
}
fn update_attempts(
    items: &[Item],
    outcome: WakeupOutcome,
    attempted: &mut HashSet<String>,
    failed: &mut HashSet<String>,
) {
    for item in items {
        match outcome {
            WakeupOutcome::Delivered => {
                failed.remove(&item.key);
            }
            WakeupOutcome::Blocked => {
                attempted.remove(&item.key);
                failed.insert(item.key.clone());
            }
            WakeupOutcome::Cancelled => {
                attempted.remove(&item.key);
                failed.remove(&item.key);
            }
            WakeupOutcome::Failed => {
                failed.insert(item.key.clone());
            }
        }
    }
}
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(60).collect()
}
fn message(items: &[Item]) -> String {
    let mut parts = Vec::new();
    let questions = items
        .iter()
        .filter(|i| i.kind == "prompt" || i.kind == "review")
        .count();
    if questions > 0 {
        parts.push(format!("{questions} 个待你处理的请求"));
    }
    let results: Vec<_> = items
        .iter()
        .filter(|i| i.kind != "prompt" && i.kind != "review")
        .collect();
    if !results.is_empty() {
        let labels: Vec<_> = results
            .iter()
            .map(|i| {
                let role = i
                    .role
                    .as_deref()
                    .map(clean)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "任务".into());
                let status = match i.status.as_deref() {
                    Some("succeeded") => "成功",
                    Some("failed") => "失败",
                    Some("blocked") => "受阻",
                    _ => "已结束",
                };
                format!("{role}（{status}）")
            })
            .collect();
        parts.push(format!(
            "{} 项结果就绪：{}",
            results.len(),
            labels.join("、")
        ));
    }
    format!(
        "[AgentNave] {}。请调用 AgentNave 的 wait(wait_seconds=0) 收取。",
        parts.join("，")
    )
}

#[cfg(test)]
mod tests;
