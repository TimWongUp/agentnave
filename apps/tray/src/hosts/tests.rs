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
    (temp, Hosts::new(context))
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
    hosts
        .context
        .registry()
        .into_iter()
        .find(|h| h.id == id)
        .unwrap()
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
fn fake(hosts: &mut Hosts, host: &Host, written: &str, removed: &str) {
    let base = hosts.context.home.parent().unwrap().to_path_buf();
    let wanted = base.join("wanted");
    let deleted = base.join("removed");
    save(&wanted, written);
    save(&deleted, removed);
    for (key, path) in [
        ("FAKE_CONFIG", host.config.clone()),
        ("FAKE_WRITTEN", wanted),
        ("FAKE_REMOVED", deleted),
        ("FAKE_LOG", base.join("cli.log")),
    ] {
        hosts
            .context
            .env
            .insert(key.into(), path.display().to_string());
    }
    executable_file(
        &hosts.context.search[0].join(host.binary),
        r#"#!/usr/bin/python3
import sys,os,pathlib,json
p=pathlib.Path(os.environ['FAKE_CONFIG'])
p.parent.mkdir(parents=True,exist_ok=True)
with open(os.environ['FAKE_LOG'],'a') as log:
 log.write(json.dumps({'args':sys.argv[1:],'home':os.environ['HOME'],'buddy':os.environ.get('CODEBUDDY_CONFIG_DIR'),'stdin':sys.stdin.read()})+'\n')
p.write_bytes(pathlib.Path(os.environ['FAKE_REMOVED' if sys.argv[2]=='remove' else 'FAKE_WRITTEN']).read_bytes())
"#,
    );
}

#[test]
fn registry_covers_overrides_without_reading_real_home() {
    let (_temp, mut hosts) = fixture();
    assert_eq!(hosts.context.registry().len(), 10);
    for (var, id, suffix) in [
        ("CODEX_HOME", "codex", "config.toml"),
        ("CODEBUDDY_CONFIG_DIR", "codebuddy", ".mcp.json"),
        ("GROK_HOME", "grok", "config.toml"),
        ("HERMES_HOME", "hermes", "config.yaml"),
        ("PI_CODING_AGENT_DIR", "pi", "mcp.json"),
    ] {
        let root = hosts.context.home.join(var);
        hosts
            .context
            .env
            .insert(var.into(), root.display().to_string());
        assert_eq!(host(&hosts, id).config, root.join(suffix));
    }
    for id in ["pi", "gemini", "opencode"] {
        assert!(!host(&hosts, id).tested);
    }
}

#[test]
fn every_host_connects_takes_over_switches_and_removes_without_losing_other_content() {
    for id in [
        "codex",
        "claude",
        "codebuddy",
        "workbuddy",
        "grok",
        "antigravity",
        "pi",
        "gemini",
        "opencode",
        "hermes",
    ] {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, id);
        let original = seed(&h);
        save(&h.config, &original);
        let before = h.read().unwrap();
        let path = hosts.context.launcher("studio").unwrap();
        let mut entry = if id == "opencode" {
            json!({"type":"local", "command":[path], "environment":{"AGENTNAVE_HOST":id},"enabled":true})
        } else if id == "hermes" {
            json!({"command":path,"args":[],"env":{"AGENTNAVE_HOST":id},"enabled":true})
        } else {
            json!({"command":path,"args":[],"env":{"AGENTNAVE_HOST":id}})
        };
        if h.cli() {
            fake(
                &mut hosts,
                &h,
                &before.render(Some(&entry)).unwrap(),
                &original,
            );
        } else {
            executable_file(
                &hosts.context.search[0].join(h.binary),
                "#!/bin/sh\nexit 0\n",
            );
        }
        let result = hosts.action("connect", &json!({"id": id})).unwrap();
        assert_eq!(result["ok"], true, "{id}: {result}");
        assert_eq!(result["status"], "connected", "{id}");
        assert_eq!(result["skill"]["state"], "app", "{id}");
        before
            .verify(&fs::read_to_string(&h.config).unwrap(), Some(&entry))
            .unwrap();
        entry[h.env_key()] = json!({"KEEP":"secret-test", "PATH":"original", "AGENTNAVE_EXCLUDED_PROVIDERS":"codex"});
        if id == "opencode" {
            entry["command"]
                .as_array_mut()
                .unwrap()
                .push(json!("--preserved"));
        } else {
            entry["args"] = json!(["--preserved", "literal"]);
        }
        entry["disabled"] = json!(false);
        entry["print"] = json!(true);
        entry["future"] = json!({"unknown": [1,2,3]});
        save(&h.config, &before.render(Some(&entry)).unwrap());
        let mut takeover = entry.clone();
        takeover[h.env_key()]["AGENTNAVE_HOST"] = json!(id);
        if h.cli() {
            let mut native = takeover.clone();
            native.as_object_mut().unwrap().remove("future");
            fake(
                &mut hosts,
                &h,
                &before.render(Some(&native)).unwrap(),
                &original,
            );
        }
        hosts.refresh();
        assert_eq!(
            h.read().unwrap().entry,
            Some(takeover.clone()),
            "takeover {id}: {}",
            hosts.state(&h)
        );
        let backups = backup_paths(&hosts.context.app).len();
        hosts.refresh();
        assert_eq!(
            backups,
            backup_paths(&hosts.context.app).len(),
            "idempotent {id}"
        );
        h.set_command(&mut takeover, &hosts.context.launcher("core").unwrap());
        if h.cli() {
            fake(
                &mut hosts,
                &h,
                &before.render(Some(&takeover)).unwrap(),
                &original,
            );
        }
        let result = hosts
            .action("entry", &json!({"id":id,"entry":"core"}))
            .unwrap();
        assert_eq!(result["ok"], true, "entry {id}: {result}");
        assert_eq!(h.read().unwrap().entry, Some(takeover), "switch {id}");
        if h.cli() {
            fake(&mut hosts, &h, "", &original);
        }
        let result = hosts.action("remove", &json!({"id":id})).unwrap();
        assert_eq!(result["ok"], true, "remove {id}: {result}");
        assert_eq!(h.read().unwrap().entry, None);
        assert_eq!(result["skill"]["state"], "missing");
        if !matches!(h.format, Format::Yaml) {
            assert_eq!(h.read().unwrap().rest, before.rest, "rest {id}");
        }
        if h.cli() {
            let log =
                fs::read_to_string(hosts.context.home.parent().unwrap().join("cli.log")).unwrap();
            for line in log.lines() {
                let call: Value = serde_json::from_str(line).unwrap();
                assert_eq!(call["stdin"], "");
                assert_eq!(call["home"], hosts.context.home.to_str().unwrap());
                if id == "codebuddy" {
                    assert_eq!(call["buddy"], h.config.parent().unwrap().to_str().unwrap());
                }
            }
        }
    }
}

#[test]
fn hermes_preserves_opaque_yaml_and_repairs_missing_launcher() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "hermes");
    let raw = "# before\nmcp_servers:\n  agentnave:\n    command: /missing/agentnave-mcp\n    env:\n      KEEP: 'yes'\n    tools:\n      include: [] # intentionally disabled\n    future:\n      nested: untouched\n  other:\n    command: keep\n# tail\nmodel:\n  value: unchanged\n";
    save(&h.config, raw);
    executable_file(
        &hosts.context.search[0].join("hermes"),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.refresh();
    let written = fs::read_to_string(&h.config).unwrap();
    assert!(written.contains("    tools:\n      include: [] # intentionally disabled\n    future:\n      nested: untouched\n"));
    assert!(written.ends_with("  other:\n    command: keep\n# tail\nmodel:\n  value: unchanged\n"));
    assert_eq!(hosts.state(&h)["entry"], "core");
    assert_eq!(hosts.state(&h)["status"], "connected");
    assert_eq!(h.read().unwrap().entry.unwrap()["env"]["KEEP"], "yes");
}

#[test]
fn collateral_changes_and_cli_failures_roll_back_and_failed_refresh_does_not_repeat() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "claude");
    let raw = r#"{"unknown":"keep","mcpServers":{"agentnave":{"command":"/missing/agentnave-studio-mcp","env":{"KEEP":"test"}}}}"#;
    save(&h.config, raw);
    let wanted = r#"{"unknown":"CORRUPTED","mcpServers":{}}"#;
    fake(&mut hosts, &h, wanted, wanted);
    hosts.refresh();
    assert_eq!(fs::read_to_string(&h.config).unwrap(), raw);
    assert_eq!(hosts.state(&h)["status"], "needs_attention");
    let count = backup_paths(&hosts.context.app).len();
    hosts.refresh();
    assert_eq!(backup_paths(&hosts.context.app).len(), count);
    assert_eq!(
        fs::read_to_string(backup_paths(&hosts.context.app).last().unwrap()).unwrap(),
        raw
    );
    executable_file(
        &hosts.context.search[0].join("claude"),
        "#!/bin/sh\nexit 1\n",
    );
    assert_eq!(
        hosts.action("remove", &json!({"id":"claude"})).unwrap()["ok"],
        false
    );
    assert_eq!(fs::read_to_string(&h.config).unwrap(), raw);
}

#[test]
fn backups_are_global_and_keep_only_the_newest_thirty() {
    let (_temp, hosts) = fixture();
    let path = hosts.context.home.join("config.json");
    for n in 0..35 {
        save(&path, &n.to_string());
        backup(
            &hosts.context.app,
            if n % 2 == 0 { "claude" } else { "grok" },
            &path,
        )
        .unwrap();
    }
    let kept = backup_paths(&hosts.context.app);
    assert_eq!(kept.len(), 30);
    assert_eq!(fs::read_to_string(&kept[0]).unwrap(), "5");
    assert_eq!(fs::read_to_string(&kept[29]).unwrap(), "34");
}

#[test]
fn skill_three_states_external_edits_old_versions_and_links_are_preserved() {
    let (_temp, hosts) = fixture();
    let path = host(&hosts, "pi").skill;
    assert_eq!(skill::state(&path)["state"], "missing");
    skill::install(&path, &hosts.context.app, "pi").unwrap();
    assert_eq!(skill::state(&path)["state"], "app");
    assert!(path.join("references/pi.md").is_file());
    save(&path.join(".agentnave-app"), "old-version");
    let manifest = path.join(".agentnave-app-manifest.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["version"] = json!("old-version");
    save(&manifest, &value.to_string());
    skill::install(&path, &hosts.context.app, "pi").unwrap();
    assert_eq!(skill::state(&path)["version"], VERSION);
    save(&path.join("SKILL.md"), "externally modified");
    assert_eq!(skill::state(&path)["state"], "external");
    skill::install(&path, &hosts.context.app, "pi").unwrap();
    skill::remove(&path, &hosts.context.app, "pi").unwrap();
    assert_eq!(
        fs::read_to_string(path.join("SKILL.md")).unwrap(),
        "externally modified"
    );
    #[cfg(unix)]
    {
        let link = host(&hosts, "claude").skill;
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(skill::state(&link)["state"], "external");
        skill::install(&link, &hosts.context.app, "claude").unwrap();
        skill::remove(&link, &hosts.context.app, "claude").unwrap();
        assert!(link.is_symlink());
    }
}

#[test]
fn get_routes_are_read_only_and_orphaned_entries_are_not_taken_over() {
    let (_temp, hosts) = fixture();
    let h = host(&hosts, "pi");
    save(
        &h.config,
        r#"{"mcpServers":{"agentnave":{"command":"/missing/agentnave-mcp"}}}"#,
    );
    let original = fs::read(&h.config).unwrap();
    assert_eq!(hosts.state(&h)["status"], "orphaned");
    let app = hosts.context.app.clone();
    let root = hosts.context.home.join(".agentnave");
    let reader = Reader::new(root.clone());
    reader.configure_hosts(hosts.context);
    for path in ["/app/hosts", "/app/onboarding"] {
        assert_eq!(reader.request("GET", path, None, b"").status, 200);
    }
    assert!(!app.exists());
    assert!(!root.exists());
    assert_eq!(fs::read(&h.config).unwrap(), original);
    for path in [
        "/app/hosts/refresh",
        "/app/hosts/connect",
        "/app/hosts/remove",
        "/app/hosts/entry",
        "/app/onboarding/dismiss",
    ] {
        assert_eq!(reader.request("GET", path, None, b"").status, 405);
        assert_eq!(reader.request("POST", path, None, b"{}").status, 403);
    }
    assert_eq!(
        reader
            .request("POST", "/app/hosts/refresh", Some("1"), b"")
            .status,
        200
    );
    assert!(!app.exists());
    assert_eq!(fs::read(&h.config).unwrap(), original);
    assert_eq!(
        reader
            .request("POST", "/app/onboarding/dismiss", Some("1"), b"")
            .status,
        200
    );
    assert!(app.join("onboarding.dismissed").is_file());
}

#[test]
fn onboarding_uses_real_saved_state_and_shadowing_only_reports() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    let entry = json!({"command":hosts.context.launcher("studio"),"env":{"AGENTNAVE_HOST":"pi"}});
    save(
        &h.config,
        &Document::parse("", Format::Json, "mcpServers")
            .unwrap()
            .render(Some(&entry))
            .unwrap(),
    );
    let project = hosts.context.project.join(".pi/mcp.json");
    let raw = r#"{"mcpServers":{"agentnave":{"command":"project"}}}"#;
    save(&project, raw);
    assert_eq!(hosts.state(&h)["shadowed_by"], project.to_str().unwrap());
    hosts.refresh();
    assert_eq!(fs::read_to_string(project).unwrap(), raw);
    let root = hosts.context.home.join("data");
    let initial = crate::onboarding::state(&hosts, &root);
    assert_eq!(initial["steps"][0]["done"], true);
    assert_eq!(initial["steps"][1]["done"], true);
    assert_eq!(initial["steps"][2]["done"], false);
    assert_eq!(initial["steps"][3]["done"], false);
    save(
        &root.join("members.json"),
        r#"{"version":1,"clis":{"pi":{"models":{"x":{}}}},"modes":{}}"#,
    );
    save(
        &root.join("conversations/a/state.json"),
        r#"{"manager_session":"manager"}"#,
    );
    let after = crate::onboarding::state(&hosts, &root);
    assert_eq!(after["steps"][2]["done"], true);
    assert_eq!(after["steps"][3]["done"], true);
}

#[test]
fn orphan_removal_without_cli_preserves_external_skill_and_other_servers() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    let raw = "[mcp_servers.other]\ncommand = 'keep'\n[mcp_servers.agentnave]\ncommand = '/missing/agentnave-mcp'\n";
    save(&h.config, raw);
    save(&h.skill.join("SKILL.md"), "external");
    assert_eq!(hosts.state(&h)["status"], "orphaned");
    let result = hosts.action("remove", &json!({"id":"codex"})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert!(h.read().unwrap().entry.is_none());
    assert!(
        fs::read_to_string(&h.config)
            .unwrap()
            .contains("command = 'keep'")
    );
    assert_eq!(
        fs::read_to_string(h.skill.join("SKILL.md")).unwrap(),
        "external"
    );
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
fn missing_service_and_broken_config_do_not_write_and_failed_new_config_is_removed() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 0\n");
    let studio = hosts.context.launcher("studio").unwrap();
    fs::remove_file(&studio).unwrap();
    assert_eq!(
        hosts.action("connect", &json!({"id":"pi"})).unwrap()["ok"],
        false
    );
    assert!(!h.config.exists());
    assert!(!hosts.context.app.exists());
    executable_file(&studio, "#!/bin/sh\nexit 99\n");
    fake(&mut hosts, &h, "{broken", "");
    assert_eq!(
        hosts.action("connect", &json!({"id":"pi"})).unwrap()["ok"],
        false
    );
    assert!(!h.config.exists());
    save(&h.config, "{broken");
    let before = fs::read(&h.config).unwrap();
    hosts.refresh();
    assert_eq!(hosts.state(&h)["status"], "needs_attention");
    assert_eq!(fs::read(&h.config).unwrap(), before);
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
fn opencode_layered_overrides_select_the_effective_entry_and_keep_skill_root() {
    let (_temp, mut hosts) = fixture();
    let global = host(&hosts, "opencode").config;
    save(
        &global,
        r#"{"mcp":{"agentnave":{"type":"local","command":["/global"]}}}"#,
    );
    let extra = hosts.context.home.join("extra.json");
    let custom = hosts.context.home.join("custom");
    hosts
        .context
        .env
        .insert("OPENCODE_CONFIG".into(), extra.display().to_string());
    hosts
        .context
        .env
        .insert("OPENCODE_CONFIG_DIR".into(), custom.display().to_string());
    save(
        &extra,
        r#"{"mcp":{"agentnave":{"type":"local","command":["/extra"]}}}"#,
    );
    save(
        &custom.join("opencode.jsonc"),
        "{/* comment */ \"future\":true}",
    );
    assert_eq!(host(&hosts, "opencode").config, extra);
    assert_eq!(
        host(&hosts, "opencode").skill,
        custom.join("skills/agentnave-manager")
    );
    save(
        &custom.join("opencode.jsonc"),
        r#"{"mcp":{"agentnave":{"type":"local","command":["/custom"]}}}"#,
    );
    assert_eq!(
        host(&hosts, "opencode").config,
        custom.join("opencode.jsonc")
    );
    save(&custom.join("opencode.jsonc"), "{broken");
    assert_eq!(
        hosts.state(&host(&hosts, "opencode"))["status"],
        "needs_attention"
    );
}
