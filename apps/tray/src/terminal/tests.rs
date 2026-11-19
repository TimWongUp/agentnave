use std::path::Path;
use std::time::{Duration, Instant};

use portable_pty::CommandBuilder;

use super::session::{KEPT, Output, Session, cli_args, command, parent_session};
use super::{Running, Terminals};

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

fn text(chunk: &super::session::Chunk) -> String {
    String::from_utf8_lossy(&chunk.bytes).into_owned()
}

/// What a page has been handed is replay when it asks for it again, and it stops at the mark:
/// output that arrived while no page read is new, however early in the stream the page starts.
#[test]
fn a_replay_stops_at_the_watermark_and_later_output_is_new() {
    let mut output = Output::default();
    output.push(b"screen ");
    let first = output.take(0);
    assert!(!first.replay);
    assert_eq!((text(&first).as_str(), first.end), ("screen ", 7));

    // The page went away and the CLI asked a question meanwhile.
    output.push(b"query");
    let again = output.take(0);
    assert!(again.replay);
    assert_eq!((text(&again).as_str(), again.end), ("screen ", 7));
    let fresh = output.take(again.end);
    assert!(!fresh.replay);
    assert_eq!(text(&fresh), "query");

    // Everything handed out so far replays from the start, and nothing new follows.
    let all = output.take(0);
    assert!(all.replay);
    assert_eq!(text(&all), "screen query");
    assert!(output.take(all.end).bytes.is_empty());
}

/// Dropping old output keeps the mark where it was: the kept part of what was delivered replays,
/// what came after is new, and a mark that fell off the start leaves nothing to replay.
#[test]
fn the_watermark_survives_old_output_being_dropped() {
    let mut output = Output::default();
    output.push(&vec![b'a'; KEPT]);
    assert!(!output.take(0).replay);
    output.push(&[b'b'; 100]);
    let replay = output.take(0);
    assert!(replay.replay);
    assert_eq!((replay.start, replay.end), (100, KEPT as u64));
    assert_eq!(replay.bytes.len(), KEPT - 100);
    let fresh = output.take(replay.end);
    assert!(!fresh.replay);
    assert_eq!(fresh.bytes, vec![b'b'; 100]);

    // More output than is kept arrives before anyone reads: all of it is new.
    output.push(&vec![b'c'; KEPT + 10]);
    let all_new = output.take(0);
    assert!(!all_new.replay);
    assert_eq!(all_new.start, output.start);
}

/// The latest DECSET/DECRST 2031 counts, whether it comes alone or among other modes, and
/// however the output is cut into reads or trimmed afterwards.
#[test]
fn the_appearance_subscription_follows_the_latest_mode_2031() {
    let mut output = Output::default();
    let subscribed = |output: &mut Output| output.take(0).appearance;
    assert!(!subscribed(&mut output));
    output.push(b"\x1b[?2004;2031h");
    assert!(subscribed(&mut output));
    output.push(b"\x1b[?2031l");
    assert!(!subscribed(&mut output));
    // Cut anywhere in the sequence, including inside the number.
    for cut in 1..b"\x1b[?25;2031h".len() {
        let sequence = b"\x1b[?25;2031h";
        let mut split = Output::default();
        split.push(&sequence[..cut]);
        split.push(&sequence[cut..]);
        assert!(subscribed(&mut split), "cut at {cut}");
    }
    // Other numbers, other sequences and text that merely looks alike do not count.
    for ignored in [
        &b"\x1b[?12031h"[..],
        b"\x1b[?20310h",
        b"\x1b[2031h",
        b"\x1b[?2031$p",
        b"\x1b[?2031 h",
        b"2031h",
    ] {
        let mut other = Output::default();
        other.push(ignored);
        assert!(!subscribed(&mut other), "{ignored:?}");
    }
    // Subscribed, then far more output than is kept: the state stays.
    output.push(b"\x1b[?2031h");
    output.push(&vec![b'x'; KEPT * 2]);
    assert!(subscribed(&mut output));
}

/// The output route says whether the bytes are a replay and whether the CLI subscribes to
/// appearance changes, in headers that never touch the byte stream.
#[cfg(unix)]
#[test]
fn the_output_route_marks_replays_and_the_appearance_subscription() {
    let session =
        Session::start(shell("printf '\\033[?2004;2031hready'; sleep 30"), 80, 24).unwrap();
    assert!(output_until(&session, "ready").0.contains("ready"));
    let terminals = Terminals::default();
    terminals.lock().open.push(Running {
        id: 1,
        provider: "claude".to_owned(),
        cwd: std::env::temp_dir(),
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        started: std::time::SystemTime::now(),
    });
    let header = |reply: &crate::response::Response, name: &str| {
        reply
            .headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    };
    // output_until read it all, so the page asking from the start is a replay that ends there.
    let replay = terminals.output(Some(1), Some(0));
    assert_eq!(header(&replay, "X-Terminal-Replay").as_deref(), Some("1"));
    assert_eq!(
        header(&replay, "X-Terminal-Appearance").as_deref(),
        Some("1")
    );
    let end = header(&replay, "X-Terminal-End").unwrap();

    // Whatever the CLI writes after that point is new, not a replay.
    session.stop();
    while session.exit().is_none() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let live = terminals.output(Some(1), Some(end.parse().unwrap()));
    assert_eq!(header(&live, "X-Terminal-Replay"), None);
}

#[test]
fn a_new_claude_chat_is_started_with_its_id_as_separate_arguments() {
    let id = super::session::fresh_session_id();
    assert_eq!(
        cli_args("claude", None, Some(&id)),
        ["--session-id", id.as_str()]
    );
    // Continuing names the chat already; other CLIs take no ID at all.
    assert_eq!(
        cli_args("claude", Some("old"), Some(&id)),
        ["--resume", "old"]
    );
    assert_eq!(cli_args("codex", None, Some(&id)), ["--no-daemon"]);
    assert!(cli_args("grok", None, Some(&id)).is_empty());
    let second = super::session::fresh_session_id();
    assert_ne!(id, second);
    let parts: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(parts, [8, 4, 4, 4, 12]);
    assert_eq!(id.as_bytes()[14], b'4');
    assert!(
        id.bytes()
            .all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

#[test]
fn a_cli_starts_at_the_size_the_page_measured_within_sane_bounds() {
    use super::{Attach, launch_size};
    assert_eq!(launch_size(None, None), (100, 30));
    assert_eq!(launch_size(Some(132), Some(41)), (132, 41));
    assert_eq!(launch_size(Some(1), Some(1)), (20, 5));
    assert_eq!(launch_size(Some(99_999), Some(99_999)), (500, 200));
    let attach = Attach::of("session=3&workspace=s-a&cols=90&rows=33&light=1&confirm=1");
    assert_eq!(
        (attach.size, attach.light, attach.confirm),
        ((90, 33), true, true)
    );
    let plain = Attach::of("workspace=s-a");
    assert_eq!(
        (plain.size, plain.light, plain.confirm),
        ((100, 30), false, false)
    );
}

/// A chat whose relay lives outside every App terminal is open elsewhere; whether its processes
/// are stopped shows as paused. A relay under an App terminal, an ended or unrelated one is not.
#[cfg(unix)]
#[test]
fn a_relay_outside_the_terminals_means_the_chat_is_open_elsewhere() {
    let root = tempfile::tempdir().unwrap();
    let key = format!("s-{}", "d".repeat(32));
    let mut relay = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = connections.join(format!("{}-x.json", relay.id()));
    let write = |ended: Option<u64>| {
        let value = serde_json::json!({"host": "claude", "bosses": [key], "ended_ms": ended});
        std::fs::write(&receipt, value.to_string()).unwrap();
    };
    write(None);
    let find =
        |terminals: &[(u64, u32)], key: &str| super::elsewhere::find(root.path(), key, terminals);

    let found = find(&[(7, 4_000_000)], &key).expect("open outside the terminals");
    assert!(!found.paused);
    // A terminal whose CLI is the relay's parent holds the chat itself.
    assert_eq!(find(&[(7, std::process::id())], &key), None);
    assert_eq!(
        find(&[(7, 4_000_000)], &format!("s-{}", "e".repeat(32))),
        None
    );

    // Ctrl+Z stops it.
    unsafe {
        libc::kill(relay.id() as libc::pid_t, libc::SIGSTOP);
    }
    let stopped = (0..50).find_map(|_| {
        std::thread::sleep(Duration::from_millis(100));
        find(&[(7, 4_000_000)], &key).filter(|found| found.paused)
    });
    assert!(stopped.is_some(), "a stopped relay must show as paused");

    write(Some(1));
    assert_eq!(find(&[(7, 4_000_000)], &key), None);
    relay.kill().unwrap();
    relay.wait().unwrap();
    write(None);
    assert_eq!(
        find(&[(7, 4_000_000)], &key),
        None,
        "a dead relay is no open chat"
    );
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

/// Confirmed workspaces stay attached to a terminal across relay exit and CLI exit (latest last)
/// until the terminal itself is closed.
#[cfg(unix)]
#[test]
fn terminal_remembers_confirmed_workspaces_until_closed() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::create(root.path());
    let first = format!("s-{}", "1".repeat(32));
    let second = format!("s-{}", "2".repeat(32));
    db.execute(
        "INSERT INTO workspaces (id, owner, owner_host) VALUES ('s-first', ?1, 'codex'), ('s-second', ?2, 'codex')",
        [&first, &second],
    )
    .unwrap();
    let session = Session::start(shell("sleep 30"), 80, 24).unwrap();
    let cli_pid = session.pid().unwrap();
    let terminals = Terminals::default();
    terminals.lock().open.push(Running {
        id: 1,
        provider: "codex".to_owned(),
        cwd: std::env::temp_dir(),
        session: session.clone(),
        workspaces: Vec::new(),
        native: None,
        started: std::time::SystemTime::now(),
    });
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = connections.join(format!("{cli_pid}-x.json"));

    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&first]}).to_string(),
    )
    .unwrap();
    let (_, open) = terminals.snapshot(root.path());
    assert_eq!(open[0].2["workspaces"], serde_json::json!(["s-first"]));

    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&second, &first]}).to_string(),
    )
    .unwrap();
    let (_, open) = terminals.snapshot(root.path());
    assert_eq!(
        open[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );

    // Relay ends first, then its connection file disappears and the CLI exits: remembered stays.
    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&first], "ended_ms": 1}).to_string(),
    )
    .unwrap();
    assert_eq!(
        terminals.snapshot(root.path()).1[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );

    std::fs::remove_file(&receipt).unwrap();
    session.stop();
    assert!(exit_of(&session).is_some());
    assert_eq!(
        terminals.snapshot(root.path()).1[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );

    terminals.close(Some(1));
    assert!(terminals.snapshot(root.path()).1.is_empty());
}

#[test]
fn codex_terminal_passes_no_daemon_as_separate_argv_and_other_clis_do_not() {
    assert_eq!(cli_args("codex", None, None), ["--no-daemon"]);
    assert!(cli_args("claude", None, None).is_empty());
    assert!(cli_args("grok", None, None).is_empty());
    assert_eq!(
        cli_args("codex", Some("id"), None),
        ["resume", "--no-daemon", "id"]
    );
    assert_eq!(cli_args("claude", Some("id"), None), ["--resume", "id"]);

    let codex = command(
        Path::new("/usr/local/bin/codex"),
        &cli_args("codex", None, None),
        Path::new("/tmp"),
    );
    let argv: Vec<String> = codex
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv.last().map(String::as_str), Some("--no-daemon"));
    assert_eq!(
        argv.get(argv.len().saturating_sub(2)).map(String::as_str),
        Some("/usr/local/bin/codex")
    );
    // If wrapped in a login shell, `-c <script>` must not embed `--no-daemon` into the script string.
    if let Some(at) = argv.iter().position(|arg| arg == "-c") {
        assert!(!argv[at + 1].contains("--no-daemon"), "{argv:?}");
    }

    let claude = command(
        Path::new("/usr/local/bin/claude"),
        &cli_args("claude", None, None),
        Path::new("/tmp"),
    );
    let claude_argv: Vec<String> = claude
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        claude_argv.last().map(String::as_str),
        Some("/usr/local/bin/claude")
    );
    assert!(!claude_argv.iter().any(|arg| arg == "--no-daemon"));
}

#[test]
fn zcode_desktop_bundle_is_told_where_its_provider_config_lives() {
    let bundle = std::path::Path::new("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs");
    let env = super::session::cli_env("zcode", bundle);
    assert!(env.iter().any(|(name, value)| *name == "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE"
        && value == "/Applications/ZCode.app/Contents/Resources/config/provider/zcode-builtin.json")
        || std::env::var_os("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE").is_some());
    // An installed `zcode` command and other CLIs find their own config.
    assert!(
        super::session::cli_env("zcode", std::path::Path::new("/usr/local/bin/zcode")).is_empty()
    );
    assert!(super::session::cli_env("codex", bundle).is_empty());
}
