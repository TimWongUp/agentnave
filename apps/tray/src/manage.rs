//! Tray-only cleanup. Keep the lock inode and never walk into seat directories.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::data::Overview;

const RETRY: Duration = Duration::from_millis(100);
const ATTEMPTS: usize = 11;

struct State {
    value: Value,
    pending: bool,
    status: String,
}

fn read_state(directory: &Path, id: &str) -> Result<State, String> {
    let path = directory.join("state.json");
    if !fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("会话状态不是普通文件".to_owned());
    }
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if value["version"] != 2
        || value["id"] != id
        || !matches!(value["kind"].as_str(), Some("show" | "task"))
        || !value["title"].is_string()
    {
        return Err("会话状态无法识别或版本不受支持".to_owned());
    }
    // Missing collections have the same empty defaults as Conversation in store.py.
    for key in ["rounds", "tasks"] {
        if value.get(key).is_some_and(|v| !v.is_array()) {
            return Err("会话记录格式错误".to_owned());
        }
    }
    let mut pending = false;
    for round in value["rounds"].as_array().into_iter().flatten() {
        let Some(seats) = round["seats"].as_array() else {
            return Err("轮次缺少席位记录".to_owned());
        };
        // Exactly RoundRecord.pending_review(), including earlier rounds.
        pending |= seats.iter().any(|seat| seat["review"] == "pending");
    }
    let rounds = value["rounds"].as_array();
    let tasks = value["tasks"].as_array();
    let status = if pending {
        "review"
    } else if let Some(round) = rounds.and_then(|r| r.last()) {
        round["status"].as_str().unwrap_or("idle")
    } else if tasks.is_some_and(|tasks| tasks.iter().any(|t| t["status"] == "running")) {
        "running"
    } else if let Some(task) = tasks.and_then(|t| t.last()) {
        task["status"].as_str().unwrap_or("idle")
    } else {
        "idle"
    }
    .to_owned();
    Ok(State {
        value,
        pending,
        status,
    })
}

/// Reject path aliases before opening the shared lock or reading state.
fn directory(root: &Path, id: &str) -> io::Result<PathBuf> {
    let path = root.join("conversations").join(id);
    if !fs::symlink_metadata(&path)?.is_dir() {
        return Err(io::Error::other("会话目录不是普通目录"));
    }
    Ok(path)
}

fn lock_file(directory: &Path) -> io::Result<File> {
    let path = directory.join(".lock");
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => return Err(io::Error::other("锁不是普通文件")),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn result(id: &str, status: &str, message: &str) -> Value {
    json!({"id": id, "status": status, "message": message})
}

pub fn delete(root: &Path, id: &str) -> Value {
    let directory = match directory(root, id) {
        Ok(path) => path,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return result(id, "missing", "会话已不存在");
        }
        Err(e) => return result(id, "error", &e.to_string()),
    };
    let lock = match lock_file(&directory) {
        Ok(lock) => lock,
        Err(e) => return result(id, "error", &e.to_string()),
    };
    // Probe locks are short-lived. Retry for at most one second before reporting contention.
    for attempt in 0..ATTEMPTS {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if attempt + 1 < ATTEMPTS => thread::sleep(RETRY),
            Err(TryLockError::WouldBlock) => {
                return result(id, "in_use", "正在被某个宿主使用，请空闲后重试");
            }
            Err(e) => return result(id, "error", &e.to_string()),
        }
    }
    // Re-read only after acquiring the lock. A saved running status now means a crashed writer.
    if !directory.join("state.json").exists() {
        return result(id, "missing", "会话已不存在");
    }
    let state = match read_state(&directory, id) {
        Ok(state) => state,
        Err(e) => return result(id, "error", &e),
    };
    if state.pending {
        return result(id, "pending_review", "有待审回答，请先由主控处理");
    }
    // Delete the derived view first: on failure the authoritative state is still retryable.
    for name in ["views.json", "state.json"] {
        if let Err(e) = fs::remove_file(directory.join(name))
            && !(name == "views.json" && e.kind() == io::ErrorKind::NotFound)
        {
            return result(id, "error", &format!("删除 {name} 失败：{e}"));
        }
    }
    result(id, "deleted", "已删除会话记录；席位文件保留")
    // Closing the File releases the lock without removing its inode.
}

pub fn list(root: &Path, overview: Overview) -> Value {
    let mut items = Vec::new();
    let mut unavailable = Vec::new();
    let entries = match fs::read_dir(root.join("conversations")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return json!({"items": [], "unavailable": [], "missing": true});
        }
        Err(e) => return json!({"error": e.to_string()}),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                unavailable.push(json!({"id": "", "reason": e.to_string()}));
                continue;
            }
        };
        let id = entry.file_name().to_string_lossy().into_owned();
        if !crate::data::valid_id(&id) {
            continue;
        }
        let path = match directory(root, &id) {
            Ok(path) => path,
            Err(e) => {
                unavailable.push(json!({"id": id, "reason": e.to_string()}));
                continue;
            }
        };
        if !path.join("state.json").exists() {
            continue;
        }
        let lock = lock_file(&path);
        let probe = lock.as_ref().map(|file| file.try_lock());
        let (held, error) = match probe {
            Ok(Ok(())) => (true, None),
            Ok(Err(TryLockError::WouldBlock)) => (false, None),
            Ok(Err(e)) => (false, Some(e.to_string())),
            Err(e) => (false, Some(e.to_string())),
        };
        let state = match read_state(&path, &id) {
            Ok(state) => state,
            Err(e) => {
                if path.join("state.json").exists() {
                    unavailable.push(json!({"id": id, "reason": e}));
                }
                continue;
            }
        };
        let snapshot = overview.items.iter().find(|item| item.id == id);
        let status = if held && state.status == "running" {
            "interrupted"
        } else {
            &state.status
        };
        let reason = if let Some(error) = error {
            error
        } else if state.pending {
            "有待审回答，请先由主控处理".to_owned()
        } else if !held {
            "正在被某个宿主使用，请空闲后重试".to_owned()
        } else {
            String::new()
        };
        let updated = snapshot
            .map(|item| item.updated.as_str())
            .unwrap_or_default();
        let updated_ms = fs::metadata(path.join("state.json"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis());
        items.push(json!({
            "id": id, "title": state.value["title"], "mode": state.value["kind"],
            "status": status, "updated": updated, "updated_ms": updated_ms,
            "deletable": reason.is_empty(), "reason": reason,
            "group": snapshot.and_then(crate::stage::chat_group),
        }));
        // The probe's File drops here, before reading the next conversation.
    }
    json!({"items": items, "unavailable": unavailable, "missing": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Reader;

    fn fixture(root: &Path, id: &str) -> PathBuf {
        let path = root.join("conversations").join(id);
        fs::create_dir_all(path.join("seats/a")).unwrap();
        fs::write(path.join("state.json"), json!({"version":2,"id":id,"title":"测试", "kind":"task", "tasks":[{"status":"running"}]}).to_string()).unwrap();
        fs::write(path.join("views.json"), "{}").unwrap();
        fs::write(path.join("seats/a/output.txt"), "keep").unwrap();
        fs::write(path.join("other.json"), "keep").unwrap();
        fs::write(path.join(".lock"), "0").unwrap();
        path
    }

    #[test]
    fn cleanup_keeps_lock_inode_and_all_other_files() {
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "idle");
        let mut state: Value =
            serde_json::from_slice(&fs::read(path.join("state.json")).unwrap()).unwrap();
        state["tasks"] = json!([]);
        fs::write(path.join("state.json"), state.to_string()).unwrap();
        let lock = lock_file(&path).unwrap();
        assert_eq!(delete(root.path(), "idle")["status"], "deleted");
        assert!(!path.join("state.json").exists());
        assert!(!path.join("views.json").exists());
        for name in [".lock", "other.json", "seats/a/output.txt"] {
            assert!(path.join(name).is_file());
        }
        // A handle opened before cleanup still excludes a handle opened afterward. A process
        // another test spawns meanwhile briefly inherits cleanup's descriptor until its exec
        // closes it, so allow the same short retry as cleanup itself.
        let mut locked = lock.try_lock();
        for _ in 1..ATTEMPTS {
            if !matches!(locked, Err(TryLockError::WouldBlock)) {
                break;
            }
            thread::sleep(RETRY);
            locked = lock.try_lock();
        }
        locked.unwrap();
        assert!(matches!(
            lock_file(&path).unwrap().try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(lock);
        assert_eq!(delete(root.path(), "idle")["status"], "missing");
    }

    #[test]
    fn locked_and_pending_records_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "busy");
        let lock = lock_file(&path).unwrap();
        lock.try_lock().unwrap();
        assert_eq!(delete(root.path(), "busy")["status"], "in_use");
        drop(lock);
        let mut state: Value =
            serde_json::from_slice(&fs::read(path.join("state.json")).unwrap()).unwrap();
        state["rounds"] = json!([{"status":"finished","seats":[{"review":"pending"}]},{"status":"running","seats":[]}]);
        fs::write(path.join("state.json"), state.to_string()).unwrap();
        assert_eq!(delete(root.path(), "busy")["status"], "pending_review");
        assert!(path.join("state.json").exists());
        assert!(path.join("views.json").exists());
    }

    #[test]
    fn protocol_requires_post_action_and_valid_batch() {
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "a1");
        let reader = Reader::new(root.path().to_owned());
        let route = "/app/conversations/delete";
        for method in ["GET", "PUT", "DELETE", "OPTIONS"] {
            assert_eq!(
                reader
                    .request(method, route, Some("1"), br#"{"ids":["a1"]}"#)
                    .status,
                405
            );
        }
        assert_eq!(
            reader
                .request("POST", route, None, br#"{"ids":["a1"]}"#)
                .status,
            403
        );
        for body in [
            br#"{"ids":["a1","../outside"]}"#.as_slice(),
            b"{}",
            b"invalid",
            br#"{"ids":[]}"#,
        ] {
            assert_eq!(reader.request("POST", route, Some("1"), body).status, 400);
        }
        assert!(path.join("state.json").exists());
        let response = reader.request("POST", route, Some("1"), br#"{"ids":["a1","gone"]}"#);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["results"][0]["status"], "deleted");
        assert_eq!(body["results"][1]["status"], "missing");
        assert_eq!(
            reader
                .request("POST", "/manage/state", Some("1"), b"{}")
                .status,
            405
        );
    }

    #[test]
    fn management_lists_state_without_snapshots_and_probes_locks() {
        let root = tempfile::tempdir().unwrap();
        let stale = fixture(root.path(), "stale");
        fs::remove_file(stale.join("views.json")).unwrap();
        let busy = fixture(root.path(), "busy");
        let lock = lock_file(&busy).unwrap();
        lock.try_lock().unwrap();
        let reader = Reader::new(root.path().to_owned());
        let response = reader.respond("/manage/state");
        let payload: Value = serde_json::from_slice(&response.body).unwrap();
        let items = payload["items"].as_array().unwrap();
        let stale = items.iter().find(|v| v["id"] == "stale").unwrap();
        assert_eq!(stale["status"], "interrupted");
        assert_eq!(stale["deletable"], true);
        let busy = items.iter().find(|v| v["id"] == "busy").unwrap();
        assert_eq!(busy["status"], "running");
        assert_eq!(busy["deletable"], false);
        // Crash residue is deletable even without a derived snapshot.
        assert_eq!(delete(root.path(), "stale")["status"], "deleted");
        let after = list(root.path(), reader.overview());
        assert_eq!(after["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn unreadable_state_and_failed_removal_keep_authoritative_history() {
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "broken");
        fs::write(path.join("state.json"), "{broken").unwrap();
        assert_eq!(delete(root.path(), "broken")["status"], "error");
        assert!(path.join("views.json").exists());
        let path = fixture(root.path(), "no_view");
        fs::remove_file(path.join("views.json")).unwrap();
        fs::create_dir(path.join("views.json")).unwrap();
        assert_eq!(delete(root.path(), "no_view")["status"], "error");
        assert!(path.join("state.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_conversation_or_lock_cannot_redirect_cleanup() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "original");
        symlink(&path, root.path().join("conversations/alias")).unwrap();
        assert_eq!(delete(root.path(), "alias")["status"], "error");
        fs::remove_file(path.join(".lock")).unwrap();
        symlink(path.join("other.json"), path.join(".lock")).unwrap();
        assert_eq!(delete(root.path(), "original")["status"], "error");
        assert!(path.join("state.json").exists());
        assert_eq!(fs::read_to_string(path.join("other.json")).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn python_flock_excludes_rust_and_cleanup_retries_then_rereads() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        let path = fixture(root.path(), "python");
        // Any Python 3 works: only the standard library's fcntl is used, as store.py does.
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(
                r#"
import fcntl, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
with (path / '.lock').open('a+b') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    print('locked', flush=True)
    sys.stdin.readline()
    state = json.loads((path / 'state.json').read_text())
    state['rounds'] = [{'seats': [{'review': 'pending'}]}]
    (path / 'state.json').write_text(json.dumps(state))
"#,
            )
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "locked");
        assert_eq!(delete(root.path(), "python")["status"], "in_use");
        let result = thread::scope(|scope| {
            let deleting = scope.spawn(|| delete(root.path(), "python"));
            thread::sleep(Duration::from_millis(150));
            child.stdin.take().unwrap().write_all(b"release\n").unwrap();
            assert!(child.wait().unwrap().success());
            deleting.join().unwrap()
        });
        assert_eq!(result["status"], "pending_review");
        assert!(path.join("state.json").exists());
    }
}
