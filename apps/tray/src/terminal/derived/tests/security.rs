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
            _hook_dir: None,
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
    assert_eq!(err, LAUNCH_CONFIG_MISMATCH_ERR);

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
    assert_eq!(err2, LAUNCH_CONFIG_MISMATCH_ERR);

    // Different cwd on resume must also be rejected (E2).
    let other_dir = tempfile::tempdir().unwrap();
    let mut other_cwd = original_cfg.clone();
    other_cwd.cwd = other_dir.path().display().to_string();
    let handle_cwd = coord.register_run("inv-e2-cwd");
    let err_cwd = prepare_open_or_launch(
        &coord,
        &terminals,
        &make_env("inv-e2-cwd"),
        &other_cwd,
        other_dir.path(),
        None,
        &handle_cwd,
        |_, _| unreachable!("should reject before spawning"),
    )
    .err()
    .expect("different cwd must fail");
    assert_eq!(err_cwd, LAUNCH_CONFIG_MISMATCH_ERR);

    // Symlink pointing to the same canonical cwd should still reuse.
    let alias_parent = tempfile::tempdir().unwrap();
    let alias_cwd = alias_parent.path().join("cwd-alias");
    std::os::unix::fs::symlink(dir.path(), &alias_cwd).unwrap();
    let mut alias_cfg = original_cfg.clone();
    alias_cfg.cwd = alias_cwd.display().to_string();
    let handle3 = coord.register_run("inv-d2-ok");
    let (claimed, reused) = prepare_open_or_launch(
        &coord,
        &terminals,
        &make_env("inv-d2-ok"),
        &alias_cfg,
        &alias_cwd,
        None,
        &handle3,
        |_, _| unreachable!("should reuse existing terminal"),
    )
    .expect("matching canonical config should reuse");
    assert!(reused);
    assert_eq!(claimed.id, tid);

    // Codex reuse path must also reject a mismatched cwd (E2).
    let codex_sid = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let codex_tid = terminals.register_derived(NewDerived {
        provider: "codex".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: Some(codex_sid.to_owned()),
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
            _hook_dir: None,
            codex_ready_file: None,
            active_invocation_id: None,
            launch_config: None,
        },
    });
    let mut codex_env = make_env("inv-e2-codex-cwd");
    codex_env.session_id = Some(codex_sid.to_owned());
    let codex_cfg = RunConfig {
        provider: "codex".to_owned(),
        account: None,
        account_dir: None,
        cwd: other_dir.path().display().to_string(),
        isolation: None,
        model: None,
        effort: None,
        permissions: json!({}),
    };
    let codex_parent = ParentRef {
        boss_key: "claude:boss".to_owned(),
        workspace_id: Some("ws".to_owned()),
    };
    let codex_handle = coord.register_run("inv-e2-codex-cwd");
    let codex_err = codex::open_or_launch_codex(
        &coord,
        &terminals,
        &codex_env,
        &codex_cfg,
        &codex_parent,
        other_dir.path(),
        None,
        &codex_handle,
    )
    .err()
    .expect("codex resume with different cwd must fail");
    assert_eq!(codex_err, LAUNCH_CONFIG_MISMATCH_ERR);

    terminals.remove_derived_terminal(codex_tid);
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

#[cfg(unix)]
#[test]
fn submit_prompt_waits_for_delayed_bracketed_paste_and_fails_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let coord = Coordinator::new(
        dir.path().to_path_buf(),
        std::process::id(),
        super::super::super::spawn_requests::current_process_started_at(),
        &["claude"],
    );
    let terminals = Terminals::default();

    // 1) CLI emits OSC idle first, then enables ?2004h 150ms later -> submit_prompt waits and succeeds.
    let delayed_script = r#"
import os, time, tty
tty.setraw(0)
os.write(1, b"\x1b]7501;state=idle\x07")
time.sleep(0.15)
os.write(1, b"\x1b[?2004h")
time.sleep(5)
"#;
    let mut cmd1 = CommandBuilder::new("python3");
    cmd1.args(["-c", delayed_script]);
    cmd1.cwd(dir.path());
    let s1 = Session::start(cmd1, 80, 24).unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(3);
    while s1.osc_snapshot().1.as_deref() != Some("idle") && Instant::now() < ready_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(s1.osc_snapshot().1.as_deref(), Some("idle"));
    s1.record_writes();
    coord.insert_test_receipt("req-e1-ok", "inv-e1-ok");
    let h1 = coord.register_run("inv-e1-ok");
    let d1 = DerivedHandle {
        id: 101,
        provider: "claude".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: s1.clone(),
        native: Some("sid-e1-ok".to_owned()),
        account: None,
        launch_config: None,
        hook_dir: None,
        codex: None,
    };
    let sent = tui::submit_prompt(&coord, &terminals, &d1, true, "req-e1-ok", "a\nb", &h1);
    assert_eq!(sent.as_deref(), Some("a\nb"));
    assert_eq!(
        s1.recorded_writes(),
        vec![b"\x1b[200~a\nb\x1b[201~".to_vec(), b"\r".to_vec()]
    );
    s1.stop();

    // 2) CLI never enables ?2004h -> submit_prompt times out and fails with clear reason.
    let never_script = r#"
import os, time, tty
tty.setraw(0)
os.write(1, b"\x1b]7501;state=idle\x07")
time.sleep(5)
"#;
    let mut cmd2 = CommandBuilder::new("python3");
    cmd2.args(["-c", never_script]);
    cmd2.cwd(dir.path());
    let s2 = Session::start(cmd2, 80, 24).unwrap();
    let ready_deadline2 = Instant::now() + Duration::from_secs(3);
    while s2.osc_snapshot().1.as_deref() != Some("idle") && Instant::now() < ready_deadline2 {
        thread::sleep(Duration::from_millis(10));
    }
    s2.record_writes();
    coord.insert_test_receipt("req-e1-fail", "inv-e1-fail");
    let h2 = coord.register_run("inv-e1-fail");
    let d2 = DerivedHandle {
        id: 102,
        provider: "claude".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: s2.clone(),
        native: Some("sid-e1-fail".to_owned()),
        account: None,
        launch_config: None,
        hook_dir: None,
        codex: None,
    };
    assert!(
        tui::submit_prompt(&coord, &terminals, &d2, true, "req-e1-fail", "a\nb", &h2).is_none()
    );
    assert!(s2.recorded_writes().is_empty());
    let term = coord
        .run_receipt_by_invocation("inv-e1-fail")
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(term.status, "failed");
    assert!(
        term.reason
            .as_deref()
            .is_some_and(|r| r.contains("终端未进入可粘贴状态"))
    );
    s2.stop();
}
