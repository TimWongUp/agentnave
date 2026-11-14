use super::*;

fn receipt_context() -> Context {
    Context {
        home: Default::default(),
        app: Default::default(),
        data: Default::default(),
        project: Default::default(),
        env: Default::default(),
        search: Vec::new(),
        applications: Vec::new(),
    }
}

#[test]
fn fixed_receipt_observations_default_to_unknown_and_allow_explicit_absence() {
    let context = receipt_context();
    let modified = SystemTime::now() + Duration::from_secs(60);
    let mut probe = Probe::fixed(Some(HashMap::new()));
    assert!(matches!(
        probe.receipt_process(&context, 42, Some(100_000), modified),
        State::Unknown
    ));
    probe = probe.with_receipt_processes(Some(HashMap::new()));
    assert!(matches!(
        probe.receipt_process(&context, 42, None, modified),
        State::Stopped
    ));
    probe = probe.with_receipt_processes(None);
    assert!(matches!(
        probe.receipt_process(&context, 42, Some(100_000), modified),
        State::Unknown
    ));
}

#[test]
fn receipt_process_start_times_allow_two_seconds_before_declaring_pid_reuse() {
    let context = receipt_context();
    let modified = SystemTime::now();
    let mut probe = Probe::fixed(None).with_receipt_processes(Some(HashMap::from([(42, 102)])));
    for started in [100_000, 100_999, 102_000, 110_000] {
        assert!(matches!(
            probe.receipt_process(&context, 42, Some(started), modified),
            State::Running(102)
        ));
    }
    probe = probe.with_receipt_processes(Some(HashMap::from([(42, 103)])));
    assert!(matches!(
        probe.receipt_process(&context, 42, Some(100_000), modified),
        State::Stopped
    ));
}

#[test]
fn receipt_process_with_unknown_start_time_is_alive_without_a_reuse_claim() {
    let context = receipt_context();
    let modified = SystemTime::now();
    let mut probe = Probe::fixed(None).with_receipt_processes(Some(HashMap::from([(42, 900)])));
    for started in [None, Some(0)] {
        assert!(matches!(
            probe.receipt_process(&context, 42, started, modified),
            State::Running(900)
        ));
    }
    probe = probe.with_receipt_processes(Some(HashMap::from([(42, 0)])));
    assert!(matches!(
        probe.receipt_process(&context, 42, Some(100_000), modified),
        State::Running(0)
    ));
}

#[test]
fn receipt_newer_than_cached_snapshot_cannot_be_declared_dead_or_trigger_a_scan() {
    let context = receipt_context();
    let snapshot = SystemTime::now();
    let modified = snapshot + Duration::from_secs(1);
    let checked = Instant::now();
    let mut probe = Probe {
        checked: Some(checked),
        observed_at: Some(snapshot),
        receipt_processes: Some(HashMap::from([(42, 103)])),
        ..Probe::default()
    };
    for _ in 0..2 {
        assert!(matches!(
            probe.receipt_process(&context, 43, None, modified),
            State::Unknown
        ));
        assert!(matches!(
            probe.receipt_process(&context, 42, Some(100_000), modified),
            State::Unknown
        ));
    }
    assert!(matches!(
        probe.receipt_process(&context, 42, None, modified),
        State::Running(103)
    ));
    assert_eq!(probe.checked, Some(checked));
    for (pid, started) in [(43, None), (42, Some(100_000))] {
        assert!(matches!(
            probe.receipt_process(&context, pid, started, snapshot),
            State::Stopped
        ));
    }
}

#[cfg(windows)]
#[test]
fn installed_windows_shim_contents_resolve_in_a_clean_fixture() {
    // Captured from the Windows validation machine; no installed CLI or user profile required.
    for (name, contents, relative) in [
        (
            "codebuddy.cmd",
            include_str!("fixtures/codebuddy.cmd"),
            "node_modules/@tencent-ai/codebuddy-code/bin/codebuddy",
        ),
        ("pi.cmd", include_str!("fixtures/pi.cmd"), "pi-launcher.js"),
    ] {
        let dir = tempfile::Builder::new()
            .prefix("real shim with spaces ")
            .tempdir()
            .unwrap();
        let shim = dir.path().join(name);
        let script = dir.path().join(relative);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&shim, contents).unwrap();
        std::fs::write(&script, "script").unwrap();
        std::fs::write(dir.path().join("node.exe"), "node").unwrap();
        assert_eq!(
            shim_script(&shim),
            Some(script.canonicalize().unwrap()),
            "{name}"
        );
    }
}

#[cfg(windows)]
#[test]
fn npm_extensionless_script_and_spaces_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("codebuddy.cmd");
    let script = dir
        .path()
        .join("node_modules/package with spaces/bin/codebuddy");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "#!/usr/bin/env node").unwrap();
    std::fs::write(dir.path().join("node.exe"), "").unwrap();
    std::fs::write(&shim, "IF EXIST \"%dp0%\\node.exe\" (\nSET \"_prog=%dp0%\\node.exe\"\n)\n\"%_prog%\" \"%dp0%\\node_modules\\package with spaces\\bin\\codebuddy\" %*\n").unwrap();
    assert_eq!(shim_script(&shim), Some(script.canonicalize().unwrap()));
}

#[cfg(all(windows, feature = "windows-live-tests"))]
#[test]
fn windows_live_shims() {
    let context = Context::live(
        dirs::data_dir()
            .unwrap()
            .join("io.github.timwongup.agentnave"),
    );
    for (id, shim) in [
        (
            "codebuddy",
            dirs::data_local_dir()
                .unwrap()
                .join("Programs/nodejs/codebuddy.cmd"),
        ),
        ("pi", context.home.join(".pi/agent/bin/pi.cmd")),
    ] {
        let script = shim_script(&shim).expect("real installed shim must resolve");
        assert!(script.is_file());
        println!("{id}: shim={} script={}", shim.display(), script.display());
    }
    let running = observe(&context).expect("live Windows process snapshot");
    println!("observed host start times: {running:?}");
}

#[test]
fn a_canonical_path_and_a_plain_one_look_the_same() {
    assert_eq!(
        path_key(Path::new(r"D:\x\pi-launcher.js")),
        path_key(Path::new(r"\\?\D:\x\pi-launcher.js"))
    );
    // Verbatim UNC only exists on Windows; elsewhere the same two strings differ by design.
    #[cfg(windows)]
    assert_eq!(
        path_key(Path::new(r"\\?\UNC\srv\share\a.js")),
        path_key(Path::new(r"\\srv\share\a.js"))
    );
    assert_ne!(
        path_key(Path::new(r"D:\a.js")),
        path_key(Path::new(r"D:\b.js"))
    );
}
#[test]
fn our_own_server_and_supervisor_are_recognized() {
    for arg in [
        r"C:\Users\timwo\AppData\Roaming\io.github.timwongup.agentnave\service\0.58.0\tools\agentnave\Scripts\python.exe",
        r"C:\Users\timwo\.local\bin\agentnave-mcp.exe",
        "from agentnave.mcp_server import main; main()",
        r"C:\x\agentnave\mcp_server.py",
        r"C:\x\agentnave\windows_supervisor.py",
    ] {
        assert!(
            agentnave_arg(arg) || tool_environment(arg),
            "{arg} should be ours"
        );
    }
}

/// Only a whole path or the generated `-c` source counts. A prompt that merely mentions the
/// module must not turn the user's own host into "the app": that would hide a running host.
#[test]
fn text_that_mentions_the_module_is_not_this_app() {
    for arg in [
        r"D:\code\agentnave",
        r"C:\Users\timwo\Documents\agentnave notes.md",
        r"C:\tools\other\python.exe",
        "check agentnave.mcp_server in the tray",
        r"see C:\x\tools\agentnave\Scripts\python.exe for details",
    ] {
        assert!(!agentnave_arg(arg), "{arg}");
    }
}

/// Only the executable path decides the tool environment, so a prompt cannot claim it.
#[test]
fn the_tool_environment_is_matched_by_executable_path() {
    assert!(tool_environment(
        r"C:\Users\timwo\AppData\Roaming\io.github.timwongup.agentnave\service\0.58.0\tools\agentnave\Scripts\python.exe"
    ));
    assert!(!tool_environment(r"D:\code\agentnave"));
    assert!(!tool_environment(r"C:\tools\other\python.exe"));
}

#[test]
fn host_paths_match_exact_executables_and_main_app_processes() {
    for (path, id) in [
        (
            "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
            "deepseek",
        ),
        (
            "C:\\Program Files\\DeepSeek Harness\\DeepSeek Harness.exe",
            "deepseek",
        ),
        ("/Applications/Codex.app/Contents/MacOS/Codex", "codex"),
        ("/Applications/Devin.app/Contents/MacOS/Devin", "devin"),
        ("/home/a/.local/share/claude/versions/2.0.0", "claude"),
        ("/usr/bin/agy", "antigravity"),
        ("/usr/bin/devin", "devin"),
        ("/usr/bin/dsh.js", "deepseek"),
    ] {
        assert_eq!(identity(path), Some(id), "{path}");
    }
    for path in [
        "/tmp/codex-notes.txt",
        "/Applications/DeepSeek Harness.app/Contents/Frameworks/DeepSeek Harness Helper.app/Contents/MacOS/DeepSeek Harness Helper",
        "deepseek.example",
    ] {
        assert_eq!(identity(path), None, "{path}");
    }
}
