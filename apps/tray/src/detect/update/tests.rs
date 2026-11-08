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
        assert_eq!(
            plan.program,
            install::command_path(&path.canonicalize().unwrap())
        );
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
        ("devin", ".local/bin/devin"),
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
    fs::write(&real, "#!/bin/sh\nprintf '%s' \"$PATH\" > \"$0.path\"\n").unwrap();
    let link = temp.path().join("codex");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let plan = install::identify("codex", &link).unwrap();
    assert_eq!(plan.program, real.canonicalize().unwrap());
    execute(&plan, Duration::from_secs(5)).unwrap();
    let path = fs::read_to_string(real.with_file_name("codex.path")).unwrap();
    assert_eq!(
        std::env::split_paths(&path).next().unwrap(),
        plan.program.parent().unwrap()
    );
}

#[test]
fn homebrew_uses_its_own_prefix_and_known_names() {
    let temp = TempDir::new().unwrap();
    let brew = file(temp.path(), "brew/bin/brew");
    for (provider, store, name) in [
        ("claude", "Cellar", "claude-code"),
        ("codex", "Caskroom", "codex"),
        ("codebuddy", "Cellar", "codebuddy-code"),
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
    let custom = file(temp.path(), "unusual-app/resources/runtime/cli/bin/dsh.cmd");
    assert!(install::desktop_at(&custom, Some(&custom)));
    assert!(!install::desktop_at(
        &file(temp.path(), "npm/node_modules/@deepseek-ai/dsh/cli.js"),
        Some(&custom)
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
    for dir in [
        modules.clone(),
        temp.path().join("system/bin"),
        modules.join("@openai/codex"),
    ] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(
            install::identify("codex", &path).is_none(),
            "{}",
            dir.display()
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
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
    let cache = Mutex::new(std::collections::HashMap::new());
    let channel = Channel::Grok("stable".into());
    let now = Instant::now();
    let get = |cache: &Mutex<_>, at, value| {
        latest::cached_with(cache, &channel, at, |_| {
            assert!(
                cache.try_lock().is_ok(),
                "query must run without the cache lock"
            );
            value
        })
    };
    assert_eq!(
        get(&cache, now, Some("1.0.0".into())).as_deref(),
        Some("1.0.0")
    );
    assert_eq!(
        get(&cache, now + Duration::from_secs(21599), None).as_deref(),
        Some("1.0.0")
    );
    assert_eq!(get(&cache, now + Duration::from_secs(21600), None), None);
    assert_eq!(
        get(
            &cache,
            now + Duration::from_secs(22499),
            Some("2.0.0".into())
        ),
        None
    );
    assert_eq!(
        get(
            &cache,
            now + Duration::from_secs(22500),
            Some("2.0.0".into())
        )
        .as_deref(),
        Some("2.0.0")
    );
}

#[test]
fn preview_mutex_and_post_update_verification() {
    let locks = locks::Updates::default();
    assert!(locks.claim(true, "codex").err().unwrap().contains("预览版"));
    let guard = locks.claim(false, "codex").unwrap();
    assert!(
        locks
            .claim(false, "codex")
            .err()
            .unwrap()
            .contains("已有 CLI")
    );
    assert!(locks.claim(false, "claude").is_ok());
    drop(guard);
    assert!(locks.claim(false, "codex").is_ok());
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
    assert!(state.get("install").is_none());
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
        args: vec!["update".into()],
        command: "fake update".into(),
        via: "native",
        channel: Channel::Grok("stable".into()),
        environment: locks::Environment::Native(program.clone()),
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
            .contains("更新超时")
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
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s' \"$PATH\" > \"$0.path\"\nexit 0\n",
    )
    .unwrap();
    let path = file(
        temp.path(),
        "Node With Spaces/lib/node_modules/@openai/codex/bin/codex.js",
    );
    let link = temp.path().join("another-cli-dir/codex");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let plan = install::identify("codex", &link).unwrap();
    execute(&plan, Duration::from_secs(5)).unwrap();
    let search = fs::read_to_string(npm.with_file_name("npm.path")).unwrap();
    assert_eq!(
        std::env::split_paths(&search).next().unwrap(),
        npm.parent().unwrap().canonicalize().unwrap()
    );
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

#[test]
fn windows_npm_falls_back_to_the_detected_command_and_strips_drive_prefixes() {
    let temp = TempDir::new().unwrap();
    let binary = file(
        temp.path(),
        "Roaming/npm/node_modules/@openai/codex/bin/codex.js",
    );
    let npm = file(temp.path(), "Program Files/nodejs/npm.cmd");
    let plan = install::npm("codex", &binary, true, || Some(npm.clone())).unwrap();
    assert_eq!(plan.program, npm);
    assert_eq!(
        plan.args[3],
        temp.path().join("Roaming/npm").to_str().unwrap()
    );
    assert!(install::npm("codex", &binary, true, || None).is_none());
    let local = file(temp.path(), "Roaming/npm/npm.cmd");
    assert_eq!(
        install::npm("codex", &binary, true, || panic!("local npm wins"))
            .unwrap()
            .program,
        local
    );
    assert_eq!(
        install::command_path(Path::new(r"\\?\C:\Program Files\nodejs\npm.cmd")),
        Path::new(r"C:\Program Files\nodejs\npm.cmd")
    );
    for path in [r"\\?\UNC\server\share\npm.cmd", r"\\server\share\npm.cmd"] {
        assert_eq!(install::command_path(Path::new(path)), Path::new(path));
    }
}

#[test]
fn snapshot_checks_multiple_channels_in_parallel() {
    let temp = TempDir::new().unwrap();
    let a = file(temp.path(), ".codex/packages/standalone/1/codex");
    let b = file(temp.path(), ".local/share/claude/versions/1");
    let all = json!({"codex": {"installed": true, "path": a, "version": "1.0.0"},
        "claude": {"installed": true, "path": b, "version": "1.0.0"}});
    let started = Mutex::new(0);
    let wake = std::sync::Condvar::new();
    let state = snapshot(&all, |_| {
        let mut count = started.lock().unwrap();
        *count += 1;
        wake.notify_all();
        let (count, _) = wake
            .wait_timeout_while(count, Duration::from_secs(2), |n| *n < 2)
            .unwrap();
        assert_eq!(*count, 2, "both queries must start before either finishes");
        Some("2.0.0".into())
    });
    assert_eq!(state["clis"]["codex"]["update"], true);
    assert_eq!(state["clis"]["claude"]["update"], true);
}

#[test]
fn snapshot_codex_exposes_active_installation() {
    let temp = TempDir::new().unwrap();
    let path = file(temp.path(), ".codex/packages/standalone/1/codex");
    let all = json!({"codex": {"installed": true, "path": path, "version": "1.0.0"}});
    let state = snapshot(&all, |_| Some("2.0.0".into()));
    let installations = state["clis"]["codex"]["installations"]
        .as_array()
        .expect("Codex installations");
    let standalone = installations
        .iter()
        .find(|item| item["id"] == "standalone")
        .unwrap();
    assert_eq!(standalone["path"], all["codex"]["path"]);
    assert_eq!(standalone["version"], "1.0.0");
    assert_eq!(standalone["active"], true);
    assert_eq!(standalone["update"], true);
}

#[cfg(unix)]
mod concurrency;

#[cfg(unix)]
#[test]
fn codex_sources_have_separate_versions_and_verify_the_original_path() {
    let temp = TempDir::new().unwrap();
    let desktop = file(
        temp.path(),
        "Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    );
    fs::write(&desktop, "#!/bin/sh\necho 'codex-cli 9.0.0'\n").unwrap();
    let standalone = file(temp.path(), ".codex/packages/standalone/1/codex");
    fs::write(&standalone, "#!/bin/sh\nif [ \"$1\" = update ]; then echo 2.0.0 > \"$0.version\"; else echo read >> \"$0.calls\"; printf 'codex-cli '; cat \"$0.version\"; fi\n").unwrap();
    fs::write(standalone.with_extension("version"), "1.0.0\n").unwrap();
    let info = json!({"installed": true, "path": desktop, "version": "9.0.0"});
    let sources = [Some(desktop.clone()), Some(standalone.clone())];
    let state = codex::state(&info, &sources, &|_| Some("2.0.0".into()));
    assert_eq!(state["version"], "9.0.0");
    assert_eq!(state["update"], false);
    let installations = &state["installations"];
    assert_eq!(installations[0]["label"], "ChatGPT 内置 CLI");
    assert_eq!(installations[0]["version"], "9.0.0");
    assert_eq!(installations[0]["active"], true);
    assert_eq!(installations[0]["update"], false);
    assert_eq!(installations[1]["version"], "1.0.0");
    assert_eq!(installations[1]["active"], false);
    assert_eq!(installations[1]["update"], true);
    let again = codex::state(&info, &sources, &|_| Some("2.0.0".into()));
    assert_eq!(again, state);
    assert_eq!(
        fs::read_to_string(standalone.with_extension("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let locks = locks::Updates::default();
    let cache = Mutex::new(crate::detect::DetectionCache {
        all: Some(json!({"codex": info})),
        ..Default::default()
    });
    assert!(
        update_at(
            "codex",
            &desktop,
            false,
            &locks,
            &cache,
            |_| Some("10.0.0".into()),
            Duration::from_secs(5)
        )
        .is_err()
    );
    assert_eq!(
        update_at(
            "codex",
            &standalone,
            false,
            &locks,
            &cache,
            |_| Some("2.0.0".into()),
            Duration::from_secs(5)
        )
        .unwrap(),
        "2.0.0"
    );
    assert_eq!(cache.lock().unwrap().all.as_ref().unwrap()["codex"], info);
    let after = codex::state(&info, &sources, &|_| Some("2.0.0".into()));
    assert_eq!(after["installations"][1]["version"], "2.0.0");
    assert_eq!(after["installations"][1]["update"], false);
    // A successful command with no version change cannot be validated using the desktop's 9.0.0.
    fs::write(
        &standalone,
        "#!/bin/sh\n[ \"$1\" = update ] || echo 'codex-cli 2.0.0'\nexit 0\n",
    )
    .unwrap();
    assert!(
        update_at(
            "codex",
            &standalone,
            false,
            &locks,
            &cache,
            |_| Some("3.0.0".into()),
            Duration::from_secs(5)
        )
        .unwrap_err()
        .contains("仍是旧版本")
    );
}

#[test]
fn standalone_discovery_skips_bundled_paths_and_their_aliases() {
    let temp = TempDir::new().unwrap();
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let desktop = file(
        temp.path(),
        &format!("Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/{name}"),
    );
    let old_copy = file(temp.path(), &format!("OpenAI/Codex/bin/hash/{name}"));
    let standalone = file(temp.path(), &format!("node/bin/{name}"));
    let mut dirs = vec![
        desktop.parent().unwrap().to_owned(),
        old_copy.parent().unwrap().to_owned(),
    ];
    #[cfg(unix)]
    {
        let alias = temp.path().join("alias");
        fs::create_dir_all(&alias).unwrap();
        std::os::unix::fs::symlink(&desktop, alias.join(name)).unwrap();
        dirs.push(alias);
    }
    dirs.push(standalone.parent().unwrap().to_owned());
    assert_eq!(
        super::super::locate::standalone_codex_in(dirs),
        Some(standalone)
    );
    let legacy = file(
        temp.path(),
        "Applications/Codex.app/Contents/Resources/codex",
    );
    let info = json!({"path":legacy,"version":"0.1.0"});
    let state = codex::state(&info, &[Some(legacy), None], &|_| None);
    assert_eq!(state["installations"][0]["label"], "Codex 内置 CLI");
    assert_eq!(state["installations"][0]["update"], false);
    assert_eq!(
        validate_target("codex", Some(&json!("standalone"))).unwrap(),
        Some("standalone")
    );
    assert_eq!(validate_target("codex", None).unwrap(), None);
}
