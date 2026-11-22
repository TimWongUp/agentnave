use super::*;
use std::cell::Cell;

const SID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

/// Fake TUI: reports `working` (or `done`) via OSC 7501 and, in `esc_idle` mode,
/// returns to `idle` once it receives an Esc byte.
const FAKE_TUI: &str = r#"
import os, select, sys, time, tty
tty.setraw(0)
mode = sys.argv[1]
def osc(state):
    os.write(1, ("\x1b]7501;state=%s\x07" % state).encode())
osc("done" if mode == "done" else "working")
if mode == "esc_idle":
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if select.select([0], [], [], 0.05)[0] and b"\x1b" in os.read(0, 64):
            osc("idle")
            break
time.sleep(5)
"#;

struct Rig {
    _dir: tempfile::TempDir,
    coord: Coordinator,
    session: Arc<Session>,
    handle: DerivedHandle,
    run: Arc<RunHandle>,
    env: SpawnRequestEnvelope,
}

fn rig(mode: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects/p1");
    fs::create_dir_all(&projects).unwrap();
    fs::write(
        projects.join(format!("{SID}.jsonl")),
        "{\"type\":\"user\",\"promptId\":\"p-1\",\"message\":{\"content\":\"hi\"}}\n",
    )
    .unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        super::super::super::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("run-req", "inv");
    coord.mark_started(
        "run-req",
        Some(SID),
        Some(json!({"session_id": SID, "prompt_id": "p-1"})),
    );
    coord.insert_test_receipt("cancel-req", "inv-cancel");
    let run = coord.register_run("inv");

    let mut cmd = CommandBuilder::new("python3");
    cmd.args(["-c", FAKE_TUI, mode]);
    cmd.cwd(dir.path());
    let session = Session::start(cmd, 80, 24).unwrap();
    session.record_writes();
    let want = if mode == "done" { "done" } else { "working" };
    let until = Instant::now() + Duration::from_secs(3);
    while session.osc_snapshot().1.as_deref() != Some(want) && Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.osc_snapshot().1.as_deref(), Some(want));

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
        invocation_id: "inv".to_owned(),
        parent: None,
        session_id: Some(SID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    let handle = DerivedHandle {
        id: 1,
        provider: "claude".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: Some(SID.to_owned()),
        account: Some(Active {
            id: "acc".to_owned(),
            label: "acc".to_owned(),
            dir: dir.path().to_path_buf(),
        }),
        launch_config: None,
        codex: None,
    };
    Rig {
        _dir: dir,
        coord,
        session,
        handle,
        run,
        env,
    }
}

fn cancel_status(rig: &Rig) -> String {
    rig.coord.receipts_snapshot()["cancel-req"]
        .terminal
        .as_ref()
        .unwrap()
        .status
        .clone()
}

fn run_receipt(rig: &Rig) -> Receipt {
    rig.coord.run_receipt_by_invocation("inv").unwrap()
}

#[cfg(unix)]
#[test]
fn esc_returning_to_idle_confirms_interrupt_and_marks_run_cancelled() {
    let rig = rig("esc_idle");
    cancel_claude(&rig.coord, &rig.env, &run_receipt(&rig), &rig.handle);
    assert_eq!(rig.session.recorded_writes(), vec![b"\x1b".to_vec()]);
    assert_eq!(cancel_status(&rig), "interrupted");
    assert!(rig.run.is_interrupt_confirmed());
    assert!(!rig.run.interrupt_pending());
    rig.session.stop();
}

#[cfg(unix)]
#[test]
fn esc_without_state_change_is_not_safe_and_run_keeps_going() {
    let rig = rig("ignore");
    cancel_claude(&rig.coord, &rig.env, &run_receipt(&rig), &rig.handle);
    assert_eq!(rig.session.recorded_writes(), vec![b"\x1b".to_vec()]);
    assert_eq!(cancel_status(&rig), "not_safe_to_interrupt");
    assert!(!rig.run.is_interrupt_confirmed());
    assert!(!rig.run.interrupt_pending());
    assert_ne!(run_receipt(&rig).state, ReceiptState::Finished);
    rig.session.stop();
}

#[cfg(unix)]
#[test]
fn already_done_turn_sends_no_esc_and_run_settles_succeeded() {
    let rig = rig("done");
    cancel_claude(&rig.coord, &rig.env, &run_receipt(&rig), &rig.handle);
    assert!(rig.session.recorded_writes().is_empty());
    assert_eq!(cancel_status(&rig), "already_finished");
    assert!(!rig.run.is_interrupt_confirmed());
    tui::finish_settled(
        &rig.coord,
        &Terminals::default(),
        rig.handle.id,
        "run-req",
        Some(SID),
        false,
        rig.run.is_interrupt_confirmed(),
        "",
    );
    let status = run_receipt(&rig).terminal.unwrap().status;
    assert_eq!(status, "succeeded");
    rig.session.stop();
}

#[cfg(unix)]
#[test]
fn natural_completion_racing_esc_reports_already_finished() {
    let rig = rig("esc_idle");
    let calls = Cell::new(0);
    let probe = |_state: Option<&str>| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            interrupt::TurnProbe::Active
        } else {
            interrupt::TurnProbe::Completed
        }
    };
    interrupt::interrupt_turn_with_esc(&rig.coord, &rig.env, &rig.handle, SID, &probe);
    assert_eq!(cancel_status(&rig), "already_finished");
    assert!(!rig.run.is_interrupt_confirmed());
    rig.session.stop();
}

#[cfg(unix)]
#[test]
fn pre_start_cancel_latches_and_does_not_overwrite_natural_completion() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        super::super::super::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    coord.insert_test_receipt("run-req", "inv-pre");
    coord.insert_test_receipt("cancel-req", "inv-pre-cancel");
    let run = coord.register_run("inv-pre");
    let identity = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let cancel_env = SpawnRequestEnvelope {
        version: 1,
        request_id: "cancel-req".to_owned(),
        kind: "cancel".to_owned(),
        service: identity.clone(),
        target_tray: identity,
        invocation_id: "inv-pre".to_owned(),
        parent: None,
        session_id: None,
        config: None,
        task: None,
        expected_anchor: None,
    };
    execute_cancel(coord.clone(), Terminals::default(), cancel_env);
    // Cancel during Accepted must keep pre_start_cancelled latched (never reset to false).
    assert!(run.is_pre_start_cancelled());
    assert!(!run.is_interrupt_confirmed());
    assert_eq!(
        coord.receipts_snapshot()["cancel-req"]
            .terminal
            .as_ref()
            .map(|t| t.status.as_str()),
        Some("not_safe_to_interrupt")
    );

    // If the turn actually started and finished naturally without confirmed interrupt,
    // finish_settled must not rewrite it to cancelled.
    tui::finish_settled(
        &coord,
        &Terminals::default(),
        1,
        "run-req",
        Some(SID),
        false,
        run.is_interrupt_confirmed(),
        "",
    );
    assert_eq!(
        coord
            .run_receipt_by_invocation("inv-pre")
            .unwrap()
            .terminal
            .unwrap()
            .status,
        "succeeded"
    );
}
