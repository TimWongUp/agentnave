//! App presence and background inbox delivery.
use super::{Terminals, links};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Manager};

static STOP: AtomicBool = AtomicBool::new(false);
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
fn inboxes(root: &Path) -> HashMap<String, Vec<Item>> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut bosses: HashMap<String, Vec<Item>> = HashMap::new();
    for entry in fs::read_dir(root.join("inbox"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        let Some(pid) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if path.extension().is_none_or(|s| s != "json")
            || system.process(Pid::from_u32(pid)).is_none()
        {
            continue;
        }
        let Some(inbox) = fs::read(&path)
            .ok()
            .and_then(|s| serde_json::from_slice::<Inbox>(&s).ok())
        else {
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
                Some((
                    r.id,
                    r.session.clone(),
                    inboxes.get(links.get(&r.id)?)?.clone(),
                ))
            })
            .collect()
    };
    let mut pending = HashMap::new();
    for (id, session, items) in candidates {
        for item in &items {
            seen.entry(item.key.clone()).or_insert_with(Instant::now);
        }
        let count = items
            .iter()
            .filter(|i| {
                failed.contains(&i.key)
                    || (!attempted.contains(&i.key)
                        && seen[&i.key].elapsed() >= Duration::from_secs(60))
            })
            .count();
        pending.insert(id, count);
        let mut unique = HashSet::new();
        let items: Vec<_> = items
            .into_iter()
            .filter(|i| !attempted.contains(&i.key) && unique.insert(i.key.clone()))
            .collect();
        if items.is_empty() {
            continue;
        }
        let started = match session.begin_wakeup(&message(&items)) {
            Ok(None) => continue,
            result => {
                // A failed write may have written a prefix, so it must not be retried.
                for item in &items {
                    attempted.insert(item.key.clone());
                }
                match result {
                    Ok(Some(())) => true,
                    Err(_) => false,
                    Ok(None) => unreachable!(),
                }
            }
        };
        let delivered = if started {
            thread::sleep(Duration::from_millis(300));
            session.finish_wakeup()
        } else {
            false
        };
        if !delivered {
            for item in &items {
                failed.insert(item.key.clone());
            }
            pending.insert(id, count + items.len());
        }
    }
    for running in &mut terminals.lock().open {
        running.pending = pending.get(&running.id).copied().unwrap_or(0);
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
}
