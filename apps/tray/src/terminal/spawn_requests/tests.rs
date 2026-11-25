use super::*;
use crate::data_files::PUSH_MAX_BYTES;
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn test_coord(dir: &Path) -> Coordinator {
    Coordinator::new(
        dir.to_path_buf(),
        std::process::id(),
        current_process_started_at(),
        &["claude", "codex"],
    )
}

#[test]
fn push_document_splits_push_providers_from_terminal_providers() {
    let mut receipts = HashMap::new();
    let doc = build_push_document(
        std::process::id(),
        super::super::wakeup::PROVIDERS,
        &[],
        &mut receipts,
    );
    #[cfg(unix)]
    assert_eq!(doc["providers"], json!(["codex"]));
    #[cfg(not(unix))]
    assert_eq!(doc["providers"], json!([]));

    #[cfg(target_os = "macos")]
    assert_eq!(
        doc["terminal_providers"],
        json!(["claude", "codex", "antigravity"])
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(doc["terminal_providers"], json!([]));
}

#[test]
fn publishes_256k_limit_evicts_oldest_finished_only() {
    let dir = tempfile::tempdir().unwrap();
    let coord = test_coord(dir.path());
    let mut receipts = HashMap::new();
    let huge_sid = "s".repeat(240);
    // Add one active receipt + ~1100 finished receipts to exceed 256 KiB.
    receipts.insert(
        "active-id".to_owned(),
        ReceiptEntry {
            receipt: Receipt {
                service_pid: 123,
                invocation_id: "inv-active".to_owned(),
                kind: "run".to_owned(),
                state: ReceiptState::Started,
                session_id: Some(huge_sid.clone()),
                lease_acquired: None,
                anchor: None,
                blocked: None,
                background_tasks_pending: None,
                terminal: None,
            },
            provider: None,
            finished_at: None,
        },
    );
    let base = Instant::now();
    for i in 0..1200usize {
        receipts.insert(
            format!("00000000-0000-4000-8000-{i:012}"),
            ReceiptEntry {
                receipt: Receipt {
                    service_pid: 123,
                    invocation_id: format!("inv-{i:012}"),
                    kind: "run".to_owned(),
                    state: ReceiptState::Finished,
                    session_id: Some(huge_sid.clone()),
                    lease_acquired: Some(true),
                    anchor: None,
                    blocked: None,
                    background_tasks_pending: None,
                    terminal: Some(TerminalOutcome {
                        status: "succeeded".to_owned(),
                        reason: Some(huge_sid.clone()),
                        warnings: Vec::new(),
                    }),
                },
                provider: None,
                finished_at: Some(base + Duration::from_millis(i as u64)),
            },
        );
    }

    let doc = build_push_document(coord.pid, coord.push_providers, &[], &mut receipts);
    let encoded = serde_json::to_vec(&doc).unwrap();
    assert!(encoded.len() <= PUSH_MAX_BYTES);
    assert!(doc["requests"].get("active-id").is_some());
    assert!(doc["terminal_providers"].as_array().is_some());

    // C2: Actual persisted push file must use the same compact encoding and stay <= PUSH_MAX_BYTES.
    coord.lock().receipts = receipts;
    coord.publish().unwrap();
    let on_disk = fs::read(dir.path().join("push").join(format!("{}.json", coord.pid))).unwrap();
    assert!(on_disk.len() <= PUSH_MAX_BYTES);
}

#[test]
fn publish_failure_rolls_back_intake_and_retries_receipt_updates() {
    let dir = tempfile::tempdir().unwrap();
    ensure_spawn_requests_dir(dir.path());
    let coord = test_coord(dir.path());
    let terminals = Terminals::default();

    // Block push/ creation by placing a regular file at `push`.
    let blocker = dir.path().join("push");
    fs::write(&blocker, b"not-a-dir").unwrap();

    let req_id = "88888888-8888-4888-8888-888888888888";
    let req_path = dir
        .path()
        .join("spawn-requests")
        .join(format!("{req_id}.json"));
    fs::write(
        &req_path,
        json!({
            "version": 1,
            "request_id": req_id,
            "kind": "cancel",
            "service": {"pid": coord.pid, "started_at": coord.started_at},
            "target_tray": {"pid": coord.pid, "started_at": coord.started_at},
            "invocation_id": "99999999-9999-4999-8999-999999999999",
        })
        .to_string(),
    )
    .unwrap();

    // First poll fails to publish: receipt must be rolled back and request file kept for retry.
    coord.poll(&terminals);
    assert!(req_path.exists());
    assert!(coord.receipts_snapshot().is_empty());

    // Once push/ is writable, the next poll consumes the request and publishes its receipt.
    fs::remove_file(&blocker).unwrap();
    coord.poll(&terminals);
    assert!(!req_path.exists());
    let push_path = dir.path().join("push").join(format!("{}.json", coord.pid));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut disk_state = Value::Null;
    while Instant::now() < deadline {
        if let Ok(bytes) = fs::read(&push_path)
            && let Ok(doc) = serde_json::from_slice::<Value>(&bytes)
        {
            disk_state = doc["requests"][req_id]["state"].clone();
            if disk_state == json!("finished") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(disk_state, json!("finished"));

    // Simulate transient write failure on mark_finished: last_published must NOT advance,
    // so a subsequent poll flushes the finished receipt once writable again.
    coord.insert_test_receipt("req-late", "inv-late");
    coord.publish().unwrap();
    fs::remove_dir_all(dir.path().join("push")).unwrap();
    fs::write(&blocker, b"blocked-again").unwrap();
    coord.mark_finished("req-late", "succeeded", None, Some("sid-late"));
    fs::remove_file(&blocker).unwrap();
    coord.poll(&terminals);
    let flushed: Value = serde_json::from_slice(&fs::read(&push_path).unwrap()).unwrap();
    assert_eq!(flushed["requests"]["req-late"]["state"], json!("finished"));
}

#[test]
fn rejects_invalid_and_symlink_requests_and_deduplicates_valid_ones() {
    let dir = tempfile::tempdir().unwrap();
    ensure_spawn_requests_dir(dir.path());
    let req_dir = dir.path().join("spawn-requests");
    let coord = test_coord(dir.path());
    coord.publish().unwrap();

    // Wrong filename stem -> ignored.
    fs::write(req_dir.join("not-a-uuid.json"), b"{}").unwrap();

    // Mismatched target_tray pid -> ignored and left untouched.
    let wrong_id = "11111111-1111-4111-8111-111111111111";
    let wrong_path = req_dir.join(format!("{wrong_id}.json"));
    fs::write(
        &wrong_path,
        json!({
            "version": 1,
            "request_id": wrong_id,
            "kind": "cancel",
            "service": {"pid": coord.pid, "started_at": coord.started_at},
            "target_tray": {"pid": coord.pid + 99999, "started_at": coord.started_at},
            "invocation_id": "22222222-2222-4222-8222-222222222222",
        })
        .to_string(),
    )
    .unwrap();

    #[cfg(unix)]
    {
        let target = dir.path().join("outside.json");
        let link_id = "33333333-3333-4333-8333-333333333333";
        fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, req_dir.join(format!("{link_id}.json"))).unwrap();
    }

    let terminals = Terminals::default();
    coord.poll(&terminals);
    assert!(wrong_path.exists());
    assert!(coord.receipts_snapshot().is_empty());

    // Valid cancel for unknown invocation -> accepted published atomically before delete,
    // finished as already_finished, and re-submitting same request_id deduplicates cleanly.
    let valid_id = "44444444-4444-4444-8444-444444444444";
    let valid_path = req_dir.join(format!("{valid_id}.json"));
    let payload = json!({
        "version": 1,
        "request_id": valid_id,
        "kind": "cancel",
        "service": {"pid": coord.pid, "started_at": coord.started_at},
        "target_tray": {"pid": coord.pid, "started_at": coord.started_at},
        "invocation_id": "55555555-5555-4555-8555-555555555555",
    })
    .to_string();
    fs::write(&valid_path, &payload).unwrap();
    coord.poll(&terminals);
    assert!(!valid_path.exists());

    // Wait briefly for cancel worker to mark finished.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if coord
            .receipts_snapshot()
            .get(valid_id)
            .is_some_and(|r| r.state == ReceiptState::Finished)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let first = coord.receipts_snapshot()[valid_id].clone();
    assert_eq!(first.state, ReceiptState::Finished);
    assert_eq!(
        first.terminal.as_ref().map(|t| t.status.as_str()),
        Some("already_finished")
    );

    // Writing the same request_id again consumes the file without overwriting finished receipt.
    fs::write(&valid_path, &payload).unwrap();
    coord.poll(&terminals);
    assert!(!valid_path.exists());
    assert_eq!(
        coord.receipts_snapshot()[valid_id].state,
        ReceiptState::Finished
    );
}

#[test]
fn receipt_stays_until_service_inbox_settlement_confirms() {
    let dir = tempfile::tempdir().unwrap();
    ensure_spawn_requests_dir(dir.path());
    fs::create_dir_all(dir.path().join("inbox")).unwrap();
    let coord = test_coord(dir.path());
    let req_id = "66666666-6666-4666-8666-666666666666";
    fs::write(
        dir.path()
            .join("spawn-requests")
            .join(format!("{req_id}.json")),
        json!({
            "version": 1,
            "request_id": req_id,
            "kind": "cancel",
            "service": {"pid": coord.pid, "started_at": coord.started_at},
            "target_tray": {"pid": coord.pid, "started_at": coord.started_at},
            "invocation_id": "77777777-7777-4777-8777-777777777777",
        })
        .to_string(),
    )
    .unwrap();

    let terminals = Terminals::default();
    coord.poll(&terminals);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if coord
            .receipts_snapshot()
            .get(req_id)
            .is_some_and(|r| r.state == ReceiptState::Finished)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Wrong tray started_at in settlement must NOT clear the receipt.
    fs::write(
        dir.path().join("inbox").join(format!("{}.json", coord.pid)),
        json!({
            "version": 1,
            "pid": coord.pid,
            "settled_requests": {
                req_id: {"pid": coord.pid, "started_at": coord.started_at + 99.0}
            }
        })
        .to_string(),
    )
    .unwrap();
    coord.poll(&terminals);
    assert!(coord.receipts_snapshot().contains_key(req_id));

    // Matching tray identity clears receipt and removes it from push/<pid>.json.
    fs::write(
        dir.path().join("inbox").join(format!("{}.json", coord.pid)),
        json!({
            "version": 1,
            "pid": coord.pid,
            "settled_requests": {
                format!("{}:{}", coord.pid, coord.started_at): [req_id]
            }
        })
        .to_string(),
    )
    .unwrap();
    coord.poll(&terminals);
    assert!(!coord.receipts_snapshot().contains_key(req_id));
    let push_bytes = fs::read(dir.path().join("push").join(format!("{}.json", coord.pid))).unwrap();
    let push_doc: Value = serde_json::from_slice(&push_bytes).unwrap();
    assert!(push_doc["requests"].as_object().unwrap().is_empty());
}

#[test]
fn shared_contract_fixture_matches_tray_intake_and_receipts() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/derived-terminal-contract.json"
    ))
    .unwrap();

    let run_env: SpawnRequestEnvelope =
        serde_json::from_value(fixture["run_request"].clone()).unwrap();
    assert_eq!(run_env.version, 1);
    assert!(valid_uuid(&run_env.request_id));
    assert_eq!(run_env.kind, "run");
    let cfg = run_env.config.as_ref().unwrap();
    assert_eq!(cfg.account.as_deref(), Some("work"));
    assert!(cfg.account_dir.is_none());
    let iso = cfg.isolation.as_ref().unwrap();
    assert_eq!(
        iso.path.as_deref(),
        Some("/tmp/repo/.agentnave-worktrees/wt-abcdef012345")
    );
    assert_eq!(
        run_env
            .parent
            .as_ref()
            .and_then(|p| p.workspace_id.as_deref()),
        Some("s-0123456789abcdef0123456789abcdef")
    );

    let cancel_env: SpawnRequestEnvelope =
        serde_json::from_value(fixture["cancel_request"].clone()).unwrap();
    assert!(valid_uuid(&cancel_env.request_id));
    assert_eq!(cancel_env.kind, "cancel");
    assert_eq!(
        cancel_env
            .expected_anchor
            .as_ref()
            .and_then(|a| a.client_user_message_id.as_deref()),
        Some("11111111-1111-4111-8111-111111111111")
    );

    let mut receipts = HashMap::new();
    receipts.insert(
        run_env.request_id.clone(),
        ReceiptEntry {
            receipt: Receipt {
                service_pid: 1234,
                invocation_id: run_env.invocation_id.clone(),
                kind: "run".to_owned(),
                state: ReceiptState::Finished,
                session_id: Some("33333333-3333-4333-8333-333333333333".to_owned()),
                lease_acquired: Some(true),
                anchor: Some(json!({
                    "session_id": "33333333-3333-4333-8333-333333333333",
                    "prompt_id": "prompt-1"
                })),
                blocked: None,
                background_tasks_pending: None,
                terminal: Some(TerminalOutcome {
                    status: "succeeded".to_owned(),
                    reason: None,
                    warnings: Vec::new(),
                }),
            },
            provider: None,
            finished_at: Some(Instant::now()),
        },
    );
    receipts.insert(
        cancel_env.request_id.clone(),
        ReceiptEntry {
            receipt: Receipt {
                service_pid: 1234,
                invocation_id: cancel_env.invocation_id.clone(),
                kind: "cancel".to_owned(),
                state: ReceiptState::Finished,
                session_id: Some("33333333-3333-4333-8333-333333333333".to_owned()),
                lease_acquired: None,
                anchor: None,
                blocked: None,
                background_tasks_pending: None,
                terminal: Some(TerminalOutcome {
                    status: "interrupted".to_owned(),
                    reason: None,
                    warnings: Vec::new(),
                }),
            },
            provider: None,
            finished_at: Some(Instant::now()),
        },
    );
    let doc = build_push_document(
        5678,
        &["claude", "codex", "antigravity"],
        &[2468],
        &mut receipts,
    );
    let expected = if cfg!(target_os = "macos") {
        fixture["push_receipt"].clone()
    } else {
        let mut adjusted = fixture["push_receipt"].clone();
        adjusted["terminal_providers"] = json!([]);
        adjusted
    };
    assert_eq!(doc, expected);

    let settled = &fixture["inbox_settlement"]["settled_requests"];
    assert!(is_request_settled(
        settled,
        5678,
        1700000000.0,
        &run_env.request_id
    ));
}

#[cfg(unix)]
#[test]
fn rejects_group_or_world_writable_spawn_requests_dir_and_publishes_derived_pids() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    ensure_spawn_requests_dir(dir.path());
    let req_dir = dir.path().join("spawn-requests");
    let terminals = Terminals::default();
    let coord = Coordinator::with_terminals(
        dir.path().to_path_buf(),
        std::process::id(),
        current_process_started_at(),
        &["claude", "codex"],
        terminals.clone(),
    );

    // D3: Group-writable spawn-requests directory must be refused without consuming requests.
    fs::set_permissions(&req_dir, fs::Permissions::from_mode(0o770)).unwrap();
    let req_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
    let req_path = req_dir.join(format!("{req_id}.json"));
    fs::write(
        &req_path,
        json!({
            "version": 1,
            "request_id": req_id,
            "kind": "cancel",
            "service": {"pid": coord.pid, "started_at": coord.started_at},
            "target_tray": {"pid": coord.pid, "started_at": coord.started_at},
            "invocation_id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1",
        })
        .to_string(),
    )
    .unwrap();
    coord.poll(&terminals);
    assert!(req_path.exists());
    assert!(coord.receipts_snapshot().is_empty());

    // Restoring 0700 lets intake proceed normally.
    fs::set_permissions(&req_dir, fs::Permissions::from_mode(0o700)).unwrap();
    coord.poll(&terminals);
    assert!(!req_path.exists());

    // D5: Live derived terminal PID is published in push/<pid>.json `derived_pids`.
    let mut cmd = portable_pty::CommandBuilder::new("python3");
    cmd.args(["-c", "import time; time.sleep(5)"]);
    cmd.cwd(dir.path());
    let session = super::super::session::Session::start(cmd, 80, 24).unwrap();
    let expected_pid = session.pid().expect("derived child pid");
    let tid = terminals.register_derived(super::super::NewDerived {
        provider: "codex".to_owned(),
        cwd: dir.path().to_path_buf(),
        session: session.clone(),
        native: None,
        account: None,
        codex: None,
        info: super::super::DerivedInfo {
            parent_boss_key: "codex:boss".to_owned(),
            parent_workspace_id: Some("ws".to_owned()),
            busy: false,
            blocked_kind: None,
            isolation_id: None,
            _lease: None,
            _log_dir: None,
            _hook_dir: None,
            codex_ready_file: None,
            life: Default::default(),
            active_invocation_id: None,
            launch_config: None,
        },
    });
    // E3(a): publish() reads live_derived_pids() directly under publish_lock, never stale state.
    coord.publish().unwrap();
    let push_path = dir.path().join("push").join(format!("{}.json", coord.pid));
    let push_doc: Value = serde_json::from_slice(&fs::read(&push_path).unwrap()).unwrap();
    assert_eq!(push_doc["derived_pids"], json!([expected_pid]));
    terminals.remove_derived_terminal(tid);
    coord.poll(&terminals);
    let push_after: Value = serde_json::from_slice(&fs::read(&push_path).unwrap()).unwrap();
    assert_eq!(push_after["derived_pids"], json!([]));
    session.stop();
}
