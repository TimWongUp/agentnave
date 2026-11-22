use super::*;

#[test]
fn failed_write_stays_attempted_but_clean_blocked_or_cancelled_can_retry() {
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
        std::slice::from_ref(&item),
        WakeupOutcome::Cancelled,
        &mut attempted,
        &mut failed,
    );
    assert!(!attempted.contains("one") && !failed.contains("one"));
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
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
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
fn prune_state_preserves_attempted_while_waiting_and_clears_departed_keys() {
    let mut state = WakeupState::default();
    state.attempted.insert("review-1".into());
    state.attempted.insert("done-1".into());
    state.seen.insert("review-1".into(), Instant::now());
    state.seen.insert("done-1".into(), Instant::now());
    state.failed.insert("done-1".into());

    let mut raw = HashMap::new();
    raw.insert(
        "boss".to_owned(),
        RawBoss {
            waiting: true,
            items: vec![Item {
                key: "review-1".into(),
                kind: "review".into(),
                role: None,
                status: None,
            }],
        },
    );
    prune_state(&raw, &mut state);
    assert!(state.attempted.contains("review-1"));
    assert!(!state.seen.contains_key("review-1"));
    assert!(!state.attempted.contains("done-1"));
    assert!(!state.seen.contains_key("done-1"));
    assert!(!state.failed.contains("done-1"));
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
fn running_blocker_is_worded_as_blocked_not_ended() {
    let blocked = Item {
        key: "blocked:s-1:provider_failed".into(),
        kind: "blocked".into(),
        role: Some("审查".into()),
        status: Some("execution_blocked".into()),
    };
    let text = message(&[blocked]);
    assert!(text.contains("审查（受阻）"), "{text}");
    assert!(!text.contains("已结束"), "{text}");
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

#[cfg(unix)]
#[test]
fn codex_failed_delivery_retries_until_sent_then_deduplicates() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    let temp = tempfile::tempdir().unwrap();
    let sock = temp.path().join("wakeup-retry.sock");
    fs::create_dir(temp.path().join("connections")).unwrap();
    fs::create_dir(temp.path().join("inbox")).unwrap();

    let tid = "019a0000-0000-7000-8000-000000000099";
    let boss_key = crate::data::boss_app::boss_key("codex", tid);
    let turn_attempts = Arc::new(AtomicUsize::new(0));
    let attempts = turn_attempts.clone();

    let codex = super::super::codex_server::tests::connect_to_mock(&sock, move |msg, stream| {
        let id = msg.get("id").and_then(serde_json::Value::as_u64);
        match msg.get("method").and_then(serde_json::Value::as_str) {
            Some("initialize") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
            ),
            Some("thread/read") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "idle"}}}}).to_string(),
            ),
            Some("thread/queue/add") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unsupported"}}).to_string(),
            ),
            Some("turn/start") => {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                let reply = if count == 0 {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "transient"}})
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "result": {"turn": {"id": "t1"}}})
                };
                super::super::codex_server::tests::write_server_frame(stream, &reply.to_string());
            }
            _ => {}
        }
    });

    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
    command.args(["-c", "sleep 30"]);
    command.cwd(temp.path());
    let session = Session::start(command, 80, 24).unwrap();
    let pid = session.pid().unwrap();
    fs::write(
        temp.path()
            .join("connections")
            .join(format!("{pid}-test.json")),
        json!({"bosses": [&boss_key]}).to_string(),
    )
    .unwrap();
    let current = std::process::id();
    fs::write(
        temp.path().join("inbox").join(format!("{current}.json")),
        json!({"version": 1, "pid": current, "bosses": {&boss_key: {"waiting": false, "items": [{"key": "evt-retry", "kind": "task_end"}]}}}).to_string(),
    )
    .unwrap();

    let terminals = Terminals::default();
    terminals.lock().open.push(super::super::Running {
        id: 1,
        provider: "codex".into(),
        cwd: temp.path().into(),
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex: Some(codex.clone()),
    });

    let mut state = WakeupState::default();

    // First scan: fallback turn/start fails -> stays unattempted, marked failed/pending, retries next scan.
    scan(&terminals, temp.path(), &mut state);
    assert!(!state.attempted.contains("evt-retry"));
    assert!(state.failed.contains("evt-retry"));
    assert_eq!(terminals.lock().open[0].pending, 1);

    // Second scan: retries and succeeds -> marked attempted, cleared from failed/pending.
    scan(&terminals, temp.path(), &mut state);
    assert!(state.attempted.contains("evt-retry"));
    assert!(!state.failed.contains("evt-retry"));
    assert_eq!(terminals.lock().open[0].pending, 0);

    // Third scan: already attempted -> must not duplicate.
    scan(&terminals, temp.path(), &mut state);
    assert_eq!(turn_attempts.load(Ordering::SeqCst), 2);

    codex.close();
    session.stop();
}

#[cfg(unix)]
#[test]
fn codex_queued_wakeup_updates_on_partial_read_and_deletes_when_cleared() {
    use std::sync::Mutex;
    use std::time::SystemTime;

    let temp = tempfile::tempdir().unwrap();
    let sock = temp.path().join("wakeup-cancel.sock");
    fs::create_dir(temp.path().join("connections")).unwrap();
    fs::create_dir(temp.path().join("inbox")).unwrap();

    let tid = "019a0000-0000-7000-8000-000000000088";
    let boss_key = crate::data::boss_app::boss_key("codex", tid);
    let calls: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
    let recorded = calls.clone();

    let codex = super::super::codex_server::tests::connect_to_mock(&sock, move |msg, stream| {
        let id = msg.get("id").and_then(serde_json::Value::as_u64);
        let method = msg.get("method").and_then(serde_json::Value::as_str);
        if let Some(m) = method
            && m.starts_with("thread/queue/")
        {
            recorded
                .lock()
                .unwrap()
                .push((m.to_owned(), msg["params"].clone()));
        }
        match method {
            Some("initialize") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
            ),
            Some("thread/read") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "active", "activeFlags": []}}}}).to_string(),
            ),
            Some("thread/queue/add") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"queuedSubmission": {"id": "q-ab", "clientUserMessageId": msg["params"]["clientUserMessageId"], "input": []}}}).to_string(),
            ),
            Some("thread/queue/update") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"queuedSubmission": {"id": "q-ab", "clientUserMessageId": "evt-a,evt-b", "input": []}}}).to_string(),
            ),
            Some("thread/queue/delete") => super::super::codex_server::tests::write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"deleted": true}}).to_string(),
            ),
            _ => {}
        }
    });

    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
    command.args(["-c", "sleep 30"]);
    command.cwd(temp.path());
    let session = Session::start(command, 80, 24).unwrap();
    let pid = session.pid().unwrap();
    fs::write(
        temp.path()
            .join("connections")
            .join(format!("{pid}-test.json")),
        json!({"bosses": [&boss_key]}).to_string(),
    )
    .unwrap();
    let current = std::process::id();
    let inbox_path = temp.path().join("inbox").join(format!("{current}.json"));
    let write_items = |items: serde_json::Value| {
        fs::write(
            &inbox_path,
            json!({"version": 1, "pid": current, "bosses": {&boss_key: {"waiting": false, "items": items}}}).to_string(),
        )
        .unwrap();
    };
    write_items(json!([
        {"key": "evt-a", "kind": "task_end", "role": "Alpha", "status": "succeeded"},
        {"key": "evt-b", "kind": "task_end", "role": "Beta", "status": "succeeded"}
    ]));

    let terminals = Terminals::default();
    terminals.lock().open.push(super::super::Running {
        id: 1,
        provider: "codex".into(),
        cwd: temp.path().into(),
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex: Some(codex.clone()),
    });

    let mut state = WakeupState::default();
    scan(&terminals, temp.path(), &mut state);
    assert_eq!(state.queued.len(), 1);
    assert!(state.attempted.contains("evt-a") && state.attempted.contains("evt-b"));

    // Boss reads evt-a -> queued wakeup shrinks via thread/queue/update to Beta only.
    write_items(json!([
        {"key": "evt-b", "kind": "task_end", "role": "Beta", "status": "succeeded"}
    ]));
    scan(&terminals, temp.path(), &mut state);
    assert!(!state.attempted.contains("evt-a") && state.attempted.contains("evt-b"));
    assert_eq!(state.queued.len(), 1);
    assert_eq!(state.queued[0].item_keys, vec!["evt-b".to_owned()]);

    // Boss reads evt-b -> queued wakeup is cancelled via thread/queue/delete.
    fs::remove_file(&inbox_path).unwrap();
    scan(&terminals, temp.path(), &mut state);
    assert!(state.queued.is_empty());
    assert!(state.attempted.is_empty());

    // Subsequent scan sends nothing further.
    scan(&terminals, temp.path(), &mut state);
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0].0, "thread/queue/add");
    assert_eq!(recorded[1].0, "thread/queue/update");
    assert_eq!(recorded[1].1["queuedSubmissionId"], "q-ab");
    let updated_text = recorded[1].1["input"][0]["text"].as_str().unwrap();
    assert!(updated_text.contains("Beta") && !updated_text.contains("Alpha"));
    assert_eq!(recorded[2].0, "thread/queue/delete");
    assert_eq!(recorded[2].1["threadId"], tid);
    assert_eq!(recorded[2].1["queuedSubmissionId"], "q-ab");

    codex.close();
    session.stop();
}

#[cfg(unix)]
#[test]
fn pty_wakeup_cancels_with_backspaces_when_inbox_clears_during_hold_window() {
    use std::time::SystemTime;

    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("connections")).unwrap();
    fs::create_dir(temp.path().join("inbox")).unwrap();

    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "stty raw -echo; printf '\\033]7501;state=idle\\007ready'; sleep 30",
    ]);
    command.cwd(temp.path());
    let session = Session::start(command, 80, 24).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && session.program_state().as_deref() != Some("idle") {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.program_state().as_deref(), Some("idle"));

    let pid = session.pid().unwrap();
    let key = "s-22222222222222222222222222222222";
    fs::write(
        temp.path()
            .join("connections")
            .join(format!("{pid}-test.json")),
        json!({"bosses": [key]}).to_string(),
    )
    .unwrap();
    let current = std::process::id();
    let inbox_path = temp.path().join("inbox").join(format!("{current}.json"));
    fs::write(
        &inbox_path,
        json!({"version": 1, "pid": current, "bosses": {key: {"waiting": false, "items": [{"key": "evt-pty", "kind": "task_end"}]}}}).to_string(),
    )
    .unwrap();

    let terminals = Terminals::default();
    terminals.lock().open.push(super::super::Running {
        id: 1,
        provider: "claude".into(),
        cwd: temp.path().into(),
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex: None,
    });

    session.record_writes();
    let held = session.clone();
    let clear_path = inbox_path.clone();
    let clearer = thread::spawn(move || {
        let wait_until = Instant::now() + Duration::from_secs(5);
        while !held.is_holding() && Instant::now() < wait_until {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(held.is_holding(), "PTY wakeup never entered hold window");
        fs::remove_file(&clear_path).unwrap();
    });

    let mut state = WakeupState::default();
    scan(&terminals, temp.path(), &mut state);
    clearer.join().unwrap();

    let writes = session.recorded_writes();
    assert!(
        writes
            .iter()
            .any(|b| b.windows(11).any(|w| w == b"[AgentNave]"))
    );
    assert!(!writes.iter().any(|b| b.as_slice() == b"\r"));
    assert!(
        writes
            .iter()
            .any(|b| !b.is_empty() && b.iter().all(|&c| c == 0x7f))
    );
    assert!(!state.attempted.contains("evt-pty"));
    assert!(!state.failed.contains("evt-pty"));
    assert_eq!(terminals.lock().open[0].pending, 0);

    session.stop();
}
