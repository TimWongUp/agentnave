use super::*;
use serde_json::json;
#[cfg(unix)]
use std::time::SystemTime;

#[cfg(unix)]
fn running_terminal(
    id: u64,
    provider: &str,
    session: std::sync::Arc<super::super::session::Session>,
    derived: Option<super::super::DerivedInfo>,
) -> super::super::Running {
    super::super::Running {
        id,
        provider: provider.to_owned(),
        cwd: std::env::temp_dir(),
        session,
        workspaces: Vec::new(),
        native: None,
        title_located: false,
        title_searched: None,
        started: SystemTime::now(),
        pending: 0,
        account: None,
        codex: None,
        derived,
    }
}

#[cfg(unix)]
fn sleep_session() -> std::sync::Arc<super::super::session::Session> {
    let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
    cmd.args(["-c", "sleep 30"]);
    cmd.cwd(std::env::temp_dir());
    super::super::session::Session::start(cmd, 80, 24).unwrap()
}

#[cfg(unix)]
fn sample_item(key: &str) -> Item {
    Item {
        key: key.to_owned(),
        kind: "task_end".to_owned(),
        role: Some("实现".to_owned()),
        status: Some("succeeded".to_owned()),
    }
}

#[test]
fn ignores_stale_mtime_mismatched_started_at_and_symlinked_inbox_files() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("inbox");
    fs::create_dir(&dir).unwrap();
    let pid = std::process::id();
    let started_at = super::super::spawn_requests::current_process_started_at();
    let path = dir.join(format!("{pid}.json"));

    // New snapshot with matching started_at is accepted.
    let new_doc = json!({
        "version": 1,
        "pid": pid,
        "started_at": started_at,
        "bosses": {
            "boss-new": {
                "waiting": false,
                "in_flight": 1,
                "event_seq": 2,
                "delivered_seq": 1,
                "items": [{"key": "one", "kind": "task_end"}]
            }
        }
    });
    fs::write(&path, new_doc.to_string()).unwrap();
    let parsed = inboxes(temp.path());
    assert!(parsed["boss-new"].has_unread);
    assert_eq!(parsed["boss-new"].unread_count, 1);
    assert_eq!(parsed["boss-new"].in_flight, 1);

    // Mismatched started_at (reused PID) is rejected even when mtime is fresh.
    let stale_started = json!({
        "version": 1,
        "pid": pid,
        "started_at": started_at - 100.0,
        "bosses": {
            "boss-new": {
                "waiting": false,
                "event_seq": 2,
                "delivered_seq": 1,
                "items": [{"key": "one", "kind": "task_end"}]
            }
        }
    });
    fs::write(&path, stale_started.to_string()).unwrap();
    assert!(!inboxes(temp.path()).contains_key("boss-new"));

    // Legacy snapshot without started_at/seq fields still parses using items fallback.
    let legacy_doc = json!({
        "version": 1,
        "pid": pid,
        "bosses": {
            "boss-legacy": {
                "waiting": false,
                "items": [{"key": "old-1", "kind": "task_end"}]
            }
        }
    });
    fs::write(&path, legacy_doc.to_string()).unwrap();
    let parsed = inboxes(temp.path());
    assert!(parsed["boss-legacy"].has_unread);
    assert_eq!(parsed["boss-legacy"].unread_count, 1);

    // Stale mtime older than process start is rejected.
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let start = system.process(Pid::from_u32(pid)).unwrap().start_time();
    let old = UNIX_EPOCH + Duration::from_secs(start.saturating_sub(10));
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(!inboxes(temp.path()).contains_key("boss-legacy"));

    #[cfg(unix)]
    {
        let target = temp.path().join("target.json");
        fs::write(&target, legacy_doc.to_string()).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(!inboxes(temp.path()).contains_key("boss-legacy"));
    }
}

#[test]
fn new_and_legacy_snapshot_unread_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("inbox");
    fs::create_dir(&dir).unwrap();
    let pid = std::process::id();
    let started_at = super::super::spawn_requests::current_process_started_at();
    let path = dir.join(format!("{pid}.json"));

    let doc = json!({
        "version": 1,
        "pid": pid,
        "started_at": started_at,
        "bosses": {
            // in_flight > 0 keeps entry alive, but event_seq == delivered_seq => not unread.
            "b-inflight": {
                "waiting": false,
                "in_flight": 2,
                "event_seq": 3,
                "delivered_seq": 3,
                "items": []
            },
            // event_seq > delivered_seq without items => unread via seq diff.
            "b-seq-only": {
                "waiting": false,
                "in_flight": 0,
                "event_seq": 5,
                "delivered_seq": 3,
                "items": []
            },
            // event_seq == delivered_seq takes precedence even if stale items were present.
            "b-caught-up": {
                "waiting": false,
                "in_flight": 0,
                "event_seq": 4,
                "delivered_seq": 4,
                "items": [{"key": "stale", "kind": "task_end"}]
            },
            // Legacy entry omitting seq fields falls back to items non-empty.
            "b-legacy-unread": {
                "waiting": false,
                "items": [
                    {"key": "k1", "kind": "task_end"},
                    {"key": "k2", "kind": "prompt"}
                ]
            },
            "b-legacy-empty": {
                "waiting": false,
                "items": []
            }
        }
    });
    fs::write(&path, doc.to_string()).unwrap();
    let parsed = inboxes(temp.path());

    assert!(!parsed["b-inflight"].has_unread);
    assert_eq!(parsed["b-inflight"].unread_count, 0);
    assert_eq!(parsed["b-inflight"].in_flight, 2);

    assert!(parsed["b-seq-only"].has_unread);
    assert_eq!(parsed["b-seq-only"].unread_count, 2);

    assert!(!parsed["b-caught-up"].has_unread);
    assert_eq!(parsed["b-caught-up"].unread_count, 0);
    assert!(parsed["b-caught-up"].items.is_empty());

    assert!(parsed["b-legacy-unread"].has_unread);
    assert_eq!(parsed["b-legacy-unread"].unread_count, 2);

    assert!(!parsed["b-legacy-empty"].has_unread);
    assert_eq!(parsed["b-legacy-empty"].unread_count, 0);
}

#[cfg(unix)]
#[test]
fn badge_dwells_30s_suppresses_while_waiting_deduplicates_and_revokes_on_collect() {
    let session = sleep_session();
    let terminals = Terminals::default();
    terminals
        .lock()
        .open
        .push(running_terminal(1, "claude", session.clone(), None));

    let boss_key = "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
    let links = HashMap::from([(1u64, boss_key.clone())]);
    let mut tracker = Tracker::default();
    let mut notices = Vec::new();
    let t0 = Instant::now();

    let unread_raw = || {
        HashMap::from([(
            boss_key.clone(),
            RawBoss {
                waiting: false,
                items: vec![sample_item("evt-1")],
                in_flight: 0,
                has_unread: true,
                unread_count: 1,
            },
        )])
    };

    // T=0s and T=29s: below 30s dwell threshold -> no badge, no notification.
    tracker.scan(&terminals, &links, &unread_raw(), t0, |n| notices.push(n));
    assert_eq!(terminals.lock().open[0].pending, 0);
    assert!(notices.is_empty());

    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(29),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    assert!(notices.is_empty());

    // T=30s: dwell reached -> badge shows 1, notification fires once.
    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(30),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 1);
    assert_eq!(
        notices,
        vec![UnreadNotice {
            boss_key: boss_key.clone(),
            count: 1,
            derived: false,
        }]
    );

    // T=35s: still unread -> badge stays 1, notification is deduplicated.
    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(35),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 1);
    assert_eq!(notices.len(), 1);

    // Blocking wait starts (`waiting=true`) -> badge suppressed immediately, dwell resets.
    let waiting_raw = HashMap::from([(
        boss_key.clone(),
        RawBoss {
            waiting: true,
            items: vec![sample_item("evt-1")],
            in_flight: 1,
            has_unread: true,
            unread_count: 1,
        },
    )]);
    tracker.scan(
        &terminals,
        &links,
        &waiting_raw,
        t0 + Duration::from_secs(36),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    assert_eq!(notices.len(), 1);

    // If wait exits without collecting evt-1, dwell must restart for a fresh 30s.
    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(40),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(69),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    tracker.scan(
        &terminals,
        &links,
        &unread_raw(),
        t0 + Duration::from_secs(70),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 1);
    assert_eq!(
        notices.len(),
        1,
        "same uncollected episode must not re-notify"
    );

    // Wait collects results (entry retained because in_flight=1, but has_unread=false) -> revoked.
    let collected_raw = HashMap::from([(
        boss_key.clone(),
        RawBoss {
            waiting: false,
            items: Vec::new(),
            in_flight: 1,
            has_unread: false,
            unread_count: 0,
        },
    )]);
    tracker.scan(
        &terminals,
        &links,
        &collected_raw,
        t0 + Duration::from_secs(75),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);

    // Next episode (seq-only without items) dwells 30s -> badge shows and notification re-arms.
    let next_raw = HashMap::from([(
        boss_key.clone(),
        RawBoss {
            waiting: false,
            items: Vec::new(),
            in_flight: 0,
            has_unread: true,
            unread_count: 2,
        },
    )]);
    tracker.scan(
        &terminals,
        &links,
        &next_raw,
        t0 + Duration::from_secs(80),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    tracker.scan(
        &terminals,
        &links,
        &next_raw,
        t0 + Duration::from_secs(110),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 2);
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[1].count, 2);

    session.stop();
}

#[cfg(unix)]
#[test]
fn derived_terminal_acting_as_boss_gets_badge_and_notice_without_pty_writes() {
    let session = sleep_session();
    session.record_writes();
    let terminals = Terminals::default();
    terminals.lock().open.push(running_terminal(
        7,
        "antigravity",
        session.clone(),
        Some(super::super::DerivedInfo {
            parent_boss_key: "s-00000000000000000000000000000000".to_owned(),
            parent_workspace_id: Some("ws-parent".to_owned()),
            busy: false,
            blocked_kind: None,
            isolation_id: None,
            _lease: None,
            _log_dir: None,
            codex_ready_file: None,
            active_invocation_id: None,
            launch_config: None,
        }),
    ));

    let boss_key = "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned();
    let links = HashMap::from([(7u64, boss_key.clone())]);
    let raw = HashMap::from([(
        boss_key.clone(),
        RawBoss {
            waiting: false,
            items: vec![sample_item("evt-derived")],
            in_flight: 0,
            has_unread: true,
            unread_count: 1,
        },
    )]);

    let mut tracker = Tracker::default();
    let mut notices = Vec::new();
    let t0 = Instant::now();
    tracker.scan(&terminals, &links, &raw, t0, |n| notices.push(n));
    assert_eq!(terminals.lock().open[0].pending, 0);

    tracker.scan(
        &terminals,
        &links,
        &raw,
        t0 + Duration::from_secs(30),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 1);
    assert_eq!(
        notices,
        vec![UnreadNotice {
            boss_key,
            count: 1,
            derived: true,
        }]
    );
    assert!(
        session.recorded_writes().is_empty(),
        "unread scan must never type into a terminal PTY"
    );

    // Unbound terminal clears badge and never notifies for external bosses.
    let empty_links = HashMap::new();
    tracker.scan(
        &terminals,
        &empty_links,
        &raw,
        t0 + Duration::from_secs(60),
        |n| notices.push(n),
    );
    assert_eq!(terminals.lock().open[0].pending, 0);
    assert_eq!(notices.len(), 1);

    session.stop();
}
