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
