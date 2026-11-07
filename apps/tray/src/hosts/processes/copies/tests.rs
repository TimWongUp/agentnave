use super::*;

fn node(since: u64, exe: &str, args: &[&str]) -> Node {
    Node {
        parent: None,
        since,
        exe: exe.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
    }
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
        let got = classify(host, &[&foreground, &daemon]).unwrap();
        assert_eq!((got.kind, got.since), (kind, 1000));
    }
    let helper = node(
        1000,
        "/usr/bin/codex",
        &["codex", "app-server", "daemon", "pid-update-loop"],
    );
    assert!(classify("codex", &[&helper]).is_none());
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
        let got = classify(host, &[&child, &terminal]).unwrap();
        assert_eq!(
            (got.kind, got.since, got.bundle.as_deref()),
            (kind, 5000, bundle)
        );
    }
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

#[test]
fn trae_copies_never_borrow_the_other_rows_connection() {
    let cli = node(1000, "/bin/traecli", &["traecli"]);
    let desktop = node(
        1001,
        "/Applications/Trae CN.app/Contents/MacOS/Electron",
        &["Electron"],
    );
    assert_eq!(classify("trae", &[&cli]).unwrap().kind, "cli");
    let copy = classify("trae-desktop", &[&desktop]).unwrap();
    assert_eq!(copy.kind, "app");
    assert_eq!(copy.bundle.as_deref(), Some("/Applications/Trae CN.app"));
    assert!(classify("trae", &[&desktop]).is_none());
    assert!(classify("trae-desktop", &[&cli]).is_none());
}
