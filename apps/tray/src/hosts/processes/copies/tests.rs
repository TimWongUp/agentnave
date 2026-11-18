use super::*;

fn node(since: u64, exe: &str, args: &[&str]) -> Node {
    Node {
        parent: None,
        since,
        exe: exe.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
        cwd: None,
    }
}

fn node_in(since: u64, exe: &str, args: &[&str], cwd: &str) -> Node {
    Node {
        cwd: Some(cwd.into()),
        ..node(since, exe, args)
    }
}

fn classify_nodes(host: &str, nodes: &[&Node]) -> Option<Copy> {
    let chain: Vec<(u32, &Node)> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| ((i + 1) as u32, *n))
        .collect();
    classify(host, &chain, &HashSet::new())
}

#[test]
fn native_background_copies_outweigh_their_foreground_children() {
    for (host, args, kind) in [
        ("claude", vec!["claude", "daemon", "run"], "daemon"),
        (
            "claude",
            vec!["claude", "bg-spare", "--bg-spare", "/tmp/socket"],
            "daemon",
        ),
        (
            "codex",
            vec!["codex", "app-server", "--managed-daemon"],
            "daemon",
        ),
        ("hermes", vec!["hermes", "gateway", "run"], "gateway"),
    ] {
        let foreground = node(3000, &format!("/usr/bin/{host}"), &[host]);
        let daemon = node(1000, &format!("/usr/bin/{host}"), &args);
        let got = classify_nodes(host, &[&foreground, &daemon]).unwrap();
        assert_eq!((got.kind, got.since), (kind, 1000));
    }
    let helper = node(
        1000,
        "/usr/bin/codex",
        &["codex", "app-server", "daemon", "pid-update-loop"],
    );
    assert!(classify_nodes("codex", &[&helper]).is_none());
    assert!(daemon_helper(helper.args.iter()));
}

#[test]
fn desktop_extension_and_terminal_copies_have_distinct_guidance() {
    for (host, path, kind, bundle) in [
        (
            "codex",
            "/Applications/ChatGPT.app/Contents/Resources/CodexCLI.app/Contents/MacOS/codex",
            "app",
            Some("/Applications/ChatGPT.app"),
        ),
        (
            "codex",
            "/Users/İ/Applications/Codex.app/Contents/MacOS/Codex",
            "app",
            Some("/Users/İ/Applications/Codex.app"),
        ),
        (
            "codex",
            "/home/.vscode/extensions/openai/bin/codex",
            "ide",
            None,
        ),
        (
            "claude",
            "/Applications/Claude.app/Contents/MacOS/Claude",
            "desktop",
            Some("/Applications/Claude.app"),
        ),
        (
            "hermes",
            "/Applications/Hermes.app/Contents/MacOS/Hermes",
            "app",
            Some("/Applications/Hermes.app"),
        ),
        (
            "workbuddy",
            "/Applications/WorkBuddy.app/Contents/MacOS/Electron",
            "app",
            Some("/Applications/WorkBuddy.app"),
        ),
        ("codex", "/usr/bin/codex", "cli", None),
    ] {
        let child = node(5000, path, &[path]);
        let terminal = node(
            1000,
            "/Applications/Terminal.app/Contents/MacOS/Terminal",
            &["Terminal"],
        );
        let got = classify_nodes(host, &[&child, &terminal]).unwrap();
        assert_eq!(
            (got.kind, got.since, got.bundle.as_deref()),
            (kind, 5000, bundle)
        );
    }
}

#[test]
fn multiple_cli_sessions_are_listed_individually_and_deduplicated_per_ancestor_chain() {
    // Session 1: herdr -> zsh -> outer codex wrapper (pid 10) -> inner native codex (pid 11)
    let mut herdr = node(100, "/opt/homebrew/bin/herdr", &["herdr", "server"]);
    herdr.parent = Some(1);
    let mut zsh1 = node(200, "/bin/zsh", &["-zsh"]);
    zsh1.parent = Some(9);
    let mut outer_codex = node_in(
        2000,
        "/opt/homebrew/bin/node",
        &["node", "/opt/homebrew/bin/codex"],
        "~/Code/a",
    );
    outer_codex.parent = Some(8);
    let mut inner_codex = node_in(
        2100,
        "/opt/homebrew/lib/node_modules/@openai/codex/codex",
        &["codex"],
        "~/Code/a/sub",
    );
    inner_codex.parent = Some(10);

    // Session 2: iTerm2 -> zsh -> codex (pid 20)
    let mut iterm = node(
        100,
        "/Applications/iTerm.app/Contents/MacOS/iTerm2",
        &["iTerm2"],
    );
    iterm.parent = Some(1);
    let mut zsh2 = node(200, "/bin/zsh", &["-zsh"]);
    zsh2.parent = Some(19);
    let mut second_codex = node_in(3000, "/opt/homebrew/bin/codex", &["codex"], "~/Code/b");
    second_codex.parent = Some(18);

    let snapshot = Snapshot {
        nodes: HashMap::from([
            (9, herdr),
            (8, zsh1),
            (10, outer_codex),
            (11, inner_codex),
            (19, iterm),
            (18, zsh2),
            (20, second_codex),
        ]),
        hosts: HashMap::from([("codex", vec![10, 11, 20])]),
    };
    let copies = snapshot.old_copies("codex", 5000);
    assert_eq!(copies.len(), 2);
    assert_eq!(
        (
            copies[0].kind,
            copies[0].pid,
            copies[0].since,
            copies[0].cwd.as_deref(),
            copies[0].terminal,
        ),
        ("cli", Some(10), 2000, Some("~/Code/a"), Some("herdr"))
    );
    assert_eq!(
        (
            copies[1].kind,
            copies[1].pid,
            copies[1].since,
            copies[1].cwd.as_deref(),
            copies[1].terminal,
        ),
        ("cli", Some(20), 3000, Some("~/Code/b"), Some("iTerm2"))
    );
}

#[test]
fn terminal_programs_are_recognized_from_ancestors_without_guessing() {
    for (exe, args, expected) in [
        (
            "/opt/homebrew/bin/herdr",
            &["herdr", "server"][..],
            Some("herdr"),
        ),
        (
            "/opt/homebrew/bin/tmux",
            &["tmux: server"][..],
            Some("tmux"),
        ),
        ("/usr/bin/screen", &["SCREEN"][..], Some("screen")),
        ("/opt/homebrew/bin/zellij", &["zellij"][..], Some("zellij")),
        (
            "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            &["Terminal"][..],
            Some("Terminal"),
        ),
        (
            "/Applications/iTerm.app/Contents/MacOS/iTerm2",
            &["iTerm2"][..],
            Some("iTerm2"),
        ),
        (
            "/Applications/Ghostty.app/Contents/MacOS/ghostty",
            &["ghostty"][..],
            Some("Ghostty"),
        ),
        (
            "/Applications/WezTerm.app/Contents/MacOS/wezterm-gui",
            &["wezterm-gui"][..],
            Some("WezTerm"),
        ),
        (
            "/Applications/Alacritty.app/Contents/MacOS/alacritty",
            &["alacritty"][..],
            Some("Alacritty"),
        ),
        (
            "/Applications/kitty.app/Contents/MacOS/kitty",
            &["kitty"][..],
            Some("kitty"),
        ),
        (
            "/Applications/Warp.app/Contents/MacOS/stable",
            &["stable"][..],
            Some("Warp"),
        ),
        (
            "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
            &["Code Helper (Plugin)"][..],
            Some("VS Code"),
        ),
        (
            "/usr/local/bin/custom-launcher",
            &["custom-launcher"][..],
            None,
        ),
    ] {
        let cli = node(2000, "/usr/local/bin/claude", &["claude"]);
        let shell = node(1500, "/bin/zsh", &["-zsh"]);
        let term = node(1000, exe, args);
        let got = classify_nodes("claude", &[&cli, &shell, &term]).unwrap();
        assert_eq!(got.terminal, expected, "{exe}");
    }
}

#[test]
fn collector_preserves_multiple_cli_relays_while_deduplicating_same_session() {
    let mut term = node(
        100,
        "/Applications/Ghostty.app/Contents/MacOS/ghostty",
        &["ghostty"],
    );
    term.parent = Some(1);
    let mut cli1 = node_in(1000, "/usr/bin/claude", &["claude"], "~/p1");
    cli1.parent = Some(9);
    let mut relay1a = node(1500, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay1a.parent = Some(10);
    let mut relay1b = node(1600, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay1b.parent = Some(10);

    let mut cli2 = node_in(2000, "/usr/bin/claude", &["claude"], "~/p2");
    cli2.parent = Some(9);
    let mut relay2 = node(2500, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay2.parent = Some(20);

    let snapshot = Snapshot {
        nodes: HashMap::from([
            (9, term),
            (10, cli1),
            (11, relay1a),
            (12, relay1b),
            (20, cli2),
            (21, relay2),
        ]),
        hosts: HashMap::from([("claude", vec![10, 20])]),
    };
    let mut collector = Collector::default();
    for relay_pid in [11, 12, 21] {
        collector.push(snapshot.classify("claude", relay_pid, true).unwrap());
    }
    let copies = collector.finish();
    assert_eq!(
        copies
            .iter()
            .map(|c| (c.kind, c.pid, c.since, c.cwd.as_deref(), c.terminal))
            .collect::<Vec<_>>(),
        [
            ("cli", Some(10), 1000, Some("~/p1"), Some("Ghostty")),
            ("cli", Some(20), 2000, Some("~/p2"), Some("Ghostty")),
        ]
    );
}

#[test]
fn fallback_cli_copies_omit_pid_and_cwd_and_stay_distinct_in_collector() {
    let mut term = node(
        100,
        "/Applications/Terminal.app/Contents/MacOS/Terminal",
        &["Terminal"],
    );
    term.parent = Some(1);
    let mut unknown_a = node_in(1000, "/usr/local/bin/wrapper-a", &["wrapper-a"], "~/a");
    unknown_a.parent = Some(9);
    let mut relay_a = node(1500, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay_a.parent = Some(10);
    let mut unknown_b = node_in(2000, "/usr/local/bin/wrapper-b", &["wrapper-b"], "~/b");
    unknown_b.parent = Some(9);
    let mut relay_b = node(2500, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay_b.parent = Some(20);

    let snapshot = Snapshot {
        nodes: HashMap::from([
            (9, term),
            (10, unknown_a),
            (11, relay_a),
            (20, unknown_b),
            (21, relay_b),
        ]),
        hosts: HashMap::new(),
    };
    let copy_a = snapshot.classify("claude", 11, true).unwrap();
    let copy_b = snapshot.classify("claude", 21, true).unwrap();
    assert_eq!(copy_a.pid, None);
    assert_eq!(copy_a.cwd, None);
    assert_eq!(copy_a.terminal, Some("Terminal"));
    assert_eq!(copy_b.pid, None);
    assert_eq!(copy_b.cwd, None);

    let mut collector = Collector::default();
    collector.push(copy_a);
    collector.push(copy_b);
    let collected = collector.finish();
    assert_eq!(collected.len(), 2);
    assert_eq!(
        collected
            .iter()
            .map(|c| (c.kind, c.pid, c.since))
            .collect::<Vec<_>>(),
        [("cli", None, 1000), ("cli", None, 2000)]
    );
}

#[test]
fn snapshot_walks_parents_and_reports_each_kind_once_at_its_earliest_start() {
    let mut relay = node(6000, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    relay.parent = Some(2);
    let mut child = node(5000, "/usr/bin/codex", &["codex"]);
    child.parent = Some(3);
    let daemon = node(
        1000,
        "/usr/bin/codex",
        &["codex", "app-server", "--managed-daemon"],
    );
    let app = node(
        3000,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        &["Codex"],
    );
    let new = node(9000, "/usr/bin/codex", &["codex"]);
    let mut orphan = node(6100, "/service/1/bin/agentnave-mcp", &["agentnave-mcp"]);
    orphan.parent = Some(1);
    let snapshot = Snapshot {
        nodes: HashMap::from([
            (1, relay),
            (2, child),
            (3, daemon),
            (4, app),
            (5, new),
            (6, orphan),
        ]),
        hosts: HashMap::from([("codex", vec![2, 3, 4, 5])]),
    };
    assert_eq!(snapshot.classify("codex", 1, true).unwrap().since, 1000);
    let copies = snapshot.old_copies("codex", 7000);
    assert_eq!(
        copies.iter().map(|c| (c.kind, c.since)).collect::<Vec<_>>(),
        [("app", 3000), ("daemon", 1000)]
    );
    assert!(snapshot.classify("codex", 99, true).is_none());
    assert!(
        snapshot.classify("codex", 6, true).is_none(),
        "adopted by init"
    );
}
