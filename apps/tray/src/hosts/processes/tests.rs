use super::*;

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

#[cfg(windows)]
#[test]
#[ignore = "reads installed shims and live process observations on the validation machine"]
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
fn a_cmd_shim_points_at_the_script_that_is_actually_running() {
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("pi.cmd");
    std::fs::write(
        &shim,
        "@ECHO off\nSETLOCAL\n\"%node%\" \"%~dp0pi-launcher.js\" %*\n",
    )
    .unwrap();
    let script = dir.path().join("pi-launcher.js");
    std::fs::write(&script, "").unwrap();
    assert_eq!(shim_script(&shim), Some(script.canonicalize().unwrap()));
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
        ("/home/a/.local/share/claude/versions/2.0.0", "claude"),
        ("/usr/bin/agy", "antigravity"),
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
