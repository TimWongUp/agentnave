//! Opt-in integration with an authenticated Codex CLI; never uses the real data root.
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::super::{Start, Terminals, codex_server, wakeup};
use super::Session;
use serde_json::json;

struct Cleanup(Arc<Session>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.0.stop();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.0.exit().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        if self.0.exit().is_none()
            && let Some(pid) = self.0.pid().and_then(|pid| libc::pid_t::try_from(pid).ok())
        {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

fn answer_codex_queries(
    session: &Session,
    from: &mut u64,
    screen: &mut String,
    answered: &mut [usize; 4],
) {
    let chunk = session.read_view(*from, Duration::from_millis(100), None);
    *from = chunk.end;
    if chunk.bytes.is_empty() {
        return;
    }
    screen.push_str(&String::from_utf8_lossy(&chunk.bytes));
    let pairs: [(&str, &[u8], usize); 4] = [
        ("\x1b[6n", b"\x1b[1;1R", 0),
        ("\x1b]10;?", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\", 1),
        ("\x1b]11;?", b"\x1b]11;rgb:0000/0000/0000\x1b\\", 2),
        ("\x1b[c", b"\x1b[?62;22c", 3),
    ];
    for (needle, reply, slot) in pairs {
        let count = screen.matches(needle).count();
        while answered[slot] < count {
            let _ = session.write(reply);
            answered[slot] += 1;
        }
    }
}

#[test]
#[ignore = "真实 Codex app-server 调用，消耗额度，需要显式授权"]
fn codex_wakeup_live() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let cwd = temp.path().join("project");
    let codex_home = temp.path().join("codex-home");
    for path in [
        root.join("connections"),
        root.join("inbox"),
        cwd.clone(),
        codex_home.clone(),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    let real_codex_home = dirs::home_dir().expect("home dir").join(".codex");
    if real_codex_home.join("auth.json").is_file() {
        fs::copy(
            real_codex_home.join("auth.json"),
            codex_home.join("auth.json"),
        )
        .unwrap();
    }
    let canonical_cwd = fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
    let trust_toml = format!(
        "model = \"gpt-6-luna\"\nmodel_reasoning_effort = \"low\"\n\n[projects.{:?}]\ntrust_level = \"trusted\"\n\n[projects.{:?}]\ntrust_level = \"trusted\"\n",
        cwd.display().to_string(),
        canonical_cwd.display().to_string(),
    );
    fs::write(codex_home.join("config.toml"), trust_toml).unwrap();

    let extra: Vec<String> = [
        "-c",
        "model=gpt-6-luna",
        "-c",
        "model_reasoning_effort=low",
        "-c",
        "notify=[]",
        "-c",
        "features.hooks=false",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    let codex_home_str = codex_home.to_string_lossy().into_owned();
    let terminals = Terminals::default();
    let launch_reply = terminals.launch_with_extra(
        Start {
            provider: "codex",
            cwd: cwd.clone(),
            size: (130, 45),
            resume: None,
            workspaces: Vec::new(),
            light: false,
            account: None,
        },
        &extra,
        &[("CODEX_HOME", &codex_home_str)],
    );
    assert_eq!(
        launch_reply.status,
        200,
        "launch failed: {}",
        String::from_utf8_lossy(&launch_reply.body)
    );

    let (session, codex) = {
        let state = terminals.lock();
        let running = state.open.first().expect("codex running missing");
        (
            running.session.clone(),
            running
                .codex
                .clone()
                .expect("codex app-server client missing"),
        )
    };
    let _cleanup = Cleanup(session.clone());
    let wrapper_pid = session.pid().expect("wrapper PID unavailable");

    // Pump PTY terminal queries until the TUI renders and app-server has a connected thread.
    let mut from = 0u64;
    let mut screen = String::new();
    let mut answered = [0usize; 4];
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut thread_id: Option<String> = None;
    while Instant::now() < deadline {
        answer_codex_queries(&session, &mut from, &mut screen, &mut answered);
        assert!(
            session.exit().is_none(),
            "codex terminal exited prematurely: {screen:?}"
        );
        if codex.is_connected_for_test() {
            let tids = codex.loaded_thread_ids_for_test();
            if let Some(tid) = tids.into_iter().next() {
                thread_id = Some(tid);
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    let Some(tid) = thread_id else {
        let (connected, sock_exists, sock, last_err) = codex.debug_info_for_test();
        let tail: String = screen
            .chars()
            .rev()
            .take(1200)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        panic!(
            "Codex app-server thread did not appear: connected={connected}, sock_exists={sock_exists}, sock={sock:?}, last_err={last_err:?}, exit={:?}, screen_tail={tail:?}",
            session.exit()
        );
    };
    println!("connected to codex app-server; threadId={tid}");

    let boss_key = crate::data::boss_app::boss_key("codex", &tid);
    fs::write(
        root.join("connections")
            .join(format!("{wrapper_pid}-test.json")),
        json!({"host": "codex", "bosses": [&boss_key]}).to_string(),
    )
    .unwrap();
    let my_pid = std::process::id();
    let write_inbox = |keys: &[&str]| {
        let items: Vec<_> = keys
            .iter()
            .map(|k| json!({"key": k, "kind": "task_end", "role": "test", "status": "succeeded"}))
            .collect();
        fs::write(
            root.join("inbox").join(format!("{my_pid}.json")),
            json!({"version": 1, "pid": my_pid, "bosses": {&boss_key: {"waiting": false, "items": items}}}).to_string(),
        )
        .unwrap();
    };

    // Wait briefly for TUI input prompt, then type a user draft that must be preserved.
    let settle_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle_until {
        answer_codex_queries(&session, &mut from, &mut screen, &mut answered);
    }
    session.user_input(b"USER_DRAFT_KEEP", false).unwrap();
    let draft_until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < draft_until {
        answer_codex_queries(&session, &mut from, &mut screen, &mut answered);
    }

    write_inbox(&["codex-evt-1"]);
    let mut state = wakeup::WakeupState::default();
    wakeup::scan(&terminals, &root, &mut state);
    assert!(
        state.attempted.contains("codex-evt-1") && !state.failed.contains("codex-evt-1"),
        "codex wakeup scan did not deliver"
    );

    // Wait for TUI/server to start and finish the turn via app-server.
    let mut saw_active = false;
    let mut saw_idle_after_active = false;
    let turn_deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < turn_deadline {
        answer_codex_queries(&session, &mut from, &mut screen, &mut answered);
        match codex.thread_status_for_test(&tid) {
            Some(codex_server::ThreadStatus::Active { .. }) => saw_active = true,
            Some(codex_server::ThreadStatus::Idle) if saw_active => {
                saw_idle_after_active = true;
                break;
            }
            _ => {}
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(saw_active, "Codex thread never entered active turn");
    assert!(saw_idle_after_active, "Codex turn did not complete");

    // Re-scanning with the same event must not re-deliver; same clientUserMessageId also deduplicates.
    wakeup::scan(&terminals, &root, &mut state);
    assert!(matches!(
        codex.deliver(&boss_key, "codex-evt-1", "[AgentNave] duplicate check"),
        codex_server::Delivery::Queued { .. } | codex_server::Delivery::Started
    ));
    thread::sleep(Duration::from_secs(2));
    answer_codex_queries(&session, &mut from, &mut screen, &mut answered);
    assert!(
        screen.contains("[AgentNave]"),
        "TUI did not render delivered wakeup message"
    );
    assert!(
        screen.contains("USER_DRAFT_KEEP"),
        "TUI draft was lost during app-server delivery"
    );
    println!(
        "verified: delivered via app-server, TUI started & completed turn, draft preserved, duplicate event skipped"
    );

    // Verify closing terminal ends the whole process group cleanly.
    codex.close();
    session.stop();
    let stop_deadline = Instant::now() + Duration::from_secs(10);
    while session.exit().is_none() && Instant::now() < stop_deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        session.exit().is_some(),
        "codex wrapper process group did not exit after stop()"
    );
}
