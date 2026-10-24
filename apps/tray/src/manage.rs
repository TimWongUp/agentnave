//! Tray-only cleanup and renaming in the conversation database (ADR-0012, ADR-0026). Every
//! change happens under the conversation's OS lock, the same one its writing server holds.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::data::Overview;

const RETRY: Duration = Duration::from_millis(100);
const ATTEMPTS: usize = 11;

/// What cleanup needs to know about one saved conversation.
struct State {
    title: String,
    kind: String,
    /// Any round still holds an answer waiting for review (RoundRecord.pending_review()).
    pending: bool,
    /// The status the last saved view showed; "running" left by a crashed writer included.
    status: String,
    /// When that view was written, in milliseconds since the epoch.
    written_ms: Option<i64>,
}

fn read_state(connection: &Connection, id: &str) -> Result<Option<State>, String> {
    let row = connection
        .query_row(
            "SELECT title, kind, \
             EXISTS (SELECT 1 FROM rounds, json_each(rounds.body, '$.seats') AS seat \
                     WHERE rounds.conversation_id = conversations.id \
                     AND json_extract(seat.value, '$.review') = 'pending'), \
             (SELECT body FROM views WHERE conversation_id = conversations.id), \
             (SELECT written_ms FROM views WHERE conversation_id = conversations.id) \
             FROM conversations WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((title, kind, pending, view, written_ms)) = row else {
        return Ok(None);
    };
    let status = view
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|view| view["summary"]["status"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "idle".to_owned());
    Ok(Some(State {
        title,
        kind,
        pending,
        status,
        written_ms,
    }))
}

fn lock_file(root: &Path, id: &str) -> io::Result<File> {
    let path = crate::db::lock_path(root, id);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => return Err(io::Error::other("锁不是普通文件")),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
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

/// A writing connection and the conversation's lock, with its state read under that lock;
/// or the result to report instead.
fn locked(root: &Path, id: &str) -> Result<(Connection, File, State), Value> {
    let connection = match crate::db::write(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return Err(result(id, "missing", "会话已不存在")),
        Err(e) => return Err(result(id, "error", &e)),
    };
    // Unknown ids never create a lock file.
    match read_state(&connection, id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(result(id, "missing", "会话已不存在")),
        Err(e) => return Err(result(id, "error", &e)),
    }
    let lock = lock_file(root, id).map_err(|e| result(id, "error", &e.to_string()))?;
    // Probe locks are short-lived. Retry for at most one second before reporting contention.
    for attempt in 0..ATTEMPTS {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if attempt + 1 < ATTEMPTS => thread::sleep(RETRY),
            Err(TryLockError::WouldBlock) => {
                return Err(result(id, "in_use", "正在被某个宿主使用，请空闲后重试"));
            }
            Err(e) => return Err(result(id, "error", &e.to_string())),
        }
    }
    // Re-read only after acquiring the lock. A saved running status now means a crashed writer.
    match read_state(&connection, id) {
        Ok(Some(state)) => Ok((connection, lock, state)),
        Ok(None) => Err(result(id, "missing", "会话已不存在")),
        Err(e) => Err(result(id, "error", &e)),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Rename an idle conversation; servers reload it once its revision changes.
pub fn rename(root: &Path, id: &str, title: &str) -> Value {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 || title.chars().any(char::is_control) {
        return result(id, "error", "标题需为 1–200 个字符，且不含换行或控制字符");
    }
    let (mut connection, _lock, _state) = match locked(root, id) {
        Ok(found) => found,
        Err(report) => return report,
    };
    let renamed = (|| -> rusqlite::Result<()> {
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE conversations SET title = ?1, rev = rev + 1 WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
        // The derived view follows at once so the office shows the new title before any server
        // projects it again; an unreadable view is left for the next server to replace.
        let view: Option<String> = transaction
            .query_row(
                "SELECT body FROM views WHERE conversation_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(mut view) = view.and_then(|body| serde_json::from_str::<Value>(&body).ok()) {
            for key in ["summary", "director", "public"] {
                if view[key].is_object() {
                    view[key]["title"] = json!(title);
                }
            }
            transaction.execute(
                "UPDATE views SET body = ?1, rev = rev + 1, written_ms = ?2 \
                 WHERE conversation_id = ?3",
                rusqlite::params![view.to_string(), now_ms(), id],
            )?;
        }
        transaction.commit()
    })();
    match renamed {
        Ok(()) => json!({"id": id, "status": "renamed", "title": title, "message": "已改名"}),
        Err(e) => result(id, "error", &format!("保存标题失败：{e}")),
    }
}

pub fn delete(root: &Path, id: &str) -> Value {
    let (connection, _lock, state) = match locked(root, id) {
        Ok(found) => found,
        Err(report) => return report,
    };
    if state.pending {
        return result(id, "pending_review", "有待审回答，请先由 boss 处理");
    }
    // Entries, rounds, tasks and the view go with it; the lock file and seat files stay.
    match connection.execute("DELETE FROM conversations WHERE id = ?1", [id]) {
        Ok(_) => result(id, "deleted", "已删除会话记录；席位文件保留"),
        Err(e) => result(id, "error", &format!("删除失败：{e}")),
    }
    // Closing the File releases the lock without removing its inode.
}

pub fn list(root: &Path, overview: Overview) -> Value {
    let connection = match crate::db::read(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return json!({"items": [], "unavailable": [], "missing": true}),
        Err(e) => return json!({"error": e}),
    };
    let ids: Result<Vec<String>, _> = connection
        .prepare("SELECT id FROM conversations")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect());
    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let mut items = Vec::new();
    let mut unavailable = Vec::new();
    for id in ids {
        if !crate::data::valid_id(&id) {
            continue;
        }
        let lock = lock_file(root, &id);
        let probe = lock.as_ref().map(|file| file.try_lock());
        let (held, error) = match probe {
            Ok(Ok(())) => (true, None),
            Ok(Err(TryLockError::WouldBlock)) => (false, None),
            Ok(Err(e)) => (false, Some(e.to_string())),
            Err(e) => (false, Some(e.to_string())),
        };
        let state = match read_state(&connection, &id) {
            Ok(Some(state)) => state,
            Ok(None) => continue, // deleted since the listing
            Err(e) => {
                unavailable.push(json!({"id": id, "reason": e}));
                continue;
            }
        };
        let snapshot = overview.items.iter().find(|item| item.id == id);
        let status = if held && state.status == "running" {
            "interrupted"
        } else if state.pending {
            "review"
        } else {
            &state.status
        };
        let reason = if let Some(error) = error {
            error
        } else if state.pending {
            "有待审回答，请先由 boss 处理".to_owned()
        } else if !held {
            "正在被某个宿主使用，请空闲后重试".to_owned()
        } else {
            String::new()
        };
        let updated = snapshot
            .map(|item| item.updated.as_str())
            .unwrap_or_default();
        items.push(json!({
            "id": id, "title": state.title, "mode": state.kind,
            "status": status, "updated": updated, "updated_ms": state.written_ms,
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

    /// A saved conversation with a running task, its view, and an isolated seat's files.
    fn fixture(root: &Path, id: &str) {
        crate::db::put(
            root,
            id,
            &json!({"kind": "task", "title": "测试", "tasks": [{"status": "running"}]}),
            &json!({"version": 1, "summary": {"id": id, "title": "测试", "status": "running"},
                "director": {"title": "测试"}, "public": null}),
        );
        let seat = root.join("seats").join(id).join("a");
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("output.txt"), "keep").unwrap();
    }

    fn set(root: &Path, sql: &str, id: &str) {
        crate::db::write(root)
            .unwrap()
            .unwrap()
            .execute(sql, [id])
            .unwrap();
    }

    fn count(root: &Path, table: &str, id: &str) -> i64 {
        let column = if table == "conversations" {
            "id"
        } else {
            "conversation_id"
        };
        crate::db::read(root)
            .unwrap()
            .unwrap()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// A process another test spawns may briefly inherit the tray's descriptor until its
    /// exec closes it, so allow the same short retry as cleanup itself.
    fn take(lock: &File) {
        let mut held = lock.try_lock();
        for _ in 1..ATTEMPTS {
            if !matches!(held, Err(TryLockError::WouldBlock)) {
                break;
            }
            thread::sleep(RETRY);
            held = lock.try_lock();
        }
        held.unwrap();
    }

    #[test]
    fn cleanup_cascades_rows_and_keeps_lock_inode_and_seat_files() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "idle");
        set(
            root.path(),
            "DELETE FROM tasks WHERE conversation_id = ?1",
            "idle",
        );
        let lock = lock_file(root.path(), "idle").unwrap();
        assert_eq!(delete(root.path(), "idle")["status"], "deleted");
        for table in ["conversations", "tasks", "views"] {
            assert_eq!(count(root.path(), table, "idle"), 0);
        }
        assert!(root.path().join("seats/idle/a/output.txt").is_file());
        // A handle opened before cleanup still excludes a handle opened afterward.
        take(&lock);
        assert!(matches!(
            lock_file(root.path(), "idle").unwrap().try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(lock);
        assert_eq!(delete(root.path(), "idle")["status"], "missing");
        assert_eq!(delete(root.path(), "never")["status"], "missing");
        assert!(!crate::db::lock_path(root.path(), "never").exists());
    }

    #[test]
    fn rename_rewrites_conversation_and_view_under_the_lock() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "named");
        assert_eq!(
            rename(root.path(), "named", " 新标题 ")["status"],
            "renamed"
        );
        let connection = crate::db::read(root.path()).unwrap().unwrap();
        let (title, rev): (String, i64) = connection
            .query_row(
                "SELECT title, rev FROM conversations WHERE id = 'named'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), rev), ("新标题", 2)); // servers reload the new revision
        assert_eq!(count(root.path(), "tasks", "named"), 1); // everything else kept
        let view: String = connection
            .query_row(
                "SELECT body FROM views WHERE conversation_id = 'named'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let view: Value = serde_json::from_str(&view).unwrap();
        assert_eq!(view["summary"]["title"], "新标题");
        assert_eq!(view["director"]["title"], "新标题");
        assert!(view["public"].is_null());
        assert_eq!(rename(root.path(), "named", "a\nb")["status"], "error");
        assert_eq!(rename(root.path(), "named", " ")["status"], "error");
        let lock = lock_file(root.path(), "named").unwrap();
        take(&lock);
        assert_eq!(rename(root.path(), "named", "被占用")["status"], "in_use");
        drop(lock);
        assert_eq!(rename(root.path(), "gone", "x")["status"], "missing");
    }

    #[test]
    fn locked_and_pending_records_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "busy");
        let lock = lock_file(root.path(), "busy").unwrap();
        lock.try_lock().unwrap();
        assert_eq!(delete(root.path(), "busy")["status"], "in_use");
        drop(lock);
        crate::db::put(
            root.path(),
            "busy",
            &json!({"kind": "show", "rounds": [
                {"status": "finished", "seats": [{"review": "pending"}]},
                {"status": "running", "seats": []}]}),
            &json!({"version": 1, "summary": {"id": "busy", "status": "running"}}),
        );
        assert_eq!(delete(root.path(), "busy")["status"], "pending_review");
        assert_eq!(count(root.path(), "conversations", "busy"), 1);
    }

    #[test]
    fn protocol_requires_post_action_and_valid_batch() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "a1");
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
        assert_eq!(count(root.path(), "conversations", "a1"), 1);
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
    fn management_lists_conversations_and_probes_locks() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "stale");
        fixture(root.path(), "busy");
        let lock = lock_file(root.path(), "busy").unwrap();
        lock.try_lock().unwrap();
        let reader = Reader::new(root.path().to_owned());
        let response = reader.respond("/manage/state");
        let payload: Value = serde_json::from_slice(&response.body).unwrap();
        let items = payload["items"].as_array().unwrap();
        let stale = items.iter().find(|v| v["id"] == "stale").unwrap();
        assert_eq!(stale["status"], "interrupted");
        assert_eq!(stale["deletable"], true);
        assert!(stale["updated_ms"].as_i64().is_some());
        let busy = items.iter().find(|v| v["id"] == "busy").unwrap();
        assert_eq!(busy["status"], "running");
        assert_eq!(busy["deletable"], false);
        // A crashed writer's running record is deletable.
        assert_eq!(delete(root.path(), "stale")["status"], "deleted");
        let after = list(root.path(), reader.overview());
        assert_eq!(after["items"].as_array().unwrap().len(), 1);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(list(empty.path(), Overview::default())["missing"], true);
    }

    #[test]
    fn unreadable_rounds_keep_the_conversation() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "broken");
        crate::db::write(root.path())
            .unwrap()
            .unwrap()
            .execute(
                "INSERT INTO rounds (id, conversation_id, number, body) \
                 VALUES ('r', 'broken', 1, '{broken')",
                [],
            )
            .unwrap();
        assert_eq!(delete(root.path(), "broken")["status"], "error");
        assert_eq!(count(root.path(), "conversations", "broken"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_cannot_redirect_cleanup() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "original");
        let target = root.path().join("other.json");
        fs::write(&target, "keep").unwrap();
        let lock = crate::db::lock_path(root.path(), "original");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        symlink(&target, &lock).unwrap();
        assert_eq!(delete(root.path(), "original")["status"], "error");
        assert_eq!(count(root.path(), "conversations", "original"), 1);
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn python_flock_excludes_rust_and_cleanup_retries_then_rereads() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "python");
        // Any Python 3 works: only the standard library's fcntl and sqlite3 are used, as the
        // servers do.
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(
                r#"
import fcntl, json, pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1])
with (root / 'locks' / 'python.lock').open('a+b') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    print('locked', flush=True)
    sys.stdin.readline()
    with sqlite3.connect(root / 'agentnave.db') as db:
        db.execute("INSERT INTO rounds (id, conversation_id, number, body) VALUES "
                   "('r', 'python', 1, ?)", (json.dumps({'seats': [{'review': 'pending'}]}),))
"#,
            )
            .arg(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        fs::create_dir_all(root.path().join("locks")).unwrap();
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
        assert_eq!(count(root.path(), "conversations", "python"), 1);
    }
}
