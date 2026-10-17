use super::*;
use crate::data::Reader;
use tempfile::TempDir;

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
    for name in ["agentnave-mcp", "agentnave-studio-mcp"] {
        executable_file(
            &home.join(".local/share/uv/tools/agentnave/bin").join(name),
            "#!/bin/sh\nexit 99\n",
        );
    }
    let mut hosts = Hosts::new(context);
    hosts.shell = Some(Ok(BTreeMap::new()));
    (temp, hosts)
}
fn executable_file(path: &Path, text: &str) {
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
        Format::Json => format!("{{\n// keep comment\n\"unknown\": {{\"a\": [1,2]}},\n\"{}\": {{\"other\": {{\"command\": \"keep\"}}}}}}", host.key),
        Format::Toml => "# keep comment\nunknown = { a = [1, 2] }\n[mcp_servers.other]\ncommand = 'keep'\n".into(),
        Format::Yaml => "# keep comment\nunknown:\n  - untouched\nmcp_servers:\n  other:\n    command: keep\ntail: 'exact' # keep\n".into(),
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
        &json!({"mcpServers":{"agentnave":{"command":hosts.launcher("studio")}}}).to_string(),
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
    save(&h.config,&json!({"unknown":1,"mcpServers":{"agentnave":{"command":hosts.launcher("core"),"env":{}}}}).to_string());
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
fn external_skill_links_do_not_block_healthy_hosts_or_mcp_actions() {
    for linked_parent in [false, true] {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, "workbuddy");
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        save(
            &h.config,
            &json!({"mcpServers":{"agentnave":{"command":hosts.launcher("core")}}}).to_string(),
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
        assert_eq!(hosts.state(&h)["skill"]["state"], "external");
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
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_to_string(outside.join("SKILL.md")).unwrap(),
            "externally managed"
        );
    }
}

#[test]
fn service_detection_and_connect_share_shell_tool_directory_resolution() {
    for var in ["UV_TOOL_DIR", "XDG_DATA_HOME"] {
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
        assert_eq!(
            result["host"]["command"],
            json!(tools.join("agentnave/bin/agentnave-studio-mcp"))
        );
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
        assert_eq!(
            result["host"]["command"],
            json!(env_tools.join("agentnave/bin/agentnave-studio-mcp"))
        );
    }
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
fn revision_multilayer_removal_does_not_fall_back_to_another_registration() {
    let (_temp, mut hosts) = fixture();
    let global = hosts.context.home.join(".config/opencode/opencode.json");
    let custom = hosts.context.home.join("custom.json");
    let env_root = hosts.context.home.join("env-opencode");
    let user_root = hosts.context.home.join("user-opencode");
    let env_file = env_root.join("opencode.jsonc");
    let user_file = user_root.join("opencode.json");
    hosts
        .context
        .env
        .insert("OPENCODE_CONFIG".into(), custom.display().to_string());
    hosts
        .context
        .env
        .insert("OPENCODE_CONFIG_DIR".into(), env_root.display().to_string());
    hosts
        .action("root", &json!({"id":"opencode", "path":user_root}))
        .unwrap();
    executable_file(
        &hosts.context.search[0].join("opencode"),
        "#!/bin/sh\nexit 99\n",
    );
    for path in [&global, &custom, &env_file, &user_file] {
        save(path,&json!({"keep":true,"mcp":{"agentnave":{"command":[hosts.launcher("studio")],"environment":{}}}}).to_string());
    }
    let project = hosts.context.project.join("opencode.json");
    save(&project, r#"{"mcp":{"agentnave":{"command":["project"]}}}"#);
    let result = hosts.action("remove", &json!({"id":"opencode"})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains(global.to_str().unwrap())
    );
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains(custom.to_str().unwrap())
    );
    let state = hosts.refresh();
    let h = state["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == "opencode")
        .unwrap();
    assert_eq!(h["status"], "available");
    assert!(
        h["layers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["scope"] != "project")
            .all(|l| l["has_entry"] == false)
    );
    assert!(fs::read_to_string(project).unwrap().contains("project"));
    assert_eq!(backup_paths(&hosts.context.app).len(), 4);
}

#[test]
fn all_host_formats_connect_switch_refresh_and_remove_without_running_host_clis() {
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
        let result = hosts
            .action("connect", &json!({"id":id,"entry":"studio"}))
            .unwrap();
        assert_eq!(result["ok"], true, "{id}: {result}");
        assert_eq!(result["host"]["tested"], false);
        assert_eq!(result["host"]["status"], "connected");
        assert_eq!(result["host"]["skill"]["state"], "app");
        assert_eq!(
            hosts
                .action("entry", &json!({"id":id,"entry":"core"}))
                .unwrap()["ok"],
            true
        );
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
    assert_eq!(values.len(), 1);
    assert_eq!(values["CODEX_HOME"], root.to_str().unwrap());
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
        &json!({"mcpServers":{"agentnave":{"command":hosts.launcher("core")}}}).to_string(),
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
fn skill_ownership_links_and_refresh_version_update_preserve_external_content() {
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
        assert_eq!(skill::state(path, &hosts.context.home)["version"], VERSION);
        assert_eq!(h.config.exists(), keep_entry);
    }
    save(&path.join("SKILL.md"), "external");
    assert_eq!(skill::state(path, &hosts.context.home)["state"], "external");
    hosts.action("remove", &json!({"id":"pi"})).unwrap();
    assert_eq!(
        fs::read_to_string(path.join("SKILL.md")).unwrap(),
        "external"
    );
    #[cfg(unix)]
    {
        let other = host(&hosts, "claude");
        let link = other.skill.unwrap();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(path, &link).unwrap();
        skill::install(&link, &hosts.context.app, "claude", &hosts.context.home).unwrap();
        skill::remove(&link, &hosts.context.app, "claude", &hosts.context.home).unwrap();
        assert_eq!(
            skill::state(&link, &hosts.context.home)["state"],
            "external"
        );
        assert!(link.is_symlink());
    }
}

#[test]
fn get_is_read_only_onboarding_uses_cache_and_post_routes_require_action_header() {
    let (_temp, hosts) = fixture();
    let app = hosts.context.app.clone();
    let root = hosts.context.home.join("data");
    let reader = Reader::new(root.clone());
    reader.configure_hosts(hosts.context);
    for path in ["/app/hosts", "/app/onboarding"] {
        assert_eq!(reader.request("GET", path, None, b"").status, 200);
    }
    assert!(!app.exists());
    assert!(!root.exists());
    for path in [
        "/app/hosts/refresh",
        "/app/hosts/root",
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
fn backups_remain_global_and_bounded_at_thirty() {
    let (_temp, hosts) = fixture();
    let path = hosts.context.home.join("config");
    for n in 0..35 {
        save(&path, &n.to_string());
        backup(&hosts.context.app, "pi", &path, &hosts.context.home).unwrap();
    }
    let kept = backup_paths(&hosts.context.app);
    assert_eq!(kept.len(), 30);
    assert_eq!(fs::read_to_string(&kept[0]).unwrap(), "5");
}

#[test]
fn jsonc_and_quoted_yaml_values_survive_and_ambiguous_yaml_is_rejected() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "opencode");
    let raw = "{\n// keep\n\"mcp\": { /* preserve */ \"other\": {\"type\":\"remote\",}, },\n\"future\": [1,2,],\n}";
    let doc = Document::parse(raw, h.format, h.key).unwrap();
    let entry =
        json!({"type":"local","command":["/x"],"environment":{"AGENTNAVE_HOST":"opencode"}});
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
