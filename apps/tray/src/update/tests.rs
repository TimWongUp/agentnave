use super::*;
use std::collections::BTreeMap;

#[cfg(windows)]
mod manifest;

#[test]
fn notes_an_older_service() {
    let tools = tempfile::tempdir().unwrap();
    let site = tools.path().join("agentnave/lib/python3.12/site-packages");
    std::fs::create_dir_all(site.join("agentnave-0.1.0.dist-info")).unwrap();
    std::fs::create_dir_all(site.join("mcp-2.1.1.dist-info")).unwrap();
    let tools = Some(tools.path().to_path_buf());

    let note = service(&tools);
    assert_eq!(note["version"], "0.1.0");
    assert!(
        note["command"]
            .as_str()
            .unwrap()
            .ends_with(&format!("@v{VERSION}\""))
    );

    std::fs::rename(
        site.join("agentnave-0.1.0.dist-info"),
        site.join(format!("agentnave-{VERSION}.dist-info")),
    )
    .unwrap();
    assert!(service(&tools).is_null());
}

#[test]
fn a_found_update_is_used_for_a_day() {
    let now = SystemTime::now();
    assert!(!check_fresh(None, now));
    assert!(check_fresh(Some(now - Duration::from_secs(60)), now));
    assert!(!check_fresh(Some(now - FOUND_FOR), now));
}

// `sleep` and `sh` come from Git for Windows on a CI runner, not from the OS; a plain Windows
// machine has neither, so the timeout path is covered on POSIX only.
#[cfg(unix)]
#[test]
fn a_command_over_its_limit_is_stopped_and_reported() {
    let mut slow = Command::new("sleep");
    slow.arg("30");
    let started = Instant::now();
    let error = output_within(slow, Duration::from_millis(100)).unwrap_err();
    assert!(error.contains("没有完成"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let mut fast = Command::new("sh");
    fast.args(["-c", "echo nope >&2; exit 3"]);
    let output = output_within(fast, Duration::from_secs(10)).unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "nope");
}

#[cfg(unix)]
#[test]
fn command_timeout_reaps_descendants_holding_output_pipes() {
    let mut command = Command::new("sh");
    command.args(["-c", "trap '' TERM; sleep 2 & wait"]);
    let started = Instant::now();
    assert!(output_within(command, Duration::from_millis(100)).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn service_state_follows_the_upgrade() {
    let outdated = json!({"state": "outdated", "version": "0.1.0", "error": null, "command": "uv"});
    assert_eq!(service_state(outdated.clone(), Upgrade::Idle), outdated);
    assert_eq!(
        service_state(outdated.clone(), Upgrade::Running),
        json!({"state": "upgrading", "version": "0.1.0", "error": null})
    );
    let failed = service_state(outdated, Upgrade::Failed("uv 返回失败".into()));
    assert_eq!(
        (failed["state"].as_str(), failed["error"].as_str()),
        (Some("failed"), Some("uv 返回失败"))
    );
    assert_eq!(
        service_state(Value::Null, Upgrade::Done(Instant::now()))["state"],
        "done"
    );
    assert_eq!(
        service_state(Value::Null, Upgrade::Failed("x".into())),
        json!({"state":"failed", "error":"x"})
    );
    assert_eq!(
        service_state(Value::Null, Upgrade::Installing)["state"],
        "installing"
    );
}

#[test]
fn restart_marker_is_written_once_and_cleared() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path().join("app-data");
    assert!(!take_marker(&dir));
    write_marker(&dir).unwrap();
    assert!(dir.join(UPGRADE_MARKER).is_file());
    assert!(take_marker(&dir));
    assert!(!take_marker(&dir));
}

#[test]
fn notes_keep_the_releases_after_this_app_newest_first() {
    let body = json!([
        {"version": "0.46.1", "notes": "a"},
        {"version": "0.48.0", "notes": "c"},
        {"version": "0.47.0", "notes": "b"},
        {"version": "0.49.0", "notes": "d"},
        {"version": "0.47.5"},
    ])
    .to_string();
    let notes = release_notes(Some(&body), "0.46.1", "0.48.0");
    let versions: Vec<_> = notes["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["version"].as_str().unwrap())
        .collect();
    assert_eq!(versions, ["0.48.0", "0.47.0"]);
    assert_eq!(notes["link"], Value::Null);
    assert_eq!(
        release_notes(Some(&body), "0.40.0", "0.48.0")["link"],
        RELEASES
    );
    let old = "https://github.com/TimWongUp/agentnave/releases/tag/v0.47.0";
    assert_eq!(
        release_notes(Some(old), "0.46.1", "0.47.0"),
        json!({"items": [], "link": old})
    );
    assert_eq!(
        release_notes(None, "0.46.1", "0.47.0"),
        json!({"items": [], "link": null})
    );
}

#[test]
fn the_updated_version_is_read_once() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path().join("app-data");
    assert_eq!(take_updated(&dir), None);
    write_updated(&dir, "0.48.0").unwrap();
    assert_eq!(take_updated(&dir).as_deref(), Some("0.48.0"));
    assert_eq!(take_updated(&dir), None);
}

#[test]
fn last_checked_is_utc_rfc3339() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_152_000);
    assert_eq!(rfc3339(at), "2026-10-16T12:00:00Z");
    assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    let leap = SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400);
    assert_eq!(rfc3339(leap), "2000-02-29T00:00:00Z");
}

#[test]
fn extracts_embedded_wheel_from_gzip_archive_and_parses_proxy() {
    use std::io::Write;
    let wheel = extract_wheel(EMBEDDED_WHEEL, VERSION).expect("embedded wheel");
    assert!(wheel.starts_with(b"PK\x03\x04"));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"tar-header-padding").unwrap();
    gz.write_all(EMBEDDED_WHEEL).unwrap();
    let archive = gz.finish().unwrap();
    assert_eq!(
        wheel_from_archive(&archive, VERSION).as_deref(),
        Some(wheel.as_slice())
    );
    assert_eq!(wheel_from_archive(&archive, "99.99.99"), None);

    let env = std::collections::BTreeMap::from([
        ("HTTPS_PROXY".to_owned(), "  ".to_owned()),
        ("all_proxy".to_owned(), "socks5://127.0.0.1:7890".to_owned()),
    ]);
    assert_eq!(
        proxy_url(&env).map(|u| u.to_string()).as_deref(),
        Some("socks5://127.0.0.1:7890")
    );
}

#[test]
fn a_service_at_or_above_this_build_is_left_alone() {
    // The claim is taken after the version check, so the second read decides: a version already
    // installed by another path must not be reinstalled over its running files.
    assert!(!should_install(Some("0.59.1"), "0.59.1"));
    assert!(!should_install(Some("0.60.0"), "0.59.1"));
    assert!(should_install(Some("0.58.0"), "0.59.1"));
    assert!(should_install(None, "0.59.1"));
}

#[test]
fn the_mirror_retry_changes_only_the_index_and_leaves_the_original_alone() {
    let dirs = ToolDirs {
        home: PathBuf::from("/home/u"),
        tools: PathBuf::from("/tools"),
        bin: PathBuf::from("/bin"),
        network: BTreeMap::new(),
    };
    let mirrored = with_mirror(&dirs);
    assert_eq!(
        mirrored.network.get("UV_DEFAULT_INDEX").map(String::as_str),
        Some(MIRROR_INDEX)
    );
    assert!(
        dirs.network.is_empty(),
        "the caller's directories stay untouched"
    );
}

#[test]
fn github_is_tried_before_the_mirrors() {
    if update_override().is_some() {
        // An explicit endpoint takes the first slot by design; this test covers the default order.
        return;
    }
    let endpoints = update_endpoints();
    let urls: Vec<String> = endpoints.iter().map(|(_, url)| url.to_string()).collect();
    assert!(urls.len() >= 3, "{urls:?}");
    // GitHub is the only source we control: a mirror must never be able to hide a release by
    // answering "no update" first.
    assert_eq!(urls[0], MANIFEST, "{urls:?}");
    assert!(
        endpoints.iter().skip(1).all(|(prefix, _)| prefix.is_some()),
        "{urls:?}"
    );
}
