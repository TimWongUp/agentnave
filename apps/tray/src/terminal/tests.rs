use std::time::{Duration, Instant};

use portable_pty::CommandBuilder;

use super::session::{KEPT, Output, Session, parent_session};

/// Reads until `wanted` shows up in the output or the CLI exits, within a few seconds.
fn output_until(session: &Session, wanted: &str) -> (String, Option<u32>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut text, mut from) = (String::new(), 0);
    while Instant::now() < deadline {
        let chunk = session.read(from, Duration::from_millis(200));
        text.push_str(&String::from_utf8_lossy(&chunk.bytes));
        from = chunk.end;
        if text.contains(wanted) || chunk.exit.is_some() {
            return (text, chunk.exit);
        }
    }
    (text, session.exit())
}

/// Waits for the CLI to exit, reading whatever it still writes.
fn exit_of(session: &Session) -> Option<u32> {
    output_until(session, "\u{0}").1
}

fn shell(script: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", script]);
    command.cwd(std::env::temp_dir());
    command
}

#[cfg(unix)]
#[test]
fn a_cli_reads_keys_and_its_output_and_exit_are_kept() {
    let session = Session::start(shell("printf ready; read line; echo got:$line"), 80, 24).unwrap();
    assert!(output_until(&session, "ready").0.contains("ready"));
    session.write(b"keys\r").unwrap();
    let (text, _) = output_until(&session, "got:keys");
    assert!(text.contains("got:keys"), "{text:?}");
    assert_eq!(exit_of(&session), Some(0));
    // A page that attaches later replays from the start.
    let replay = session.read(0, Duration::ZERO);
    assert_eq!(replay.start, 0);
    assert!(String::from_utf8_lossy(&replay.bytes).contains("got:keys"));
}

#[cfg(unix)]
#[test]
fn stop_hangs_up_the_cli_with_its_children() {
    let session = Session::start(shell("sleep 60 & printf started; wait"), 80, 24).unwrap();
    assert!(output_until(&session, "started").0.contains("started"));
    session.resize(100, 30).unwrap();
    session.stop();
    assert!(
        exit_of(&session).is_some(),
        "the CLI and the sleep holding the terminal must both end"
    );
}

#[test]
fn old_output_is_dropped_past_the_limit() {
    let mut output = Output::default();
    output.push(&vec![b'a'; KEPT]);
    output.push(b"tail");
    assert_eq!(output.start, 4);
    assert_eq!(output.end(), KEPT as u64 + 4);
}

#[test]
fn a_parent_session_is_not_passed_on_but_the_users_settings_are() {
    for name in [
        "CLAUDECODE",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
        "AGENTNAVE_HOST_SESSION_ID",
        "TERM_PROGRAM",
    ] {
        assert!(parent_session(name), "{name}");
    }
    for name in [
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "CODEBUDDY_CONFIG_DIR",
        "HOME",
        "PATH",
    ] {
        assert!(!parent_session(name), "{name}");
    }
}

/// A relay running under a terminal's CLI names the workspaces that CLI is the boss of: the one a
/// chat owns, and an older chat's own; a chat that never dispatched names none.
#[test]
fn a_relay_under_a_terminal_names_its_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::create(root.path());
    let (owned, older, idle) = (
        format!("s-{}", "a".repeat(32)),
        format!("s-{}", "b".repeat(32)),
        format!("s-{}", "c".repeat(32)),
    );
    db.execute(
        "INSERT INTO workspaces (id, owner, owner_host) VALUES ('s-handed', ?1, 'claude')",
        [&owned],
    )
    .unwrap();
    db.execute(
        "INSERT INTO conversations (id, kind, title, created, host_source, manager_session, \
         review, isolated, seats, sessions, rev) \
         VALUES ('c-1', 'task', 't', '2026-11-18T00:00:00Z', 'unknown', ?1, 0, 0, '[]', '{}', 1)",
        [&older],
    )
    .unwrap();
    // This test process stands in for the terminal's CLI and a child of it for the relay.
    let mut relay = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = serde_json::json!({"host": "claude", "bosses": [owned, idle, older]});
    std::fs::write(
        connections.join(format!("{}-x.json", relay.id())),
        receipt.to_string(),
    )
    .unwrap();
    let found = super::links::workspaces(root.path(), &[(7, std::process::id()), (8, 1)]);
    relay.kill().unwrap();
    relay.wait().unwrap();
    assert_eq!(found.get(&7), Some(&vec!["s-handed".to_owned(), older]));
    assert_eq!(found.get(&8), None);
}
