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
        coord.insert_test_receipt("cancel-req", "inv-codex");
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
            session: session.clone(),
            native: Some(TID.to_owned()),
            account: None,
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
