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
    // The App labels the rows it connects, so an unlabelled record is not this row's.
    save(
        &dir.join("unlabelled.json"),
        &json!({"version":1,"host":"codex","started_ms":1,"fault_ms":4000}).to_string(),
    );
    let state = hosts.state(&h);
    assert_eq!(state["fault"], json!({"count":2,"at_ms":2000}));
    let result = hosts
        .action("dismiss-fault", &json!({"id":"codex"}))
        .unwrap();
    assert_eq!(result["ok"], true);
    assert!(result["host"]["fault"].is_null());
    assert!(!dir.join("a.json").exists() && !dir.join("b.json").exists());
    assert!(dir.join("other.json").exists() && dir.join("unlabelled.json").exists());
}

#[test]
fn a_newer_healthy_connection_answers_older_faults() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    let root = hosts.context.home.join(".agentnave");
    let dir = root.join("connection-faults");
    save(&dir.join("before.json"), &record("codex", "codex", 1000));
    save(&dir.join("after.json"), &record("codex", "codex", 9500));
    save(
        &dir.join("unlabelled.json"),
        &json!({"version":1,"host":"codex","started_ms":1,"fault_ms":4000}).to_string(),
    );
    save(
        &dir.join("unknown-boss.json"),
        &json!({"version":1,"host":"grok","started_ms":1,"fault_ms":4000}).to_string(),
    );
    let hash = "a".repeat(64);
    // A connection that closed normally with the tools it was given, started after the first.
    save(
        &root.join("connections/closed.json"),
        &json!({"version":1,"host":"codex","host_app":"codex","started_ms":9000,
            "ended_ms":9100,"runtime_tools_hash":hash,"host_tools_hash":hash})
        .to_string(),
    );
    assert_eq!(hosts.state(&h)["fault"], json!({"count":1,"at_ms":9500}));
    // An unlabelled connection, such as a hand-written project entry's, proves nothing here.
    save(
        &root.join("connections/unlabelled.json"),
        &json!({"version":1,"host":"codex","started_ms":9800}).to_string(),
    );
    assert_eq!(hosts.state(&h)["fault"], json!({"count":1,"at_ms":9500}));
    fs::remove_file(root.join("connections/unlabelled.json")).unwrap();
    // Reading only filters; the refresh removes what is answered or can no longer be owned.
    assert!(dir.join("before.json").exists());
    hosts.refresh_probed();
    assert!(!dir.join("before.json").exists() && !dir.join("unlabelled.json").exists());
    assert!(dir.join("after.json").exists() && dir.join("unknown-boss.json").exists());
    // A connection running now answers every fault before it.
    save(
        &root.join("connections/live.json"),
        &json!({"version":1,"host":"codex","host_app":"codex","started_ms":9900}).to_string(),
    );
    assert!(hosts.state(&h)["fault"].is_null());
}
