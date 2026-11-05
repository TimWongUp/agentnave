use super::*;

fn lock(root: &Path, id: &str) -> fs::File {
    let path = crate::db::lock_path(root, id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"owner evidence").unwrap();
    fs::File::options()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

fn now() -> String {
    crate::stage::iso(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64,
    )
}

#[test]
fn orphaned_task_stops_running_across_api_caches_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let view = task_snapshot("task", "running", &now());
    crate::db::put(
        root,
        "task",
        &json!({"kind": "task", "tasks": view["director"]["tasks"]}),
        &view,
    );
    let owner = lock(root, "task");
    owner.try_lock().unwrap();
    let database_before = fs::read(crate::db::path(root)).unwrap();
    let reader = Reader::new(root.to_path_buf());
    let path = format!("/app/chats/{MANAGER}");
    assert_eq!(reader.overview().running, 1);
    assert_eq!(state(&reader, "/app/stage")["counts"]["running"], 1);
    assert_eq!(
        state(&reader, "/app/chats")["groups"][0]["status"],
        "running"
    );
    assert!(
        state(&reader, &path)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["kind"] == "typing")
    );

    // No revision or heartbeat change: releasing the OS lock alone must invalidate responses.
    drop(owner);
    let overview = reader.overview();
    assert_eq!(overview.running, 0);
    assert_eq!(overview.items[0].status, "interrupted");
    let projected = reader.scan().1[0].view.clone();
    assert!(Arc::ptr_eq(&projected, &reader.scan().1[0].view));
    assert_eq!(state(&reader, "/app/stage")["counts"]["running"], 0);
    assert_eq!(
        state(&reader, "/app/chats")["groups"][0]["status"],
        "failed"
    );
    let chat = state(&reader, &path);
    let messages = chat["messages"].as_array().unwrap();
    assert!(!messages.iter().any(|m| m["kind"] == "typing"));
    let notice = messages.iter().find(|m| m["kind"] == "notice").unwrap();
    assert_eq!(notice["status"], "interrupted");
    assert_eq!(notice["text"], "已中断");
    assert!(!notice["detail"].as_str().unwrap().is_empty());
    assert!(
        messages
            .iter()
            .any(|m| m["text"] == view["director"]["tasks"][0]["output"])
    );
    assert_eq!(fs::read(crate::db::path(root)).unwrap(), database_before);
    let connection = crate::db::read(root).unwrap().unwrap();
    assert_eq!(view_body(&connection, "task").unwrap(), view.to_string());
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    assert_eq!(
        fs::read(crate::db::lock_path(root, "task")).unwrap(),
        b"owner evidence"
    );
}

#[test]
fn orphaned_heartbeat_expiry_and_owner_changes_do_not_need_a_new_revision() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // An arbitrarily old task with no new output is still alive while its owner holds it.
    save(
        root,
        &task_snapshot("task", "running", "2020-01-01T00:00:00Z"),
    );
    let owner = lock(root, "task");
    owner.try_lock().unwrap();
    let reader = Reader::new(root.to_path_buf());
    assert_eq!(reader.overview().running, 1);
    let live = root.join("live/owner.json");
    write(
        &live,
        &json!({"version": 2, "conversations": {"task": {"tasks": {"t-task": {"output": "live tail"}}}}}),
    );
    let path = format!("/app/chats/{MANAGER}");
    let chat = state(&reader, &path);
    assert_eq!(chat["live"], true);
    drop(owner);
    // A recently released owner can leave a fresh heartbeat: keep showing it until expiry.
    assert_eq!(state(&reader, &path)["status"], "running");
    fs::File::options()
        .write(true)
        .open(&live)
        .unwrap()
        .set_modified(SystemTime::now() - LIVE_FRESH - Duration::from_secs(1))
        .unwrap();
    let chat = state(&reader, &path);
    assert_eq!(chat["live"], false);
    assert!(
        !chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["kind"] == "typing")
    );
    assert_eq!(reader.overview().running, 0);
    // Cached recovery never replaces the saved view: a new owner is conservative again.
    let owner = fs::File::options()
        .read(true)
        .write(true)
        .open(crate::db::lock_path(root, "task"))
        .unwrap();
    owner.try_lock().unwrap();
    assert_eq!(state(&reader, &path)["status"], "running");
    assert_eq!(reader.overview().running, 1);
    drop(owner);
    assert_eq!(state(&reader, &path)["status"], "failed");
}

#[test]
fn orphaned_projection_rereads_completed_view_and_ignores_newer_live_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stale = Arc::new(task_snapshot("task", "running", &now()));
    save(root, &stale);
    drop(lock(root, "task"));
    let reader = Reader::new(root.to_path_buf());
    let connection = crate::db::read(root).unwrap().unwrap();
    let recovered = crate::data::orphaned::view(&reader, &connection, "task", &stale);
    assert_eq!(recovered["summary"]["status"], "interrupted");
    // Simulate a writer completing after scan read rows/body but before its lock probe.
    let done = task_snapshot("task", "succeeded", &now());
    save(root, &done);
    let latest = crate::data::orphaned::view(&reader, &connection, "task", &stale);
    assert_eq!(latest["director"]["tasks"][0]["status"], "succeeded");
    assert_eq!(latest["summary"]["status"], "succeeded");
    assert_eq!(*latest, done);
    assert_eq!(
        state(&reader, &format!("/app/chats/{MANAGER}"))["status"],
        "idle"
    );
    // A live file published after the initial scan also prevents correction.
    save(root, &stale);
    write(
        &root.join("live/new-owner.json"),
        &json!({"version": 2, "conversations": {"task": {"tasks": {}}}}),
    );
    let latest = crate::data::orphaned::view(&reader, &connection, "task", &stale);
    assert_eq!(latest["director"]["tasks"][0]["status"], "running");
}

#[test]
fn orphaned_meeting_preserves_completed_failed_and_review_answers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let at = now();
    let mut show = office_show("show", &at, &at);
    show["director"]["review"] = json!(true);
    show["director"]["seats"] = json!([
        {"id": "a", "provider": "claude", "model": "m"},
        {"id": "b", "provider": "codex", "model": "m"},
        {"id": "c", "provider": "grok", "model": "m"},
        {"id": "d", "provider": "pi", "model": "m"},
        {"id": "e", "provider": "deepseek", "model": "m"},
        {"id": "f", "provider": "zcode", "model": "m"}]);
    show["director"]["speaking"] = json!(["c"]);
    show["director"]["round"] = json!({"number": 1, "status": "review", "seats": [
        {"seat_id": "a", "status": "succeeded", "output": "answer"},
        {"seat_id": "b", "status": "succeeded", "review": "pending", "output": "review answer"},
        {"seat_id": "c", "status": "running", "output": null},
        {"seat_id": "d", "status": "pending"},
        {"seat_id": "e", "status": "failed", "error": "original failure", "output": "partial"},
        {"seat_id": "f", "status": "succeeded", "output": "unreviewed answer"}]});
    save(root, &show);
    drop(lock(root, "show"));
    let reader = Reader::new(root.to_path_buf());
    let (overview, snapshots) = reader.scan();
    assert_eq!((overview.running, overview.review), (0, 2));
    assert_eq!(overview.items[0].status, "review");
    let director = &snapshots[0].view["director"];
    let seats = director["round"]["seats"].as_array().unwrap();
    assert_eq!(seats[0], show["director"]["round"]["seats"][0]);
    assert_eq!(seats[1], show["director"]["round"]["seats"][1]);
    assert_eq!(seats[2]["status"], "interrupted");
    assert_eq!(seats[3]["status"], "not_started");
    assert_eq!(seats[4], show["director"]["round"]["seats"][4]);
    assert_eq!(seats[5]["review"], "pending");
    assert_eq!(director["speaking"], json!([]));
    assert_eq!(director["round"]["status"], "review");
    assert_eq!(
        state(&reader, "/app/stage")["counts"],
        json!({"running": 0, "review": 2})
    );
    let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
    assert_eq!(chat["status"], "review");
    let messages = chat["messages"].as_array().unwrap();
    assert!(!messages.iter().any(|m| m["kind"] == "typing"));
    for answer in ["answer", "review answer", "unreviewed answer"] {
        assert!(messages.iter().any(|m| m["text"] == answer));
    }
    assert!(
        messages
            .iter()
            .any(|m| m["status"] == "interrupted" && m["text"] == "已中断")
    );
    let connection = crate::db::read(root).unwrap().unwrap();
    assert_eq!(view_body(&connection, "show").unwrap(), show.to_string());

    // Without pending answers the round itself also ends, rather than retaining running.
    show["director"]["review"] = json!(false);
    show["director"]["round"]["status"] = json!("running");
    show["director"]["round"]["seats"][1]["review"] = Value::Null;
    save(root, &show);
    let (_, snapshots) = reader.scan();
    assert_eq!(
        snapshots[0].view["director"]["round"]["status"],
        "interrupted"
    );
    assert_eq!(snapshots[0].view["director"]["status"], "interrupted");
}

#[test]
fn orphaned_without_a_plain_readable_lock_is_not_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    save(
        root,
        &task_snapshot("task", "running", "2020-01-01T00:00:00Z"),
    );
    let reader = Reader::new(root.to_path_buf());
    let path = crate::db::lock_path(root, "task");
    assert_eq!(reader.overview().running, 1);
    assert!(!path.exists());
    fs::create_dir_all(&path).unwrap();
    assert_eq!(reader.overview().running, 1);
    fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let target = root.join("other.lock");
        fs::write(&target, b"untouched").unwrap();
        symlink(&target, &path).unwrap();
        assert_eq!(reader.overview().running, 1);
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        fs::remove_file(&path).unwrap();
        drop(lock(root, "task"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(reader.overview().running, 1);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
