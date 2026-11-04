use super::*;
use std::fs;
use tempfile::TempDir;

fn file(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "fake").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn native_and_desktop_installations() {
    let temp = TempDir::new().unwrap();
    for (provider, relative) in [
        ("claude", ".local/share/claude/versions/2.0.0"),
        ("codex", ".codex/packages/standalone/1.0.0/bin/codex"),
        ("grok", ".grok/bin/grok"),
        ("grok", ".grok/downloads/1.0/grok"),
    ] {
        let path = file(temp.path(), relative);
        let plan = install::identify(provider, &path).unwrap();
        assert_eq!(plan.via, "native");
        assert_eq!(plan.program, path.canonicalize().unwrap());
        assert_eq!(plan.command, format!("{provider} update"));
    }
    let windows = file(temp.path(), ".local/bin/claude.exe");
    assert_eq!(
        install::classify("claude", &windows, true).unwrap().via,
        "native"
    );
    for (provider, relative) in [
        (
            "codex",
            "Applications/ChatGPT.app/Contents/Resources/.codex/packages/standalone/codex",
        ),
        (
            "deepseek",
            "Applications/DeepSeek Harness.app/Contents/Resources/dsh",
        ),
        (
            "claude",
            "Applications/Claude.app/Contents/node_modules/@anthropic-ai/claude-code/cli.js",
        ),
        ("zcode", ".grok/bin/zcode"),
        ("antigravity", ".grok/bin/agy"),
        ("codex", "unknown/codex"),
    ] {
        assert!(install::identify(provider, &file(temp.path(), relative)).is_none());
    }
}

#[cfg(unix)]
#[test]
fn follows_symlinks_to_the_same_detected_installation() {
    let temp = TempDir::new().unwrap();
    let real = file(temp.path(), ".codex/packages/standalone/1/codex");
    let link = temp.path().join("codex");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        install::identify("codex", &link).unwrap().program,
        real.canonicalize().unwrap()
    );
}

#[test]
fn homebrew_uses_its_own_prefix_and_known_names() {
    let temp = TempDir::new().unwrap();
    let brew = file(temp.path(), "brew/bin/brew");
    for (provider, store, name) in [
        ("claude", "Cellar", "claude-code"),
        ("codex", "Caskroom", "codex"),
        ("pi", "Cellar", "pi-coding-agent"),
        ("grok", "Caskroom", "grok-build"),
    ] {
        let path = file(
            temp.path(),
            &format!("brew/{store}/{name}/1/bin/{provider}"),
        );
        let plan = install::identify(provider, &path).unwrap();
        assert_eq!(plan.program, brew.canonicalize().unwrap());
        assert_eq!(plan.via, "homebrew");
        assert_eq!(plan.args.contains(&"--cask".into()), store == "Caskroom");
    }
    let wrong = file(temp.path(), "brew/Cellar/unrelated/1/bin/codex");
    assert!(install::identify("codex", &wrong).is_none());
}

#[test]
fn npm_preserves_package_and_prefix() {
    let temp = TempDir::new().unwrap();
    file(temp.path(), "node/bin/npm");
    for (provider, package) in [
        ("claude", "@anthropic-ai/claude-code"),
        ("codex", "@openai/codex"),
        ("codebuddy", "@tencent-ai/codebuddy-code"),
        ("pi", "@earendil-works/pi-coding-agent"),
        ("pi", "@mariozechner/pi-coding-agent"),
        ("deepseek", "@deepseek-ai/dsh"),
    ] {
        let path = file(
            temp.path(),
            &format!("node/lib/node_modules/{package}/bin/cli.js"),
        );
        // Model Unix installation paths even when these tests run on Windows.
        let plan = install::classify(provider, &path, false).unwrap();
        assert_eq!(
            plan.args,
            vec![
                "install",
                "-g",
                "--prefix",
                temp.path().join("node").to_str().unwrap(),
                &format!("{package}@latest")
            ]
        );
        assert!(!plan.command.contains(temp.path().to_str().unwrap()));
        assert_eq!(plan.channel, Channel::Npm(package.into()));
    }
    let windows = file(temp.path(), "win/node_modules/@openai/codex/bin/codex.js");
    let npm = file(temp.path(), "win/npm.cmd");
    let plan = install::classify("codex", &windows, true).unwrap();
    assert_eq!(plan.program, npm);
    assert!(plan.command.starts_with("npm.cmd "));
}

#[test]
fn windows_shims_and_desktop_directories() {
    let temp = TempDir::new().unwrap();
    for package in [
        "@earendil-works/pi-coding-agent",
        "@mariozechner/pi-coding-agent",
    ] {
        let binary = file(temp.path(), &format!("node_modules/{package}/dist/cli.js"));
        let shim = file(temp.path(), "pi.cmd");
        fs::write(
            &shim,
            format!(
                "@echo off\r\n\"%dp0%\\node_modules\\{}\\dist\\cli.js\" %*",
                package.replace('/', "\\")
            ),
        )
        .unwrap();
        assert_eq!(
            install::target(&shim, "pi", true),
            Some(binary.canonicalize().unwrap())
        );
    }
    for path in [
        r"C:\Users\User\AppData\Local\OpenAI\Codex\bin\abc\codex.exe",
        r"C:\Users\User\.codex\plugins\.plugin-appserver\codex.exe",
        r"D:\Apps\DeepSeek Harness\resources\runtime\cli\bin\dsh.cmd",
        r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_1.0\app\codex.exe",
        r"D:\Apps\ZCode\resources\zcode.exe",
    ] {
        assert!(install::desktop(Path::new(path), true), "{path}");
    }
    assert!(!install::desktop(
        Path::new(r"C:\Users\User\npm\node_modules\@openai\codex\bin\codex.js"),
        true
    ));
}

#[cfg(unix)]
#[test]
fn readonly_npm_directory_is_not_updatable() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    file(temp.path(), "system/bin/npm");
    let path = file(
        temp.path(),
        "system/lib/node_modules/@openai/codex/bin/codex.js",
    );
    let modules = temp.path().join("system/lib/node_modules");
    fs::set_permissions(&modules, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(install::identify("codex", &path).is_none());
    fs::set_permissions(&modules, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn version_ordering_and_channel_parsers() {
    for (a, b) in [
        ("1.10.0", "1.9.9"),
        ("2.0.0", "2.0.0-rc.1"),
        ("2.0.0-rc.10", "2.0.0-rc.2"),
        ("1.0.1", "1"),
        ("1.2.1", "1.2"),
    ] {
        if b == "1" {
            assert!(!newer(a, b));
        } else {
            assert!(newer(a, b), "{a} > {b}");
        }
        assert!(!newer(b, a));
    }
    for (a, b) in [
        ("1.0", "1.0.0"),
        ("v1.0.0+build", "1.0.0"),
        ("bad", "1.0.0"),
    ] {
        assert!(!newer(a, b));
        assert!(!newer(b, a));
    }
    assert_eq!(
        latest::parse_npm(r#"{"version":"1.2.3"}"#).as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        latest::parse_brew(r#"{"formulae":[{"versions":{"stable":"2.0.0"}}]}"#, false).as_deref(),
        Some("2.0.0")
    );
    assert_eq!(
        latest::parse_brew(r#"{"casks":[{"version":"3.0.1,build12"}]}"#, true).as_deref(),
        Some("3.0.1")
    );
    assert!(latest::parse_npm(r#"{"version":"latest"}"#).is_none());
    assert!(latest::parse_brew("{}", false).is_none());
    assert_eq!(
        install::parse_channel("[cli]\nchannel = 'preview'"),
        "preview"
    );
    assert_eq!(
        install::parse_channel("[cli]\nchannel = '../bad'"),
        "stable"
    );
    assert_eq!(install::parse_channel("broken"), "stable");
    let mut urls = Vec::new();
    assert_eq!(
        latest::grok_with("stable", |url| {
            urls.push(url.to_owned());
            (urls.len() == 2).then(|| " 1.0.46\n".into())
        })
        .as_deref(),
        Some("1.0.46")
    );
    assert_eq!(
        urls,
        [
            "https://x.ai/cli/stable",
            "https://storage.googleapis.com/grok-build-public-artifacts/cli/stable"
        ]
    );
}

#[test]
fn channel_cache_success_failure_and_expiry() {
    use std::time::Instant;
    let mut cache = std::collections::HashMap::new();
    let channel = Channel::Grok("stable".into());
    let now = Instant::now();
    let get = |cache: &mut _, at, value| latest::cached_with(cache, &channel, at, |_| value);
    assert_eq!(
        get(&mut cache, now, Some("1.0.0".into())).as_deref(),
        Some("1.0.0")
    );
    assert_eq!(
        get(&mut cache, now + Duration::from_secs(21599), None).as_deref(),
        Some("1.0.0")
    );
    assert_eq!(
        get(&mut cache, now + Duration::from_secs(21600), None),
        None
    );
    assert_eq!(
        get(
            &mut cache,
            now + Duration::from_secs(22499),
            Some("2.0.0".into())
        ),
        None
    );
    assert_eq!(
        get(
            &mut cache,
            now + Duration::from_secs(22500),
            Some("2.0.0".into())
        )
        .as_deref(),
        Some("2.0.0")
    );
}

#[test]
fn preview_mutex_and_post_update_verification() {
    let lock = Mutex::new(());
    assert!(claim(true, &lock).unwrap_err().contains("Preview"));
    let guard = claim(false, &lock).unwrap();
    assert!(claim(false, &lock).unwrap_err().contains("Another"));
    drop(guard);
    assert!(claim(false, &lock).is_ok());
    assert!(
        verify("codex update", "1.0.0", Some("1.0.0"), "2.0.0")
            .unwrap_err()
            .contains("仍是旧版本")
    );
    assert!(verify("codex update", "1.0.0", Some("2.0.0"), "2.0.0").is_ok());
    assert!(verify("codex update", "1.0.0", None, "2.0.0").is_err());
}

#[test]
fn snapshot_uses_injected_versions_and_install_commands_are_os_specific() {
    let temp = TempDir::new().unwrap();
    let path = file(temp.path(), ".grok/bin/grok");
    let all = json!({"grok": {"installed": true, "path": path, "version": "1.0.0"},
        "zcode": {"installed": true, "version": "1.0.0"}});
    let state = snapshot(&all, |_| Some("2.0.0".into()));
    assert_eq!(state["clis"]["grok"]["update"], true);
    assert_eq!(state["clis"]["zcode"]["update"], false);
    assert_eq!(snapshot(&all, |_| None)["clis"]["grok"]["update"], false);
    let mac = install::commands("macos");
    let linux = install::commands("linux");
    let windows = install::commands("windows");
    assert_eq!(mac["codex"].as_array().unwrap().len(), 3);
    assert_eq!(linux["codex"].as_array().unwrap().len(), 2);
    assert!(
        windows["codex"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("powershell ")
    );
    assert!(
        windows["pi"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("npm.cmd ")
    );
    for p in ["grok", "antigravity", "zcode"] {
        assert!(mac[p].as_array().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn fake_update_receives_arguments_environment_and_empty_stdin() {
    let temp = TempDir::new().unwrap();
    let program = file(temp.path(), "fake");
    fs::write(&program, "#!/bin/sh\n[ \"$NO_COLOR\" = 1 ] || exit 2\n[ \"$1\" = update ] || exit 3\nread line && exit 4\nprintf 'first\\nsecond\\n\\033[31mthird\\033[0m\\n\\nfourth\\nfifth\\n' >&2\nexit 1\n").unwrap();
    let plan = Plan {
        program: program.clone(),
        search_path: None,
        args: vec!["update".into()],
        command: "fake update".into(),
        via: "native",
        channel: Channel::Grok("stable".into()),
    };
    assert_eq!(
        execute(&plan, Duration::from_secs(5)).unwrap_err(),
        "fake update: third\nfourth\nfifth"
    );
    fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(execute(&plan, Duration::from_secs(5)).is_ok());
    fs::write(&program, "#!/bin/sh\nsleep 10\n").unwrap();
    assert!(
        execute(&plan, Duration::from_millis(30))
            .unwrap_err()
            .contains("timed out")
    );
    assert_eq!(
        error_tail("\x1b]8;;https://example.com\x1b\\label\x1b]8;;\x07\n"),
        "label"
    );
}

#[cfg(unix)]
#[test]
fn fake_npm_updates_the_original_prefix_even_with_spaces() {
    let temp = TempDir::new().unwrap();
    let npm = file(temp.path(), "Node With Spaces/bin/npm");
    fs::write(
        &npm,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nexit 0\n",
    )
    .unwrap();
    let path = file(
        temp.path(),
        "Node With Spaces/lib/node_modules/@openai/codex/bin/codex.js",
    );
    let plan = install::identify("codex", &path).unwrap();
    execute(&plan, Duration::from_secs(5)).unwrap();
    let received = fs::read_to_string(npm.with_file_name("npm.args")).unwrap();
    assert_eq!(
        received.lines().collect::<Vec<_>>(),
        [
            "install",
            "-g",
            "--prefix",
            temp.path()
                .join("Node With Spaces")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap(),
            "@openai/codex@latest"
        ]
    );
}
