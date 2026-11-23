//! Derived-session `spawn-requests/` intake, push receipts, and inbox settlement (ADR-0109).

mod types;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::Terminals;
use super::derived;
use super::links;
use crate::data_files::{PUSH_MAX_BYTES, create_private_dir, valid_uuid, write_atomic_compact};
pub(super) use types::*;

const MAX_REQUEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 240;
const MAX_WARNINGS: usize = 8;
const MAX_REMINDER_TASKS: usize = 16;
const DEAD_RECEIPT_TTL: Duration = Duration::from_secs(60);

#[cfg(target_os = "macos")]
pub(super) const TERMINAL_PROVIDERS: &[&str] = &["claude", "codex", "antigravity"];
#[cfg(not(target_os = "macos"))]
pub(super) const TERMINAL_PROVIDERS: &[&str] = &[];

struct ReceiptEntry {
    receipt: Receipt,
    provider: Option<String>,
    finished_at: Option<Instant>,
}

struct CoordinatorState {
    receipts: HashMap<String, ReceiptEntry>,
    active_runs: HashMap<String, Arc<RunHandle>>,
    published_once: bool,
    last_published: Option<Value>,
    last_dir_error: Option<String>,
}

#[derive(Clone)]
pub(super) struct Coordinator {
    pub(super) root: PathBuf,
    pub(super) pid: u32,
    pub(super) started_at: f64,
    pub(super) push_providers: &'static [&'static str],
    terminals: Option<Terminals>,
    inner: Arc<Mutex<CoordinatorState>>,
    publish_lock: Arc<Mutex<()>>,
}

pub(super) fn truncate_short(text: &str) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = clean.trim();
    let mut out = String::new();
    for (idx, ch) in trimmed.chars().enumerate() {
        if idx >= MAX_TEXT_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub(super) fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub(super) fn current_process_started_at() -> f64 {
    let pid = std::process::id();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(Pid::from_u32(pid))
        .map_or_else(now_unix_secs, |p| p.start_time() as f64)
}

impl Coordinator {
    pub(super) fn with_terminals(
        root: PathBuf,
        pid: u32,
        started_at: f64,
        push_providers: &'static [&'static str],
        terminals: Terminals,
    ) -> Self {
        Self::create(root, pid, started_at, push_providers, Some(terminals))
    }

    #[cfg(test)]
    pub(super) fn new(
        root: PathBuf,
        pid: u32,
        started_at: f64,
        push_providers: &'static [&'static str],
    ) -> Self {
        Self::create(root, pid, started_at, push_providers, None)
    }

    fn create(
        root: PathBuf,
        pid: u32,
        started_at: f64,
        push_providers: &'static [&'static str],
        terminals: Option<Terminals>,
    ) -> Self {
        Self {
            root,
            pid,
            started_at,
            push_providers,
            terminals,
            inner: Arc::new(Mutex::new(CoordinatorState {
                receipts: HashMap::new(),
                active_runs: HashMap::new(),
                published_once: false,
                last_published: None,
                last_dir_error: None,
            })),
            publish_lock: Arc::new(Mutex::new(())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CoordinatorState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn sync_derived_pids(&self, terminals: &Terminals) -> Result<(), String> {
        self.publish_with(Some(terminals))
    }

    pub(super) fn publish(&self) -> Result<(), String> {
        self.publish_with(None)
    }

    pub(super) fn publish_with(&self, terminals: Option<&Terminals>) -> Result<(), String> {
        let _publish_guard = self.publish_lock.lock().unwrap_or_else(|p| p.into_inner());
        let derived_pids = terminals
            .or(self.terminals.as_ref())
            .map(Terminals::live_derived_pids)
            .unwrap_or_default();
        let (doc, evicted) = {
            let state = self.lock();
            let (doc, evicted) = build_push_document_plan(
                self.pid,
                self.push_providers,
                &derived_pids,
                &state.receipts,
            );
            if state.published_once && state.last_published.as_ref() == Some(&doc) {
                return Ok(());
            }
            (doc, evicted)
        };
        write_atomic_compact(&self.root.join("push"), &format!("{}.json", self.pid), &doc)?;
        let mut state = self.lock();
        for id in evicted {
            state.receipts.remove(&id);
        }
        state.published_once = true;
        state.last_published = Some(doc);
        Ok(())
    }

    pub(super) fn register_run(&self, invocation_id: &str) -> Arc<RunHandle> {
        let handle = Arc::new(RunHandle::new());
        self.lock()
            .active_runs
            .insert(invocation_id.to_owned(), handle.clone());
        handle
    }

    pub(super) fn active_run(&self, invocation_id: &str) -> Option<Arc<RunHandle>> {
        self.lock().active_runs.get(invocation_id).cloned()
    }

    pub(super) fn clear_run(&self, invocation_id: &str) {
        self.lock().active_runs.remove(invocation_id);
    }

    #[cfg(test)]
    pub(super) fn insert_test_receipt(&self, request_id: &str, invocation_id: &str) {
        self.lock().receipts.insert(
            request_id.to_owned(),
            ReceiptEntry {
                receipt: Receipt {
                    service_pid: self.pid,
                    invocation_id: invocation_id.to_owned(),
                    kind: "run".to_owned(),
                    state: ReceiptState::Accepted,
                    session_id: None,
                    lease_acquired: None,
                    anchor: None,
                    blocked: None,
                    background_tasks_pending: None,
                    terminal: None,
                },
                provider: None,
                finished_at: None,
            },
        );
    }

    pub(super) fn has_unsettled_run_for_session(
        &self,
        provider: &str,
        session_id: &str,
        exclude_request_id: &str,
    ) -> bool {
        self.lock().receipts.iter().any(|(req_id, entry)| {
            req_id != exclude_request_id
                && entry.receipt.kind == "run"
                && entry.receipt.state != ReceiptState::Finished
                && entry.receipt.session_id.as_deref() == Some(session_id)
                && entry
                    .provider
                    .as_deref()
                    .is_none_or(|recorded| recorded == provider)
        })
    }

    pub(super) fn run_receipt_by_invocation(&self, invocation_id: &str) -> Option<Receipt> {
        self.lock()
            .receipts
            .values()
            .find(|entry| {
                entry.receipt.kind == "run" && entry.receipt.invocation_id == invocation_id
            })
            .map(|entry| entry.receipt.clone())
    }

    pub(super) fn set_lease_acquired(&self, request_id: &str, acquired: bool) {
        self.mutate_receipt(request_id, |r| {
            r.lease_acquired = Some(acquired);
        });
    }

    pub(super) fn set_session_id(&self, request_id: &str, session_id: &str) {
        let sid = truncate_short(session_id);
        self.mutate_receipt(request_id, |r| {
            r.session_id = Some(sid);
        });
    }

    pub(super) fn set_blocked(&self, request_id: &str, blocked: Option<BlockedReceipt>) {
        self.mutate_receipt(request_id, |r| {
            r.blocked = blocked.map(|b| BlockedReceipt {
                kind: truncate_short(&b.kind),
                since: b.since,
                reason: truncate_short(&b.reason),
            });
        });
    }

    pub(super) fn set_background_tasks_pending(
        &self,
        request_id: &str,
        reminder: BackgroundTasksReminder,
    ) {
        self.mutate_receipt(request_id, |r| {
            r.background_tasks_pending = Some(BackgroundTasksReminder {
                code: truncate_short(&reminder.code),
                since: reminder.since,
                count: reminder.count,
                tasks: reminder
                    .tasks
                    .into_iter()
                    .take(MAX_REMINDER_TASKS)
                    .map(|t| truncate_short(&t))
                    .filter(|t| !t.is_empty())
                    .collect(),
                reason: truncate_short(&reminder.reason),
            });
        });
    }

    pub(super) fn mark_started(
        &self,
        request_id: &str,
        session_id: Option<&str>,
        anchor: Option<Value>,
    ) {
        self.mutate_receipt(request_id, |r| {
            if r.state < ReceiptState::Started {
                r.state = ReceiptState::Started;
            }
            if let Some(sid) = session_id {
                r.session_id = Some(truncate_short(sid));
            }
            if anchor.is_some() {
                r.anchor = anchor;
            }
            r.blocked = None;
        });
    }

    pub(super) fn update_anchor(&self, request_id: &str, anchor: Value) {
        self.mutate_receipt(request_id, |r| {
            r.anchor = Some(anchor);
        });
    }

    pub(super) fn mark_finished(
        &self,
        request_id: &str,
        status: &str,
        reason: Option<&str>,
        session_id: Option<&str>,
    ) {
        self.mark_finished_with(request_id, status, reason, session_id, None, Vec::new());
    }

    pub(super) fn mark_finished_with(
        &self,
        request_id: &str,
        status: &str,
        reason: Option<&str>,
        session_id: Option<&str>,
        anchor_patch: Option<Value>,
        warnings: Vec<String>,
    ) {
        let mut changed = false;
        let warnings: Vec<String> = warnings
            .into_iter()
            .take(MAX_WARNINGS)
            .map(|w| truncate_short(&w))
            .filter(|w| !w.is_empty())
            .collect();
        let outcome = TerminalOutcome {
            status: truncate_short(status),
            reason: reason.map(truncate_short).filter(|s| !s.is_empty()),
            warnings,
        };
        {
            let mut state = self.lock();
            if let Some(entry) = state.receipts.get_mut(request_id)
                && entry.receipt.state != ReceiptState::Finished
            {
                if let Some(Value::Object(patch)) = anchor_patch {
                    match entry.receipt.anchor.as_mut() {
                        Some(Value::Object(existing)) => {
                            for (k, v) in patch {
                                existing.entry(k).or_insert(v);
                            }
                        }
                        _ => {
                            entry.receipt.anchor = Some(Value::Object(patch));
                        }
                    }
                }
                entry.receipt.state = ReceiptState::Finished;
                entry.receipt.blocked = None;
                entry.receipt.background_tasks_pending = None;
                entry.receipt.terminal = Some(outcome);
                if let Some(sid) = session_id {
                    entry.receipt.session_id = Some(truncate_short(sid));
                }
                entry.finished_at = Some(Instant::now());
                changed = true;
            }
        }
        if changed {
            let _ = self.publish();
        }
    }

    pub(super) fn on_terminal_closed(&self, invocation_id: &str) {
        if let Some(handle) = self.active_run(invocation_id) {
            handle.request_pre_start_cancel();
        }
        let req_id = {
            let state = self.lock();
            state
                .receipts
                .iter()
                .find(|(_, e)| {
                    e.receipt.kind == "run"
                        && e.receipt.invocation_id == invocation_id
                        && e.receipt.state != ReceiptState::Finished
                })
                .map(|(id, _)| id.clone())
        };
        if let Some(id) = req_id {
            self.mark_finished(&id, "closed", Some("派生终端已关闭"), None);
        }
    }

    fn mutate_receipt(&self, request_id: &str, f: impl FnOnce(&mut Receipt)) {
        {
            let mut state = self.lock();
            if let Some(entry) = state.receipts.get_mut(request_id)
                && entry.receipt.state != ReceiptState::Finished
            {
                f(&mut entry.receipt);
            } else {
                return;
            }
        }
        let _ = self.publish();
    }

    pub(super) fn poll(&self, terminals: &Terminals) {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        self.reconcile_settled(&system);
        self.scan_requests(terminals, &system);
        let _ = self.publish_with(Some(terminals));
    }

    fn scan_requests(&self, terminals: &Terminals, system: &System) {
        let dir = self.root.join("spawn-requests");
        match verify_spawn_requests_dir(&dir) {
            Ok(()) => {
                self.lock().last_dir_error = None;
            }
            Err(reason) => {
                if !reason.is_empty() {
                    let mut state = self.lock();
                    if state.last_dir_error.as_deref() != Some(reason.as_str()) {
                        eprintln!("跳过读取 spawn-requests 目录：{reason}");
                        state.last_dir_error = Some(reason);
                    }
                }
                return;
            }
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();

        for path in paths {
            self.process_request_file(&path, terminals, system);
        }
    }

    fn process_request_file(&self, path: &Path, terminals: &Terminals, system: &System) {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        if !valid_uuid(stem) {
            return;
        }
        let Some(env) = read_request_envelope(path) else {
            return;
        };
        if env.version != 1 || env.request_id != stem {
            return;
        }
        if env.target_tray.pid != self.pid
            || (env.target_tray.started_at - self.started_at).abs() > 2.0
        {
            return;
        }
        if !verify_service_identity(&env.service, system) {
            return;
        }
        if !matches!(env.kind.as_str(), "run" | "cancel") {
            return;
        }
        if env.kind == "run"
            && (env.config.is_none()
                || env.task.is_none()
                || env
                    .parent
                    .as_ref()
                    .is_none_or(|p| p.workspace_id.as_deref().is_none_or(str::is_empty)))
        {
            return;
        }

        // Deduplicate by request_id in memory: never execute a second time.
        // Register the RunHandle synchronously on intake so a cancel processed right after
        // never races ahead of run thread startup.
        let (already_seen, run_handle) = {
            let mut state = self.lock();
            if state.receipts.contains_key(&env.request_id) {
                (true, None)
            } else {
                let lease_acquired = env
                    .config
                    .as_ref()
                    .and_then(|c| c.isolation.as_ref())
                    .map(|_| false);
                state.receipts.insert(
                    env.request_id.clone(),
                    ReceiptEntry {
                        receipt: Receipt {
                            service_pid: env.service.pid,
                            invocation_id: env.invocation_id.clone(),
                            kind: env.kind.clone(),
                            state: ReceiptState::Accepted,
                            session_id: env.session_id.clone(),
                            lease_acquired,
                            anchor: None,
                            blocked: None,
                            background_tasks_pending: None,
                            terminal: None,
                        },
                        provider: env.config.as_ref().map(|c| c.provider.clone()),
                        finished_at: None,
                    },
                );
                let handle = if env.kind == "run" {
                    let h = Arc::new(RunHandle::new());
                    state
                        .active_runs
                        .insert(env.invocation_id.clone(), h.clone());
                    Some(h)
                } else {
                    None
                };
                (false, handle)
            }
        };

        // Always publish accepted/existing receipt atomically BEFORE removing the request file.
        if self.publish().is_err() {
            if !already_seen {
                let mut state = self.lock();
                state.receipts.remove(&env.request_id);
                if run_handle.is_some() {
                    state.active_runs.remove(&env.invocation_id);
                }
            }
            return;
        }
        let _ = fs::remove_file(path);
        if already_seen {
            return;
        }

        let coordinator = self.clone();
        let terminals = terminals.clone();
        std::thread::spawn(move || match env.kind.as_str() {
            "run" => {
                let handle =
                    run_handle.unwrap_or_else(|| coordinator.register_run(&env.invocation_id));
                derived::execute_run(coordinator, terminals, env, handle);
            }
            "cancel" => derived::execute_cancel(coordinator, terminals, env),
            _ => {}
        });
    }

    fn reconcile_settled(&self, system: &System) {
        let checks: Vec<(String, u32, Option<Instant>)> = {
            let state = self.lock();
            state
                .receipts
                .iter()
                .filter(|(_, e)| e.receipt.state == ReceiptState::Finished)
                .map(|(id, e)| (id.clone(), e.receipt.service_pid, e.finished_at))
                .collect()
        };
        if checks.is_empty() {
            return;
        }
        let mut inbox_cache: HashMap<u32, Option<Value>> = HashMap::new();
        let mut remove_ids = Vec::new();
        for (req_id, svc_pid, finished_at) in checks {
            let settled_val = inbox_cache
                .entry(svc_pid)
                .or_insert_with(|| read_settled_for_service(&self.root, svc_pid, system));
            let confirmed = settled_val
                .as_ref()
                .is_some_and(|v| is_request_settled(v, self.pid, self.started_at, &req_id));
            let service_dead_long_enough = system.process(Pid::from_u32(svc_pid)).is_none()
                && finished_at.is_some_and(|at| at.elapsed() >= DEAD_RECEIPT_TTL);
            if confirmed || service_dead_long_enough {
                remove_ids.push(req_id);
            }
        }
        if !remove_ids.is_empty() {
            {
                let mut state = self.lock();
                for id in remove_ids {
                    state.receipts.remove(&id);
                }
            }
            let _ = self.publish();
        }
    }

    #[cfg(test)]
    pub(super) fn receipts_snapshot(&self) -> HashMap<String, Receipt> {
        self.lock()
            .receipts
            .iter()
            .map(|(k, v)| (k.clone(), v.receipt.clone()))
            .collect()
    }
}

fn build_push_document_plan(
    pid: u32,
    providers: &[&str],
    derived_pids: &[u32],
    receipts: &HashMap<String, ReceiptEntry>,
) -> (Value, Vec<String>) {
    let mut excluded: HashSet<&str> = HashSet::new();
    let serialize = |excluded: &HashSet<&str>| -> Value {
        let entries: serde_json::Map<String, Value> = receipts
            .iter()
            .filter(|(k, _)| !excluded.contains(k.as_str()))
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::to_value(&v.receipt).unwrap_or(Value::Null),
                )
            })
            .collect();
        json!({
            "version": 1,
            "pid": pid,
            "providers": providers,
            "terminal_providers": TERMINAL_PROVIDERS,
            "derived_pids": derived_pids,
            "requests": entries,
        })
    };
    let mut evicted = Vec::new();
    let mut doc = serialize(&excluded);
    // Keep push/<pid>.json strictly under PUSH_MAX_BYTES (256 KiB) using the same compact
    // encoding written by `write_atomic_compact`.
    while serde_json::to_vec(&doc).map_or(0, |b| b.len()) > PUSH_MAX_BYTES {
        let oldest_finished = receipts
            .iter()
            .filter(|(id, e)| {
                !excluded.contains(id.as_str()) && e.receipt.state == ReceiptState::Finished
            })
            .min_by_key(|(_, e)| e.finished_at)
            .map(|(id, _)| id.as_str());
        let Some(evict) = oldest_finished else {
            break;
        };
        excluded.insert(evict);
        evicted.push(evict.to_owned());
        doc = serialize(&excluded);
    }
    (doc, evicted)
}

#[cfg(test)]
fn build_push_document(
    pid: u32,
    providers: &[&str],
    derived_pids: &[u32],
    receipts: &mut HashMap<String, ReceiptEntry>,
) -> Value {
    let (doc, evicted) = build_push_document_plan(pid, providers, derived_pids, receipts);
    for id in evicted {
        receipts.remove(&id);
    }
    doc
}

fn verify_spawn_requests_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Err(String::new());
    }
    let meta = fs::symlink_metadata(dir).map_err(|e| format!("无法读取目录属性：{e}"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("spawn-requests 不是普通目录".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            return Err(format!(
                "目录属主 uid={} 与当前用户 uid={} 不一致",
                meta.uid(),
                uid
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "目录权限 {:04o} 允许组或其他用户写入",
                meta.mode() & 0o7777
            ));
        }
    }
    Ok(())
}

fn read_request_envelope(path: &Path) -> Option<SpawnRequestEnvelope> {
    if fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_REQUEST_BYTES {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            eprintln!(
                "拒绝领取请求文件 {}：属主 uid={} 与当前用户 uid={} 不一致",
                path.display(),
                meta.uid(),
                uid
            );
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn verify_service_identity(service: &ProcessIdentity, system: &System) -> bool {
    let Some(process) = system.process(Pid::from_u32(service.pid)) else {
        return false;
    };
    if process.start_time() == 0 {
        return false;
    }
    (service.started_at - process.start_time() as f64).abs() <= 2.0
}

fn read_settled_for_service(root: &Path, service_pid: u32, system: &System) -> Option<Value> {
    let path = root.join("inbox").join(format!("{service_pid}.json"));
    if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
        return None;
    }
    let process = system.process(Pid::from_u32(service_pid))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let millis = meta
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
    let parsed: Value = serde_json::from_slice(&bytes).ok()?;
    if parsed["version"].as_u64() != Some(1)
        || parsed["pid"].as_u64() != Some(u64::from(service_pid))
    {
        return None;
    }
    parsed.get("settled_requests").cloned()
}

fn tray_matches(entry: &Value, my_pid: u32, my_started_at: f64) -> bool {
    let target = entry
        .get("tray")
        .or_else(|| entry.get("target_tray"))
        .unwrap_or(entry);
    let pid = target["pid"]
        .as_u64()
        .or_else(|| target["tray_pid"].as_u64());
    let started = target["started_at"]
        .as_f64()
        .or_else(|| target["tray_started_at"].as_f64());
    pid == Some(u64::from(my_pid)) && started.is_some_and(|s| (s - my_started_at).abs() <= 2.0)
}

fn list_contains_id(list: Option<&Value>, request_id: &str) -> bool {
    match list {
        Some(Value::Array(items)) => items.iter().any(|v| {
            v.as_str() == Some(request_id) || v["request_id"].as_str() == Some(request_id)
        }),
        Some(Value::Object(map)) => map.contains_key(request_id),
        _ => false,
    }
}

pub(super) fn is_request_settled(
    settled: &Value,
    my_pid: u32,
    my_started_at: f64,
    request_id: &str,
) -> bool {
    match settled {
        Value::Object(map) => {
            if let Some(entry) = map.get(request_id)
                && tray_matches(entry, my_pid, my_started_at)
            {
                return true;
            }
            if let Some(entry) = map.get(&my_pid.to_string())
                && tray_matches(entry, my_pid, my_started_at)
                && (list_contains_id(entry.get("requests"), request_id)
                    || list_contains_id(entry.get("request_ids"), request_id))
            {
                return true;
            }
            let pid_prefix = format!("{my_pid}:");
            for (key, val) in map {
                if let Some(started_str) = key.strip_prefix(&pid_prefix)
                    && let Ok(started) = started_str.parse::<f64>()
                    && (started - my_started_at).abs() <= 2.0
                    && (list_contains_id(Some(val), request_id)
                        || list_contains_id(val.get("requests"), request_id)
                        || list_contains_id(val.get("request_ids"), request_id))
                {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => items.iter().any(|entry| {
            tray_matches(entry, my_pid, my_started_at)
                && (entry["request_id"].as_str() == Some(request_id)
                    || list_contains_id(entry.get("requests"), request_id)
                    || list_contains_id(entry.get("request_ids"), request_id))
        }),
        _ => false,
    }
}

pub(super) fn ensure_spawn_requests_dir(root: &Path) {
    let _ = create_private_dir(&root.join("spawn-requests"));
}

#[allow(dead_code)]
fn _unused_hashset_anchor(_: &HashSet<String>) {}

#[cfg(test)]
mod tests;
