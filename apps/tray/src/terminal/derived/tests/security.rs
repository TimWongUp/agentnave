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
            life: Default::default(),
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
            life: Default::default(),
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

#[cfg(unix)]
#[test]
fn s1_transcript_and_hook_reject_symlinks_and_fifos_without_blocking() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let real_sid = "11111111-1111-4111-8111-111111111111";
    let link_sid = "22222222-2222-4222-8222-222222222222";
    let real_file = proj.join(format!("{real_sid}.jsonl"));
    fs::write(&real_file, "{}\n").unwrap();
    let link_file = proj.join(format!("{link_sid}.jsonl"));
    std::os::unix::fs::symlink(&real_file, &link_file).unwrap();

    // Symlinked transcript path must be rejected.
    let homes = vec![dir.path().to_path_buf()];
    assert!(native_logs::claude_transcript_path(&homes, link_sid).is_none());
    assert!(native_logs::claude_transcript_path(&homes, real_sid).is_some());

    // Symlinked hook events file must neither be read nor written by Stop hook.
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let target = dir.path().join("victim.txt");
    fs::write(&target, "orig\n").unwrap();
    let events_file = claude_hook::events_path(hook_dir.path());
    std::os::unix::fs::symlink(&target, &events_file).unwrap();

    let settings = claude_hook::stop_hook_settings(hook_dir.path());
    let cmd = settings[0]["hooks"][0]["command"].as_str().unwrap();
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
        .write_all(br#"{"hook_event_name":"Stop","session_id":"s1","background_tasks":[]}"#)
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(fs::read_to_string(&target).unwrap(), "orig\n");
    let mut offset = 0;
    assert!(
        claude::turn_state::read_hook_events(&events_file, &mut offset)
            .events
            .is_empty()
    );

    // FIFO at events.jsonl must not block read_hook_events.
    fs::remove_file(&events_file).unwrap();
    let c_path = std::ffi::CString::new(events_file.as_os_str().as_encoded_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(rc, 0);
    let read_res = claude::turn_state::read_hook_events(&events_file, &mut offset);
    assert!(read_res.events.is_empty());
}

#[cfg(unix)]
#[test]
fn s1_hook_budgets_and_queue_caps_drop_with_warning() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let events_file = claude_hook::events_path(hook_dir.path());
    let settings = claude_hook::stop_hook_settings(hook_dir.path());
    let cmd = settings[0]["hooks"][0]["command"].as_str().unwrap();

    // Hook stdin > 1 MiB must be dropped without creating/appending events.jsonl.
    let huge = format!(
        "{{\"hook_event_name\":\"Stop\",\"session_id\":\"s1\",\"last_assistant_message\":\"{}\"}}",
        "x".repeat(1_048_600)
    );
    let mut child = Command::new("/bin/sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(huge.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(!events_file.exists());

    // Oversized line + excess events per poll must be dropped and record warnings.
    let sid = "33333333-3333-4333-8333-333333333333";
    {
        let mut f = fs::File::create(&events_file).unwrap();
        let big_line = format!(
            "{{\"hook_event_name\":\"Stop\",\"session_id\":\"{sid}\",\"pad\":\"{}\"}}\n",
            "a".repeat(claude::turn_state::MAX_HOOK_LINE_BYTES + 128)
        );
        f.write_all(big_line.as_bytes()).unwrap();
        for i in 0..(claude::turn_state::MAX_HOOK_EVENTS_PER_POLL + 10) {
            writeln!(
                f,
                "{}",
                json!({"hook_event_name":"Stop","session_id":sid,"prompt_id":format!("p-{i}")})
            )
            .unwrap();
        }
    }
    let mut tracker = claude::turn_state::ClaudeTurnTracker::new(
        sid,
        0,
        claude::ClaudeMonitorConfig {
            signal_pair_timeout: Duration::from_secs(30),
            bg_reminder_delay: Duration::from_secs(60),
        },
    );
    let now = Instant::now();
    assert!(tracker.ingest_hook_file(&events_file, now).is_none());
    let scan = native_logs::ClaudeDispatchScan::default();
    let (decision, _, _) = tracker.step("p-start", &scan, None, false, now);
    assert_eq!(decision, claude::turn_state::ClaudeTurnDecision::Continue);

    // Pending pair queues must cap at MAX_PENDING_PAIR_QUEUE and record a warning on settlement.
    let mut scan_q = native_logs::ClaudeDispatchScan {
        found_start: true,
        segment_end_row: 200,
        ..Default::default()
    };
    for i in 0..(claude::turn_state::MAX_PENDING_PAIR_QUEUE + 5) {
        scan_q.turn_durations.push(native_logs::ClaudeTurnDuration {
            row_index: i + 1,
            prompt_id: Some(format!("pd-{i}")),
        });
    }
    let (d_q, _, _) = tracker.step("p-start", &scan_q, None, false, now);
    assert_eq!(d_q, claude::turn_state::ClaudeTurnDecision::Continue);

    // Settle after timeout and confirm budget warnings were preserved.
    let later = now + Duration::from_secs(31);
    let mut settled_warnings = Vec::new();
    for _ in 0..=claude::turn_state::MAX_PENDING_PAIR_QUEUE {
        let (dec, _, _) = tracker.step("p-start", &scan_q, None, false, later);
        if let claude::turn_state::ClaudeTurnDecision::Settled { warnings, .. } = dec {
            settled_warnings = warnings;
            break;
        }
    }
    assert!(
        settled_warnings
            .iter()
            .any(|w| w.contains("单行事件超过上限"))
    );
    assert!(
        settled_warnings
            .iter()
            .any(|w| w.contains("单轮事件数超过上限"))
    );
    assert!(
        settled_warnings
            .iter()
            .any(|w| w.contains("待配对队列超过上限"))
    );
}

#[test]
fn s1_hook_requires_session_id_and_pairs_matching_prompt_ids() {
    use std::time::{Duration, Instant};

    let sid = "44444444-4444-4444-8444-444444444444";
    assert!(
        claude::turn_state::parse_hook_event(
            &json!({"hook_event_name":"Stop","background_tasks":[]})
        )
        .is_none()
    );
    assert!(
        claude::turn_state::parse_hook_event(
            &json!({"hook_event_name":"Stop","session_id":"","background_tasks":[]})
        )
        .is_none()
    );
    assert!(
        claude::turn_state::parse_hook_event(
            &json!({"hook_event_name":"StopFailure","session_id":null,"error":"rate_limit"})
        )
        .is_none()
    );

    let dir = tempfile::tempdir().unwrap();
    let events_file = dir.path().join("events.jsonl");
    // Unrelated prompt_id "p-other" must NOT pair with duration for "p-1".
    fs::write(
        &events_file,
        format!(
            "{}\n",
            json!({
                "hook_event_name": "Stop",
                "session_id": sid,
                "prompt_id": "p-other",
                "background_tasks": []
            })
        ),
    )
    .unwrap();
    let mut tracker = claude::turn_state::ClaudeTurnTracker::new(
        sid,
        0,
        claude::ClaudeMonitorConfig {
            signal_pair_timeout: Duration::from_secs(30),
            bg_reminder_delay: Duration::from_secs(60),
        },
    );
    let now = Instant::now();
    assert!(tracker.ingest_hook_file(&events_file, now).is_none());
    let scan = native_logs::ClaudeDispatchScan {
        found_start: true,
        owned_prompt_ids: vec!["p-1".to_owned()],
        turn_durations: vec![native_logs::ClaudeTurnDuration {
            row_index: 2,
            prompt_id: Some("p-1".to_owned()),
        }],
        segment_end_row: 3,
        ..Default::default()
    };
    let (decision, _, _) = tracker.step("p-1", &scan, None, false, now);
    assert_eq!(decision, claude::turn_state::ClaudeTurnDecision::Continue);

    // Once the matching Stop(prompt_id="p-1") arrives, it pairs with p-1; after the stray
    // Stop("p-other") times out and is ignored, it settles cleanly with end_prompt_id == "p-1".
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&events_file)
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({
                "hook_event_name": "Stop",
                "session_id": sid,
                "prompt_id": "p-1",
                "background_tasks": []
            })
        )
        .unwrap();
    }
    assert!(tracker.ingest_hook_file(&events_file, now).is_none());
    let (mid_dec, _, _) = tracker.step("p-1", &scan, None, false, now);
    assert_eq!(mid_dec, claude::turn_state::ClaudeTurnDecision::Continue);
    let (decision2, _, _) = tracker.step("p-1", &scan, None, false, now + Duration::from_secs(31));
    assert!(matches!(
        decision2,
        claude::turn_state::ClaudeTurnDecision::Settled {
            end_prompt_id: Some(ref pid),
            ref warnings,
            ..
        } if pid == "p-1" && warnings.iter().any(|w| w.contains("不匹配当前回合"))
    ));
}

#[cfg(unix)]
#[test]
fn s1_oversized_transcript_line_counts_as_row_and_skips_empty_cleanup() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("projects/p1");
    fs::create_dir_all(&proj).unwrap();

    // 1) claude_dispatch_scan counts an >8 MiB line as a row while skipping JSON parsing.
    let sid1 = "66666666-6666-4666-8666-666666666666";
    let t1 = proj.join(format!("{sid1}.jsonl"));
    {
        let mut f = fs::File::create(&t1).unwrap();
        writeln!(
            f,
            "{}",
            json!({"type": "user", "promptId": "p0", "message": {"content": "go"}})
        )
        .unwrap();
        f.write_all(b"{\"type\":\"assistant\",\"pad\":\"").unwrap();
        f.write_all(&vec![b'x'; native_logs::MAX_TRANSCRIPT_LINE_BYTES + 1024])
            .unwrap();
        f.write_all(b"\"}\n").unwrap();
        writeln!(
            f,
            "{}",
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "ok"}]}})
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            json!({"type": "system", "subtype": "turn_duration"})
        )
        .unwrap();
    }
    let scan = native_logs::claude_dispatch_scan(&t1, "p0");
    assert_eq!(scan.segment_end_row, 4);
    assert_eq!(scan.last_reply_end_row, Some(4));

    // 2) drop_empty_claude_session must NOT delete a transcript that skipped an oversized line.
    let sid2 = "77777777-7777-4777-8777-777777777777";
    let t2 = proj.join(format!("{sid2}.jsonl"));
    {
        let mut f = fs::File::create(&t2).unwrap();
        writeln!(
            f,
            "{}",
            json!({"type": "user", "promptId": "p0", "message": {"content": "go"}})
        )
        .unwrap();
        f.write_all(b"{\"type\":\"assistant\",\"pad\":\"").unwrap();
        f.write_all(&vec![b'y'; native_logs::MAX_TRANSCRIPT_LINE_BYTES + 1024])
            .unwrap();
        f.write_all(b"\"}\n").unwrap();
    }
    let homes = vec![dir.path().to_path_buf()];
    claude_hook::drop_empty_claude_session(&homes, sid2);
    assert!(t2.exists());
}

#[cfg(unix)]
#[test]
fn s1_oversized_user_prompt_submit_fails_closed_with_block() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let hook_dir = claude_hook::prepare_claude_hook(true).unwrap();
    let settings = claude_hook::hook_settings(hook_dir.path());
    let cmd = settings[0]["hooks"][0]["command"].as_str().unwrap();

    let huge = format!(
        "{{\"permission_mode\":\"bypassPermissions\",\"prompt\":\"{}\"}}",
        "x".repeat(1_048_600)
    );
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
        .write_all(huge.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let decision: Value = serde_json::from_str(stdout.trim()).expect("must emit block JSON");
    assert_eq!(decision["decision"], json!("block"));
    assert_eq!(
        claude_hook::read_observed_mode(hook_dir.path()).as_deref(),
        Some("default")
    );
}
