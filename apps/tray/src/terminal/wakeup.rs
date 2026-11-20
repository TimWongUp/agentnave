//! App presence and background inbox delivery.
#[cfg(unix)]
use super::codex_server::{Client as CodexClient, Delivery};
use super::{Terminals, links, session::Session, session::WakeupOutcome};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
    sync::{
        Arc,
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

pub(super) fn start(app: AppHandle) {
    STOP.store(false, Ordering::SeqCst);
    let root = crate::data_files::data_root();
    let pid = std::process::id();
    if let Err(error) = crate::data_files::write_atomic(
        &root.join("push"),
        &format!("{pid}.json"),
        &json!({"version": 1, "pid": pid, "providers": PROVIDERS}),
    ) {
        eprintln!("无法登记终端推送：{error}");
        return;
    }
    thread::spawn(move || {
        let (mut attempted, mut seen, mut failed) =
            (HashSet::new(), HashMap::new(), HashSet::new());
        while !STOP.load(Ordering::SeqCst) {
            scan(
                &app.state::<Terminals>(),
                &root,
                &mut attempted,
                &mut seen,
                &mut failed,
            );
            thread::sleep(Duration::from_millis(500));
        }
    });
}
pub(super) fn shutdown() {
    STOP.store(true, Ordering::SeqCst);
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

fn inboxes(root: &Path) -> HashMap<String, Vec<Item>> {
    let dir = root.join("inbox");
    if dir.is_symlink() {
        return HashMap::new();
    }
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut bosses: HashMap<String, Vec<Item>> = HashMap::new();
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
            if !boss.waiting {
                bosses.entry(key).or_default().extend(boss.items);
            }
        }
    }
    bosses
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
                    && seen[&i.key].elapsed() >= Duration::from_secs(60))
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

fn deliver_pty(
    session: &Session,
    selected: &[Item],
    attempted: &mut HashSet<String>,
    failed: &mut HashSet<String>,
) -> bool {
    let started = match session.begin_wakeup(&message(selected)) {
        Ok(None) => return false,
        result => {
            // A failed write may have written a prefix, so it must not be retried.
            for item in selected {
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
        session.finish_wakeup()
    } else {
        WakeupOutcome::Failed
    };
    update_attempts(selected, outcome, attempted, failed);
    true
}

pub(super) fn scan(
    terminals: &Terminals,
    root: &Path,
    attempted: &mut HashSet<String>,
    seen: &mut HashMap<String, Instant>,
    failed: &mut HashSet<String>,
) {
    let links = links::bosses(root, &terminals.live_pids());
    let inboxes = inboxes(root);
    let candidates: Vec<_> = {
        let state = terminals.lock();
        state
            .open
            .iter()
            .filter(|r| PROVIDERS.contains(&r.provider.as_str()))
            .filter_map(|r| {
                let boss_key = links.get(&r.id)?.clone();
                let items = inboxes.get(&boss_key)?.clone();
                Some(Candidate {
                    id: r.id,
                    provider: r.provider.clone(),
                    session: r.session.clone(),
                    boss_key,
                    items,
                    #[cfg(unix)]
                    codex: r.codex.clone(),
                })
            })
            .collect()
    };
    let mut pending = HashMap::new();
    for candidate in candidates {
        for item in &candidate.items {
            seen.entry(item.key.clone()).or_insert_with(Instant::now);
        }
        pending.insert(
            candidate.id,
            pending_count(&candidate.items, attempted, seen, failed),
        );
        let mut unique = HashSet::new();
        let selected: Vec<_> = candidate
            .items
            .iter()
            .filter(|i| !attempted.contains(&i.key) && unique.insert(i.key.clone()))
            .cloned()
            .collect();
        if selected.is_empty() {
            continue;
        }
        let handled = if candidate.provider == "codex" {
            #[cfg(unix)]
            {
                let Some(codex) = &candidate.codex else {
                    continue;
                };
                match codex.deliver(
                    &candidate.boss_key,
                    &event_key(&selected),
                    &message(&selected),
                ) {
                    Delivery::NotReady => false,
                    Delivery::Sent => {
                        for item in &selected {
                            attempted.insert(item.key.clone());
                            failed.remove(&item.key);
                        }
                        true
                    }
                    Delivery::Failed => {
                        for item in &selected {
                            attempted.insert(item.key.clone());
                            failed.insert(item.key.clone());
                        }
                        true
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (&candidate.boss_key, &candidate.provider);
                false
            }
        } else {
            deliver_pty(&candidate.session, &selected, attempted, failed)
        };
        if handled {
            pending.insert(
                candidate.id,
                pending_count(&candidate.items, attempted, seen, failed),
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
mod tests {
    use super::*;
    #[test]
    fn failed_write_stays_attempted_but_clean_blocked_cancel_can_retry() {
        let item = Item {
            key: "one".into(),
            kind: "task_end".into(),
            role: None,
            status: None,
        };
        let mut attempted = HashSet::from(["one".to_owned()]);
        let mut failed = HashSet::new();
        update_attempts(
            std::slice::from_ref(&item),
            WakeupOutcome::Failed,
            &mut attempted,
            &mut failed,
        );
        assert!(attempted.contains("one") && failed.contains("one"));
        update_attempts(
            std::slice::from_ref(&item),
            WakeupOutcome::Blocked,
            &mut attempted,
            &mut failed,
        );
        assert!(!attempted.contains("one") && failed.contains("one"));
        attempted.insert("one".to_owned());
        update_attempts(
            &[item],
            WakeupOutcome::Delivered,
            &mut attempted,
            &mut failed,
        );
        assert!(attempted.contains("one") && !failed.contains("one"));
    }

    #[test]
    fn ignores_stale_and_symlinked_inbox_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("inbox");
        fs::create_dir(&dir).unwrap();
        let pid = std::process::id();
        let path = dir.join(format!("{pid}.json"));
        let document = json!({"version":1,"pid":pid,"bosses":{"boss":{"waiting":false,"items":[{"key":"one","kind":"task_end"}]}}});
        fs::write(&path, document.to_string()).unwrap();
        assert!(inboxes(temp.path()).contains_key("boss"));
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let start = system.process(Pid::from_u32(pid)).unwrap().start_time();
        let old = UNIX_EPOCH + Duration::from_secs(start.saturating_sub(10));
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!inboxes(temp.path()).contains_key("boss"));
        #[cfg(unix)]
        {
            let target = temp.path().join("target.json");
            fs::write(&target, document.to_string()).unwrap();
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(!inboxes(temp.path()).contains_key("boss"));
        }
    }

    #[test]
    fn prompt_precedes_results_and_role_is_sanitized() {
        let result = Item {
            key: "1".into(),
            kind: "task_end".into(),
            role: Some("重构\n危险".into()),
            status: Some("succeeded".into()),
        };
        let prompt = Item {
            key: "2".into(),
            kind: "prompt".into(),
            role: None,
            status: None,
        };
        let text = message(&[result, prompt]);
        assert!(text.starts_with("[AgentNave] 1 个待你处理的请求"));
        assert!(text.contains("重构危险（成功）"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn push_providers_include_codex_only_on_unix() {
        #[cfg(unix)]
        assert!(PROVIDERS.contains(&"codex"));
        #[cfg(not(unix))]
        assert!(!PROVIDERS.contains(&"codex"));
        assert_eq!(
            event_key(&[Item {
                key: "k1".into(),
                kind: "task_end".into(),
                role: None,
                status: None,
            }]),
            "k1"
        );
    }
}
