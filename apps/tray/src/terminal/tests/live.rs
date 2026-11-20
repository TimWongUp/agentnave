//! Opt-in integration with an authenticated Claude CLI; never uses the real data root.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::super::{Running, Terminals, session, theme, wakeup};
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
    });
    let (mut attempted, mut seen, mut failed) = (HashSet::new(), HashMap::new(), HashSet::new());
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
    wakeup::scan(&terminals, &root, &mut attempted, &mut seen, &mut failed);
    assert!(
        attempted.contains("one") && !failed.contains("one"),
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
    wakeup::scan(&terminals, &root, &mut attempted, &mut seen, &mut failed);
    assert!(!attempted.contains("two"), "draft delivered at 3 seconds");
    thread::sleep(Duration::from_secs(7));
    wakeup::scan(&terminals, &root, &mut attempted, &mut seen, &mut failed);
    assert!(!attempted.contains("two"), "draft delivered at 10 seconds");
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
    wakeup::scan(&terminals, &root, &mut attempted, &mut seen, &mut failed);
    inject.join().unwrap();
    assert!(
        attempted.contains("two") && !failed.contains("two"),
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
