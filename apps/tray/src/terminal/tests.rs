use std::path::Path;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use portable_pty::CommandBuilder;

#[cfg(unix)]
use super::session::Session;
use super::session::{KEPT, Output, cli_args, command, parent_session};
#[cfg(unix)]
use super::{Running, Terminals};

#[test]
fn desktop_chat_is_refused_for_resume_and_handed_over_with_selected_cli() {
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("project");
    std::fs::create_dir(&cwd).unwrap();
    let id = "01234567-89ab-4cde-8fab-0123456789ab";
    let record = root.path().join(format!("{id}.jsonl"));
    std::fs::write(
        &record,
        format!(
            "{{\"entrypoint\":\"claude-desktop-3p\",\"cwd\":{}}}",
            json!(cwd)
        ),
    )
    .unwrap();
    let workspace = crate::data::boss_app::boss_key("claude", id);
    let db = crate::db::create(root.path());
    db.execute("INSERT INTO workspaces (id, owner, owner_host, owner_file, title, note, released, pending) VALUES (?1, ?1, 'claude', ?2, 'test', 'keep', 'old', 1)", rusqlite::params![workspace, record.to_str().unwrap()]).unwrap();
    let error = super::chat_session_in(root.path(), &workspace, "claude", None)
        .err()
        .unwrap();
    assert!(error.contains("不能在这里续接"));
    let account_dir = root.path().join("account");
    std::fs::create_dir(&account_dir).unwrap();
    std::fs::write(root.path().join("accounts.json"), json!({"version":1,"accounts":[{"id":"work","provider":"claude","label":"工作","dir":account_dir,"managed":false}]}).to_string()).unwrap();
    for provider in ["claude", "codex"] {
        let payload = json!({"workspace": workspace, "provider": provider, "account": if provider == "claude" { Some("work") } else { None }, "cols": 91, "rows": 27});
        let reply = super::Terminals::default().handover_in(
            root.path(),
            &payload,
            |_| true,
            |start, args, env| {
                assert_eq!(start.provider, provider);
                assert_eq!(start.cwd, cwd);
                assert_eq!(start.size, (91, 27));
                assert!(start.resume.is_none());
                assert_eq!(
                    start.account.as_ref().map(|a| a.id.as_str()),
                    (provider == "claude").then_some("work")
                );
                let prompt = format!("接手工作区 {workspace}");
                if provider == "claude" || !cfg!(unix) {
                    assert_eq!(args, [prompt]);
                } else {
                    assert_eq!(env, [("AGENTNAVE_HANDOVER_PROMPT", prompt.as_str())]);
                }
                crate::response::Response::json(&json!({"ok":true}))
            },
        );
        assert_eq!(reply.status, 200);
        let (pending, note): (bool, String) = db
            .query_row(
                "SELECT pending, note FROM workspaces WHERE id = ?1",
                [&workspace],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(pending);
        assert_eq!(note, "keep");
    }

    let refused = |payload: serde_json::Value| {
        let reply = super::Terminals::default().handover_in(
            root.path(),
            &payload,
            |_| true,
            |_, _, _| panic!("nothing starts"),
        );
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        (
            reply.status,
            body["error"].as_str().unwrap_or_default().to_owned(),
        )
    };
    // An account that is not a name is refused, not taken for the default account.
    let odd = json!({"workspace": workspace, "provider": "claude", "account": 7});
    assert_eq!(refused(odd).0, 400);
    // Only an account that is gone is told as removed; a broken accounts file says so itself.
    let _ = db.execute("ALTER TABLE workspaces ADD COLUMN owner_account TEXT", []);
    db.execute(
        "UPDATE workspaces SET owner_account = 'gone' WHERE id = ?1",
        [&workspace],
    )
    .unwrap();
    let plain = json!({"workspace": workspace, "provider": "codex"});
    assert_eq!(
        refused(plain.clone()),
        (409, "这段聊天的账号已被移除".to_owned())
    );
    std::fs::write(root.path().join("accounts.json"), "{").unwrap();
    let (status, error) = refused(plain);
    assert_eq!(status, 409);
    assert!(error.contains("accounts.json"), "{error}");
}

/// Reads until `wanted` shows up in the output or the CLI exits, within a few seconds.
#[cfg(unix)]
fn output_until(session: &Session, wanted: &str) -> (String, Option<u32>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut text, mut from) = (String::new(), 0);
    while Instant::now() < deadline {
        let chunk = session.read_view(from, Duration::from_millis(200), None);
        text.push_str(&String::from_utf8_lossy(&chunk.bytes));
        from = chunk.end;
        if text.contains(wanted) || chunk.exit.is_some() {
            return (text, chunk.exit);
        }
    }
    (text, session.exit())
}

/// Waits for the CLI to exit, reading whatever it still writes.
#[cfg(unix)]
fn exit_of(session: &Session) -> Option<u32> {
    output_until(session, "\u{0}").1
}

#[cfg(unix)]
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
    let replay = session.read_view(0, Duration::ZERO, None);
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
        title_located: false,
        title_searched: None,
        started: std::time::SystemTime::now(),
        pending: 0,
        account: None,
        #[cfg(unix)]
        codex: None,
        derived: None,
    });
    let header = |reply: &crate::response::Response, name: &str| {
        reply
            .headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    };
    // output_until read it all, so the page asking from the start is a replay that ends there.
    let replay = terminals.output(Some(1), Some(0), Some("v1"));
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
    let live = terminals.output(Some(1), Some(end.parse().unwrap()), Some("v1"));
    assert_eq!(header(&live, "X-Terminal-Replay"), None);
}

#[test]
fn a_new_claude_chat_is_started_with_its_id_as_separate_arguments() {
    let id = super::session::fresh_session_id();
    assert_eq!(
        cli_args("claude", None, Some(&id)),
        ["--session-id", id.as_str()]
    );
    assert_eq!(
        cli_args("grok", None, Some(&id)),
        ["--session-id", id.as_str()]
    );
    // Continuing names the chat already; other CLIs take no fresh ID at all.
    assert_eq!(
        cli_args("claude", Some("old"), Some(&id)),
        ["--resume", "old"]
    );
    assert_eq!(
        cli_args("grok", Some("old"), Some(&id)),
        ["--resume", "old"]
    );
    assert_eq!(
        cli_args("antigravity", Some("old"), Some(&id)),
        ["--conversation", "old"]
    );
    assert!(cli_args("antigravity", None, Some(&id)).is_empty());
    assert_eq!(cli_args("codex", None, Some(&id)), ["--no-daemon"]);
    assert!(cli_args("pi", None, Some(&id)).is_empty());
    for provider in ["claude", "codex", "grok", "antigravity"] {
        assert!(super::session::resumes(provider), "{provider}");
    }
    assert!(!super::session::resumes("pi"));
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
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
        "CLAUDE_CODE_ACCOUNT_UUID",
        "CODEX_THREAD_ID",
        "GROK_SESSION_ID",
        "GROK_SANDBOX",
        "ANTIGRAVITY_CONVERSATION_ID",
        "ANTIGRAVITY_LS_ADDRESS",
        "ANTIGRAVITY_CSRF_TOKEN",
        "AGENTNAVE_HOST_SESSION_ID",
        "HERDR_PANE_ID",
        "HERDR_SESSION",
        "HERDR_SOCKET",
        "Herdr_Pane_Id",
        "T3CODE_THREAD_ID",
        "T3CODE_HOME",
        "T3code_Thread_Id",
        "T3_ACP_MCP_NODE",
        "T3_ACP_MCP_ENTRYPOINT",
        "ELECTRON_RUN_AS_NODE",
        "PWD",
        "OLDPWD",
        "TERM_PROGRAM",
    ] {
        assert!(parent_session(name), "{name}");
    }
    for name in [
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "CODEBUDDY_CONFIG_DIR",
        "HERDR",
        "MY_HERDR_FLAG",
        "T3CODE",
        "MY_T3CODE_FLAG",
        "HOME",
        "PATH",
    ] {
        assert!(!parent_session(name), "{name}");
    }

    let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
    cmd.env("HERDR_PANE_ID", "pane-1");
    cmd.env("HERDR_SESSION", "sess-1");
    cmd.env("T3CODE_THREAD_ID", "thread-1");
    cmd.env("T3CODE_HOME", "/tmp/t3");
    cmd.env("KEEP_ME", "ok");
    super::session::strip_parent_session(
        &mut cmd,
        [
            "HERDR_PANE_ID",
            "HERDR_SESSION",
            "T3CODE_THREAD_ID",
            "T3CODE_HOME",
            "KEEP_ME",
        ]
        .into_iter()
        .map(std::ffi::OsString::from),
    );
    assert_eq!(cmd.get_env("HERDR_PANE_ID"), None);
    assert_eq!(cmd.get_env("HERDR_SESSION"), None);
    assert_eq!(cmd.get_env("T3CODE_THREAD_ID"), None);
    assert_eq!(cmd.get_env("T3CODE_HOME"), None);
    assert_eq!(cmd.get_env("KEEP_ME").and_then(|v| v.to_str()), Some("ok"));
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

/// A relay at least two hops below a terminal's CLI is bound to it by walking the ancestry a
/// generation at a time; a relay under an unrelated process is not, and no terminals bind nothing.
#[cfg(unix)]
#[test]
fn bosses_binds_a_relay_two_hops_below_its_terminal() {
    let root = tempfile::tempdir().unwrap();
    let key = format!("s-{}", "d".repeat(32));
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let pid_file = root.path().join("relay.pid");
    // The outer shell stays resident, so `sleep` is a grandchild of this process standing in for
    // a terminal's CLI instead of being exec-optimised into it.
    let mut shell = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            &format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let relay = loop {
        let appeared = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pid| {
                super::links::snapshot(&[*pid])
                    .process(sysinfo::Pid::from_u32(*pid))
                    .is_some()
            });
        if let Some(pid) = appeared {
            break pid;
        }
        assert!(Instant::now() < deadline, "relay process never appeared");
        std::thread::sleep(Duration::from_millis(20));
    };
    let parent = super::links::snapshot(&[relay])
        .process(sysinfo::Pid::from_u32(relay))
        .and_then(|process| process.parent())
        .expect("relay parent");
    let grandparent = super::links::snapshot(&[parent.as_u32()])
        .process(parent)
        .and_then(|process| process.parent())
        .expect("shell parent");
    assert_ne!(parent.as_u32(), std::process::id());
    assert_eq!(grandparent.as_u32(), std::process::id());

    std::fs::write(
        connections.join(format!("{relay}-x.json")),
        serde_json::json!({"host": "codex", "bosses": [&key]}).to_string(),
    )
    .unwrap();

    let mut bystander = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    assert_eq!(
        super::links::chats(root.path(), &[(7, std::process::id())])
            .get(&7)
            .and_then(|keys| keys.last()),
        Some(&key)
    );
    assert!(super::links::chats(root.path(), &[(8, bystander.id())]).is_empty());
    assert!(super::links::chats(root.path(), &[]).is_empty());

    bystander.kill().unwrap();
    bystander.wait().unwrap();
    unsafe { libc::kill(relay as i32, libc::SIGKILL) };
    let _ = shell.kill();
    let _ = shell.wait();
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
        title_located: false,
        title_searched: None,
        started: std::time::SystemTime::now(),
        pending: 0,
        account: None,
        #[cfg(unix)]
        codex: None,
        derived: None,
    });
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = connections.join(format!("{cli_pid}-x.json"));

    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&first]}).to_string(),
    )
    .unwrap();
    let (_, open) = terminals.snapshot_with(root.path(), Duration::ZERO);
    assert_eq!(open[0].2["workspaces"], serde_json::json!(["s-first"]));

    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&second, &first]}).to_string(),
    )
    .unwrap();
    let (_, open) = terminals.snapshot_with(root.path(), Duration::ZERO);
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
        terminals.snapshot_with(root.path(), Duration::ZERO).1[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );

    std::fs::remove_file(&receipt).unwrap();
    session.stop();
    assert!(exit_of(&session).is_some());
    assert_eq!(
        terminals.snapshot_with(root.path(), Duration::ZERO).1[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );

    terminals.close(Some(1));
    assert!(
        terminals
            .snapshot_with(root.path(), Duration::ZERO)
            .1
            .is_empty()
    );
}

#[test]
fn codex_terminal_passes_no_daemon_for_cli_args_and_other_clis_do_not() {
    assert_eq!(cli_args("codex", None, None), ["--no-daemon"]);
    assert!(cli_args("claude", None, None).is_empty());
    assert!(cli_args("grok", None, None).is_empty());
    assert!(cli_args("antigravity", None, None).is_empty());
    assert_eq!(
        cli_args("codex", Some("id"), None),
        ["resume", "--no-daemon", "id"]
    );
    assert_eq!(cli_args("claude", Some("id"), None), ["--resume", "id"]);
    assert_eq!(cli_args("grok", Some("id"), None), ["--resume", "id"]);
    assert_eq!(
        cli_args("antigravity", Some("id"), None),
        ["--conversation", "id"]
    );

    #[cfg(unix)]
    {
        let sock = super::codex_server::socket_path(42);
        let (program, args) = super::codex_server::launch_args(
            Path::new("/usr/local/bin/codex"),
            &sock,
            Path::new("/tmp"),
            None,
            &[],
        );
        let codex = command(&program, &args, Path::new("/tmp"));
        let argv: Vec<String> = codex
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            argv.iter()
                .any(|arg| arg == sock.to_string_lossy().as_ref())
        );
        assert!(
            argv.iter()
                .any(|arg| arg.contains("app-server") && arg.contains("--remote"))
        );
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
fn only_clis_with_a_terminal_ui_are_offered() {
    use std::path::Path;
    assert!(!super::has_terminal_ui(
        "deepseek",
        Path::new("/usr/local/bin/dsh")
    ));
    let bundle = Path::new("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs");
    assert!(!super::has_terminal_ui("zcode", bundle));
    assert!(super::has_terminal_ui(
        "zcode",
        Path::new("/usr/local/bin/zcode")
    ));
    assert!(super::has_terminal_ui("codex", bundle));
}

#[test]
fn only_the_current_view_advances_the_replay_watermark() {
    let mut output = Output::default();
    output.push(b"screen ");
    output.register_view(Some("v1"));
    let initial = output.take_for(0, Some("v1"));
    assert!(!initial.replay);
    assert_eq!(initial.end, 7);

    // Page reloads: v2 becomes current and replays up to 7.
    output.register_view(Some("v2"));
    let replay = output.take_for(0, Some("v2"));
    assert!(replay.replay);
    assert_eq!(replay.end, 7);

    // New output arrives while v1's stale poll wakes up first: v1 gets bytes without moving watermark.
    output.push(b"\x1b[?996n");
    let stale = output.take_for(7, Some("v1"));
    assert!(!stale.replay);
    assert_eq!(text(&stale), "\x1b[?996n");

    // v2 reads from 7 and must still see it as new (not replay), advancing the watermark.
    let current = output.take_for(7, Some("v2"));
    assert!(!current.replay);
    assert_eq!(text(&current), "\x1b[?996n");

    // Even if v1 polls again later, it cannot reclaim currency or advance the watermark.
    output.push(b"tail");
    let stale_again = output.take_for(14, Some("v1"));
    assert_eq!(text(&stale_again), "tail");
    let current_tail = output.take_for(14, Some("v2"));
    assert!(!current_tail.replay);
    assert_eq!(text(&current_tail), "tail");
}

#[cfg(unix)]
#[test]
fn reused_pid_with_older_started_ms_is_treated_as_exited() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::create(root.path());
    let key = format!("s-{}", "f".repeat(32));
    db.execute(
        "INSERT INTO workspaces (id, owner, owner_host) VALUES ('s-reused', ?1, 'claude')",
        [&key],
    )
    .unwrap();
    let mut relay = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = connections.join(format!("{}-x.json", relay.id()));

    // A stale receipt from an earlier process whose PID was later reused by `sleep`.
    std::fs::write(
        &receipt,
        serde_json::json!({"host": "claude", "bosses": [&key], "started_ms": 1_000}).to_string(),
    )
    .unwrap();
    assert_eq!(
        super::elsewhere::find(root.path(), &key, &[(7, 4_000_000)]),
        None
    );
    assert!(
        super::links::workspaces(root.path(), &[(7, std::process::id())]).is_empty(),
        "stale receipt with reused PID must not attach to a terminal"
    );

    // Matching current started_ms marks the same process as genuinely alive.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    std::fs::write(
        &receipt,
        serde_json::json!({"host": "claude", "bosses": [&key], "started_ms": now_ms}).to_string(),
    )
    .unwrap();
    assert!(super::elsewhere::find(root.path(), &key, &[(7, 4_000_000)]).is_some());
    assert_eq!(
        super::links::workspaces(root.path(), &[(7, std::process::id())]).get(&7),
        Some(&vec!["s-reused".to_owned()])
    );
    relay.kill().unwrap();
    relay.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn unlocated_terminal_title_lookups_are_throttled_per_terminal() {
    let session = Session::start(shell("sleep 30"), 80, 24).unwrap();
    let terminals = Terminals::default();
    {
        let mut state = terminals.lock();
        state.open.push(Running {
            id: 1,
            provider: "codex".to_owned(),
            cwd: std::env::temp_dir(),
            session: session.clone(),
            workspaces: Vec::new(),
            native: None,
            title_located: false,
            title_searched: None,
            started: std::time::SystemTime::now(),
            pending: 0,
            account: None,
            #[cfg(unix)]
            codex: None,
            derived: None,
        });
        state.open.push(Running {
            id: 2,
            provider: "claude".to_owned(),
            cwd: std::env::temp_dir(),
            session: session.clone(),
            workspaces: Vec::new(),
            native: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            title_located: false,
            title_searched: None,
            started: std::time::SystemTime::now(),
            pending: 0,
            account: None,
            #[cfg(unix)]
            codex: None,
            derived: None,
        });
    }

    let mut calls = 0usize;
    let first = terminals.titles_with(Duration::from_secs(60), |_, native, _, _, _, _| {
        calls += 1;
        (native.map(str::to_owned), false, None)
    });
    assert!(first.is_empty());
    assert_eq!(calls, 2);

    // Within the throttle window, neither unlocated terminal scans again.
    terminals.titles_with(Duration::from_secs(60), |_, native, _, _, _, _| {
        calls += 1;
        (native.map(str::to_owned), false, None)
    });
    assert_eq!(calls, 2);

    // Once the window expires and records are located, both resolve and stay unthrottled.
    let resolved = terminals.titles_with(Duration::ZERO, |provider, _, _, _, _, _| {
        calls += 1;
        (
            Some(format!("{provider}-id")),
            true,
            Some(crate::data::boss_app::CliTitle {
                text: format!("{provider} title"),
                custom: false,
            }),
        )
    });
    assert_eq!(calls, 4);
    assert_eq!(resolved[&1].text, "codex title");
    assert_eq!(resolved[&2].text, "claude title");

    let cached = terminals.titles_with(Duration::from_secs(60), |provider, _, _, _, _, _| {
        calls += 1;
        (
            None,
            true,
            Some(crate::data::boss_app::CliTitle {
                text: format!("{provider} title"),
                custom: false,
            }),
        )
    });
    assert_eq!(calls, 6);
    assert_eq!(cached[&1].text, "codex title");
    session.stop();
    let _ = exit_of(&session);
}

#[cfg(unix)]
#[test]
fn claimed_native_session_ids_are_excluded_for_other_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let session = Session::start(shell("sleep 30"), 80, 24).unwrap();
    let now = std::time::SystemTime::now();
    let terminals = Terminals::default();
    {
        let mut state = terminals.lock();
        state.open.push(Running {
            id: 1,
            provider: "antigravity".to_owned(),
            cwd: work.clone(),
            session: session.clone(),
            workspaces: Vec::new(),
            native: Some("s1".to_owned()),
            title_located: true,
            title_searched: None,
            started: now - Duration::from_secs(20),
            pending: 0,
            account: None,
            #[cfg(unix)]
            codex: None,
            derived: None,
        });
        state.open.push(Running {
            id: 2,
            provider: "antigravity".to_owned(),
            cwd: work.clone(),
            session: session.clone(),
            workspaces: Vec::new(),
            native: None,
            title_located: false,
            title_searched: None,
            started: now - Duration::from_secs(15),
            pending: 0,
            account: None,
            #[cfg(unix)]
            codex: None,
            derived: None,
        });
        state.open.push(Running {
            id: 3,
            provider: "antigravity".to_owned(),
            cwd: work.clone(),
            session: session.clone(),
            workspaces: Vec::new(),
            native: None,
            title_located: false,
            title_searched: None,
            started: now,
            pending: 0,
            account: None,
            #[cfg(unix)]
            codex: None,
            derived: None,
        });
    }

    // Terminal 1's `s1` is passed in `exclude`; Terminal 2 claims `s2` and adds it to `exclude`
    // immediately so Terminal 3 sees both `s1` and `s2` excluded in the same pass.
    let mut seen: Vec<(Option<String>, Vec<String>)> = Vec::new();
    let titles = terminals.titles_with(Duration::ZERO, |_, native, _, _, exclude, _| {
        let mut ex: Vec<String> = exclude.iter().cloned().collect();
        ex.sort();
        seen.push((native.map(str::to_owned), ex));
        if native.is_none() && !exclude.contains("s2") {
            (
                Some("s2".to_owned()),
                true,
                Some(crate::data::boss_app::CliTitle {
                    text: "T2".to_owned(),
                    custom: false,
                }),
            )
        } else {
            (None, false, None)
        }
    });
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].0.as_deref(), Some("s1"));
    assert_eq!(seen[1], (None, vec!["s1".to_owned()]));
    assert_eq!(seen[2], (None, vec!["s1".to_owned(), "s2".to_owned()]));
    assert_eq!(titles.get(&2).map(|t| t.text.as_str()), Some("T2"));

    session.stop();
    let _ = exit_of(&session);
}

#[cfg(unix)]
#[test]
fn terminal_state_snapshot_throttles_connection_scans_while_elsewhere_stays_fresh() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::create(root.path());
    let first = format!("s-{}", "1".repeat(32));
    let second = format!("s-{}", "2".repeat(32));
    let other = format!("s-{}", "3".repeat(32));
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
        title_located: false,
        title_searched: None,
        started: std::time::SystemTime::now(),
        pending: 0,
        account: None,
        #[cfg(unix)]
        codex: None,
        derived: None,
    });
    let connections = root.path().join("connections");
    std::fs::create_dir_all(&connections).unwrap();
    let receipt = connections.join(format!("{cli_pid}-x.json"));
    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&first]}).to_string(),
    )
    .unwrap();

    // First snapshot scans and records `s-first`.
    let (_, open) = terminals.snapshot(root.path());
    assert_eq!(open[0].2["workspaces"], serde_json::json!(["s-first"]));

    // An immediate update within LINKS_TTL is not re-scanned by snapshot().
    std::fs::write(
        &receipt,
        serde_json::json!({"host": "codex", "bosses": [&second, &first]}).to_string(),
    )
    .unwrap();
    let (_, open) = terminals.snapshot(root.path());
    assert_eq!(open[0].2["workspaces"], serde_json::json!(["s-first"]));

    // Elsewhere conflict detection still reads the latest connection files immediately.
    let mut external = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    std::fs::write(
        connections.join(format!("{}-y.json", external.id())),
        serde_json::json!({"host": "claude", "bosses": [&other]}).to_string(),
    )
    .unwrap();
    assert!(super::elsewhere::find(root.path(), &other, &[(1, cli_pid)]).is_some());
    external.kill().unwrap();
    external.wait().unwrap();

    // Once the throttle window expires, snapshot picks up `s-second`.
    let (_, open) = terminals.snapshot_with(root.path(), Duration::ZERO);
    assert_eq!(
        open[0].2["workspaces"],
        serde_json::json!(["s-second", "s-first"])
    );
    session.stop();
    let _ = exit_of(&session);
}

#[test]
fn thread_desk_key_matches_service_hash() {
    assert_eq!(
        super::thread_desk_key("antigravity", "680b3822-c887-4303-b482-4d3ed38279b1"),
        "thread:1fd2c8006815717d"
    );
}

#[cfg(unix)]
#[test]
fn status_probe_is_answered_once_even_when_output_is_replayed() {
    let script = format!(
        "stty -echo -icanon min 1 time 0; printf '\\033]7501;?\\033\\\\'; dd bs=1 count={} 2>/dev/null | wc -c; printf ready; dd bs=1 count=1 2>/dev/null | od -An -t u1; printf done",
        super::status::REPLY.len()
    );
    let session = Session::start(shell(&script), 80, 24).unwrap();
    let (first, _) = output_until(&session, "ready");
    assert!(
        first.contains(&super::status::REPLY.len().to_string()),
        "{first:?}"
    );
    let replay = session.read_view(0, Duration::ZERO, None);
    assert!(replay.replay);
    session.write(b"z").unwrap();
    let (text, _) = output_until(&session, "done");
    assert!(text.contains("122"), "{text:?}");
}

#[cfg(unix)]
mod live;

mod accounts;
