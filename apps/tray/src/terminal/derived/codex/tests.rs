use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;
use crate::terminal::codex_server::tests::{connect_to_mock, write_server_frame};

const TID: &str = "019a0000-0000-7000-8000-000000000110";
const TURN: &str = "turn-110";

fn rollout_line(kind: &str, extra: Value) -> String {
    let mut payload = json!({"type": kind, "turn_id": TURN});
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"type": "event_msg", "payload": payload}).to_string() + "\n"
}

/// An app-server whose `thread/read` always reports the turn with `status`.
fn mock_with_read_status(
    sock: &Path,
    rollout: &Path,
    status: Arc<Mutex<&'static str>>,
) -> Arc<codex_server::Client> {
    let rollout = rollout.display().to_string();
    let codex = connect_to_mock(sock, move |msg, stream| {
        let id = msg.get("id").and_then(Value::as_u64);
        let result = match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({}),
            Some("thread/loaded/list") => json!({"data": []}),
            Some("thread/read") => json!({"thread": {
                "id": TID,
                "path": rollout,
                "turns": [{"id": TURN, "status": *status.lock().unwrap()}],
            }}),
            _ => return,
        };
        write_server_frame(
            stream,
            &json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        );
    });
    assert!(codex.is_connected());
    codex
}

#[test]
fn thread_read_interrupted_needs_rollout_proof_before_it_cancels() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("rollout-110.jsonl");
    fs::write(&rollout, rollout_line("task_started", json!({}))).unwrap();
    let status = Arc::new(Mutex::new("interrupted"));
    let codex = mock_with_read_status(&dir.path().join("a.sock"), &rollout, status.clone());
    let homes = [dir.path().to_path_buf()];
    let reconcile = || check_turn_reconciled(&codex, TID, TURN, &mut None, &homes).0;

    // Codex reports a turn that is running in the rollout as interrupted while the thread is not
    // yet active; that alone is not an interruption.
    assert_eq!(reconcile(), CodexTurnOutcome::Running);

    let mut file = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
    file.write_all(rollout_line("turn_aborted", json!({"reason": "interrupted"})).as_bytes())
        .unwrap();
    assert_eq!(reconcile(), CodexTurnOutcome::Interrupted);

    *status.lock().unwrap() = "completed";
    assert_eq!(reconcile(), CodexTurnOutcome::Completed);
    codex.close();
}

#[test]
fn early_terminal_exit_fails_unless_the_turn_is_proven_done_or_interrupted() {
    let exited = Some(143);
    assert_eq!(
        settle_turn(CodexTurnOutcome::Completed, None, exited),
        ("succeeded", None)
    );
    assert_eq!(
        settle_turn(CodexTurnOutcome::Interrupted, None, exited).0,
        "cancelled"
    );
    assert_eq!(
        settle_turn(CodexTurnOutcome::Running, None, exited),
        ("failed", Some("Codex 终端提前退出（exit 143）".to_owned()))
    );
    assert_eq!(
        settle_turn(CodexTurnOutcome::Failed, Some("boom".into()), exited),
        ("failed", Some("boom".to_owned()))
    );
}

#[test]
fn cancel_codex_waits_for_turn_completed_after_interrupt_issued() {
    use crate::terminal::spawn_requests::{
        ProcessIdentity, SpawnRequestEnvelope, current_process_started_at,
    };
    use portable_pty::CommandBuilder;

    let cases: [(&str, Option<&str>, &str); 3] = [
        ("interrupted", Some("interrupted"), "interrupted"),
        ("completed", Some("completed"), "already_finished"),
        ("timeout", None, "not_safe_to_interrupt"),
    ];
    for (name, notify_status, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let rollout = dir.path().join(format!("rollout-{name}.jsonl"));
        fs::write(&rollout, rollout_line("task_started", json!({}))).unwrap();
        let rollout_str = rollout.display().to_string();
        let notify = notify_status.map(str::to_owned);
        let codex = connect_to_mock(
            &dir.path().join(format!("{name}.sock")),
            move |msg, stream| {
                let id = msg.get("id").and_then(Value::as_u64);
                match msg.get("method").and_then(Value::as_str) {
                    Some("initialize") => {
                        write_server_frame(
                            stream,
                            &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                        );
                    }
                    Some("thread/loaded/list") => {
                        write_server_frame(
                            stream,
                            &json!({"jsonrpc": "2.0", "id": id, "result": {"data": []}})
                                .to_string(),
                        );
                    }
                    Some("thread/read") => {
                        write_server_frame(
                            stream,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {"thread": {
                                    "id": TID,
                                    "path": rollout_str,
                                    "turns": [{"id": TURN, "status": "inProgress"}],
                                }}
                            })
                            .to_string(),
                        );
                    }
                    Some("turn/interrupt") => {
                        write_server_frame(
                            stream,
                            &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                        );
                        if let Some(status) = &notify {
                            write_server_frame(
                                stream,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "turn/completed",
                                    "params": {
                                        "threadId": TID,
                                        "turn": {"id": TURN, "status": status},
                                    }
                                })
                                .to_string(),
                            );
                        }
                    }
                    _ => {}
                }
            },
        );
        assert!(codex.is_connected());

        let coord = Coordinator::new(
            dir.path().to_path_buf(),
            std::process::id(),
            current_process_started_at(),
            &["codex"],
        );
        coord.insert_test_receipt("run-req", "inv-codex");
        coord.mark_started(
            "run-req",
            Some(TID),
            Some(json!({"session_id": TID, "turn_id": TURN})),
        );
        coord.insert_test_receipt("cancel-req", "inv-codex-cancel");
        let run_receipt = coord.run_receipt_by_invocation("inv-codex").unwrap();

        let mut cmd = CommandBuilder::new("python3");
        cmd.args(["-c", "import time; time.sleep(5)"]);
        cmd.cwd(dir.path());
        let session = Session::start(cmd, 80, 24).unwrap();
        let identity = ProcessIdentity {
            pid: std::process::id(),
            started_at: coord.started_at,
        };
        let env = SpawnRequestEnvelope {
            version: 1,
            request_id: "cancel-req".to_owned(),
            kind: "cancel".to_owned(),
            service: identity.clone(),
            target_tray: identity,
            invocation_id: "inv-codex".to_owned(),
            parent: None,
            session_id: Some(TID.to_owned()),
            config: None,
            task: None,
            expected_anchor: None,
        };
        let handle = DerivedHandle {
            id: 1,
            provider: "codex".to_owned(),
            cwd: dir.path().to_path_buf(),
            session: session.clone(),
            native: Some(TID.to_owned()),
            account: None,
            launch_config: None,
            hook_dir: None,
            codex: Some(codex.clone()),
        };

        cancel_codex(&coord, &env, &run_receipt, &handle);
        let actual = coord.receipts_snapshot()["cancel-req"]
            .terminal
            .as_ref()
            .unwrap()
            .status
            .clone();
        assert_eq!(actual, expected, "case {name}");
        session.stop();
        codex.close();
    }
}

#[test]
fn cancel_codex_exits_promptly_when_run_finishes_during_wait() {
    use crate::terminal::spawn_requests::{
        ProcessIdentity, SpawnRequestEnvelope, current_process_started_at,
    };
    use portable_pty::CommandBuilder;

    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("rollout-closed.jsonl");
    fs::write(&rollout, rollout_line("task_started", json!({}))).unwrap();
    let rollout_str = rollout.display().to_string();
    let codex = connect_to_mock(&dir.path().join("closed.sock"), move |msg, stream| {
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                );
            }
            Some("thread/loaded/list") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"data": []}}).to_string(),
                );
            }
            Some("thread/read") => {
                write_server_frame(
                    stream,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"thread": {
                            "id": TID,
                            "path": rollout_str,
                            "turns": [{"id": TURN, "status": "inProgress"}],
                        }}
                    })
                    .to_string(),
                );
            }
            Some("turn/interrupt") => {
                // Issued, but no turn/completed will ever arrive (simulating closed terminal).
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                );
            }
            _ => {}
        }
    });
    assert!(codex.is_connected());

    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        current_process_started_at(),
        &["codex"],
    );
    coord.insert_test_receipt("run-req", "inv-codex-closed");
    coord.mark_started(
        "run-req",
        Some(TID),
        Some(json!({"session_id": TID, "turn_id": TURN})),
    );
    coord.insert_test_receipt("cancel-req", "inv-codex-closed-cancel");
    let run_receipt = coord.run_receipt_by_invocation("inv-codex-closed").unwrap();

    let mut cmd = CommandBuilder::new("python3");
    cmd.args(["-c", "import time; time.sleep(5)"]);
    cmd.cwd(dir.path());
    let session = Session::start(cmd, 80, 24).unwrap();
    let identity = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-req".to_owned(),
        kind: "cancel".to_owned(),
        service: identity.clone(),
        target_tray: identity,
        invocation_id: "inv-codex-closed".to_owned(),
        parent: None,
        session_id: Some(TID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    let handle = DerivedHandle {
        id: 1,
        provider: "codex".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: Some(TID.to_owned()),
        account: None,
        launch_config: None,
        hook_dir: None,
        codex: Some(codex.clone()),
    };

    let coord_closer = coord.clone();
    let closer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        coord_closer.on_terminal_closed("inv-codex-closed");
    });
    let started = Instant::now();
    cancel_codex(&coord, &env, &run_receipt, &handle);
    let elapsed = started.elapsed();
    closer.join().unwrap();

    assert_eq!(
        coord.receipts_snapshot()["cancel-req"]
            .terminal
            .as_ref()
            .map(|t| t.status.as_str()),
        Some("already_finished")
    );
    assert!(
        elapsed < INTERRUPT_CONFIRM_TIMEOUT,
        "should return promptly when run finishes (took {elapsed:?})"
    );
    session.stop();
    codex.close();
}

#[test]
fn codex_run_fails_startup_and_never_starts_turn_when_pid_publish_fails() {
    use crate::terminal::spawn_requests::{
        ParentRef, ProcessIdentity, SpawnRequestEnvelope, current_process_started_at,
    };
    use portable_pty::CommandBuilder;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = tempfile::tempdir().unwrap();
    // Block push/ writes with a regular file so publishing derived_pids must fail.
    fs::write(dir.path().join("push"), b"not-a-dir").unwrap();

    let turn_started = Arc::new(AtomicBool::new(false));
    let turn_started_srv = turn_started.clone();
    let codex = connect_to_mock(&dir.path().join("pubfail.sock"), move |msg, stream| {
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                );
            }
            Some("thread/loaded/list") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [TID]}}).to_string(),
                );
            }
            Some("turn/start") => {
                turn_started_srv.store(true, Ordering::SeqCst);
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"turn": {"id": TURN}}})
                        .to_string(),
                );
            }
            _ => {}
        }
    });
    assert!(codex.is_connected());

    let terminals = Terminals::default();
    let coord = Coordinator::with_terminals(
        dir.path().to_path_buf(),
        std::process::id(),
        current_process_started_at(),
        &["codex"],
        terminals.clone(),
    );
    coord.insert_test_receipt("req-e3b", "inv-e3b");
    let run_handle = coord.register_run("inv-e3b");

    let mut cmd = CommandBuilder::new("python3");
    cmd.args(["-c", "import time; time.sleep(5)"]);
    cmd.cwd(dir.path());
    let session = Session::start(cmd, 80, 24).unwrap();
    let tid = terminals.register_derived(NewDerived {
        provider: "codex".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: Some(TID.to_owned()),
        account: None,
        codex: Some(codex.clone()),
        info: DerivedInfo {
            parent_boss_key: "codex:boss".to_owned(),
            parent_workspace_id: Some("ws".to_owned()),
            busy: false,
            blocked_kind: None,
            isolation_id: None,
            _lease: None,
            _log_dir: None,
            _hook_dir: None,
            codex_ready_file: None,
            active_invocation_id: None,
            launch_config: None,
        },
    });
    let identity = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let env = SpawnRequestEnvelope {
        version: 1,
        request_id: "req-e3b".to_owned(),
        kind: "run".to_owned(),
        service: identity.clone(),
        target_tray: identity,
        invocation_id: "inv-e3b".to_owned(),
        parent: None,
        session_id: Some(TID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    let cfg = RunConfig {
        provider: "codex".to_owned(),
        account: None,
        account_dir: None,
        cwd: dir.path().display().to_string(),
        isolation: None,
        model: None,
        effort: None,
        permissions: json!({}),
    };
    let parent = ParentRef {
        boss_key: "codex:boss".to_owned(),
        workspace_id: Some("ws".to_owned()),
    };

    run_codex(
        &coord,
        &terminals,
        &env,
        &cfg,
        &parent,
        dir.path(),
        None,
        "hello",
        &run_handle,
    );

    assert!(!turn_started.load(Ordering::SeqCst));
    let term = coord
        .run_receipt_by_invocation("inv-e3b")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(term.status, "failed");
    assert!(
        term.reason
            .as_deref()
            .is_some_and(|r| r.contains("发布派生终端进程标记失败"))
    );

    // F1: When PID publish succeeds and pre-start cancel arrives before start_turn,
    // settle as cancelled, clean up turn ownership, and never call turn/start.
    fs::remove_file(dir.path().join("push")).unwrap();
    coord.insert_test_receipt("req-f1", "inv-f1");
    let cancel_handle = coord.register_run("inv-f1");
    let claimed = terminals
        .try_claim_derived_turn(tid, "inv-f1", None)
        .expect("reused terminal should be idle after E3(b) cleanup");
    cancel_handle.request_pre_start_cancel();
    let turn_cfg = build_turn_config("hello", "inv-f1", dir.path(), &cfg);
    assert_eq!(
        publish_and_start_codex_turn(
            &coord,
            &terminals,
            &claimed,
            &codex,
            true,
            "req-f1",
            TID,
            &turn_cfg,
            &cancel_handle,
        ),
        None
    );
    assert!(!turn_started.load(Ordering::SeqCst));
    assert!(
        dir.path()
            .join("push")
            .join(format!("{}.json", coord.pid))
            .is_file()
    );
    assert!(terminals.derived_by_invocation("inv-f1").is_none());
    let f1_receipt = coord.run_receipt_by_invocation("inv-f1").unwrap();
    assert_eq!(f1_receipt.session_id.as_deref(), Some(TID));
    let f1_term = f1_receipt.terminal.unwrap();
    assert_eq!(f1_term.status, "cancelled");
    assert_eq!(f1_term.reason.as_deref(), Some("启动前已取消"));

    terminals.remove_derived_terminal(tid);
    session.stop();
    codex.close();
}

#[test]
fn build_turn_config_does_not_enable_network_access_for_workspace_write() {
    let cwd = Path::new("/tmp/repo");
    for perms in [
        json!({"approve_for_me": true}),
        json!({"sandbox": "workspace-write"}),
    ] {
        let cfg = RunConfig {
            provider: "codex".to_owned(),
            account: None,
            account_dir: None,
            cwd: cwd.display().to_string(),
            isolation: None,
            model: None,
            effort: None,
            permissions: perms,
        };
        let turn_cfg = build_turn_config("hello", "client-1", cwd, &cfg);
        let sandbox = turn_cfg.sandbox_policy.expect("workspaceWrite policy");
        assert_eq!(sandbox["type"], json!("workspaceWrite"));
        assert!(sandbox.get("networkAccess").is_none());
    }
}
