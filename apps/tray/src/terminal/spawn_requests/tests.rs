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
                terminal: None,
            },
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
                    terminal: Some(TerminalOutcome {
                        status: "succeeded".to_owned(),
                        reason: Some(huge_sid.clone()),
                    }),
                },
                finished_at: Some(base + Duration::from_millis(i as u64)),
            },
        );
    }

    let doc = build_push_document(coord.pid, coord.push_providers, &mut receipts);
    let encoded = serde_json::to_vec(&doc).unwrap();
    assert!(encoded.len() <= PUSH_MAX_BYTES);
    assert!(doc["requests"].get("active-id").is_some());
    assert!(doc["terminal_providers"].as_array().is_some());
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
