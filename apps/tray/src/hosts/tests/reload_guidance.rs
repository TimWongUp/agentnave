use super::*;

#[test]
fn dispatched_receipts_never_supply_boss_evidence_or_block_it() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    hosts.processes = Mutex::new(processes::Probe::fixed(None));
    let dir = hosts.context.home.join(".agentnave/connections");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    hosts.settings["codex"]["reload_changed"]["at_ms"] = json!(now - 100);
    let mut receipt = json!({"version":1,"host":"codex","started_ms":now-50,"ended_ms":now,
        "runtime_version":"0.22.0","runtime_tools_hash":"a".repeat(64),"host_tools_hash":"a".repeat(64),"dispatched":true});
    let path = dir.join("ended.json");
    save(&path, &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "unverified");
    hosts.remember_reloads();
    assert!(hosts.settings["codex"]["reload_confirmed"].is_null());
    receipt.as_object_mut().unwrap().remove("ended_ms");
    for age in [0, 60] {
        save(&path, &receipt.to_string());
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - std::time::Duration::from_secs(age))
            .unwrap();
        let state = hosts.state(&h);
        assert_eq!(state["reload"]["state"], "unverified");
        assert_eq!(state["reload"]["connections"], 0);
    }
    receipt["dispatched"] = json!(false);
    save(&dir.join("boss.json"), &receipt.to_string());
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
}

#[cfg(unix)]
#[test]
fn native_copy_actions_use_resolved_fake_clis_and_preserve_host_files() {
    let (_temp, mut hosts) = fixture();
    let capture = hosts.context.home.join("args");
    for (id, kind, args) in [
        ("claude", "daemon", "daemon\nstop\n--any\n"),
        ("codex", "daemon", "app-server\ndaemon\nrestart\n"),
        ("hermes", "gateway", "gateway\nrestart\n"),
    ] {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/args\"\n",
        );
        save(&h.config, &seed(&h));
        let before = fs::read(&h.config).unwrap();
        let result = hosts
            .action("reload-copy", &json!({"id":id,"kind":kind}))
            .unwrap();
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(fs::read_to_string(&capture).unwrap(), args);
        assert_eq!(fs::read(&h.config).unwrap(), before);
        assert!(!hosts.context.app.join("hosts.json").exists());
    }
    assert!(
        hosts
            .action("reload-copy", &json!({"id":"codex","kind":"cli"}))
            .is_err()
    );
    executable_file(
        &hosts.context.search[0].join("codex"),
        "#!/bin/sh\nexit 3\n",
    );
    assert_eq!(
        hosts
            .action("reload-copy", &json!({"id":"codex","kind":"daemon"}))
            .unwrap()["ok"],
        false
    );
    let reader = Reader::new(hosts.context.home.join(".agentnave"));
    reader.use_hosts(hosts);
    let path = "/app/hosts/reload-copy";
    assert_eq!(reader.request("GET", path, None, b"").status, 405);
    assert_eq!(reader.request("POST", path, None, b"{}").status, 403);
    assert_eq!(
        reader
            .request("POST", path, Some("1"), br#"{"id":"pi","kind":"daemon"}"#)
            .status,
        400
    );
}
