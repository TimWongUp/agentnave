use super::*;

#[test]
fn idle_host_uses_install_history_and_remembers_new_connection() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "workbuddy");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"workbuddy"})).unwrap();
    let token = hosts.reload_token(&h).unwrap();
    hosts.settings["workbuddy"]["reload_confirmed"] = json!({"v":RELOAD_FORMAT,"token":token});
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let path = hosts.context.app.join("service/install-history.json");
    let history = |names: Value| {
        json!([
            {"version":"0.21.0","switched_at_ms":(now-100)*1000,"tool_names":["old"]},
            {"version":"0.22.0","switched_at_ms":(now-10)*1000,"tool_names":names}
        ])
    };
    let running = |at| {
        Mutex::new(processes::Probe::fixed(Some(HashMap::from([(
            "workbuddy",
            at,
        )]))))
    };
    hosts.processes = running(now - 50);
    save(&path, &history(json!(["new"])).to_string());
    assert_eq!(hosts.reload_state(&h, &token)["reason"], "tools_upgrade");
    save(&path, &history(json!(["old"])).to_string());
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    save(&path, &history(Value::Null).to_string());
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    save(&path, &history(json!(["new"])).to_string());
    hosts.processes = Mutex::new(processes::Probe::fixed(Some(HashMap::new())));
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    hosts.processes = running(now - 5);
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    hosts.processes = running(now - 10);
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    hosts.processes = running(now - 50);
    save(&path, "[]");
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
    save(&path, &history(json!(["new"])).to_string());
    let receipt = json!({"version":1,"host":"workbuddy","started_ms":(now-5)*1000,
        "ended_ms":now*1000,"runtime_version":"0.22.0",
        "runtime_tools_hash":"a".repeat(64),"host_tools_hash":"a".repeat(64)});
    let receipt_path = hosts.context.home.join(".agentnave/connections/test.json");
    save(&receipt_path, &receipt.to_string());
    assert_eq!(hosts.reload_state(&h, &token)["state"], "current");
    hosts.remember_reloads();
    fs::remove_file(receipt_path).unwrap();
    assert_eq!(hosts.reload_state(&h, &token)["state"], "idle");
}
