use super::*;

fn record(host: &str, app: &str, fault_ms: u64) -> String {
    json!({"version":1,"host":host,"host_app":app,"started_ms":fault_ms-1,"fault_ms":fault_ms,
        "reason":"service_exited"})
    .to_string()
}

fn age(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn fault_records_show_per_host_until_dismissed_or_expired() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    let dir = hosts.context.home.join(".agentnave/connection-faults");
    assert!(hosts.state(&h)["fault"].is_null());
    save(&dir.join("a.json"), &record("codex", "codex", 1000));
    save(&dir.join("b.json"), &record("codex", "codex", 2000));
    save(&dir.join("other.json"), &record("claude", "claude", 3000));
    save(&dir.join("old.json"), &record("codex", "codex", 500));
    age(&dir.join("old.json"), 90_000);
    save(&dir.join("bad.json"), "{");
    save(
        &dir.join("not-a-fault.json"),
        &json!({"version":1,"host":"codex","host_app":"codex"}).to_string(),
    );
    let state = hosts.state(&h);
    assert_eq!(state["fault"], json!({"count":2,"at_ms":2000}));
    // A new connection on the same row proves nothing about the old chats.
    save(
        &hosts.context.home.join(".agentnave/connections/new.json"),
        &json!({"version":1,"host":"codex","host_app":"codex","started_ms":9000}).to_string(),
    );
    assert_eq!(hosts.state(&h)["fault"]["count"], 2);
    let result = hosts
        .action("dismiss-fault", &json!({"id":"codex"}))
        .unwrap();
    assert_eq!(result["ok"], true);
    assert!(result["host"]["fault"].is_null());
    assert!(!dir.join("a.json").exists() && !dir.join("b.json").exists());
    assert!(dir.join("other.json").exists());
}
