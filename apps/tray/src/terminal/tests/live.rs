//! Opt-in integration with an authenticated Claude CLI; never uses the real data root.
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::super::{Running, Start, Terminals, codex_server, session, theme, wakeup};
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

fn wait_state(session: &Session, wanted: &[&str], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if session
            .program_state()
            .as_deref()
            .is_some_and(|s| wanted.contains(&s))
        {
            return true;
        }
        if session.exit().is_some() {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

fn inbox(root: &Path, keys: &[&str]) {
    let pid = std::process::id();
    let items: Vec<_> = keys
        .iter()
        .map(|key| json!({"key":key,"kind":"task_end","role":"test","status":"succeeded"}))
        .collect();
    fs::write(root.join("inbox").join(format!("{pid}.json")), json!({"version":1,"pid":pid,"bosses":{"s-11111111111111111111111111111111":{"waiting":false,"items":items}}}).to_string()).unwrap();
}

fn launch(root: &Path, cwd: &Path) -> Arc<Session> {
    let program = super::super::installed_path("claude").expect("Claude CLI not installed");
    let chosen = session::fresh_session_id();
    let style = theme::for_cli("claude", false);
    let args = [
        session::cli_args("claude", None, Some(&chosen)),
        style.args,
        vec![
            "--model".into(),
            "haiku".into(),
            "--strict-mcp-config".into(),
            "--mcp-config".into(),
            "{\"mcpServers\":{}}".into(),
        ],
    ]
    .concat();
    let mut command = session::command(&program, &args, cwd);
    command.env("AGENTNAVE_DATA_DIR", root);
    for (key, value) in style.env {
        command.env(key, value);
    }
    Session::start(command, 120, 40).expect("could not launch Claude")
}

fn wait_for_ready(session: &Session) {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut from = 0;
    let mut screen = String::new();
    let mut trusted = false;
    let mut da_replies = 0;
    while Instant::now() < deadline {
        let chunk = session.read_view(from, Duration::from_millis(200), None);
        from = chunk.end;
        screen.push_str(&String::from_utf8_lossy(&chunk.bytes));
        let da_queries = screen.matches("\x1b[c").count();
        while da_replies < da_queries {
            session.write(b"\x1b[?1;2c").unwrap();
            da_replies += 1;
        }
        if !trusted
            && screen.to_lowercase().contains("trust")
            && screen.to_lowercase().contains("folder")
            && screen.contains("confirm")
        {
            thread::sleep(Duration::from_millis(300));
            session.user_input(b"\x1b[B", false).unwrap();
            thread::sleep(Duration::from_millis(100));
            session.user_input(b"\r", false).unwrap();
            trusted = true;
            println!("trust: accepted directory prompt");
        }
        if session
            .program_state()
            .as_deref()
            .is_some_and(|s| s != "blocked")
        {
            println!("startup: OSC 7501 state={:?}", session.program_state());
            return;
        }
        if chunk.exit.is_some() {
            break;
        }
    }
    let tail: String = screen
        .chars()
        .rev()
        .take(1200)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    panic!(
        "Claude did not report OSC 7501; exit={:?}; screen tail={tail:?}",
        session.exit()
    );
}

#[test]
#[ignore = "真实 Claude Haiku 调用，消耗额度，需要显式授权"]
fn claude_wakeup_live() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let cwd = temp.path().join("project");
    for path in [root.join("connections"), root.join("inbox"), cwd.clone()] {
        fs::create_dir_all(path).unwrap();
    }
    let session = launch(&root, &cwd);
    let _cleanup = Cleanup(session.clone());
    let pid = session.pid().expect("Claude PID unavailable");
    fs::write(
        root.join("connections").join(format!("{pid}-test.json")),
        json!({"bosses":["s-11111111111111111111111111111111"]}).to_string(),
    )
    .unwrap();
    let terminals = Terminals::default();
    terminals.lock().open.push(Running {
        id: 1,
        provider: "claude".into(),
        cwd,
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex: None,
        derived: None,
    });
    let mut state = wakeup::WakeupState::default();
    wait_for_ready(&session);
    assert!(
        wait_state(
            &session,
            &["idle", "done", "error"],
            Duration::from_secs(45)
        ),
        "Claude not ready"
    );
    thread::sleep(Duration::from_secs(4));
    inbox(&root, &["one"]);
    wakeup::scan(&terminals, &root, &mut state);
    assert!(
        state.attempted.contains("one") && !state.failed.contains("one"),
        "first scan did not deliver"
    );
    assert!(
        wait_state(&session, &["working"], Duration::from_secs(30)),
        "Claude did not become working"
    );
    println!("(a) first inbox delivered; OSC state=working");
    assert!(
        wait_state(
            &session,
            &["idle", "done", "error"],
            Duration::from_secs(90)
        ),
        "first turn did not finish"
    );
    session.record_writes();
    session.user_input(b"Reply briefly. ", false).unwrap();
    inbox(&root, &["one", "two"]);
    thread::sleep(Duration::from_secs(3));
    wakeup::scan(&terminals, &root, &mut state);
    assert!(
        !state.attempted.contains("two"),
        "draft delivered at 3 seconds"
    );
    thread::sleep(Duration::from_secs(7));
    wakeup::scan(&terminals, &root, &mut state);
    assert!(
        !state.attempted.contains("two"),
        "draft delivered at 10 seconds"
    );
    println!("(b) draft blocked delivery at 3s and 10s");
    session.set_draft_wait(Duration::from_secs(2));
    let held = session.clone();
    let inject = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !held.is_holding() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(held.is_holding(), "wakeup never entered holding state");
        held.user_input(b"HELD123", false).unwrap();
    });
    wakeup::scan(&terminals, &root, &mut state);
    inject.join().unwrap();
    assert!(
        state.attempted.contains("two") && !state.failed.contains("two"),
        "draft scan did not deliver"
    );
    assert!(
        wait_state(&session, &["working"], Duration::from_secs(30)),
        "draft turn did not start"
    );
    println!("(b) shortened draft wait delivered; OSC state=working");
    let writes = session.recorded_writes();
    assert!(writes.iter().any(|bytes| bytes == b"Reply briefly. "));
    assert!(writes.iter().any(|bytes| {
        bytes
            .windows(b"[AgentNave]".len())
            .any(|s| s == b"[AgentNave]")
    }));
    assert!(
        writes.iter().any(|bytes| bytes == b"\rHELD123"),
        "held input was not sent immediately after Enter: {writes:?}"
    );
    println!("(c) PTY accepted Enter followed by exact held bytes HELD123");
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
