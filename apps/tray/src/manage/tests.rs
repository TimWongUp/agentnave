use super::*;
use crate::data::Reader;

#[test]
fn cleanup_rechecks_recent_siblings_before_deleting_old_history() {
    let root = tempfile::tempdir().unwrap();
    for id in ["old", "newly-finished"] {
        fixture(root.path(), id);
        set(
            root.path(),
            "UPDATE conversations SET created = '2020-01-01T00:00:00Z', manager_session = 's-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE id = ?1",
            id,
        );
        set(
            root.path(),
            "UPDATE views SET body = json_set(body, '$.summary.updated', '2020-01-01T00:00:00Z') WHERE conversation_id = ?1",
            id,
        );
    }
    let ids = vec!["old".to_owned(), "newly-finished".to_owned()];
    let cutoff = crate::stage::timestamp("2026-10-01T00:00:00Z").unwrap() / 1_000;
    // A conversation created since the preview preserves the whole workspace.
    assert!(
        cleanup(root.path(), &ids[..1], cutoff)
            .iter()
            .all(|r| r["status"] == "changed")
    );
    set(
        root.path(),
        "UPDATE views SET body = json_set(body, '$.summary.updated', '2026-10-25T00:00:00Z') WHERE conversation_id = ?1",
        "newly-finished",
    );
    assert!(
        cleanup(root.path(), &ids, cutoff)
            .iter()
            .all(|r| r["status"] == "changed")
    );
    assert_eq!(count(root.path(), "conversations", "old"), 1);
    assert_eq!(count(root.path(), "conversations", "newly-finished"), 1);
    set(
        root.path(),
        "DELETE FROM views WHERE conversation_id = ?1",
        "newly-finished",
    );
    assert!(
        cleanup(root.path(), &ids, cutoff)
            .iter()
            .all(|r| r["status"] == "changed")
    );
    set(
        root.path(),
        "UPDATE conversations SET manager_session = NULL WHERE id = ?1",
        "newly-finished",
    );
    let workspace = "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(handover(root.path(), workspace)["status"], "waiting");
    assert!(
        cleanup(root.path(), &ids[..1], cutoff)
            .iter()
            .all(|r| r["status"] == "deleted")
    );
    // The emptied workspace's handover goes with its last conversation.
    let handovers: i64 = crate::db::read(root.path())
        .unwrap()
        .unwrap()
        .query_row("SELECT count(*) FROM workspaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(handovers, 0);
    assert_eq!(delete(root.path(), "newly-finished")["status"], "deleted");
}

#[test]
fn a_workspace_with_only_a_boss_stays_until_it_is_deleted_itself() {
    let root = tempfile::tempdir().unwrap();
    let only_boss = "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let used = "s-cccccccccccccccccccccccccccccccc";
    fixture(root.path(), "gone");
    set(
        root.path(),
        "UPDATE tasks SET status = 'succeeded' WHERE conversation_id = ?1",
        "gone",
    );
    set(
        root.path(),
        "UPDATE conversations SET manager_session = ?1 WHERE id = 'gone'",
        used,
    );
    for id in [only_boss, used] {
        set(
            root.path(),
            "INSERT INTO workspaces (id, owner, pending, title) VALUES (?1, ?1, 1, '发版')",
            id,
        );
    }
    let rows = || -> Vec<String> {
        crate::db::read(root.path())
            .unwrap()
            .unwrap()
            .prepare("SELECT id FROM workspaces")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };

    // Deleting another workspace's last conversation removes that workspace, not this one.
    assert_eq!(delete(root.path(), "gone")["status"], "deleted");
    assert_eq!(rows(), [only_boss]);
    assert_eq!(handover(root.path(), only_boss)["status"], "waiting");
    assert_eq!(forget(root.path(), only_boss)["status"], "deleted");
    assert!(rows().is_empty());
    assert_eq!(forget(root.path(), only_boss)["status"], "missing");
}

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
    assert_eq!(
        rename(root.path(), "named", &"汉".repeat(25))["status"],
        "error"
    );
    assert_eq!(
        rename(root.path(), "named", &"🚀".repeat(25))["status"],
        "error"
    );
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
        br#"{"ids":["a1"],"updated_before":"yesterday"}"#,
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
