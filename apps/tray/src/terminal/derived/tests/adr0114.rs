use super::*;
use std::io::Write;
use std::process::{Command, Stdio};

const CLAUDE_SID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const AGY_CID: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

fn sleep_session_with_osc(cwd: &Path, osc: &str) -> Arc<Session> {
    let mut cmd = CommandBuilder::new("python3");
    cmd.args([
        "-c",
        "import os, sys, time; os.write(1, sys.argv[1].encode()); time.sleep(10)",
        osc,
    ]);
    cmd.cwd(cwd);
    let session = Session::start(cmd, 80, 24).unwrap();
    session.record_writes();
    let deadline = Instant::now() + Duration::from_secs(3);
    while session.osc_snapshot().0 == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(15));
    }
    assert!(session.osc_snapshot().0 > 0, "OSC state did not arrive");
    session
}

fn register_test_derived(
    terminals: &Terminals,
    provider: &str,
    cwd: &Path,
    session: &Arc<Session>,
    sid: &str,
    inv_id: &str,
    hook_dir: Option<PathBuf>,
) -> DerivedHandle {
    let tid = terminals.register_derived(NewDerived {
        provider: provider.to_owned(),
        cwd: cwd.to_path_buf(),
        session: session.clone(),
        native: Some(sid.to_owned()),
        account: None,
        codex: None,
        info: DerivedInfo {
            parent_boss_key: "claude:boss".to_owned(),
            parent_workspace_id: Some("ws".to_owned()),
            busy: true,
            blocked_kind: None,
            isolation_id: None,
            _lease: None,
            _log_dir: None,
            _hook_dir: None,
            codex_ready_file: None,
            active_invocation_id: Some(inv_id.to_owned()),
            launch_config: None,
        },
    });
    DerivedHandle {
        id: tid,
        provider: provider.to_owned(),
        cwd: cwd.to_path_buf(),
        session: session.clone(),
        native: Some(sid.to_owned()),
        account: None,
        launch_config: None,
        hook_dir,
        codex: None,
    }
}

#[test]
fn claude_permanent_stop_hook_injected_without_fallback_and_filters_events() {
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let cfg = RunConfig {
        provider: "claude".to_owned(),
        account: None,
        account_dir: None,
        cwd: "/tmp".to_owned(),
        isolation: None,
        model: None,
        effort: None,
        permissions: json!({"permission_mode": "bypassPermissions"}),
    };
    let args = build_claude_args(&cfg, None, Some(CLAUDE_SID), Some(hook_dir.path()));
    let idx = args.iter().position(|a| a == "--settings").unwrap();
    let settings: Value = serde_json::from_str(&args[idx + 1]).unwrap();
    assert!(settings["hooks"]["Stop"].is_array());
    assert!(settings["hooks"]["StopFailure"].is_array());
    assert!(settings["hooks"].get("UserPromptSubmit").is_none());

    let cmd = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let payloads = [
        // SubagentStop must be ignored.
        r#"{"hook_event_name":"SubagentStop","session_id":"s1","agent_type":""}"#,
        // First Stop with waking background task.
        r#"{"hook_event_name":"Stop","session_id":"s1","prompt_id":"p1","background_tasks":[{"id":"b1","type":"shell","status":"running"}]}"#,
        // Second Stop (proves permanent, not one-shot) with empty background_tasks.
        r#"{"hook_event_name":"Stop","session_id":"s1","prompt_id":"p2","background_tasks":[]}"#,
        // Old Claude Stop missing background_tasks field altogether.
        r#"{"hook_event_name":"Stop","session_id":"s1","prompt_id":"p3"}"#,
        // StopFailure preserved with details.
        r#"{"hook_event_name":"StopFailure","session_id":"s1","error":"rate_limit","error_details":"slow down"}"#,
    ];
    for body in payloads {
        let mut child = Command::new("/bin/sh")
            .args(["-c", cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        assert!(child.wait_with_output().unwrap().status.success());
    }

    let raw = fs::read_to_string(claude_hook::events_path(hook_dir.path())).unwrap();
    let lines: Vec<Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["hook_event_name"], json!("Stop"));
    assert_eq!(lines[0]["background_tasks"][0]["id"], json!("b1"));
    assert_eq!(lines[1]["background_tasks"], json!([]));
    assert!(lines[2].get("background_tasks").is_none());
    assert_eq!(lines[3]["hook_event_name"], json!("StopFailure"));
}

#[test]
fn claude_waits_for_waking_bg_tasks_pairs_task_notification_and_writes_end_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    // Pre-dispatch user chat event in events.jsonl must be ignored via baseline offset.
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({"hook_event_name":"Stop","session_id":CLAUDE_SID,"prompt_id":"p-old","background_tasks":[]})
        ),
    )
    .unwrap();
    let hook_offset = native_logs::file_len(&events_file);

    // Sub-turn 0 finishes with a waking shell bg task.
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"promptId\":\"p-1\",\"message\":{\"content\":\"run build\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"started bg\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\",\"uuid\":\"d1\",\"parentUuid\":\"a1\"}\n"
        ),
    )
    .unwrap();
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-1",
                "background_tasks": [{"id": "bg-1", "type": "shell", "status": "running"}]
            })
        )
        .unwrap();
    }

    // Even when OSC reports done immediately, monitor must NOT settle while bg-1 is running.
    let session = sleep_session_with_osc(
        dir.path(),
        "\x1b]7501;state=working\x07\x1b]7501;state=done\x07",
    );
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-bg", "inv-bg");
    let handle = coord.register_run("inv-bg");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-bg",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: hook_offset,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };

    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-bg",
            CLAUDE_SID,
            &baseline,
            "run build",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(500),
                bg_reminder_delay: Duration::from_secs(60),
            },
        )
    });

    // Wait until monitor enters bg_only_waiting and verify it did NOT settle early.
    let wait_bg = Instant::now() + Duration::from_secs(3);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_bg {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(handle.is_bg_only_waiting());
    assert_eq!(
        coord.run_receipt_by_invocation("inv-bg").unwrap().state,
        ReceiptState::Started
    );

    // Task notification wakes sub-turn 1; native_logs must NOT treat it as Superseded.
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "uuid": "u2",
                "promptId": "p-notif-1",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification><task-id>bg-1</task-id></task-notification>"}
            })
        )
        .unwrap();
    }
    assert_eq!(
        native_logs::claude_turn_state(&transcript, "p-1"),
        native_logs::ClaudeTurnState::Active
    );

    // Sub-turn 1 completes with only non-waking `dream` task -> must settle cleanly!
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","uuid":"a2","parentUuid":"u2","message":{"content":[{"type":"text","text":"build ok"}]}})
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"system","subtype":"turn_duration","uuid":"d2","parentUuid":"a2"})
        )
        .unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-notif-1",
                "background_tasks": [{"id": "d-1", "type": "dream", "status": "running"}]
            })
        )
        .unwrap();
    }

    monitor.join().unwrap();
    let receipt = coord.run_receipt_by_invocation("inv-bg").unwrap();
    assert_eq!(receipt.state, ReceiptState::Finished);
    let term = receipt.terminal.unwrap();
    assert_eq!(term.status, "succeeded");
    assert!(term.warnings.is_empty());
    let anchor = receipt.anchor.unwrap();
    assert_eq!(anchor["prompt_id"], json!("p-1"));
    assert_eq!(anchor["end_prompt_id"], json!("p-notif-1"));
    assert_eq!(anchor["end_row_index"], json!(6));
    session.stop();
}

#[test]
fn claude_old_stop_missing_background_tasks_settles_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join(format!("{CLAUDE_SID}.jsonl")),
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-old-ver\",\"message\":{\"content\":\"hi\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
        ),
    )
    .unwrap();
    fs::write(
        claude_hook::events_path(hook_dir.path()),
        format!(
            "{}\n",
            json!({"hook_event_name":"Stop","session_id":CLAUDE_SID,"prompt_id":"p-old-ver"})
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-old", "inv-old");
    let handle = coord.register_run("inv-old");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-old",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };

    claude::monitor_claude_turn(
        &coord,
        &terminals,
        &launched,
        "req-old",
        CLAUDE_SID,
        &baseline,
        "hi",
        &handle,
        Some(hook_dir.path()),
        false,
    );
    let term = coord
        .run_receipt_by_invocation("inv-old")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(term.status, "succeeded");
    assert!(
        term.warnings
            .iter()
            .any(|w| w.contains("后台任务完成未确认"))
    );
    session.stop();
}

#[test]
fn claude_reminder_ignores_monitor_intermediate_event_and_bg_only_cancel_sends_no_keys() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-mon\",\"message\":{\"content\":\"watch\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"watching\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
        ),
    )
    .unwrap();
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-mon",
                "background_tasks": [{"id": "mon-1", "type": "monitor", "status": "running"}]
            })
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=idle\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-mon", "inv-mon");
    coord.insert_test_receipt("cancel-mon", "inv-mon-cancel");
    let handle = coord.register_run("inv-mon");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-mon",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };

    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-mon",
            CLAUDE_SID,
            &baseline,
            "watch",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(500),
                bg_reminder_delay: Duration::from_millis(250),
            },
        )
    });

    // Wait 120ms, then inject a Monitor intermediate event keeping the SAME monitor task id.
    thread::sleep(Duration::from_millis(120));
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "promptId": "p-mon-tick",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification><task-id>mon-1</task-id></task-notification>"}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"still watching"}]}})
        )
        .unwrap();
        writeln!(f, "{}", json!({"type":"system","subtype":"turn_duration"})).unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-mon-tick",
                "background_tasks": [{"id": "mon-1", "type": "monitor", "status": "running"}]
            })
        )
        .unwrap();
    }

    // Reminder should fire with count/task id and WITHOUT body text.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut rem = None;
    while Instant::now() < deadline {
        if let Some(r) = coord.run_receipt_by_invocation("inv-mon")
            && let Some(bg) = r.background_tasks_pending
        {
            rem = Some(bg);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let rem = rem.expect("expected background_tasks_pending reminder");
    assert_eq!(rem.code, "background_tasks_pending");
    assert_eq!(rem.count, Some(1));
    assert_eq!(rem.tasks, vec!["monitor:mon-1".to_owned()]);
    assert!(!rem.reason.contains("watching"));

    // Wait for the second paired Stop to leave monitor in bg_only_waiting before cancelling.
    let wait_ready = Instant::now() + Duration::from_secs(2);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_ready {
        thread::sleep(Duration::from_millis(15));
    }
    assert!(handle.is_bg_only_waiting());

    // Now cancel while only monitor bg task remains -> ZERO keys sent to PTY, settles succeeded!
    let run_receipt = coord.run_receipt_by_invocation("inv-mon").unwrap();
    let id = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let cancel_env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-mon".to_owned(),
        kind: "cancel".to_owned(),
        service: id.clone(),
        target_tray: id,
        invocation_id: "inv-mon".to_owned(),
        parent: None,
        session_id: Some(CLAUDE_SID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    cancel_claude(&coord, &cancel_env, &run_receipt, &launched);
    monitor.join().unwrap();

    assert!(session.recorded_writes().is_empty());
    assert_eq!(
        coord.receipts_snapshot()["cancel-mon"]
            .terminal
            .as_ref()
            .map(|t| t.status.as_str()),
        Some("already_finished")
    );
    let final_run = coord.run_receipt_by_invocation("inv-mon").unwrap();
    assert_eq!(final_run.state, ReceiptState::Finished);
    let term = final_run.terminal.unwrap();
    assert_eq!(term.status, "succeeded");
    assert!(term.warnings.iter().any(|w| w.contains("后台任务留给用户")));
    assert_eq!(final_run.anchor.unwrap()["end_row_index"], json!(6));
    session.stop();
}

#[test]
fn agy_requires_done_with_reply_reminds_while_working_and_bg_cancel_sends_no_keys() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = native_logs::agy_transcript_path(dir.path(), AGY_CID);
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"step_index\":0,\"content\":\"<USER_REQUEST>run bg</USER_REQUEST>\"}\n",
            "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"started server\"}\n"
        ),
    )
    .unwrap();

    // 1) OSC stays `working` after final reply -> fires reminder and bg-only cancel sends no keys.
    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["antigravity"],
    );
    coord.insert_test_receipt("req-agy", "inv-agy");
    coord.insert_test_receipt("cancel-agy", "inv-agy-cancel");
    let handle = coord.register_run("inv-agy");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "antigravity",
        dir.path(),
        &session,
        AGY_CID,
        "inv-agy",
        None,
    );
    let baseline = tui::TurnBaseline {
        claude_homes: Vec::new(),
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };

    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let home_t = dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        tui::monitor_agy_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-agy",
            Some(AGY_CID),
            None,
            &baseline,
            "run bg",
            &handle_t,
            Some(home_t),
            tui::AgyMonitorConfig {
                done_reply_wait: Duration::from_millis(300),
                bg_reminder_delay: Duration::from_millis(150),
            },
        );
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut rem = None;
    while Instant::now() < deadline {
        if let Some(r) = coord.run_receipt_by_invocation("inv-agy")
            && let Some(bg) = r.background_tasks_pending
        {
            rem = Some(bg);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let rem = rem.expect("expected agy background_tasks_pending reminder");
    assert_eq!(rem.code, "background_tasks_pending");
    assert!(!rem.reason.contains("started server"));

    let run_receipt = coord.run_receipt_by_invocation("inv-agy").unwrap();
    let id = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let cancel_env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-agy".to_owned(),
        kind: "cancel".to_owned(),
        service: id.clone(),
        target_tray: id,
        invocation_id: "inv-agy".to_owned(),
        parent: None,
        session_id: Some(AGY_CID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    cancel_agy(&coord, &cancel_env, &run_receipt, &launched);
    monitor.join().unwrap();
    assert!(session.recorded_writes().is_empty());
    let final_run = coord.run_receipt_by_invocation("inv-agy").unwrap();
    let term = final_run.terminal.unwrap();
    assert_eq!(term.status, "succeeded");
    assert!(term.warnings.iter().any(|w| w.contains("后台任务留给用户")));
    assert_eq!(final_run.anchor.unwrap()["end_row_index"], json!(2));
    session.stop();

    // 2) OSC `done` without any final PLANNER_RESPONSE -> bounded wait then fails truthfully.
    let no_reply_cid = "cccccccc-3333-4333-8333-cccccccccccc";
    let t2 = native_logs::agy_transcript_path(dir.path(), no_reply_cid);
    fs::create_dir_all(t2.parent().unwrap()).unwrap();
    fs::write(
        &t2,
        "{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"step_index\":0,\"content\":\"<USER_REQUEST>no reply</USER_REQUEST>\"}\n",
    )
    .unwrap();
    let session2 = sleep_session_with_osc(
        dir.path(),
        "\x1b]7501;state=working\x07\x1b]7501;state=done\x07",
    );
    coord.insert_test_receipt("req-agy-empty", "inv-agy-empty");
    let handle2 = coord.register_run("inv-agy-empty");
    let launched2 = register_test_derived(
        &terminals,
        "antigravity",
        dir.path(),
        &session2,
        no_reply_cid,
        "inv-agy-empty",
        None,
    );
    let baseline2 = tui::TurnBaseline {
        claude_homes: Vec::new(),
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    tui::monitor_agy_turn_with_config(
        &coord,
        &terminals,
        &launched2,
        "req-agy-empty",
        Some(no_reply_cid),
        None,
        &baseline2,
        "no reply",
        &handle2,
        Some(dir.path().to_path_buf()),
        tui::AgyMonitorConfig {
            done_reply_wait: Duration::from_millis(200),
            bg_reminder_delay: Duration::from_secs(60),
        },
    );
    let term2 = coord
        .run_receipt_by_invocation("inv-agy-empty")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(term2.status, "failed");
    assert!(
        term2
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("未在转录中读到最终回复"))
    );
    session2.stop();
}

#[test]
fn resume_returns_busy_when_session_has_unsettled_dispatch_for_claude_and_codex() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude", "codex"],
    );
    let terminals = Terminals::default();
    let sid = "dddddddd-4444-4444-8444-dddddddddddd";

    // Unsettled dispatch (Started) on `sid`.
    coord.insert_test_receipt("req-1", "inv-1");
    coord.mark_started("req-1", Some(sid), Some(json!({"session_id": sid})));

    let id = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let env2 = SpawnRequestEnvelope {
        version: 1,
        request_id: "req-2".to_owned(),
        kind: "run".to_owned(),
        service: id.clone(),
        target_tray: id,
        invocation_id: "inv-2".to_owned(),
        parent: None,
        session_id: Some(sid.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    let cfg_claude = RunConfig {
        provider: "claude".to_owned(),
        account: None,
        account_dir: None,
        cwd: dir.path().display().to_string(),
        isolation: None,
        model: None,
        effort: None,
        permissions: json!({}),
    };
    let handle2 = coord.register_run("inv-2");
    let err = prepare_open_or_launch(
        &coord,
        &terminals,
        &env2,
        &cfg_claude,
        dir.path(),
        None,
        &handle2,
        |_, _| unreachable!("must reject busy before spawning"),
    )
    .err()
    .expect("claude resume must be busy");
    assert_eq!(err, UNSETTLED_DISPATCH_BUSY_ERR);

    let cfg_codex = RunConfig {
        provider: "codex".to_owned(),
        ..cfg_claude
    };
    let parent = ParentRef {
        boss_key: "claude:boss".to_owned(),
        workspace_id: Some("ws".to_owned()),
    };
    let err_codex = codex::open_or_launch_codex(
        &coord,
        &terminals,
        &env2,
        &cfg_codex,
        &parent,
        dir.path(),
        None,
        &handle2,
    )
    .err()
    .expect("codex resume must be busy");
    assert_eq!(err_codex, UNSETTLED_DISPATCH_BUSY_ERR);
}

#[test]
fn claude_priority_stop_failure_interrupt_timeout_fallback_and_user_interjection() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();

    // 1) StopFailure fails immediately without turn_duration.
    let hook1 = claude_hook::prepare_claude_hook(false).unwrap();
    let sid1 = "11111111-aaaa-4aaa-8aaa-111111111111";
    fs::write(
        proj.join(format!("{sid1}.jsonl")),
        "{\"type\":\"user\",\"promptId\":\"p-sf\",\"message\":{\"content\":\"go\"}}\n",
    )
    .unwrap();
    fs::write(
        claude_hook::events_path(hook1.path()),
        format!(
            "{}\n",
            json!({"hook_event_name":"StopFailure","session_id":sid1,"error":"rate_limit","error_details":"429"})
        ),
    )
    .unwrap();
    let s1 = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    let terminals = Terminals::default();
    coord.insert_test_receipt("req-sf", "inv-sf");
    let h1 = coord.register_run("inv-sf");
    let l1 = register_test_derived(&terminals, "claude", dir.path(), &s1, sid1, "inv-sf", None);
    let base = |pre: u64| tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: pre,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    claude::monitor_claude_turn(
        &coord,
        &terminals,
        &l1,
        "req-sf",
        sid1,
        &base(0),
        "go",
        &h1,
        Some(hook1.path()),
        false,
    );
    let t1 = coord
        .run_receipt_by_invocation("inv-sf")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(t1.status, "failed");
    assert!(
        t1.reason
            .as_deref()
            .is_some_and(|r| r.contains("rate_limit"))
    );
    s1.stop();

    // 2) Transcript `[Request interrupted by user for tool use]` cancels without Stop/turn_duration.
    let hook2 = claude_hook::prepare_claude_hook(false).unwrap();
    let sid2 = "22222222-aaaa-4aaa-8aaa-222222222222";
    fs::write(
        proj.join(format!("{sid2}.jsonl")),
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-int\",\"message\":{\"content\":\"run bash\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Request interrupted by user for tool use]\"}]}}\n"
        ),
    )
    .unwrap();
    let s2 = sleep_session_with_osc(dir.path(), "\x1b]7501;state=idle\x07");
    coord.insert_test_receipt("req-int", "inv-int");
    let h2 = coord.register_run("inv-int");
    let l2 = register_test_derived(&terminals, "claude", dir.path(), &s2, sid2, "inv-int", None);
    claude::monitor_claude_turn(
        &coord,
        &terminals,
        &l2,
        "req-int",
        sid2,
        &base(0),
        "run bash",
        &h2,
        Some(hook2.path()),
        false,
    );
    assert_eq!(
        coord
            .run_receipt_by_invocation("inv-int")
            .unwrap()
            .terminal
            .unwrap()
            .status,
        "cancelled"
    );
    s2.stop();

    // 3) Queued user interjection + missing Stop timeout fallback -> settles with both warnings.
    let hook3 = claude_hook::prepare_claude_hook(false).unwrap();
    let sid3 = "33333333-aaaa-4aaa-8aaa-333333333333";
    fs::write(
        proj.join(format!("{sid3}.jsonl")),
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-q\",\"message\":{\"content\":\"work\"}}\n",
            "{\"type\":\"attachment\",\"attachment\":{\"type\":\"queued_command\",\"prompt\":\"also check tests\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
        ),
    )
    .unwrap();
    let s3 = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    coord.insert_test_receipt("req-q", "inv-q");
    let h3 = coord.register_run("inv-q");
    let l3 = register_test_derived(&terminals, "claude", dir.path(), &s3, sid3, "inv-q", None);
    claude::monitor_claude_turn_with_config(
        &coord,
        &terminals,
        &l3,
        "req-q",
        sid3,
        &base(0),
        "work",
        &h3,
        Some(hook3.path()),
        false,
        claude::ClaudeMonitorConfig {
            signal_pair_timeout: Duration::from_millis(150),
            bg_reminder_delay: Duration::from_secs(60),
        },
    );
    let t3 = coord
        .run_receipt_by_invocation("inv-q")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(t3.status, "succeeded");
    assert!(t3.warnings.iter().any(|w| w.contains("后台任务完成未确认")));
    assert!(t3.warnings.iter().any(|w| w.contains("参与")));
    s3.stop();

    // 4) Human sends new message after reply -> settles bounded strictly before human row.
    let hook4 = claude_hook::prepare_claude_hook(false).unwrap();
    let sid4 = "44444444-aaaa-4aaa-8aaa-444444444444";
    fs::write(
        proj.join(format!("{sid4}.jsonl")),
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-sup\",\"message\":{\"content\":\"start\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"partial\"}]}}\n",
            "{\"type\":\"user\",\"promptId\":\"p-human\",\"message\":{\"content\":\"hey wait\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"human reply\"}]}}\n"
        ),
    )
    .unwrap();
    let s4 = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    coord.insert_test_receipt("req-sup", "inv-sup");
    let h4 = coord.register_run("inv-sup");
    let l4 = register_test_derived(&terminals, "claude", dir.path(), &s4, sid4, "inv-sup", None);
    claude::monitor_claude_turn(
        &coord,
        &terminals,
        &l4,
        "req-sup",
        sid4,
        &base(0),
        "start",
        &h4,
        Some(hook4.path()),
        false,
    );
    let r4 = coord.run_receipt_by_invocation("inv-sup").unwrap();
    let t4 = r4.terminal.unwrap();
    assert_eq!(t4.status, "succeeded");
    assert!(t4.warnings.iter().any(|w| w.contains("用户参与")));
    assert_eq!(r4.anchor.unwrap()["end_row_index"], json!(2));
    s4.stop();
}

#[test]
fn claude_task_notification_without_prompt_id_does_not_regress_end_prompt_id_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"promptId\":\"p-start\",\"message\":{\"content\":\"go\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"step 0\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\",\"uuid\":\"d1\",\"parentUuid\":\"a1\"}\n"
        ),
    )
    .unwrap();
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-start",
                "background_tasks": [{"id": "bg-1", "type": "shell", "status": "running"}]
            })
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-nopid", "inv-nopid");
    let handle = coord.register_run("inv-nopid");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-nopid",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-nopid",
            CLAUDE_SID,
            &baseline,
            "go",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(400),
                bg_reminder_delay: Duration::from_secs(60),
            },
        )
    });

    let wait_bg = Instant::now() + Duration::from_secs(2);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_bg {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(handle.is_bg_only_waiting());

    // Subturn 1 task-notification without promptId whose parentUuid chains to a1, plus Stop without prompt_id.
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "uuid": "u2",
                "parentUuid": "a1",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification><task-id>bg-1</task-id></task-notification>"}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","uuid":"a2","parentUuid":"u2","message":{"content":[{"type":"text","text":"step 1"}]}})
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"system","subtype":"turn_duration","uuid":"d2","parentUuid":"a2"})
        )
        .unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "background_tasks": []
            })
        )
        .unwrap();
    }

    monitor.join().unwrap();
    let receipt = coord.run_receipt_by_invocation("inv-nopid").unwrap();
    let anchor = receipt.anchor.unwrap();
    assert_eq!(anchor["end_row_index"], json!(6));
    assert_ne!(anchor.get("end_prompt_id"), Some(&json!("p-start")));
    session.stop();
}

#[test]
fn agy_settles_when_done_transitions_to_idle_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = native_logs::agy_transcript_path(dir.path(), AGY_CID);
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"step_index\":0,\"content\":\"<USER_REQUEST>fast</USER_REQUEST>\"}\n",
            "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"quick reply\"}\n"
        ),
    )
    .unwrap();
    // Coalesced working -> done -> idle before the first poll reads osc_snapshot.
    let session = sleep_session_with_osc(
        dir.path(),
        "\x1b]7501;state=working\x07\x1b]7501;state=done\x07\x1b]7501;state=idle\x07",
    );
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["antigravity"],
    );
    coord.insert_test_receipt("req-fast", "inv-fast");
    let handle = coord.register_run("inv-fast");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "antigravity",
        dir.path(),
        &session,
        AGY_CID,
        "inv-fast",
        None,
    );
    let baseline = tui::TurnBaseline {
        claude_homes: Vec::new(),
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let home_t = dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        tui::monitor_agy_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-fast",
            Some(AGY_CID),
            None,
            &baseline,
            "fast",
            &handle_t,
            Some(home_t),
            tui::AgyMonitorConfig {
                done_reply_wait: Duration::from_millis(250),
                bg_reminder_delay: Duration::from_secs(60),
            },
        );
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while coord.run_receipt_by_invocation("inv-fast").unwrap().state != ReceiptState::Finished
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    let receipt = coord.run_receipt_by_invocation("inv-fast").unwrap();
    if receipt.state != ReceiptState::Finished {
        session.stop();
        monitor.join().unwrap();
        panic!("agy monitor hung when done transitioned quickly to idle");
    }
    monitor.join().unwrap();
    assert_eq!(receipt.terminal.unwrap().status, "succeeded");
    assert_eq!(receipt.anchor.unwrap()["end_row_index"], json!(2));
    session.stop();
}

#[test]
fn agy_does_not_settle_early_on_subturn0_final_after_background_wake() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = native_logs::agy_transcript_path(dir.path(), AGY_CID);
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"step_index\":0,\"content\":\"<USER_REQUEST>run</USER_REQUEST>\"}\n",
            "{\"type\":\"PLANNER_RESPONSE\",\"step_index\":1,\"tool_calls\":[{\"name\":\"run_command\"}]}\n",
            "{\"type\":\"GENERIC\",\"status\":\"RUNNING\",\"content\":\"running as a background task with task id: t-bg\"}\n",
            "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"interim\"}\n",
            "{\"type\":\"SYSTEM_MESSAGE\",\"content\":\"Task id \\\"t-bg\\\" finished\"}\n"
        ),
    )
    .unwrap();
    let session = sleep_session_with_osc(
        dir.path(),
        "\x1b]7501;state=working\x07\x1b]7501;state=done\x07",
    );
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["antigravity"],
    );
    coord.insert_test_receipt("req-agy-wake", "inv-agy-wake");
    let handle = coord.register_run("inv-agy-wake");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "antigravity",
        dir.path(),
        &session,
        AGY_CID,
        "inv-agy-wake",
        None,
    );
    let baseline = tui::TurnBaseline {
        claude_homes: Vec::new(),
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let home_t = dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        tui::monitor_agy_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-agy-wake",
            Some(AGY_CID),
            None,
            &baseline,
            "run",
            &handle_t,
            Some(home_t),
            tui::AgyMonitorConfig {
                done_reply_wait: Duration::from_millis(500),
                bg_reminder_delay: Duration::from_secs(60),
            },
        );
    });
    thread::sleep(Duration::from_millis(120));
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type": "PLANNER_RESPONSE", "content": "final after bg"})
        )
        .unwrap();
    }
    monitor.join().unwrap();
    let receipt = coord.run_receipt_by_invocation("inv-agy-wake").unwrap();
    assert_eq!(receipt.terminal.unwrap().status, "succeeded");
    assert_eq!(receipt.anchor.unwrap()["end_row_index"], json!(6));
    session.stop();
}

#[test]
fn claude_stop_recognizes_mcp_tool_remote_mcp_and_fallback_task_ids() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-mcp\",\"message\":{\"content\":\"run\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"working\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
        ),
    )
    .unwrap();
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-mcp",
                "background_tasks": [
                    {"task_id": "m1", "type": "mcp_tool", "status": "running"},
                    {"tool_use_id": "rm1", "type": "remote_mcp", "status": "running"},
                    {"type": "shell", "status": "running"},
                    {"type": "shell", "status": "running"}
                ]
            })
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-mcp", "inv-mcp");
    let handle = coord.register_run("inv-mcp");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-mcp",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-mcp",
            CLAUDE_SID,
            &baseline,
            "run",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(300),
                bg_reminder_delay: Duration::from_millis(260),
            },
        )
    });

    // Must enter bg_only_waiting because mcp_tool / remote_mcp / shell tasks are waking.
    let wait_bg = Instant::now() + Duration::from_secs(2);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_bg {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(handle.is_bg_only_waiting());

    // At 150ms, one of the two un-ID'd shell tasks finishes -> reminder timer MUST reset!
    thread::sleep(Duration::from_millis(150));
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "promptId": "p-mcp-2",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification>one shell done</task-notification>"}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"1 left"}]}})
        )
        .unwrap();
        writeln!(f, "{}", json!({"type":"system","subtype":"turn_duration"})).unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-mcp-2",
                "background_tasks": [{"type": "shell", "status": "running"}]
            })
        )
        .unwrap();
    }
    // Wait until 320ms from start (still < 150ms + 260ms): reminder must NOT have fired yet.
    thread::sleep(Duration::from_millis(170));
    assert!(
        coord
            .run_receipt_by_invocation("inv-mcp")
            .unwrap()
            .background_tasks_pending
            .is_none()
    );

    // Wait past reset delay (150ms + 260ms = 410ms) and confirm reminder cites distinct fallback ID.
    let rem_deadline = Instant::now() + Duration::from_secs(2);
    let mut rem = None;
    while Instant::now() < rem_deadline {
        if let Some(r) = coord.run_receipt_by_invocation("inv-mcp")
            && let Some(bg) = r.background_tasks_pending
        {
            rem = Some(bg);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(rem.is_some());

    // Finish cleanly.
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "promptId": "p-mcp-3",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification>all done</task-notification>"}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}})
        )
        .unwrap();
        writeln!(f, "{}", json!({"type":"system","subtype":"turn_duration"})).unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-mcp-3",
                "background_tasks": []
            })
        )
        .unwrap();
    }
    monitor.join().unwrap();
    session.stop();
}

#[test]
fn claude_stop_timeout_with_bg_tasks_keeps_waiting_and_allows_bg_only_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    // Subturn 0 has NO turn_duration yet, only Stop(waking=[bg-1]).
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-to\",\"message\":{\"content\":\"go\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"waiting bg\"}]}}\n"
        ),
    )
    .unwrap();
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-to",
                "background_tasks": [{"id": "bg-1", "type": "shell", "status": "running"}]
            })
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=idle\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-to", "inv-to");
    let handle = coord.register_run("inv-to");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-to",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-to",
            CLAUDE_SID,
            &baseline,
            "go",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(120),
                bg_reminder_delay: Duration::from_secs(60),
            },
        )
    });

    // After Stop times out waiting for turn_duration, bg_only_waiting MUST become true!
    let wait_bg = Instant::now() + Duration::from_secs(2);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_bg {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(handle.is_bg_only_waiting());

    // Now late turn_duration arrives; even past another pair timeout, monitor MUST NOT settle early!
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(f, "{}", json!({"type":"system","subtype":"turn_duration"})).unwrap();
    }
    thread::sleep(Duration::from_millis(260));
    assert_eq!(
        coord.run_receipt_by_invocation("inv-to").unwrap().state,
        ReceiptState::Started
    );
    assert!(handle.is_bg_only_waiting());

    // bg-only cancel works without sending keys.
    coord.insert_test_receipt("cancel-to", "inv-to-cancel");
    let run_receipt = coord.run_receipt_by_invocation("inv-to").unwrap();
    let id = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let cancel_env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-to".to_owned(),
        kind: "cancel".to_owned(),
        service: id.clone(),
        target_tray: id,
        invocation_id: "inv-to".to_owned(),
        parent: None,
        session_id: Some(CLAUDE_SID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    cancel_claude(&coord, &cancel_env, &run_receipt, &launched);
    monitor.join().unwrap();
    assert!(session.recorded_writes().is_empty());
    assert_eq!(
        coord
            .run_receipt_by_invocation("inv-to")
            .unwrap()
            .terminal
            .unwrap()
            .status,
        "succeeded"
    );
    session.stop();
}

#[test]
fn try_cancel_bg_only_timeout_resets_cancel_latch() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-latch", "inv-latch");
    coord.insert_test_receipt("cancel-latch", "inv-latch-cancel");
    let handle = coord.register_run("inv-latch");
    handle.set_bg_only_waiting(true);
    let id = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let cancel_env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-latch".to_owned(),
        kind: "cancel".to_owned(),
        service: id.clone(),
        target_tray: id,
        invocation_id: "inv-latch".to_owned(),
        parent: None,
        session_id: Some(CLAUDE_SID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    assert!(!try_cancel_bg_only(&coord, &cancel_env, CLAUDE_SID));
    assert!(!handle.is_bg_only_cancel_requested());
}

#[test]
fn claude_reminder_does_not_fire_during_foreground_subturn_after_task_notification() {
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let transcript = proj.join(format!("{CLAUDE_SID}.jsonl"));
    let events_file = claude_hook::events_path(hook_dir.path());

    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"promptId\":\"p-rem\",\"message\":{\"content\":\"go\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"started\"}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
        ),
    )
    .unwrap();
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-rem",
                "background_tasks": [{"id": "bg-1", "type": "shell", "status": "running"}]
            })
        ),
    )
    .unwrap();

    let session = sleep_session_with_osc(dir.path(), "\x1b]7501;state=working\x07");
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        crate::terminal::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("req-rem", "inv-rem");
    let handle = coord.register_run("inv-rem");
    let terminals = Terminals::default();
    let launched = register_test_derived(
        &terminals,
        "claude",
        dir.path(),
        &session,
        CLAUDE_SID,
        "inv-rem",
        Some(hook_dir.path().to_path_buf()),
    );
    let baseline = tui::TurnBaseline {
        claude_homes: vec![dir.path().to_path_buf()],
        pre_offset: 0,
        hook_events_offset: 0,
        agy_baseline_cids: Vec::new(),
        agy_prev_idx: None,
        seq_before: 0,
    };
    let coord_t = coord.clone();
    let terms_t = terminals.clone();
    let launched_t = launched.clone();
    let handle_t = handle.clone();
    let hook_path = hook_dir.path().to_path_buf();
    let monitor = thread::spawn(move || {
        claude::monitor_claude_turn_with_config(
            &coord_t,
            &terms_t,
            &launched_t,
            "req-rem",
            CLAUDE_SID,
            &baseline,
            "go",
            &handle_t,
            Some(&hook_path),
            false,
            claude::ClaudeMonitorConfig {
                signal_pair_timeout: Duration::from_millis(300),
                bg_reminder_delay: Duration::from_millis(180),
            },
        )
    });

    let wait_bg = Instant::now() + Duration::from_secs(2);
    while !handle.is_bg_only_waiting() && Instant::now() < wait_bg {
        thread::sleep(Duration::from_millis(15));
    }
    assert!(handle.is_bg_only_waiting());

    // Task notification wakes subturn 1 (foreground active, no Stop/turn_duration yet).
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "type": "user",
                "promptId": "p-rem-1",
                "promptSource": "system",
                "turnOrigin": "task_notification",
                "origin": {"kind": "task-notification"},
                "message": {"content": "<task-notification><task-id>bg-1</task-id></task-notification>"}
            })
        )
        .unwrap();
    }
    // Wait past bg_reminder_delay while subturn 1 is active in foreground -> must NOT remind stale bg-1!
    thread::sleep(Duration::from_millis(260));
    assert!(
        coord
            .run_receipt_by_invocation("inv-rem")
            .unwrap()
            .background_tasks_pending
            .is_none()
    );

    // Finish subturn 1 cleanly.
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}})
        )
        .unwrap();
        writeln!(f, "{}", json!({"type":"system","subtype":"turn_duration"})).unwrap();
    }
    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": CLAUDE_SID,
                "prompt_id": "p-rem-1",
                "background_tasks": []
            })
        )
        .unwrap();
    }
    monitor.join().unwrap();
    session.stop();
}
