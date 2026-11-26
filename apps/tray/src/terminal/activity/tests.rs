use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use portable_pty::CommandBuilder;

use super::super::Running;
use super::super::codex_server::{Client as CodexClient, ThreadStatus};
use super::super::session::Session;
use crate::data::boss_app::boss_key;

fn fake_session_with_osc(states: &[&str]) -> Arc<Session> {
    let script = if states.is_empty() {
        "stty -echo; printf up; while read -r _line; do :; done".to_owned()
    } else {
        "stty -echo; printf up; while read -r line; do printf '\\033]7501;state=%s\\007' \"$line\"; done"
            .to_owned()
    };
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", &script]);
    cmd.cwd(std::env::temp_dir());
    let session = Session::start(cmd, 80, 24).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.recent_screen_text(64).contains("up") {
        assert!(Instant::now() < deadline, "fake cli startup timeout");
        thread::sleep(Duration::from_millis(10));
    }
    for state in states {
        let before = session.osc_snapshot().0;
        session.write(format!("{state}\n").as_bytes()).unwrap();
        while session.osc_snapshot().0 <= before {
            assert!(Instant::now() < deadline, "osc signal timeout for {state}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    session
}

fn boss_running(
    provider: &str,
    session: Arc<Session>,
    workspaces: Vec<String>,
    codex: Option<Arc<CodexClient>>,
) -> Running {
    Running {
        id: 1,
        provider: provider.to_owned(),
        cwd: std::env::temp_dir(),
        session,
        workspaces,
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex,
        derived: None,
    }
}

#[test]
fn boss_activity_tracks_osc_signals_and_unknown_default() {
    let dir = tempfile::tempdir().unwrap();
    let unknown = boss_running("claude", fake_session_with_osc(&[]), vec![], None);
    assert_eq!(unknown.boss_activity(), "unknown");
    assert_eq!(unknown.describe(dir.path())["activity"], "unknown");

    for (osc, expected) in [
        ("blocked", "blocked"),
        ("working", "working"),
        ("idle", "idle"),
        ("done", "idle"),
        ("error", "idle"),
    ] {
        let running = boss_running("claude", fake_session_with_osc(&[osc]), vec![], None);
        assert_eq!(running.boss_activity(), expected, "osc={osc}");
        assert_eq!(running.describe(dir.path())["activity"], expected);
    }
}

#[test]
fn codex_boss_resolves_cached_thread_status_by_boss_key() {
    let tid = "11111111-1111-4111-8111-111111111111";
    let ws = boss_key("codex", tid);
    let client = CodexClient::new_for_test();
    let running = boss_running(
        "codex",
        fake_session_with_osc(&[]),
        vec![ws],
        Some(client.clone()),
    );
    assert_eq!(running.boss_activity(), "unknown");

    client.set_thread_status_for_test(
        tid,
        ThreadStatus::Active {
            waiting_on_approval: false,
        },
    );
    assert_eq!(running.boss_activity(), "working");

    client.set_thread_status_for_test(
        tid,
        ThreadStatus::Active {
            waiting_on_approval: true,
        },
    );
    assert_eq!(running.boss_activity(), "blocked");

    client.set_thread_status_for_test(tid, ThreadStatus::Idle);
    assert_eq!(running.boss_activity(), "idle");

    client.set_thread_status_for_test(tid, ThreadStatus::Unknown);
    assert_eq!(running.boss_activity(), "unknown");
}

#[test]
fn codex_boss_follows_the_latest_confirmed_workspace() {
    let old = "11111111-1111-4111-8111-111111111111";
    let current = "22222222-2222-4222-8222-222222222222";
    let client = CodexClient::new_for_test();
    client.set_thread_status_for_test(old, ThreadStatus::Idle);
    client.set_thread_status_for_test(
        current,
        ThreadStatus::Active {
            waiting_on_approval: false,
        },
    );
    let mut running = boss_running(
        "codex",
        fake_session_with_osc(&[]),
        vec![boss_key("codex", old), boss_key("codex", current)],
        Some(client),
    );
    assert_eq!(running.boss_activity(), "working");
    // The terminal keeps the first chat's thread as its native one after moving on.
    running.native = Some(old.to_owned());
    assert_eq!(running.boss_activity(), "working");
}
