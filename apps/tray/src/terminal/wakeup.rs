//! App presence, Codex structured inbox push (ADR-0105), and background unread scan (ADR-0114).
#[cfg(unix)]
use super::codex_server::{Client as CodexClient, Delivery};
#[cfg(any(unix, test))]
use super::unread::Item;
use super::unread::{self, RawBoss, UnreadNotice};
use super::{Terminals, lifecycle, links};
#[cfg(unix)]
use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

static STOP: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
pub(super) const PROVIDERS: &[&str] = &["codex"];
#[cfg(not(unix))]
pub(super) const PROVIDERS: &[&str] = &[];

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
    pub(super) failed: HashSet<String>,
    #[cfg(unix)]
    queued: Vec<QueuedWake>,
    pub(super) unread: unread::Tracker,
}

static COORDINATOR: OnceLock<Mutex<Option<super::spawn_requests::Coordinator>>> = OnceLock::new();

fn coordinator_slot() -> &'static Mutex<Option<super::spawn_requests::Coordinator>> {
    COORDINATOR.get_or_init(|| Mutex::new(None))
}

#[cfg(all(test, unix))]
pub(super) fn set_coordinator(coordinator: super::spawn_requests::Coordinator) {
    *coordinator_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(coordinator);
}

fn coordinator() -> Option<super::spawn_requests::Coordinator> {
    coordinator_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

pub(super) fn on_terminal_closed(invocation_id: &str) {
    if let Some(coord) = coordinator() {
        coord.on_terminal_closed(invocation_id);
    }
}

/// Publishes a derived terminal's new process before its CLI can speak to the service.
pub(super) fn sync_derived_pids(terminals: &Terminals) {
    if let Some(coord) = coordinator() {
        let _ = coord.sync_derived_pids(terminals);
    }
}

pub(super) fn start(app: AppHandle) {
    STOP.store(false, Ordering::SeqCst);
    let root = crate::data_files::data_root();
    super::spawn_requests::ensure_spawn_requests_dir(&root);
    let pid = std::process::id();
    let started_at = super::spawn_requests::current_process_started_at();
    let terminals = app.state::<Terminals>().inner().clone();
    let coordinator = super::spawn_requests::Coordinator::with_terminals(
        root.clone(),
        pid,
        started_at,
        PROVIDERS,
        terminals.clone(),
    );
    if let Err(error) = coordinator.publish() {
        eprintln!("无法登记终端推送：{error}");
        return;
    }
    *coordinator_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(coordinator.clone());
    thread::spawn(move || {
        terminals.prune_saved_launches(&root);
        let mut state = WakeupState::default();
        while !STOP.load(Ordering::SeqCst) {
            coordinator.poll(&terminals);
            let now = Instant::now();
            let seen = scan_with(&terminals, &root, &mut state, now, |notice| {
                crate::alerts::notify_unread(&app, notice.count, notice.derived);
            });
            lifecycle::tick(&terminals, &coordinator, &root, &seen, now);
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

fn prune_state(raw: &HashMap<String, RawBoss>, state: &mut WakeupState) {
    let live_keys: HashSet<&str> = raw
        .values()
        .filter(|boss| boss.has_unread)
        .flat_map(|boss| boss.items.iter().map(|i| i.key.as_str()))
        .collect();
    state.attempted.retain(|k| live_keys.contains(k.as_str()));
    state.failed.retain(|k| live_keys.contains(k.as_str()));
}

#[cfg(unix)]
fn fresh_unattempted(root: &Path, boss_key: &str, attempted: &HashSet<String>) -> Vec<Item> {
    let raw = unread::inboxes(root);
    let Some(boss) = raw.get(boss_key) else {
        return Vec::new();
    };
    if boss.waiting || !boss.has_unread {
        return Vec::new();
    }
    let mut unique = HashSet::new();
    boss.items
        .iter()
        .filter(|i| !attempted.contains(&i.key) && unique.insert(i.key.clone()))
        .cloned()
        .collect()
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
        let raw_items = raw
            .get(&q.boss_key)
            .filter(|b| b.has_unread)
            .map_or(&[][..], |b| b.items.as_slice());
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

#[cfg(unix)]
struct Candidate {
    id: u64,
    boss_key: String,
    items: Vec<Item>,
    codex: Arc<CodexClient>,
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
    let mut delivered = Vec::new();
    let outcome = candidate.codex.deliver_if_fresh(&candidate.boss_key, || {
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

#[cfg(unix)]
fn push_codex(
    terminals: &Terminals,
    root: &Path,
    links: &HashMap<u64, String>,
    raw: &HashMap<String, RawBoss>,
    state: &mut WakeupState,
) -> bool {
    reconcile_codex_queued(terminals, links, raw, state);
    let candidates: Vec<_> = {
        let open = terminals.lock();
        open.open
            .iter()
            .filter(|r| r.derived.is_none() && PROVIDERS.contains(&r.provider.as_str()))
            .filter_map(|r| {
                let codex = r.codex.clone()?;
                let boss_key = links.get(&r.id)?.clone();
                let boss = raw.get(&boss_key)?;
                if boss.waiting || !boss.has_unread || boss.items.is_empty() {
                    return None;
                }
                Some(Candidate {
                    id: r.id,
                    boss_key,
                    items: boss.items.clone(),
                    codex,
                })
            })
            .collect()
    };
    let mut delivered_any = false;
    for candidate in candidates {
        if candidate
            .items
            .iter()
            .all(|i| state.attempted.contains(&i.key))
        {
            continue;
        }
        if deliver_codex(root, &candidate, state) {
            delivered_any = true;
        }
    }
    delivered_any
}

#[cfg(all(test, unix))]
pub(super) fn scan(terminals: &Terminals, root: &Path, state: &mut WakeupState) {
    scan_with(terminals, root, state, Instant::now(), |_| {});
}

/// What a scan saw, for the lifecycle of derived terminals.
pub(super) struct Observed {
    /// The boss chats each live terminal's CLI holds, the current one last.
    pub(super) chats: HashMap<u64, Vec<String>>,
    pub(super) inboxes: HashMap<String, RawBoss>,
}

pub(super) fn scan_with(
    terminals: &Terminals,
    root: &Path,
    state: &mut WakeupState,
    now: Instant,
    notify: impl FnMut(UnreadNotice),
) -> Observed {
    let chats = links::chats(root, &terminals.live_pids());
    let links: HashMap<u64, String> = chats
        .iter()
        .filter_map(|(id, keys)| Some((*id, keys.last()?.clone())))
        .collect();
    let raw = unread::inboxes(root);
    prune_state(&raw, state);
    #[cfg(unix)]
    let raw = if push_codex(terminals, root, &links, &raw, state) {
        unread::inboxes(root)
    } else {
        raw
    };
    state.unread.scan(terminals, &links, &raw, now, notify);
    Observed {
        chats,
        inboxes: raw,
    }
}

#[cfg(any(unix, test))]
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(60).collect()
}

#[cfg(any(unix, test))]
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
                    Some("blocked" | "execution_blocked") => "受阻",
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
