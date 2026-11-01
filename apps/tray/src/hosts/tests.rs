use super::*;
use crate::data::Reader;
use tempfile::TempDir;

mod versioned;

fn fixture() -> (TempDir, Hosts) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("home");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&home).unwrap();
    let context = Context {
        home: home.clone(),
        app: base.join("app"),
        project: base.join("project"),
        env: BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            ("PATH".into(), bin.display().to_string()),
            ("SHELL".into(), "/bin/sh".into()),
        ]),
        search: vec![bin],
        applications: vec![base.join("Applications")],
    };
    executable_file(
        &home.join(".local/share/uv/tools/agentnave/bin/agentnave-mcp"),
        "#!/bin/sh\nexit 99\n",
    );
    save(&home.join(".local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave-0.22.0.dist-info/entry_points.txt"),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n");
    let mut hosts = Hosts::new(context);
    hosts.shell = Some(Ok(BTreeMap::new()));
    hosts.processes = Mutex::new(processes::Probe::fixed(Some(HashMap::new())));
    (temp, hosts)
}
/// Where uv puts a console script in a tool environment.
fn launcher_in(tools: &Path) -> PathBuf {
    if cfg!(windows) {
        tools.join("agentnave/Scripts/agentnave-mcp.exe")
    } else {
        tools.join("agentnave/bin/agentnave-mcp")
    }
}
/// A fake CLI under the name this platform looks for; only POSIX tests ever run its script.
fn executable_file(path: &Path, text: &str) {
    let launcher = path.ends_with("agentnave/bin/agentnave-mcp");
    let path = &if !cfg!(windows) {
        path.to_owned()
    } else if launcher {
        launcher_in(path.parent().unwrap().parent().unwrap().parent().unwrap())
    } else {
        path.with_extension("exe")
    };
    save(path, text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
fn host(hosts: &Hosts, id: &str) -> Host {
    hosts.registry().into_iter().find(|h| h.id == id).unwrap()
}
fn save(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn seed(host: &Host) -> String {
    match host.format {
        Format::Zcode => "{\n\"unknown\":true,\"mcp\":{\"servers\":{\"other\":{\"command\":\"keep\"}}}}".into(),
        Format::Json => format!("{{\n// keep comment\n\"unknown\": {{\"a\": [1,2]}},\n\"{}\": {{\"other\": {{\"command\": \"keep\"}}}}}}", host.key),
        Format::Toml => "# keep comment\nunknown = { a = [1, 2] }\n[mcp_servers.other]\ncommand = 'keep'\n".into(),
        Format::Yaml => "# keep comment\nunknown:\n  - untouched\nmcp_servers:\n  other:\n    command: keep\ntail: 'exact' # keep\n".into(),
        Format::DshPatch => "# keep comment\n- id: it's opaque\n- id: agent-default-model # keep\n  config:\n    model: keep\n- id: plugin-manager\n  disabled: !!js '!ctx.get(''profileContext'')'\n- insert:\n    - id: other\n      config:\n        command: keep\n".into(),
    }
}
#[test]
fn revision_concurrent_unrelated_change_is_preserved_and_edit_rebased() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    save(
        &h.config,
        r#"{"unknown":"before","mcpServers":{"agentnave":{"command":"/old","env":{}}}}"#,
    );
    let before = h.read().unwrap();
    let mut desired = before.entry.clone().unwrap();
    desired["env"]["AGENTNAVE_HOST"] = json!("workbuddy");
    save(
        &h.config,
        r#"{"unknown":"concurrent","mcpServers":{"agentnave":{"command":"/old","env":{"NEW":"keep"}}}}"#,
    );
    assert!(hosts.write(&h, &before, Some(&desired)).is_ok());
    let after = h.read().unwrap();
    assert_eq!(after.rest["unknown"], "concurrent");
    assert_eq!(after.entry.unwrap()["env"]["NEW"], "keep");
}

#[test]
#[cfg(unix)]
fn revision_parent_symlink_is_rejected_without_touching_target() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    let before = h.read().unwrap();
    let outside = hosts.context.home.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, h.config.parent().unwrap()).unwrap();
    assert!(
        hosts
            .write(&h, &before, Some(&json!({"command":"/x"})))
            .is_err()
    );
    assert!(!outside.join("mcp.json").exists());
    assert_eq!(hosts.state(&h)["attention"]["code"], "symlink_path");
    let home_link = hosts.context.home.parent().unwrap().join("home-link");
    std::os::unix::fs::symlink(&hosts.context.home, &home_link).unwrap();
    assert!(check_path(&home_link.join("ordinary/config.json"), &home_link).is_ok());
}

#[test]
fn revision_bare_command_uses_entry_path_without_replacement() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    let private = hosts.context.home.join("private-bin");
    executable_file(&private.join("agentnave-mcp"), "#!/bin/sh\nexit 99\n");
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{"command":"agentnave-mcp","env":{"PATH":private}}}})
            .to_string(),
    );
    hosts.refresh();
    assert_eq!(h.read().unwrap().entry.unwrap()["command"], "agentnave-mcp");
    let mut entry = h.read().unwrap().entry.unwrap();
    entry["env"]["PATH"] = json!("${UNRESOLVED_PATH}");
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":entry}}).to_string(),
    );
    hosts.refresh();
    assert_eq!(hosts.state(&h)["command_state"], "unknown");
    assert_eq!(hosts.state(&h)["status"], "connected");
    assert_eq!(h.read().unwrap().entry.unwrap()["command"], "agentnave-mcp");
}

#[test]
fn revision_healthy_unclaimed_entry_is_connected() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{"command":hosts.launcher()}}}).to_string(),
    );
    let state = hosts.state(&h);
    assert_eq!(state["status"], "connected");
    assert_eq!(state["has_host_env"], false);
}

#[test]
#[cfg(unix)]
fn revision_unrelated_change_does_not_retry_failed_takeover() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    save(
        &h.config,
        &json!({"unknown":1,"mcpServers":{"agentnave":{"command":hosts.launcher(),"env":{}}}})
            .to_string(),
    );
    // Fail at the backup boundary, then remove that obstacle. An unwanted retry would now
    // succeed and visibly add AGENTNAVE_HOST, even when last_action timestamps are identical.
    let backups = hosts.context.app.join("backups");
    save(&backups, "not a directory");
    hosts.refresh();
    let first = hosts.state(&h)["last_action"].clone();
    assert_eq!(first["ok"], false);
    fs::remove_file(&backups).unwrap();
    let mut raw: Value = serde_json::from_slice(&fs::read(&h.config).unwrap()).unwrap();
    raw["unknown"] = json!(2);
    save(&h.config, &raw.to_string());
    hosts.refresh();
    assert_eq!(fs::read_to_string(&h.config).unwrap(), raw.to_string());
    assert_eq!(hosts.state(&h)["has_host_env"], false);
    assert!(!backups.exists());
    assert_eq!(hosts.state(&h)["last_action"], first);
    assert_eq!(
        hosts.action("connect", &json!({"id":h.id})).unwrap()["ok"],
        true
    );
    assert_eq!(hosts.state(&h)["has_host_env"], true);
}

#[test]
#[cfg(unix)]
fn skill_links_are_replaced_and_linked_folders_left_without_blocking_mcp_actions() {
    for linked_parent in [false, true] {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, "workbuddy");
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        save(
            &h.config,
            &json!({"mcpServers":{"agentnave":{"command":hosts.launcher()}}}).to_string(),
        );
        let skill = h.skill.as_ref().unwrap();
        let link = if linked_parent {
            skill.parent().unwrap()
        } else {
            skill.as_path()
        };
        let outside = hosts.context.home.join("external-skills");
        save(&outside.join("SKILL.md"), "externally managed");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, link).unwrap();
        let expected = if linked_parent { "blocked" } else { "external" };
        assert_eq!(hosts.state(&h)["skill"]["state"], expected);
        assert_eq!(hosts.state(&h)["status"], "connected");
        assert!(hosts.state(&h)["attention"].is_null());
        hosts.refresh();
        assert_eq!(hosts.state(&h)["has_host_env"], true);
        assert_eq!(hosts.state(&h)["status"], "connected");
        assert_eq!(
            hosts.action("remove", &json!({"id":h.id})).unwrap()["ok"],
            true
        );
        assert!(h.read().unwrap().entry.is_none());
        assert_eq!(
            hosts.action("connect", &json!({"id":h.id})).unwrap()["ok"],
            true
        );
        assert!(h.read().unwrap().entry.is_some());
        // A linked Skill becomes the App's copy; a linked folder is never written through.
        assert_eq!(link.is_symlink(), linked_parent);
        let expected = if linked_parent { "blocked" } else { "app" };
        assert_eq!(hosts.state(&h)["skill"]["state"], expected);
        assert_eq!(
            fs::read_to_string(outside.join("SKILL.md")).unwrap(),
            "externally managed"
        );
    }
}

#[test]
fn service_detection_and_connect_share_shell_tool_directory_resolution() {
    let vars: &[&str] = if cfg!(windows) {
        &["UV_TOOL_DIR"]
    } else {
        &["UV_TOOL_DIR", "XDG_DATA_HOME"]
    };
    for &var in vars {
        let (_temp, mut hosts) = fixture();
        let root = hosts.context.home.join("shell-data");
        let tools = if var == "UV_TOOL_DIR" {
            root.clone()
        } else {
            root.join("uv/tools")
        };
        fs::create_dir_all(tools.parent().unwrap()).unwrap();
        fs::rename(hosts.context.home.join(".local/share/uv/tools"), &tools).unwrap();
        hosts.shell = Some(Ok(BTreeMap::from([(
            var.into(),
            root.display().to_string(),
        )])));
        executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
        assert_eq!(hosts.service()["installed"], true);
        let result = hosts.action("connect", &json!({"id":"pi"})).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["host"]["command"], json!(launcher_in(&tools)));
        let env_root = hosts.context.home.join("env-data");
        let env_tools = if var == "UV_TOOL_DIR" {
            env_root.clone()
        } else {
            env_root.join("uv/tools")
        };
        fs::create_dir_all(env_tools.parent().unwrap()).unwrap();
        fs::rename(&tools, &env_tools).unwrap();
        hosts
            .context
            .env
            .insert(var.into(), env_root.display().to_string());
        assert_eq!(hosts.service()["installed"], true);
        let result = hosts.action("connect", &json!({"id":"pi"})).unwrap();
        assert_eq!(result["host"]["command"], json!(launcher_in(&env_tools)));
    }
}

#[test]
#[cfg(unix)]
fn upgrade_directory_snapshot_is_available_while_host_configuration_blocks() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};
    let (_temp, hosts) = fixture();
    let reader = Arc::new(Reader::new(hosts.context.project.clone()));
    let shell = reader.probe_tools(&hosts.context);
    let tools = reader.tool_dirs().unwrap().tools;
    fs::create_dir_all(
        tools.join("agentnave/lib/python3.12/site-packages/agentnave-0.1.0.dist-info"),
    )
    .unwrap();
    let fifo = hosts.context.app.join("hosts.json");
    fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let background = reader.clone();
    let worker = std::thread::spawn(move || {
        background.configure_hosts(hosts.context, shell);
        background.refresh_hosts();
    });
    // Opening this writer succeeds only once the background reader has reached the FIFO.
    // Keep it open without bytes so configuration loading stays blocked, including its lock.
    let started = Instant::now();
    let mut writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
        {
            Ok(file) => break file,
            Err(_) if started.elapsed() < Duration::from_secs(2) => std::thread::yield_now(),
            Err(e) => panic!("configuration did not reach FIFO: {e}"),
        }
    };
    let (tx, rx) = mpsc::channel();
    let lookup = std::thread::spawn(move || {
        let dirs = reader.tool_dirs().unwrap();
        tx.send(crate::update::service(&Some(dirs.tools))).unwrap();
    });
    let status = rx.recv_timeout(Duration::from_millis(500));
    writer.write_all(b"{}").unwrap();
    drop(writer);
    worker.join().unwrap();
    lookup.join().unwrap();
    assert_eq!(
        status.expect("upgrade must not wait for host configuration")["version"],
        "0.1.0"
    );
}

#[test]
#[cfg(unix)]
fn service_installer_passes_cached_shell_directories_to_uv_and_version_checks() {
    let (_temp, mut hosts) = fixture();
    hosts.shell = None;
    assert!(hosts.tool_dirs().is_none());
    let tools = hosts.context.home.join("shell-tools");
    let bin = hosts.context.home.join("shell-bin");
    hosts.shell = Some(Ok(BTreeMap::from([
        ("UV_TOOL_DIR".into(), tools.display().to_string()),
        ("UV_TOOL_BIN_DIR".into(), bin.display().to_string()),
        (
            "UV_DEFAULT_INDEX".into(),
            "https://pypi.tuna.tsinghua.edu.cn/simple".into(),
        ),
        ("HTTPS_PROXY".into(), "http://127.0.0.1:7890".into()),
    ])));
    let dirs = hosts.tool_dirs().unwrap();
    let site = tools.join("agentnave/lib/python3.12/site-packages");
    fs::create_dir_all(site.join("agentnave-0.1.0.dist-info")).unwrap();
    assert_eq!(
        crate::update::service(&Some(dirs.tools.clone()))["version"],
        "0.1.0"
    );
    let uv = hosts.context.home.join("fake-uv");
    save(&uv.with_extension("tools"), tools.to_str().unwrap());
    save(&uv.with_extension("bin"), bin.to_str().unwrap());
    executable_file(
        &uv,
        &r#"#!/bin/sh
tools=$(cat "$0.tools")
bin=$(cat "$0.bin")
[ "$UV_TOOL_DIR" = "$tools" ] && [ "$UV_TOOL_BIN_DIR" = "$bin" ] || exit 7
[ "$UV_DEFAULT_INDEX" = "https://pypi.tuna.tsinghua.edu.cn/simple" ] || exit 11
[ "$HTTPS_PROXY" = "http://127.0.0.1:7890" ] || exit 12
[ -f "$6" ] || exit 13
printf '%s\n' "$UV_TOOL_DIR" "$UV_TOOL_BIN_DIR" "$HOME" "$1 $2" >> "$0.calls"
if [ "$2" = install ]; then
  site="$tools/agentnave/lib/python3.12/site-packages"
  rmdir "$site/agentnave-0.1.0.dist-info" || exit 8
  mkdir -p "$site/agentnave-@VERSION@.dist-info" "$tools/agentnave/bin" "$bin" || exit 9
  printf '[console_scripts]\nagentnave-mcp = agentnave.relay:main\n' > "$site/agentnave-@VERSION@.dist-info/entry_points.txt"
  for entry in agentnave-mcp; do
    printf '#!/bin/sh\nexit 99\n' > "$tools/agentnave/bin/$entry"
    chmod 700 "$tools/agentnave/bin/$entry" || exit 10
  done
fi
"#
        .replace("@VERSION@", VERSION),
    );
    crate::update::install_service(&uv, &dirs).unwrap();
    let expected = format!(
        "{}\n{}\n{}\ntool install\n",
        tools.display(),
        bin.display(),
        hosts.context.home.display()
    );
    assert_eq!(
        fs::read_to_string(uv.with_extension("calls")).unwrap(),
        expected
    );
    assert_eq!(hosts.service()["installed"], true);
    assert!(crate::update::service(&Some(hosts.tool_dirs().unwrap().tools)).is_null());
}

#[test]
#[cfg(unix)]
fn settings_path_errors_preserve_code_without_prefixing_the_message() {
    let (_temp, hosts) = fixture();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    let target = hosts.context.home.join("external-settings.json");
    save(&target, "{}");
    fs::create_dir_all(&hosts.context.app).unwrap();
    std::os::unix::fs::symlink(&target, hosts.context.app.join("hosts.json")).unwrap();
    let hosts = Hosts::new(hosts.context);
    let state = hosts.state(&host(&hosts, "pi"));
    assert_eq!(state["attention"]["code"], "symlink_path");
    assert_eq!(
        state["attention"]["message"],
        "配置路径经过符号链接，请手动处理"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
}

#[test]
fn removal_leaves_project_configuration_untouched() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    let missing = hosts.context.home.join("old-bin/agentnave-mcp");
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{"command":missing,"args":[]}}}).to_string(),
    );
    let project = hosts.context.project.join(".pi/mcp.json");
    save(
        &project,
        r#"{"mcpServers":{"agentnave":{"command":"project"}}}"#,
    );
    let project_bytes = fs::read(&project).unwrap();
    hosts.refresh();
    assert_eq!(fs::read(&project).unwrap(), project_bytes);
    let result = hosts.action("remove", &json!({"id":"pi"})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert!(h.read().unwrap().entry.is_none());
    assert_eq!(fs::read(&project).unwrap(), project_bytes);
}

#[test]
fn all_host_formats_connect_refresh_and_remove_without_running_host_clis() {
    for id in IDS {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, id);
        let marker = hosts.context.home.join("CLI_WAS_RUN");
        executable_file(
            &hosts.context.search[0].join(h.binary),
            &format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
        );
        save(&h.config, &seed(&h));
        let original = h.read().unwrap();
        let result = hosts.action("connect", &json!({"id":id})).unwrap();
        assert_eq!(result["ok"], true, "{id}: {result}");
        if id == "deepseek" {
            // dsh would otherwise cut `wait` off after 60 s.
            assert_eq!(
                h.read().unwrap().entry.unwrap()["toolCallTimeoutMs"],
                310_000
            );
        }
        assert_eq!(
            result["host"]["tested"],
            [
                "codex",
                "claude",
                "codebuddy",
                "workbuddy",
                "grok",
                "antigravity",
                "pi",
                "deepseek",
                "hermes"
            ]
            .contains(&id)
        );
        assert_eq!(result["host"]["status"], "connected");
        assert_eq!(result["host"]["skill"]["state"], "app");
        assert!(
            result["host"]["reload_hint"]
                .as_str()
                .unwrap()
                .contains("重启")
        );
        assert!(result["host"]["attention"].is_null());
        let before = fs::read(&h.config).unwrap();
        hosts.refresh();
        assert_eq!(fs::read(&h.config).unwrap(), before);
        assert_eq!(
            hosts.action("remove", &json!({"id":id})).unwrap()["ok"],
            true
        );
        assert!(h.read().unwrap().entry.is_none());
        if !matches!(h.format, Format::Yaml) {
            assert_eq!(h.read().unwrap().rest, original.rest);
        } else {
            let raw = fs::read_to_string(&h.config).unwrap();
            assert!(raw.contains("    command: keep"));
            assert!(raw.ends_with("tail: 'exact' # keep\n"));
        }
        assert!(!marker.exists());
    }
}

#[test]
fn first_connection_detects_running_hosts_without_mcp_receipts() {
    let (_temp, mut hosts) = fixture();
    for id in IDS {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        hosts.processes = Mutex::new(processes::Probe::fixed(Some(HashMap::from([(id, 1)]))));
        let connected = hosts.action("connect", &json!({"id":id})).unwrap();
        assert_eq!(connected["ok"], true, "{id}: {connected}");
        assert_eq!(connected["host"]["reload"]["state"], "required", "{id}");
        assert_eq!(connected["host"]["reload"]["reason"], "host_running");
        assert!(connected["message"].as_str().unwrap().contains("重启"));
        let changed = hosts.settings[id]["reload_changed"].clone();
        hosts.refresh();
        assert_eq!(hosts.settings[id]["reload_changed"], changed);
        assert_eq!(hosts.state(&h)["reload"]["state"], "required");
        let repeated = hosts.action("connect", &json!({"id":id})).unwrap();
        assert!(!repeated["message"].as_str().unwrap().contains("后生效"));
        hosts.processes = Mutex::new(processes::Probe::fixed(None));
        assert_eq!(hosts.state(&h)["reload"]["state"], "unverified");
        hosts.processes = Mutex::new(processes::Probe::fixed(Some(HashMap::new())));
        assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
        let after_change = changed["at_ms"].as_u64().unwrap() / 1000 + 1;
        hosts.processes = Mutex::new(processes::Probe::fixed(Some(HashMap::from([(
            id,
            after_change,
        )]))));
        assert_eq!(hosts.state(&h)["reload"]["state"], "unverified");
        let dir = hosts.context.home.join(".agentnave/connections");
        save(
            &dir.join(format!("{id}.json")),
            &json!({
                "version":1,"host":id,"started_ms":(after_change + 1) * 1000,
                "runtime_tools_hash":"a".repeat(64),"host_tools_hash":"a".repeat(64),
                "runtime_version":"0.22.0"
            })
            .to_string(),
        );
        assert_eq!(hosts.state(&h)["reload"]["state"], "current");
        // The proof outlives the connection while the host stays open.
        hosts.remember_reloads();
        fs::remove_file(dir.join(format!("{id}.json"))).unwrap();
        assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
    }
}

#[test]
fn reload_confirmation_is_per_host_persistent_and_invalidated_by_changes() {
    let (_temp, mut hosts) = fixture();
    for id in ["codex", "pi"] {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        assert_eq!(hosts.state(&h)["reload"]["state"], "not_connected");
        assert_eq!(
            hosts.action("connect", &json!({"id":id})).unwrap()["ok"],
            true
        );
    }
    // Without a live connection nothing holds the old configuration: the next one loads it.
    let h = host(&hosts, "codex");
    assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
    // A relay that reports no interface, live since before the change, needs a manual reload.
    let dir = hosts.context.home.join(".agentnave/connections");
    for id in ["codex", "pi"] {
        save(
            &dir.join(format!("{id}.json")),
            &json!({"version":1,"host":id,"started_ms":1}).to_string(),
        );
    }
    let pending = hosts.state(&h)["reload"].clone();
    assert_eq!(pending["state"], "required");
    // A relay swapping to a new install pauses its receipt; only a longer silence clears it.
    let age = |seconds: u64| {
        fs::File::options()
            .write(true)
            .open(dir.join("codex.json"))
            .unwrap()
            .set_modified(SystemTime::now() - std::time::Duration::from_secs(seconds))
            .unwrap();
    };
    age(100);
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    age(200);
    assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
    age(0);
    hosts
        .action(
            "confirm-reload",
            &json!({"id":"codex","token":pending["token"]}),
        )
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "confirmed");
    assert_eq!(
        hosts.state(&host(&hosts, "pi"))["reload"]["state"],
        "required"
    );

    let shell = hosts.shell.clone();
    let tools = hosts.tools.clone();
    hosts = Hosts::from_probe(hosts.context, shell, tools);
    assert_eq!(hosts.state(&h)["reload"]["state"], "confirmed");
    let preferences = fs::read(hosts.context.app.join("hosts.json")).unwrap();
    hosts.snapshot();
    assert_eq!(
        fs::read(hosts.context.app.join("hosts.json")).unwrap(),
        preferences
    );

    // A version-only update does not require restarting an unchanged MCP interface/config.
    let site = hosts
        .tool_dirs()
        .unwrap()
        .tools
        .join("agentnave/lib/python3.12/site-packages");
    fs::rename(
        site.join("agentnave-0.22.0.dist-info"),
        site.join("agentnave-0.23.0.dist-info"),
    )
    .unwrap();
    let next = hosts.state(&h)["reload"].clone();
    assert_eq!(next["state"], "confirmed");
    hosts
        .action(
            "confirm-reload",
            &json!({"id":"codex","token":next["token"]}),
        )
        .unwrap();

    // An unrelated preference does not affect activation; the MCP entry does.
    let raw = fs::read_to_string(&h.config).unwrap();
    save(&h.config, &format!("{raw}\n[unrelated]\nflag = true\n"));
    assert_eq!(hosts.state(&h)["reload"]["state"], "confirmed");
    let before = h.read().unwrap();
    let mut entry = before.entry.clone().unwrap();
    entry["args"] = json!(["changed"]);
    hosts.write(&h, &before, Some(&entry)).unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    assert!(
        hosts
            .action(
                "confirm-reload",
                &json!({"id":"codex","token":pending["token"]})
            )
            .is_err()
    );
    let token = hosts.state(&h)["reload"]["token"].clone();
    hosts
        .action("confirm-reload", &json!({"id":"codex","token":token}))
        .unwrap();
    assert_eq!(
        hosts.action("remove", &json!({"id":"codex"})).unwrap()["ok"],
        true
    );
    assert_eq!(hosts.state(&h)["reload"]["state"], "not_connected");
    assert_eq!(
        hosts.action("connect", &json!({"id":"codex"})).unwrap()["ok"],
        true
    );
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    let token = hosts.state(&h)["reload"]["token"].clone();
    hosts
        .action("confirm-reload", &json!({"id":"codex","token":token}))
        .unwrap();
    save(
        &h.skill.as_ref().unwrap().join("SKILL.md"),
        "externally changed",
    );
    assert_eq!(hosts.state(&h)["reload"]["state"], "confirmed");
}

#[test]
fn connection_receipts_confirm_interfaces_and_fresh_sessions_without_hiding_stale_peers() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    let dir = hosts.context.home.join(".agentnave/connections");
    let since = hosts.settings["codex"]["reload_changed"]["at_ms"]
        .as_u64()
        .unwrap();
    let mut receipt = json!({"version":1,"host":"codex","started_ms":since+1,"runtime_version":"0.22.0","runtime_tools_hash":"a".repeat(64),"host_tools_hash":"a".repeat(64)});
    save(&dir.join("new.json"), &receipt.to_string());
    // A session started after the Skill was installed has loaded it.
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    // Unless an older one is only paused while its relay swaps installs.
    let mut paused = receipt.clone();
    paused["started_ms"] = json!(1);
    save(&dir.join("paused.json"), &paused.to_string());
    fs::File::options()
        .write(true)
        .open(dir.join("paused.json"))
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(100))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    fs::remove_file(dir.join("paused.json")).unwrap();
    let before = h.read().unwrap();
    let mut entry = before.entry.clone().unwrap();
    entry["args"] = json!(["first-config-change"]);
    hosts.write(&h, &before, Some(&entry)).unwrap();
    hosts.refresh_probed();
    receipt["started_ms"] = json!(
        hosts.settings["codex"]["reload_changed"]["at_ms"]
            .as_u64()
            .unwrap()
            + 1
    );
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    receipt["host_tools_hash"] = json!("b".repeat(64));
    save(&dir.join("old.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["reason"], "tools");
    let token = hosts.state(&h)["reload"]["token"].clone();
    hosts
        .action("confirm-reload", &json!({"id":"codex","token":token}))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    fs::remove_file(dir.join("old.json")).unwrap();
    receipt["host_tools_hash"] = json!("a".repeat(64));
    receipt["runtime_version"] = json!("0.21.0");
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "updating");
    // Long after the install, an old runtime is stuck rather than waiting to swap.
    let metadata = hosts.context.home.join(
        ".local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave-0.22.0.dist-info",
    );
    use std::time::Duration;
    let installed_at = |age: u64| {
        // Windows opens a directory only with backup semantics, and needs attribute-write access.
        #[cfg(windows)]
        let directory = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .access_mode(0x0100) // FILE_WRITE_ATTRIBUTES
                .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
                .open(&metadata)
        };
        #[cfg(not(windows))]
        let directory = fs::File::open(&metadata);
        directory
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
    };
    let now_ms = || {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    let started = receipt["started_ms"].clone();
    installed_at(3600);
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    assert_eq!(hosts.state(&h)["reload"]["reason"], "stale");
    // A connection on the new install that has not shown its version does not hide that.
    let mut fresh = receipt.clone();
    fresh["runtime_version"] = Value::Null;
    fresh["started_ms"] = json!(now_ms());
    save(&dir.join("fresh.json"), &fresh.to_string());
    assert_eq!(hosts.state(&h)["reload"]["reason"], "stale");
    fs::remove_file(dir.join("fresh.json")).unwrap();
    // Without a version long after the install, a relay started before it has not swapped:
    // before v0.31.0 it cannot read the version from 2026-07-28 hosts.
    receipt["runtime_version"] = Value::Null;
    receipt["started_ms"] = json!(now_ms() - 7_200_000);
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    assert_eq!(hosts.state(&h)["reload"]["reason"], "relay");
    installed_at(0);
    assert_eq!(hosts.state(&h)["reload"]["state"], "updating");
    // Started after the install, or at an unreadable time, the version is only unknown.
    receipt["started_ms"] = json!(now_ms() + 1);
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "unknown");
    installed_at(3600);
    receipt["started_ms"] = Value::Null;
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "unknown");
    installed_at(0);
    receipt["started_ms"] = started;
    receipt["runtime_version"] = json!("0.22.0");
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");

    // Agents read the Skill from disk on use; no part of it, front matter included, needs a
    // reload.
    let skill = h.skill.as_ref().unwrap().join("SKILL.md");
    let text = fs::read_to_string(&skill).unwrap();
    save(&skill, &format!("{text}\nchanged instructions\n"));
    hosts.refresh_probed();
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    save(
        &skill,
        &text.replacen("description:", "description: changed", 1),
    );
    hosts.refresh_probed();
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    // The session from before a config change is stale; a new one started after it clears the
    // reload without a manual confirmation.
    let before = h.read().unwrap();
    let mut entry = before.entry.clone().unwrap();
    entry["args"] = json!(["changed"]);
    hosts.write(&h, &before, Some(&entry)).unwrap();
    hosts.refresh_probed();
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    receipt["started_ms"] = json!(
        hosts.settings["codex"]["reload_changed"]["at_ms"]
            .as_u64()
            .unwrap()
            + 1
    );
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    let token = hosts.state(&h)["reload"]["token"].clone();
    hosts
        .action("confirm-reload", &json!({"id":"codex","token":token}))
        .unwrap();
    let alternate = hosts.context.home.join("alternate-codex");
    fs::rename(h.config.parent().unwrap(), &alternate).unwrap();
    hosts
        .action("root", &json!({"id":"codex","path":alternate}))
        .unwrap();
    let h = host(&hosts, "codex");
    receipt["started_ms"] = json!(
        hosts.settings["codex"]["reload_changed"]["at_ms"]
            .as_u64()
            .unwrap()
            + 1
    );
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    fs::write(dir.join("new.json"), "invalid").unwrap();
    // Records from older token formats, including a pending Skill change, are ignored.
    hosts.settings["codex"] = json!({
        "reload_confirmed":{"token":"legacy-version-fingerprint","at":"old"},
        "reload_observed":{"token":"whole-skill-tree","skill":{}},
        "reload_changed":{"v":2,"token":"whole-skill-tree","at_ms":0,"skill":true},
    });
    assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
    save(&dir.join("new.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(dir.join("new.json"))
        .unwrap();
    file.set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "idle");
}

#[test]
fn missing_commands_are_repaired_across_formats_without_changing_host_policy() {
    for id in IDS {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        save(&h.config, &seed(&h));
        assert_eq!(
            hosts.action("connect", &json!({"id":id})).unwrap()["ok"],
            true
        );
        let before = h.read().unwrap();
        let mut entry = before.entry.clone().unwrap();
        let old = hosts.context.home.join("old-bin/agentnave-mcp");
        h.set_command(&mut entry, &old);
        entry["args"] = json!(["--keep"]);
        entry["env"]["CUSTOM"] = json!("preserve");
        entry["disabled"] = json!(true);
        entry["allowedTools"] = json!(["wait_agent"]);
        save(&h.config, &before.render(Some(&entry)).unwrap());
        let mut expected = entry;
        h.set_command(&mut expected, &hosts.launcher().unwrap());
        hosts.refresh();
        let after = h.read().unwrap();
        assert_eq!(after.entry, Some(expected), "{id}");
        assert_eq!(after.rest, before.rest, "{id}");
        let bytes = fs::read(&h.config).unwrap();
        let backups = backup_paths(&hosts.context.app, CONFIG_BACKUP).len();
        hosts.refresh();
        assert_eq!(fs::read(&h.config).unwrap(), bytes);
        assert_eq!(
            backup_paths(&hosts.context.app, CONFIG_BACKUP).len(),
            backups
        );
    }
}

#[test]
fn optimistic_conflicts_rebase_then_stop_and_conditional_rollback_preserves_new_writes() {
    use transaction::{Phase, Target, edit_observed, rollback_if_ours};
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    save(&h.config, r#"{"counter":0}"#);
    let mut times = 0;
    let target = || Target {
        path: &h.config,
        home: &hosts.context.home,
        app: &hosts.context.app,
        id: h.id,
        format: h.format,
        key: h.key,
    };
    let result = edit_observed(
        target(),
        |entry| {
            *entry = Some(json!({"command":"/x"}));
            Ok(())
        },
        |phase, path| {
            if matches!(phase, Phase::Prepared) {
                times += 1;
                save(path, &json!({"counter":times}).to_string());
            }
        },
    );
    assert!(result.unwrap_err().starts_with("conflict:"));
    assert_eq!(times, 3);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&h.config).unwrap()).unwrap()["counter"],
        3
    );
    let result = edit_observed(
        target(),
        |entry| {
            *entry = Some(json!({"command":"/x"}));
            Ok(())
        },
        |phase, path| {
            if matches!(phase, Phase::Written) {
                save(path, r#"{"external":"new"}"#);
            }
        },
    );
    assert!(result.unwrap_err().starts_with("conflict:"));
    assert!(rollback_if_ours(&h.config, &hosts.context.home, b"ours", Some(b"old")).is_err());
    assert_eq!(
        fs::read_to_string(&h.config).unwrap(),
        r#"{"external":"new"}"#
    );
    save(&h.config, "ours");
    rollback_if_ours(&h.config, &hosts.context.home, b"ours", Some(b"old")).unwrap();
    assert_eq!(fs::read_to_string(&h.config).unwrap(), "old");
}

#[test]
fn roots_follow_user_env_shell_default_and_persist_first_seen_only_on_actions() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    assert_eq!(h.root.source, "default");
    assert!(!hosts.context.app.exists());
    let shellroot = hosts.context.home.join("from-shell");
    hosts.shell = Some(Ok(BTreeMap::from([(
        "PI_CODING_AGENT_DIR".into(),
        shellroot.display().to_string(),
    )])));
    assert_eq!(host(&hosts, "pi").root.source, "shell");
    let envroot = hosts.context.home.join("from-env");
    hosts
        .context
        .env
        .insert("PI_CODING_AGENT_DIR".into(), envroot.display().to_string());
    assert_eq!(host(&hosts, "pi").root.path, envroot);
    let userroot = hosts.context.home.join("from-user");
    let result = hosts
        .action("root", &json!({"id":"pi","path":userroot}))
        .unwrap();
    assert_eq!(result["host"]["root"]["source"], "user");
    hosts.refresh();
    let path = hosts.context.app.join("hosts.json");
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(saved["pi"]["first_seen"].as_str().unwrap().ends_with('Z'));
    let restart = Hosts::new(Context {
        home: hosts.context.home.clone(),
        app: hosts.context.app.clone(),
        project: hosts.context.project.clone(),
        env: hosts.context.env.clone(),
        search: hosts.context.search.clone(),
        applications: hosts.context.applications.clone(),
    });
    assert_eq!(host(&restart, "pi").root.source, "user");
    hosts
        .action("root", &json!({"id":"pi","path":null}))
        .unwrap();
    assert_eq!(host(&hosts, "pi").root.source, "env");
    assert!(
        hosts
            .action("root", &json!({"id":"pi","path":"relative"}))
            .is_err()
    );
}

#[test]
#[cfg(windows)]
fn windows_has_no_login_shell_and_finds_uv_tools_under_appdata() {
    let (_temp, mut hosts) = fixture();
    hosts.context.env.remove("SHELL");
    let appdata = hosts.context.home.join("Roaming");
    hosts
        .context
        .env
        .insert("APPDATA".into(), appdata.display().to_string());
    let (shell, dirs) = probe_tools(&hosts.context);
    assert_eq!(shell, Ok(BTreeMap::new()));
    assert_eq!(dirs.tools, appdata.join("uv").join("tools"));
    fs::create_dir_all(appdata.join("uv").join("data")).unwrap();
    let (_, dirs) = probe_tools(&hosts.context);
    assert_eq!(dirs.tools, appdata.join("uv").join("data").join("tools"));
}

#[test]
#[cfg(unix)]
fn shell_probe_is_whitelisted_cached_and_failed_probe_blocks_only_automatic_writes() {
    let (_temp, mut hosts) = fixture();
    let script = hosts.context.home.join("fake-shell");
    let root = hosts.context.home.join("shell-root");
    let marker = hosts.context.home.join("executed");
    let mut output = String::from("#!/bin/sh\n");
    for name in roots::VARIABLES {
        let value = if *name == "CODEX_HOME" {
            root.to_str().unwrap()
        } else if *name == "GROK_HOME" {
            "relative-path"
        } else {
            ""
        };
        output.push_str(&format!("printf '{name}\\0{value}\\0'\n"));
    }
    output.push_str("printf 'HTTPS_PROXY\\0http://127.0.0.1:7890\\0'\n");
    output.push_str(&format!(
        "printf 'BAD\\0$(touch {})\\0'\n",
        marker.display()
    ));
    executable_file(&script, &output);
    hosts
        .context
        .env
        .insert("SHELL".into(), script.display().to_string());
    let values = roots::probe(&hosts.context).unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values["CODEX_HOME"], root.to_str().unwrap());
    assert_eq!(values["HTTPS_PROXY"], "http://127.0.0.1:7890");
    assert!(!marker.exists());
    executable_file(&script, "#!/bin/sh\nexit 0\n");
    assert!(roots::probe(&hosts.context).is_err());
    executable_file(&script, "#!/bin/sh\nexit 1\n");
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{"command":hosts.launcher()}}}).to_string(),
    );
    let before = fs::read(&h.config).unwrap();
    hosts.refresh();
    assert_eq!(fs::read(&h.config).unwrap(), before);
    assert_eq!(
        hosts.state(&host(&hosts, "workbuddy"))["attention"]["code"],
        "root_unconfirmed"
    );
    hosts.context.env.insert(
        "CODEBUDDY_CONFIG_DIR".into(),
        h.root.path.display().to_string(),
    );
    hosts.refresh();
    assert_eq!(fs::read(&h.config).unwrap(), before);
    assert_eq!(
        hosts.state(&host(&hosts, "workbuddy"))["attention"]["code"],
        "root_unconfirmed"
    );
    assert_eq!(
        hosts.action("connect", &json!({"id":"workbuddy"})).unwrap()["ok"],
        true
    );
    assert!(!marker.exists());
}

#[test]
fn optional_skill_switch_installs_when_on_and_removes_any_copy_when_off() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    let review = h.optional("agentnave-review").unwrap();
    let state = |hosts: &Hosts| skill::state(&review, &hosts.context.home, skill::OPTIONAL[0].1);
    let switch = |enabled: bool| json!({"id":"pi","skill":"agentnave-review","enabled":enabled});
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{
            "command":hosts.launcher(),
            "env":{"AGENTNAVE_HOST":"pi"}
        }}})
        .to_string(),
    );
    hosts.refresh();
    assert_eq!(state(&hosts)["state"], "missing");
    assert_eq!(
        hosts.state(&h)["skills"]["agentnave-review"]["enabled"],
        false
    );

    let on = hosts.action("skill", &switch(true)).unwrap();
    assert_eq!(on["ok"], true);
    assert_eq!(state(&hosts)["state"], "app");
    assert_eq!(on["host"]["skills"]["agentnave-review"]["enabled"], true);
    assert!(review.join("references/reviewers.md").is_file());

    hosts.action("skill", &switch(false)).unwrap();
    assert_eq!(state(&hosts)["state"], "missing");

    save(&review.join("SKILL.md"), "mine");
    assert_eq!(state(&hosts)["state"], "external");
    let off = hosts.action("skill", &switch(false)).unwrap();
    assert_eq!(off["ok"], true);
    assert_eq!(state(&hosts)["state"], "missing");
    save(&review.join("SKILL.md"), "mine");
    hosts.action("skill", &switch(true)).unwrap();
    assert_eq!(state(&hosts)["state"], "app");
    let unknown = hosts
        .action("skill", &json!({"id":"pi","skill":"nope","enabled":true}))
        .unwrap();
    assert_eq!(unknown["ok"], false);

    let catalog = skill_catalog();
    assert_eq!(catalog["skills"][0]["name"], "agentnave-review");
    assert!(
        catalog["skills"][0]["description"]
            .as_str()
            .unwrap()
            .contains("评审")
    );
}

#[test]
fn optional_skill_off_keeps_a_copy_another_host_shares_and_wants() {
    let (temp, mut hosts) = fixture();
    let shared = temp.path().canonicalize().unwrap().join("buddy");
    hosts
        .context
        .env
        .insert("CODEBUDDY_CONFIG_DIR".into(), shared.display().to_string());
    let path = host(&hosts, "codebuddy")
        .optional("agentnave-review")
        .unwrap();
    assert_eq!(
        host(&hosts, "workbuddy").optional("agentnave-review"),
        Some(path.clone())
    );
    let switch =
        |id: &str, enabled: bool| json!({"id":id,"skill":"agentnave-review","enabled":enabled});
    hosts.action("skill", &switch("codebuddy", true)).unwrap();
    hosts.action("skill", &switch("workbuddy", true)).unwrap();
    let embedded = skill::OPTIONAL[0].1;
    skill::install(
        &path,
        &hosts.context.app,
        "codebuddy",
        &hosts.context.home,
        embedded,
    )
    .unwrap();

    let off = hosts.action("skill", &switch("workbuddy", false)).unwrap();
    assert!(off["message"].as_str().unwrap().contains("共用"));
    hosts.action("remove", &json!({"id":"workbuddy"})).unwrap();
    assert_eq!(
        skill::state(&path, &hosts.context.home, embedded)["state"],
        "app"
    );
    hosts.action("skill", &switch("codebuddy", false)).unwrap();
    assert_eq!(
        skill::state(&path, &hosts.context.home, embedded)["state"],
        "missing"
    );
}

#[test]
fn refresh_installs_missing_skill_only_for_managed_host_entry() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    let path = h.skill.as_ref().unwrap();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");

    hosts.refresh();
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["state"],
        "missing"
    );

    save(
        &h.config,
        &json!({"mcpServers":{"agentnave":{
            "command":hosts.launcher(),
            "env":{"AGENTNAVE_HOST":"pi"}
        }}})
        .to_string(),
    );
    hosts.refresh();
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["state"],
        "app"
    );
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["version"],
        VERSION
    );
    assert_eq!(hosts.state(&h)["status"], "connected");
}

#[test]
fn refresh_adds_a_required_skill_to_a_host_connected_before_it_existed() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    let (name, embedded) = skill::REQUIRED[0];
    let required = h.optional(name).unwrap();
    let state =
        |hosts: &Hosts| skill::state(&required, &hosts.context.home, embedded)["state"].clone();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");

    // The manager is an App copy but the host entry is not the App's, as after an update.
    skill::install(
        h.skill.as_ref().unwrap(),
        &hosts.context.app,
        "pi",
        &hosts.context.home,
        skill::MANAGER,
    )
    .unwrap();
    assert_eq!(state(&hosts), "missing");
    hosts.refresh();
    assert_eq!(state(&hosts), "app");
}

#[test]
#[cfg(unix)]
fn connect_skill_failure_keeps_manual_root_for_refresh_retry() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    let path = h.skill.as_ref().unwrap();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    hosts.shell = Some(Err("shell probe failed".into()));
    save(path.parent().unwrap(), "not a directory");

    let result = hosts.action("connect", &json!({"id":"pi"})).unwrap();
    assert_eq!(result["ok"], false);
    assert!(h.read().unwrap().entry.is_some());
    assert!(hosts.manual_roots.contains("pi"));

    fs::remove_file(path.parent().unwrap()).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    hosts.refresh_probed();
    let state = hosts.state(&h);
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["state"],
        "app",
        "{state}"
    );
}

#[test]
fn skill_ownership_links_and_refresh_replace_other_copies_after_a_backup() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    let path = h.skill.as_ref().unwrap();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    hosts.action("connect", &json!({"id":"pi"})).unwrap();
    let manifest = path.join(".agentnave-app-manifest.json");
    for keep_entry in [true, false] {
        if !keep_entry {
            fs::remove_file(&h.config).unwrap();
        }
        save(&path.join(".agentnave-app"), "old");
        let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["version"] = json!("old");
        save(&manifest, &value.to_string());
        hosts.refresh();
        assert_eq!(
            skill::state(path, &hosts.context.home, skill::MANAGER)["version"],
            VERSION
        );
        assert_eq!(h.config.exists(), keep_entry);
    }
    save(&path.join("SKILL.md"), "external");
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["state"],
        "external"
    );
    // More files than the backup budget: the whole copy is moved, so none is dropped.
    for n in 0..40 {
        save(&path.join(format!("notes/{n}.md")), "mine");
    }
    let backups_dir = hosts.context.app.join("backups");
    let backups = || {
        fs::read_dir(&backups_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().contains("-skill-pi-agentnave-manager"))
            .any(|p| {
                let p = p.join("agentnave-manager");
                fs::read_to_string(p.join("SKILL.md")).ok().as_deref() == Some("external")
                    && (0..40).all(|n| p.join(format!("notes/{n}.md")).is_file())
            })
    };
    hosts.action("connect", &json!({"id":"pi"})).unwrap();
    assert_eq!(
        skill::state(path, &hosts.context.home, skill::MANAGER)["state"],
        "app"
    );
    assert!(backups());
    save(&path.join("SKILL.md"), "external");
    hosts.action("remove", &json!({"id":"pi"})).unwrap();
    assert!(!path.exists());
    #[cfg(unix)]
    {
        let other = host(&hosts, "claude");
        let link = other.skill.unwrap();
        let outside = hosts.context.home.join("elsewhere");
        save(&outside.join("SKILL.md"), "elsewhere");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        skill::install(
            &link,
            &hosts.context.app,
            "claude",
            &hosts.context.home,
            skill::MANAGER,
        )
        .unwrap();
        assert!(!link.is_symlink());
        assert_eq!(
            skill::state(&link, &hosts.context.home, skill::MANAGER)["state"],
            "app"
        );
        skill::remove(
            &link,
            &hosts.context.app,
            "claude",
            &hosts.context.home,
            skill::MANAGER,
        )
        .unwrap();
        assert!(!link.exists());
        assert_eq!(
            fs::read_to_string(outside.join("SKILL.md")).unwrap(),
            "elsewhere"
        );
    }
}

#[test]
fn get_is_read_only_onboarding_uses_cache_and_post_routes_require_action_header() {
    let (_temp, hosts) = fixture();
    let app = hosts.context.app.clone();
    let root = hosts.context.home.join("data");
    let reader = Reader::new(root.clone());
    reader.configure_hosts(hosts.context, hosts.shell.unwrap());
    for path in ["/app/hosts", "/app/onboarding"] {
        assert_eq!(reader.request("GET", path, None, b"").status, 200);
    }
    assert!(!app.exists());
    assert!(!root.exists());
    for path in [
        "/app/hosts/refresh",
        "/app/hosts/root",
        "/app/hosts/confirm-reload",
        "/app/hosts/reveal-backups",
        "/app/onboarding/dismiss",
        "/app/onboarding/reopen",
    ] {
        assert_eq!(reader.request("GET", path, None, b"").status, 405);
        assert_eq!(reader.request("POST", path, None, b"{}").status, 403);
    }
    reader.request("POST", "/app/onboarding/dismiss", Some("1"), b"{}");
    assert!(app.join("onboarding.dismissed").exists());
    reader.request("POST", "/app/onboarding/reopen", Some("1"), b"{}");
    assert!(!app.join("onboarding.dismissed").exists());
    save(&root.join("members.json"), "{}");
    let value: Value =
        serde_json::from_slice(&reader.request("GET", "/app/onboarding", None, b"").body).unwrap();
    assert_eq!(value["steps"][2]["done"], true);
}

#[test]
fn config_and_skill_backups_are_each_bounded_at_thirty() {
    let (_temp, hosts) = fixture();
    let (app, home) = (&hosts.context.app, &hosts.context.home);
    let path = home.join("config");
    backup_bytes(
        app,
        CONFIG_BACKUP,
        "codex@/profiles\\work/side",
        &path,
        b"nested",
        home,
    )
    .unwrap();
    // A file from before the split counts as a config backup.
    fs::create_dir_all(app.join("backups")).unwrap();
    fs::write(
        app.join("backups/00000000000000000001-pi-config-legacy"),
        "legacy",
    )
    .unwrap();
    for n in 0..5 {
        backup_bytes(
            app,
            CONFIG_BACKUP,
            "pi",
            &path,
            n.to_string().as_bytes(),
            home,
        )
        .unwrap();
    }
    for n in 0..35 {
        save(&path, &n.to_string());
        backup(app, "pi", &path, home).unwrap();
    }
    let skills = backup_paths(app, SKILL_BACKUP);
    assert_eq!(skills.len(), 30);
    assert_eq!(fs::read_to_string(&skills[0]).unwrap(), "5");
    let configs = backup_paths(app, CONFIG_BACKUP);
    assert_eq!(configs.len(), 7);
    assert_eq!(fs::read_to_string(&configs[0]).unwrap(), "legacy");
}

#[test]
fn json_takeover_preserves_nested_indentation_order_and_untouched_lines() {
    let raw = "{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"other\": {\"command\": \"keep\"},\n        \"agentnave\": {\n            \"command\": \"/old\",\n            \"print\": true,\n            \"args\": [],\n            \"env\": {\n                \"Z_LAST\": \"keep\",\n                \"A_FIRST\": \"keep too\"\n            },\n            \"disabled\": false\n        }\n    },\n    \"tail\": [1, 2]\n}\n";
    let expected = raw.replace(
        "                \"Z_LAST\"",
        "                \"AGENTNAVE_HOST\": \"workbuddy\",\n                \"Z_LAST\"",
    );
    assert_scoped_takeover(raw, &expected, Format::Json, "mcpServers", "workbuddy");
    let doc = Document::parse(
        "{\n    \"mcpServers\": {\n        \"other\": {}\n    }\n}\n",
        Format::Json,
        "mcpServers",
    )
    .unwrap();
    let entry = json!({"command":"/new","args":[],"env":{"AGENTNAVE_HOST":"workbuddy"}});
    let written = doc.render(Some(&entry)).unwrap();
    doc.verify(&written, Some(&entry)).unwrap();
    assert!(
        written.contains("        \"agentnave\": {\n            \"args\": []"),
        "{written}"
    );
    assert!(written.contains("            \"env\": {\n                \"AGENTNAVE_HOST\": \"workbuddy\"\n            }"), "{written}");
}

#[test]
fn json_removing_the_only_entry_and_reconnecting_restores_the_file() {
    let raw = "{\n  \"mcpServers\": {\n    \"agentnave\": {\n      \"args\": [],\n      \"command\": \"/new\",\n      \"env\": {\n        \"AGENTNAVE_HOST\": \"pi\"\n      }\n    }\n  }\n}";
    let removed = Document::parse(raw, Format::Json, "mcpServers")
        .unwrap()
        .render(None)
        .unwrap();
    assert_eq!(removed, "{\n  \"mcpServers\": {\n  }\n}");
    let entry = json!({"args":[],"command":"/new","env":{"AGENTNAVE_HOST":"pi"}});
    let restored = Document::parse(&removed, Format::Json, "mcpServers")
        .unwrap()
        .render(Some(&entry))
        .unwrap();
    assert_eq!(restored, raw);
    let inline = Document::parse(
        "{\"mcpServers\": {\"agentnave\": {\"command\": \"/x\"}}}",
        Format::Json,
        "mcpServers",
    )
    .unwrap();
    assert_eq!(inline.render(None).unwrap(), "{\"mcpServers\": {}}");
}

#[test]
fn toml_takeover_preserves_table_positions_key_order_and_comments() {
    let raw = "# global\nmodel = 'keep'\n\n[marketplaces.openai-bundled]\nsource = 'keep'\n\n[mcp_servers.other]\ncommand = 'keep'\n\n# user registration\n[mcp_servers.agentnave]\ncommand = \"/old\" # launcher\nargs = []\nenabled = true\n\n[mcp_servers.agentnave.env]\nZ_LAST = 'keep'\nA_FIRST = 'keep too'\n\n[plugins.example]\nenabled = true\n";
    let expected = raw.replace(
        "A_FIRST = 'keep too'\n",
        "A_FIRST = 'keep too'\nAGENTNAVE_HOST = \"codex\"\n",
    );
    assert_scoped_takeover(raw, &expected, Format::Toml, "mcp_servers", "codex");
    let doc = Document::parse(
        "[marketplaces.example]\nsource = 'keep'\n",
        Format::Toml,
        "mcp_servers",
    )
    .unwrap();
    let entry = json!({"command":"/new","args":[],"env":{"AGENTNAVE_HOST":"codex"}});
    let written = doc.render(Some(&entry)).unwrap();
    doc.verify(&written, Some(&entry)).unwrap();
    assert!(written.starts_with(&doc.raw));
    assert!(
        written.contains("[mcp_servers.agentnave.env]\nAGENTNAVE_HOST = \"codex\"\n"),
        "{written}"
    );
}

#[test]
fn yaml_takeover_preserves_block_env_indentation_and_comments() {
    let raw = "model: keep\nmcp_servers: # registrations\n  other:\n    command: keep\n  agentnave: # managed entry\n    command: \"/old\" # launcher\n    args: []\n    env: # preserve block\n      Z_LAST: 'keep'\n      A_FIRST: \"keep too\"\n    enabled: true\ntail: 'keep'\n";
    let expected = raw.replace(
        "      A_FIRST: \"keep too\"\n",
        "      A_FIRST: \"keep too\"\n      AGENTNAVE_HOST: \"hermes\"\n",
    );
    assert_scoped_takeover(raw, &expected, Format::Yaml, "mcp_servers", "hermes");
    assert_scoped_takeover(
        &raw.replace("  ", "    "),
        &expected.replace("  ", "    "),
        Format::Yaml,
        "mcp_servers",
        "hermes",
    );
    let doc = Document::parse(
        "mcp_servers:\n  other:\n    command: keep\n",
        Format::Yaml,
        "mcp_servers",
    )
    .unwrap();
    let entry = json!({"command":"/new","args":[],"env":{"AGENTNAVE_HOST":"hermes"}});
    let written = doc.render(Some(&entry)).unwrap();
    doc.verify(&written, Some(&entry)).unwrap();
    assert!(
        written.contains("    env:\n      AGENTNAVE_HOST: \"hermes\"\n"),
        "{written}"
    );
}

fn assert_scoped_takeover(raw: &str, expected: &str, format: Format, key: &'static str, id: &str) {
    for eol in ["\n", "\r\n"] {
        let raw = raw.replace('\n', eol);
        let expected = expected.replace('\n', eol);
        assert_scoped_takeover_bytes(&raw, &expected, format, key, id);
    }
}

#[test]
fn dsh_patch_list_round_trips_and_stays_a_list_dsh_can_boot() {
    let entry =
        json!({"serverName":"agentnave","transport":"stdio","command":"/old","env":{"KEEP":"x"}});
    for raw in [
        "",
        "# mine\n[]\n",
        "[] # 暂无补丁\n",
        "- id: hmr\n  disabled: true",
    ] {
        let doc = Document::parse(raw, Format::DshPatch, "insert").unwrap();
        assert!(doc.entry.is_none());
        let written = doc.render(Some(&entry)).unwrap();
        doc.verify(&written, Some(&entry)).unwrap();
        assert!(
            written.contains("- insert:\n    - id: agentnave\n      name: \"@deepseek-ai/dsh-mcp-client\"\n      config:\n        command: \"/old\"\n"),
            "{written}"
        );
        let expected = written.replace(
            "          KEEP: \"x\"\n",
            "          KEEP: \"x\"\n          AGENTNAVE_HOST: \"deepseek\"\n",
        );
        assert_scoped_takeover(&written, &expected, Format::DshPatch, "insert", "deepseek");
        // An empty file does not boot dsh, so removing the last item leaves an empty list.
        let after = Document::parse(&written, Format::DshPatch, "insert").unwrap();
        let removed = after.render(None).unwrap();
        after.verify(&removed, None).unwrap();
        let restored = match raw {
            "" => "[]\n",
            "[] # 暂无补丁\n" => "# 暂无补丁\n[]\n",
            "- id: hmr\n  disabled: true" => "- id: hmr\n  disabled: true\n",
            _ => raw,
        };
        assert_eq!(removed, restored);
    }
    for raw in [
        "- insert:\n    - id: agentnave\n      config: {}\n    - id: other\n",
        "[{id: hmr}]\n",
        // A whole-list indent is valid YAML, but appending a column-0 item would add a root.
        "  - id: hmr\n    disabled: true\n",
        "\u{feff}- id: hmr\n",
        "---\n- id: hmr\n",
        // Another plugin's row named agentnave is not AgentNave's.
        "- insert:\n    - id: agentnave\n      name: other\n      config:\n        command: x\n",
    ] {
        assert!(
            Document::parse(raw, Format::DshPatch, "insert").is_err(),
            "{raw}"
        );
    }
}

#[test]
fn yaml_empty_env_without_final_newline_gets_a_separate_host_line() {
    let raw = "mcp_servers:\n  agentnave:\n    command: \"/old\"\n    env:";
    let expected = format!("{raw}\n      AGENTNAVE_HOST: \"hermes\"\n");
    assert_scoped_takeover(raw, &expected, Format::Yaml, "mcp_servers", "hermes");
}

fn assert_scoped_takeover_bytes(
    raw: &str,
    expected: &str,
    format: Format,
    key: &'static str,
    id: &str,
) {
    let doc = Document::parse(raw, format, key).unwrap();
    let mut entry = doc.entry.clone().unwrap();
    entry["env"]["AGENTNAVE_HOST"] = json!(id);
    let written = doc.render(Some(&entry)).unwrap();
    doc.verify(&written, Some(&entry)).unwrap();
    assert_eq!(written, expected);
    let after = Document::parse(&written, format, key).unwrap();
    assert_eq!(after.render(Some(&entry)).unwrap(), written);
    entry["command"] = json!("/new");
    let changed = after.render(Some(&entry)).unwrap();
    after.verify(&changed, Some(&entry)).unwrap();
    assert_eq!(changed, written.replace("\"/old\"", "\"/new\""));
}

#[test]
fn jsonc_and_quoted_yaml_values_survive_and_ambiguous_yaml_is_rejected() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "pi");
    let raw = "{\n// keep\n\"mcpServers\": { /* preserve */ \"other\": {\"type\":\"remote\",}, },\n\"future\": [1,2,],\n}";
    let doc = Document::parse(raw, h.format, h.key).unwrap();
    let entry = json!({"command":"/x","args":[],"env":{"AGENTNAVE_HOST":"pi"}});
    let changed = doc.render(Some(&entry)).unwrap();
    doc.verify(&changed, Some(&entry)).unwrap();
    assert!(changed.contains("// keep"));
    assert!(changed.contains("/* preserve */"));
    assert!(changed.contains("\"future\": [1,2,]"));
    let after = Document::parse(&changed, h.format, h.key).unwrap();
    after.verify(&after.render(None).unwrap(), None).unwrap();
    let raw = "mcp_servers:\n  agentnave:\n    command: '/path # literal' # comment\n    env:\n      QUOTED: 'a # b'\n      DOUBLE: \"x # y\"\n";
    let doc = Document::parse(raw, Format::Yaml, "mcp_servers").unwrap();
    let mut entry = doc.entry.clone().unwrap();
    entry["env"]["AGENTNAVE_HOST"] = json!("hermes");
    let changed = doc.render(Some(&entry)).unwrap();
    doc.verify(&changed, Some(&entry)).unwrap();
    assert_eq!(
        Document::parse(&changed, Format::Yaml, "mcp_servers")
            .unwrap()
            .entry
            .unwrap()["env"]["QUOTED"],
        "a # b"
    );
    for raw in [
        "mcp_servers: {agentnave: {command: x}}",
        "mcp_servers:\n  agentnave: &shared\n    command: x\n",
        "mcp_servers:\n  agentnave:\n    env: *shared\n",
    ] {
        assert!(Document::parse(raw, Format::Yaml, "mcp_servers").is_err());
    }
}

#[test]
fn inline_toml_and_commented_yaml_maps_are_updated_without_losing_siblings() {
    let raw = "# keep\nmcp_servers = { other = { command = 'keep' }, agentnave = { command = '/old', env = { KEEP = 'yes' } } }\n";
    let doc = Document::parse(raw, Format::Toml, "mcp_servers").unwrap();
    let mut entry = doc.entry.clone().unwrap();
    entry["env"]["AGENTNAVE_HOST"] = json!("codex");
    let changed = doc.render(Some(&entry)).unwrap();
    doc.verify(&changed, Some(&entry)).unwrap();
    let after = Document::parse(&changed, Format::Toml, "mcp_servers").unwrap();
    after.verify(&after.render(None).unwrap(), None).unwrap();
    let raw = "mcp_servers: # keep header\n  agentnave: # target\n    command: /old\n    env: # keep values\n      KEEP: yes-string\n";
    let doc = Document::parse(raw, Format::Yaml, "mcp_servers").unwrap();
    let mut entry = doc.entry.clone().unwrap();
    entry["env"]["AGENTNAVE_HOST"] = json!("hermes");
    doc.verify(&doc.render(Some(&entry)).unwrap(), Some(&entry))
        .unwrap();
}

#[test]
fn duplicate_json_target_keys_and_truncated_strings_are_rejected() {
    for raw in [
        r#"{"mcpServers":{},"mcpServers":{}}"#,
        r#"{"mcpServers":{"agentnave":{},"agentnave":{}}}"#,
        r#"{"mcpServers":{"agentnave":{"command":"x","command":"y"}}}"#,
        r#"{"mcpServers":{"agentnave":{"env":{"KEY":"a","KEY":"b"}}}}"#,
        "{\"truncated\\",
    ] {
        assert!(Document::parse(raw, Format::Json, "mcpServers").is_err());
    }
}

#[test]
fn an_equivalent_root_after_upgrade_records_no_reload() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    // Only records from before the MCP-only token, and a session that was already running.
    hosts.settings["codex"] = json!({
        "reload_observed":{"v":3,"token":"old"},
        "reload_changed":{"v":3,"token":"old","at_ms":0},
    });
    save(
        &hosts.context.home.join(".agentnave/connections/old.json"),
        &json!({"version":1,"host":"codex","started_ms":1}).to_string(),
    );
    hosts
        .action("root", &json!({"id":"codex","path":null}))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "unknown");
}

#[test]
fn zcode_nested_mcp_preserves_siblings_and_rejects_ambiguous_maps() {
    let raw = "{\n  \"other\": true,\n  \"mcp\": {\"policy\": \"keep\", \"servers\": {\"other\": {\"command\": \"keep\"}}}\n}";
    let document = Document::parse(raw, Format::Zcode, "servers").unwrap();
    let entry = json!({"type":"stdio","command":"/new","env":{"AGENTNAVE_HOST":"zcode"}});
    let changed = document.render(Some(&entry)).unwrap();
    document.verify(&changed, Some(&entry)).unwrap();
    assert!(changed.contains("\"policy\": \"keep\""));
    let next = Document::parse(&changed, Format::Zcode, "servers").unwrap();
    let removed = next.render(None).unwrap();
    next.verify(&removed, None).unwrap();
    for bad in [
        "{\"mcp\":null}",
        "{\"mcp\":{\"servers\":[]}}",
        "{\"mcp\":{},\"mcp\":{}}",
        "{\"mcp\":{\"servers\":{},\"servers\":{}}}",
    ] {
        assert!(
            Document::parse(bad, Format::Zcode, "servers").is_err(),
            "{bad}"
        );
    }
}

#[test]
fn zcode_first_entry_creates_nested_servers_and_keeps_comments() {
    let entry = json!({"type":"stdio","command":"/new","args":[],"env":{"AGENTNAVE_HOST":"zcode"}});
    for raw in [
        "",
        "{\n  // keep comment\n  \"hooks\": {}\n}",
        "{\"mcp\": {\"policy\": \"keep\"}}",
    ] {
        let document = Document::parse(raw, Format::Zcode, "servers").unwrap();
        let changed = document.render(Some(&entry)).unwrap();
        document.verify(&changed, Some(&entry)).unwrap();
        let next = Document::parse(&changed, Format::Zcode, "servers").unwrap();
        assert_eq!(next.entry.as_ref(), Some(&entry), "{changed}");
        for kept in ["// keep comment", "\"policy\": \"keep\""] {
            assert!(!raw.contains(kept) || changed.contains(kept), "{changed}");
        }
        let removed = next.render(None).unwrap();
        next.verify(&removed, None).unwrap();
    }
}

#[test]
fn zcode_fallback_servers_block_only_a_first_server() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let fallback = home.join(".agents/mcp.json");
    let empty = Document::parse("{}", Format::Zcode, "servers").unwrap();
    assert_eq!(zcode_fallback(home, &empty), None);
    save(&fallback, "{\"mcpServers\": {}}");
    assert_eq!(zcode_fallback(home, &empty), None);
    save(
        &fallback,
        "{\"mcpServers\": {\"agentnave\": {\"command\": \"old\"}}}",
    );
    assert_eq!(zcode_fallback(home, &empty), None);
    save(
        &fallback,
        "{\n  // JSONC\n  \"mcpServers\": {\"other\": {\"command\": \"keep\"}}\n}",
    );
    assert_eq!(zcode_fallback(home, &empty), Some(fallback.clone()));
    // Once ZCode's own config names a server the fallback is already ignored.
    let own = Document::parse(
        "{\"mcp\": {\"servers\": {\"mine\": {\"command\": \"x\"}}}}",
        Format::Zcode,
        "servers",
    )
    .unwrap();
    assert_eq!(zcode_fallback(home, &own), None);
}

#[test]
fn an_entry_on_another_folder_than_its_cli_gets_its_own_row() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    fs::create_dir_all(hosts.context.applications[0].join("Claude.app")).unwrap();
    executable_file(
        &hosts.context.search[0].join("codex"),
        "#!/bin/sh\nexit 99\n",
    );
    save(&home.join(".codex/auth.json"), "{}");
    hosts.codex_homes = hosts.other_codex_homes();
    assert_eq!(hosts.registry().len(), IDS.len());

    let elsewhere = home.join("claude-work");
    hosts.shell = Some(Ok(BTreeMap::from([(
        "CLAUDE_CONFIG_DIR".into(),
        elsewhere.display().to_string(),
    )])));
    save(&home.join(".codex-work/auth.json"), "{}");
    save(&home.join(".codex-profiles/side/sessions/x"), "");
    save(&home.join(".codex-backups/old/archive.tar.gz"), "");
    hosts.codex_homes = hosts.other_codex_homes();
    assert_eq!(hosts.registry().len(), IDS.len() + 3);
    assert_eq!(
        host(&hosts, "claude").config,
        elsewhere.join(".claude.json")
    );

    let nested_id = format!(
        "codex@{}",
        Path::new(".codex-profiles").join("side").display()
    );
    for (id, boss, config) in [
        ("claude-desktop", "claude", home.join(".claude.json")),
        (
            "codex@.codex-work",
            "codex",
            home.join(".codex-work/config.toml"),
        ),
        (
            nested_id.as_str(),
            "codex",
            home.join(".codex-profiles/side/config.toml"),
        ),
    ] {
        if id.starts_with("codex@") {
            save(&config, "model = 'keep'\n");
        }
        let row = host(&hosts, id);
        assert_eq!(row.config, config);
        let result = hosts.action("connect", &json!({"id":id})).unwrap();
        assert_eq!(result["ok"], true, "{id}: {result}");
        assert_eq!(result["host"]["has_host_env"], true, "{id}: {result}");
        assert_eq!(result["host"]["boss"], boss);
        let entry = row.read().unwrap().entry.unwrap();
        assert_eq!(entry["env"]["AGENTNAVE_HOST"], boss);
    }
    assert!(host(&hosts, "claude").read().unwrap().entry.is_none());

    let homes = hosts.boss_homes();
    assert!(homes.claude.contains(&elsewhere) && homes.claude.contains(&home.join(".claude")));
    assert_eq!(homes.codex.len(), 3);
}

#[cfg(not(windows))]
#[test]
fn claude_desktop_is_found_by_its_folder_without_the_app() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    hosts.shell = Some(Ok(BTreeMap::from([(
        "CLAUDE_CONFIG_DIR".into(),
        home.join("claude-work").display().to_string(),
    )])));
    assert_eq!(hosts.registry().len(), IDS.len());
    let config = if cfg!(target_os = "macos") {
        "Library/Application Support"
    } else {
        ".config"
    };
    fs::create_dir_all(home.join(config).join("Claude-3p")).unwrap();
    assert_eq!(hosts.registry().len(), IDS.len() + 1);
}

#[cfg(unix)]
#[test]
fn codex_folders_beside_codex_home_count_once_each() {
    let (temp, _hosts) = fixture();
    let base = temp.path().canonicalize().unwrap();
    let (home, data) = (base.join("home"), base.join("data"));
    save(&data.join("main/auth.json"), "{}");
    save(&data.join("codex-work/auth.json"), "{}");
    save(&data.join("other/auth.json"), "{}");
    save(&home.join(".codex/auth.json"), "{}");
    std::os::unix::fs::symlink(data.join("codex-work"), home.join("codex-link")).unwrap();
    std::os::unix::fs::symlink(data.join("main"), home.join("codex-main")).unwrap();
    assert_eq!(
        split::codex_homes(&home, &data.join("main")),
        [home.join(".codex"), data.join("codex-work")]
    );
}
