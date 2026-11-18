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

#[test]
fn a_killed_relay_that_delivered_the_tools_still_confirms_a_reload() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    hosts.processes = Mutex::new(processes::Probe::fixed(None));
    let root = hosts.context.home.join(".agentnave");
    let since = hosts.settings["codex"]["reload_changed"]["at_ms"]
        .as_u64()
        .unwrap();
    let hash = "a".repeat(64);
    let receipt = |start: u64| {
        json!({"version":1,"host":"codex","started_ms":start,"runtime_version":"0.22.0",
            "runtime_tools_hash":hash,"host_tools_hash":hash})
        .to_string()
    };
    let path = root.join("connections/17-0123456789abcdef0123456789abcdef.json");
    // The host reopened and no new connection exists yet; this one has no end mark.
    save(&path, &receipt(since + 1));
    hosts.processes =
        Mutex::new(processes::Probe::fixed(None).with_receipt_processes(Some(HashMap::new())));
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
    assert_eq!(hosts.state(&h)["reload"]["connections"], 0);
    // One from before the change, or whose server died under it, proves nothing.
    save(&path, &receipt(since));
    assert_eq!(hosts.state(&h)["reload"]["state"], "unverified");
    save(&path, &receipt(since + 1));
    let fault = root
        .join("connection-faults")
        .join(path.file_name().unwrap());
    save(
        &fault,
        &json!({"version":1,"host":"codex","host_app":"codex","started_ms":since + 1,
            "fault_ms":since + 2,"reason":"service_exited"})
        .to_string(),
    );
    assert_eq!(hosts.state(&h)["reload"]["state"], "unverified");
    fs::remove_file(&fault).unwrap();
    // Paused rather than known gone, it keeps the protection until it falls silent for long enough.
    hosts.processes = Mutex::new(
        processes::Probe::fixed(None).with_receipt_processes(Some(HashMap::from([(17, 1)]))),
    );
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(100))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "required");
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(200))
        .unwrap();
    assert_eq!(hosts.state(&h)["reload"]["state"], "current");
}

#[cfg(unix)]
#[test]
fn native_copy_actions_use_resolved_fake_clis_and_preserve_host_files() {
    let (_temp, mut hosts) = fixture();
    let capture = hosts.context.home.join("args");
    for (id, kind, args) in [
        ("claude", "daemon", "daemon\nstop\n--any\n\n"),
        ("codex", "daemon", "app-server\ndaemon\nrestart\n\n"),
        ("hermes", "gateway", "gateway\nrestart\n\n"),
    ] {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" \"${CODEX_HOME-}${CLAUDE_CONFIG_DIR-}${HERMES_HOME-}\" > \"$HOME/args\"\n",
        );
        save(&h.config, &seed(&h));
        let before = fs::read(&h.config).unwrap();
        let result = hosts
            .action("reload-copy", &json!({"id":id,"kind":kind}))
            .unwrap();
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["host"]["copy_actions"], json!([kind]));
        assert_eq!(fs::read_to_string(&capture).unwrap(), args);
        assert_eq!(fs::read(&h.config).unwrap(), before);
        assert!(!hosts.context.app.join("hosts.json").exists());
    }
    // A root the shell chose reaches the CLI; the default one above stayed unset.
    executable_file(
        &hosts.context.search[0].join("codex"),
        "#!/bin/sh\nprintf '%s' \"$CODEX_HOME\" > \"$HOME/args\"\n",
    );
    let shell_root = hosts.context.home.join("codex-root");
    hosts.shell = Some(Ok(BTreeMap::from([(
        "CODEX_HOME".into(),
        shell_root.display().to_string(),
    )])));
    hosts
        .action("reload-copy", &json!({"id":"codex","kind":"daemon"}))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&capture).unwrap(),
        shell_root.display().to_string()
    );
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

#[cfg(unix)]
struct PidGuard(Vec<u32>);

#[cfg(unix)]
impl Drop for PidGuard {
    fn drop(&mut self) {
        for &pid in &self.0 {
            if let Ok(raw) = libc::pid_t::try_from(pid) {
                unsafe { libc::kill(raw, libc::SIGKILL) };
            }
        }
    }
}

#[cfg(unix)]
fn spawn_detached(program: &Path, args: &[&str], dir: &Path) -> u32 {
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exec \"$@\" >/dev/null 2>&1 & echo $!")
        .arg("_")
        .arg(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .unwrap()
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(raw, 0) == 0 }
}

#[cfg(unix)]
#[test]
fn stop_copy_validates_pid_and_start_time_and_terminates_only_matching_cli_copy() {
    let (_temp, mut hosts) = fixture();
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    let fake_codex = hosts.context.search[0].join("codex");
    std::os::unix::fs::symlink("/bin/sleep", &fake_codex).unwrap();
    let work = hosts.context.home.join("work");
    fs::create_dir_all(&work).unwrap();

    hosts.action("connect", &json!({"id":"codex"})).unwrap();
    hosts.action("connect", &json!({"id":"pi"})).unwrap();
    hosts.processes = Mutex::new(processes::Probe::default());

    let codex_pid = spawn_detached(&fake_codex, &["60"], &work);
    let other_pid = spawn_detached(
        Path::new("/usr/bin/tail"),
        &["-f", "/dev/null"],
        &hosts.context.home,
    );
    let _guard = PidGuard(vec![codex_pid, other_pid]);

    let h = host(&hosts, "codex");
    let mut observed = None;
    for _ in 0..40 {
        hosts.processes.lock().unwrap().invalidate();
        hosts.settings["codex"]["reload_changed"]["at_ms"] = json!(u64::MAX / 2);
        let state = hosts.state(&h);
        if let Some(copy) = state["reload"]["copies"]
            .as_array()
            .and_then(|list| {
                list.iter()
                    .find(|c| c["pid"].as_u64() == Some(u64::from(codex_pid)))
            })
            .cloned()
        {
            observed = Some(copy);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let copy = observed.expect("detached fake codex should be observed as a cli copy");
    let since = copy["since"].as_u64().unwrap();
    assert_eq!(copy["kind"], "cli");
    assert_eq!(copy["cwd"], "~/work");
    hosts.settings["codex"]["reload_changed"]["at_ms"] = json!(since + 1000);
    hosts.settings["pi"]["reload_changed"]["at_ms"] = json!(since + 1000);

    // Mismatched start time, unrelated PID, or wrong host must be refused without signalling.
    let bad_since = hosts
        .action(
            "stop-copy",
            &json!({"id":"codex","pid":codex_pid,"since":since + 1000}),
        )
        .unwrap();
    assert_eq!(bad_since["ok"], false);
    assert!(alive(codex_pid));

    let bad_pid = hosts
        .action(
            "stop-copy",
            &json!({"id":"codex","pid":other_pid,"since":since}),
        )
        .unwrap();
    assert_eq!(bad_pid["ok"], false);
    assert!(alive(other_pid));

    let wrong_host = hosts
        .action(
            "stop-copy",
            &json!({"id":"pi","pid":codex_pid,"since":since}),
        )
        .unwrap();
    assert_eq!(wrong_host["ok"], false);
    assert!(alive(codex_pid));

    assert!(
        hosts
            .action("stop-copy", &json!({"id":"codex","pid":1,"since":since}))
            .is_err()
    );

    let stopped = hosts
        .action(
            "stop-copy",
            &json!({"id":"codex","pid":codex_pid,"since":since}),
        )
        .unwrap();
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert!(!alive(codex_pid));
    assert!(alive(other_pid));
    assert!(
        !stopped["host"]["reload"]["copies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["pid"].as_u64() == Some(u64::from(codex_pid)))
    );

    let reader = Reader::new(hosts.context.home.join(".agentnave"));
    reader.use_hosts(hosts);
    let path = "/app/hosts/stop-copy";
    assert_eq!(reader.request("GET", path, None, b"").status, 405);
    assert_eq!(reader.request("POST", path, None, b"{}").status, 403);
    assert_eq!(
        reader
            .request("POST", path, Some("1"), br#"{"id":"codex"}"#)
            .status,
        400
    );
}
