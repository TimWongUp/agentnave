use super::*;
use crate::terminal::DerivedLaunchConfig;

#[cfg(unix)]
#[test]
fn derived_resume_rejects_mismatched_permissions_or_model() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        super::super::super::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    let terminals = Terminals::default();
    let sid = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";

    let mut cmd = CommandBuilder::new("python3");
    cmd.args(["-c", "import time; time.sleep(5)"]);
    cmd.cwd(dir.path());
    let session = Session::start(cmd, 80, 24).unwrap();

    let original_cfg = RunConfig {
        provider: "claude".to_owned(),
        account: None,
        account_dir: None,
        cwd: dir.path().display().to_string(),
        isolation: None,
        model: Some("claude-sonnet-4-6".to_owned()),
        effort: Some("high".to_owned()),
        permissions: json!({"permission_mode": "bypassPermissions"}),
    };
    let tid = terminals.register_derived(NewDerived {
        provider: "claude".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: Some(sid.to_owned()),
        account: None,
        codex: None,
        info: DerivedInfo {
            parent_boss_key: "claude:boss".to_owned(),
            parent_workspace_id: Some("ws".to_owned()),
            busy: false,
            blocked_kind: None,
            isolation_id: None,
            _lease: None,
            _log_dir: None,
            codex_ready_file: None,
            active_invocation_id: None,
            launch_config: Some(DerivedLaunchConfig::from_run_config(&original_cfg)),
        },
    });

    let identity = ProcessIdentity {
        pid: std::process::id(),
        started_at: coord.started_at,
    };
    let make_env = |inv: &str| SpawnRequestEnvelope {
        version: 1,
        request_id: inv.to_owned(),
        kind: "run".to_owned(),
        service: identity.clone(),
        target_tray: identity.clone(),
        invocation_id: inv.to_owned(),
        parent: None,
        session_id: Some(sid.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };

    // Tightened permissions on resume must be rejected instead of reusing the looser process.
    let mut tighter = original_cfg.clone();
    tighter.permissions = json!({"permission_mode": "default"});
    let handle1 = coord.register_run("inv-d2-perm");
    let err = prepare_open_or_launch(
        &coord,
        &terminals,
        &make_env("inv-d2-perm"),
        &tighter,
        dir.path(),
        None,
        &handle1,
        |_, _| unreachable!("should reject before spawning"),
    )
    .err()
    .expect("tighter permissions must fail");
    assert_eq!(err, "派生终端以不同的权限或模型启动，需先结束该终端再续派");

    // Different model on resume must also be rejected.
    let mut other_model = original_cfg.clone();
    other_model.model = Some("claude-opus-4-6".to_owned());
    let handle2 = coord.register_run("inv-d2-model");
    let err2 = prepare_open_or_launch(
        &coord,
        &terminals,
        &make_env("inv-d2-model"),
        &other_model,
        dir.path(),
        None,
        &handle2,
        |_, _| unreachable!("should reject before spawning"),
    )
    .err()
    .expect("different model must fail");
    assert_eq!(err2, "派生终端以不同的权限或模型启动，需先结束该终端再续派");

    // Identical config reuses the open idle derived terminal.
    let handle3 = coord.register_run("inv-d2-ok");
    let (claimed, reused) = prepare_open_or_launch(
        &coord,
        &terminals,
        &make_env("inv-d2-ok"),
        &original_cfg,
        dir.path(),
        None,
        &handle3,
        |_, _| unreachable!("should reuse existing terminal"),
    )
    .expect("matching config should reuse");
    assert!(reused);
    assert_eq!(claimed.id, tid);

    terminals.remove_derived_terminal(tid);
    session.stop();
}

#[cfg(unix)]
#[test]
fn send_paste_strips_c0_controls_and_rejects_multiline_without_bracketed_paste() {
    let clean = super::super::super::session::sanitize_paste_text(
        "a\rb\x00\x03\x04\x1b[200~c\x1b[201~\td\ne",
    );
    assert_eq!(clean, "abc\td\ne");

    let dir = tempfile::tempdir().unwrap();
    let script = r#"
import os, select, time, tty
tty.setraw(0)
os.write(1, b"\x1b]7501;state=idle\x07")
buf = b""
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    if select.select([0], [], [], 0.05)[0]:
        buf += os.read(0, 1024)
        if b"ENABLE_PASTE" in buf:
            os.write(1, b"\x1b[?2004h")
            buf = b""
"#;
    let mut cmd = CommandBuilder::new("python3");
    cmd.args(["-c", script]);
    cmd.cwd(dir.path());
    let session = Session::start(cmd, 80, 24).unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(3);
    while session.osc_snapshot().1.as_deref() != Some("idle") && Instant::now() < ready_deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.osc_snapshot().1.as_deref(), Some("idle"));
    session.record_writes();

    // Without bracketed paste mode, multiline text must be rejected without writing to PTY.
    let err = session
        .send_paste("line1\nline2")
        .expect_err("multiline paste without bracketed paste must fail");
    assert!(err.contains("终端未进入可粘贴状态"));
    assert!(session.recorded_writes().is_empty());

    // Single-line text strips CR and C0 controls before writing.
    session.record_writes();
    let sent = session.send_paste("hello\rworld\x03\x07\tok").unwrap();
    assert_eq!(sent, "helloworld\tok");
    assert_eq!(session.recorded_writes(), vec![b"helloworld\tok".to_vec()]);

    // After enabling bracketed paste mode, multiline text is wrapped cleanly.
    session.write(b"ENABLE_PASTE").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut pasted = Err(String::new());
    while Instant::now() < deadline {
        session.record_writes();
        pasted = session.send_paste("line1\r\nline2");
        if pasted.is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(pasted.unwrap(), "line1\nline2");
    assert_eq!(
        session.recorded_writes(),
        vec![b"\x1b[200~line1\nline2\x1b[201~".to_vec()]
    );
    session.stop();
}
