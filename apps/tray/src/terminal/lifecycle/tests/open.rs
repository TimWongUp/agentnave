//! Opening a derived session from its desk, with or without a row or a saved launch (ADR-0124).

use super::wake::{body, relaunched};
use super::*;
use crate::response::Response;
use crate::terminal::spawn_requests::{ProcessIdentity, RunConfig, SpawnRequestEnvelope};
use crate::terminal::{DerivedHandle, DerivedLaunchConfig, thread_desk_key};

fn desk() -> String {
    thread_desk_key("claude", SID)
}

/// The service history of workspace `WS`: one task conversation with these tasks.
pub(super) fn history(root: &Path, tasks: &[Value]) {
    let db = crate::db::create(root);
    db.execute(
        "INSERT INTO conversations (id, kind, title, created, host, host_source, \
         manager_session, cwd, review, isolated, seats, sessions, rev) VALUES ('c1', 'task', \
         't', '2026-11-25T00:00:00+00:00', 'claude', 'unknown', ?1, NULL, 0, 0, '[]', '{}', 1)",
        [WS],
    )
    .unwrap();
    for (position, task) in tasks.iter().enumerate() {
        db.execute(
            "INSERT INTO tasks (id, conversation_id, position, provider, session_id, status, \
             body) VALUES (?1, 'c1', ?2, ?3, ?4, 'succeeded', ?5)",
            rusqlite::params![
                format!("t{position}"),
                position as i64,
                task["provider"].as_str(),
                task["session_id"].as_str(),
                task.to_string()
            ],
        )
        .unwrap();
    }
}

/// A call of the session that ran in an App terminal.
pub(super) fn terminal_task(provider: &str, cwd: &Path, time: &str) -> Value {
    json!({"provider": provider, "session_id": SID, "execution_mode": "terminal",
        "cwd": cwd, "model": "cheap", "effort": "low", "time": time})
}

impl Office {
    fn open(&self, launch: impl FnOnce(&WakePlan) -> Result<Relaunched, String>) -> Response {
        self.terminals
            .open_derived_in(self.root.path(), WS, &desk(), |_, _| None, launch)
    }

    fn saved_file(&self) -> PathBuf {
        self.root
            .path()
            .join("config")
            .join("derived-launches.json")
    }
}

use super::super::WakePlan;

#[test]
fn opening_a_session_without_a_row_starts_it_from_the_history_and_submits_nothing() {
    let office = Office::new();
    let root = office.root.path();
    office.terminals.save_launches_in(&root.join("config"));
    history(
        root,
        &[
            terminal_task("claude", root, "2026-11-25T01:00:00+00:00"),
            json!({"provider": "claude", "session_id": "other", "execution_mode": "terminal",
                "cwd": root, "time": "2026-11-25T02:00:00+00:00"}),
        ],
    );
    let woken = cli();
    woken.record_writes();
    let reply = office.open(|plan| {
        assert_eq!(
            (plan.provider.as_str(), plan.native.as_str()),
            ("claude", SID)
        );
        assert_eq!(plan.cwd, root);
        let config = plan.config.as_ref().unwrap();
        assert_eq!(config.permissions, json!({}), "the CLI's own permissions");
        assert_eq!(config.model.as_deref(), Some("cheap"));
        assert!(plan.assumed);
        Ok(relaunched(woken.clone()))
    });
    assert_eq!(reply.status, 200, "{:?}", body(&reply));
    let session = &body(&reply)["session"];
    let id = session["id"].as_u64().unwrap();
    assert_eq!(session["native"], SID);
    assert_eq!(session["parent_workspace_id"], WS);
    assert_eq!(session["parent_boss_key"], WS);
    assert_eq!(office.rows().len(), 1);

    // Open again: it runs, so nothing starts.
    let again = office.open(|_| panic!("it runs already"));
    assert_eq!(body(&again)["session"]["id"], id);
    assert!(woken.recorded_writes().is_empty());
    report(&woken, "idle");
    office.beat(Duration::ZERO);
    assert_eq!(office.state(id), "idle");
    assert!(
        !office.saved_file().exists(),
        "an assumed launch is not saved"
    );

    // Its process ended, opening it wakes the same row.
    woken.stop();
    ended(&woken);
    let revived = cli();
    let woke = office.open(|_| Ok(relaunched(revived.clone())));
    assert_eq!(body(&woke)["session"]["id"], id);
    assert_eq!(office.rows().len(), 1);
    revived.stop();
}

#[test]
fn what_the_history_cannot_tell_or_the_session_cannot_reach_is_refused_with_why() {
    let office = Office::new();
    let root = office.root.path();
    let refused = |desk: &str| {
        let reply = office.terminals.open_derived_in(
            root,
            WS,
            desk,
            |_, _| None,
            |_| panic!("nothing starts"),
        );
        (
            reply.status,
            body(&reply)["error"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(refused(&desk()).0, 404, "no history yet");
    let mut background = terminal_task("claude", root, "2026-11-25T01:00:00+00:00");
    background["execution_mode"] = json!("background");
    let mut released = terminal_task("antigravity", &root.join("wt"), "2026-11-25T01:00:00+00:00");
    released["isolation_id"] = json!("wt-abcdef012345");
    history(root, &[background, released]);
    assert_eq!(refused(&desk()).0, 404, "it never ran in a terminal");
    assert_eq!(refused("").0, 400);
    let agy = thread_desk_key("antigravity", SID);
    assert_eq!(refused(&agy), (409, "隔离目录已释放，无法恢复".to_owned()));
    // Nothing is left held: the next try says the same.
    assert_eq!(refused(&agy), (409, "隔离目录已释放，无法恢复".to_owned()));
    assert!(office.rows().is_empty());

    let elsewhere = office.terminals.open_derived_in(
        root,
        WS,
        &agy,
        |provider, native| {
            assert_eq!((provider, native), ("antigravity", SID));
            Some(Response::error(409, "这段聊天正在别的程序里打开（运行中）"))
        },
        |_| panic!("not before the user confirms"),
    );
    assert_eq!(elsewhere.status, 409);
}

#[test]
fn a_saved_launch_is_what_the_session_starts_with_again() {
    let office = Office::new();
    let root = office.root.path();
    let config = root.join("config");
    let elsewhere = tempfile::tempdir().unwrap();
    history(
        root,
        &[terminal_task("claude", root, "2026-11-25T01:00:00+00:00")],
    );
    fs::create_dir_all(&config).unwrap();
    let saved = json!({"version": 1, "sessions": {format!("claude:{SID}"): {
        "cwd": elsewhere.path(), "model": "saved-model", "effort": null,
        "permissions": {"permission_mode": "acceptEdits"}, "account": null, "account_dir": null,
        "isolation_id": null, "fallback_mode": "default", "parent_workspace_id": "s-saved",
        "parent_boss_key": "k-saved"}}});
    fs::write(config.join("derived-launches.json"), saved.to_string()).unwrap();
    office.terminals.save_launches_in(&config);
    let woken = cli();
    let reply = office.open(|plan| {
        assert_eq!(plan.cwd, elsewhere.path());
        assert_eq!(plan.fallback_mode.as_deref(), Some("default"));
        let config = plan.config.as_ref().unwrap();
        assert_eq!(
            config.permissions,
            json!({"permission_mode": "acceptEdits"})
        );
        assert_eq!(config.model.as_deref(), Some("saved-model"));
        assert!(!plan.assumed);
        Ok(relaunched(woken.clone()))
    });
    assert_eq!(reply.status, 200, "{:?}", body(&reply));
    assert_eq!(body(&reply)["session"]["parent_workspace_id"], "s-saved");
    woken.stop();
}

#[test]
fn opening_twice_or_beside_a_redispatch_starts_one_process() {
    let office = Office::new();
    let root = office.root.path();
    history(
        root,
        &[terminal_task("claude", root, "2026-11-25T01:00:00+00:00")],
    );
    let started = AtomicUsize::new(0);
    let woken = cli();
    let open = || {
        office.open(|_| {
            started.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(300));
            Ok(relaunched(woken.clone()))
        })
    };
    let (replies, slot) = thread::scope(|scope| {
        let clicks: Vec<_> = (0..2).map(|_| scope.spawn(open)).collect();
        until("the session is held", || {
            !office.terminals.lock().opening.is_empty()
        });
        // A re-dispatch meanwhile waits for the process the click starts, and takes that.
        let slot = office.terminals.resume_slot("claude", SID).unwrap();
        let replies: Vec<_> = clicks.into_iter().map(|c| c.join().unwrap()).collect();
        (replies, slot)
    });
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let id = body(&replies[0])["session"]["id"].clone();
    for reply in &replies {
        assert_eq!(reply.status, 200);
        assert_eq!(body(reply)["session"]["id"], id);
    }
    assert!(matches!(&slot, ResumeSlot::Live(live) if json!(live.id) == id));
    assert_eq!(office.rows().len(), 1);
    assert!(office.terminals.lock().opening.is_empty());
    woken.stop();
}

#[test]
fn a_session_the_user_opens_after_its_boss_is_gone_ends_only_after_fifteen_idle_minutes() {
    let office = Office::new();
    let root = office.root.path();
    history(
        root,
        &[terminal_task("claude", root, "2026-11-25T01:00:00+00:00")],
    );
    let (boss, boss_session) = office.boss(&[WS]);
    boss_session.stop();
    ended(&boss_session);
    let woken = cli();
    let reply = office.open(|_| Ok(relaunched(woken.clone())));
    let id = body(&reply)["session"]["id"].as_u64().unwrap();
    report(&woken, "idle");
    office.beat(Duration::ZERO);
    assert_eq!(office.state(id), "idle");
    assert_eq!(office.row(id)["parent_terminal_id"], boss);
    office.beat(IDLE_GRACE - Duration::from_secs(1));
    assert_eq!(office.state(id), "idle");
    office.beat(IDLE_GRACE);
    ended(&woken);
    assert!(office.row(id).is_null());
}

/// A re-dispatch of the session's turn with `cfg`.
fn redispatch(
    office: &Office,
    cfg: &RunConfig,
    inv: &str,
) -> Result<(DerivedHandle, bool), String> {
    let root = office.root.path();
    let coordinator = Coordinator::new(root.to_path_buf(), std::process::id(), 0.0, &["claude"]);
    let identity = ProcessIdentity {
        pid: std::process::id(),
        started_at: 0.0,
    };
    let env = SpawnRequestEnvelope {
        version: 1,
        request_id: inv.to_owned(),
        kind: "run".to_owned(),
        service: identity.clone(),
        target_tray: identity,
        invocation_id: inv.to_owned(),
        parent: None,
        session_id: Some(SID.to_owned()),
        config: None,
        task: None,
        expected_anchor: None,
    };
    let handle = coordinator.register_run(inv);
    derived::prepare_open_or_launch(
        &coordinator,
        &office.terminals,
        &env,
        cfg,
        root,
        None,
        &handle,
        |_, _| {
            let mut spec = office.spec("claude", Some(SID), cli(), Some(inv));
            spec.info.launch_config = Some(DerivedLaunchConfig::from_run_config(cfg));
            office.terminals.register_derived(spec);
            Ok(office.terminals.derived_by_invocation(inv).unwrap())
        },
    )
}

#[test]
fn a_redispatch_to_a_session_on_an_assumed_launch_starts_it_again_with_its_own() {
    let office = Office::new();
    let root = office.root.path();
    history(
        root,
        &[terminal_task("claude", root, "2026-11-25T01:00:00+00:00")],
    );
    let woken = cli();
    let reply = office.open(|_| Ok(relaunched(woken.clone())));
    let id = body(&reply)["session"]["id"].as_u64().unwrap();
    let cfg = RunConfig {
        provider: "claude".to_owned(),
        account: None,
        account_dir: None,
        cwd: root.display().to_string(),
        isolation: None,
        model: Some("cheap".to_owned()),
        effort: Some("low".to_owned()),
        permissions: json!({"permission_mode": "acceptEdits"}),
    };
    report(&woken, "working");
    assert_eq!(
        redispatch(&office, &cfg, "inv-busy").err().unwrap(),
        derived::LIVE_BUSY_ERR
    );
    report(&woken, "idle");
    let (handle, reused) = redispatch(&office, &cfg, "inv-1").unwrap();
    assert!(!reused);
    assert_eq!(handle.id, id, "the same row");
    ended(&woken);
    assert_eq!(office.rows().len(), 1);
    let (config, assumed) = office.with(id, |r| {
        let derived = r.derived.as_ref().unwrap();
        (derived.launch_config.clone(), derived.life.config_assumed)
    });
    assert_eq!(config, Some(DerivedLaunchConfig::from_run_config(&cfg)));
    assert!(!assumed);

    // Started on a launch it was given, another one is refused as before.
    office.terminals.clear_derived_turn(id, Some(SID));
    let mut other = cfg.clone();
    other.permissions = json!({"permission_mode": "default"});
    assert_eq!(
        redispatch(&office, &other, "inv-2").err().unwrap(),
        derived::LAUNCH_CONFIG_MISMATCH_ERR
    );
    handle.session.stop();
}

#[test]
fn a_launch_is_saved_once_for_only_the_owner_and_forgotten_with_its_history() {
    let office = Office::new();
    let root = office.root.path();
    let config = root.join("config");
    office.terminals.save_launches_in(&config);
    let (id, session) = office.derived("claude");
    let launch = DerivedLaunchConfig {
        cwd: root.to_path_buf(),
        model: Some("m".to_owned()),
        effort: None,
        permissions: json!({"permission_mode": "auto"}),
    };
    office.with(id, |r| {
        r.derived.as_mut().unwrap().launch_config = Some(launch.clone());
    });
    office.beat(Duration::ZERO);
    let file = office.saved_file();
    let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    let record = &saved["sessions"][format!("claude:{SID}")];
    assert_eq!(record["permissions"], json!({"permission_mode": "auto"}));
    assert_eq!(record["parent_boss_key"], PARENT);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Unchanged, it is not written again.
    fs::remove_file(&file).unwrap();
    office.beat(Duration::from_secs(1));
    assert!(!file.exists());
    office.with(id, |r| {
        r.derived.as_mut().unwrap().life.fallback_mode = Some("default".into())
    });
    office.beat(Duration::from_secs(2));
    let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        saved["sessions"][format!("claude:{SID}")]["fallback_mode"],
        "default"
    );
    session.stop();

    // Kept while the history cannot be read; forgotten once it no longer has the session.
    let fresh = Terminals::default();
    fresh.save_launches_in(&config);
    fresh.prune_saved_launches(root);
    assert!(fresh.saved_launch("claude", SID).is_some());
    history(
        root,
        &[json!({"provider": "claude", "session_id": "other"})],
    );
    fresh.prune_saved_launches(root);
    assert!(fresh.saved_launch("claude", SID).is_none());
    let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["sessions"], json!({}));
}

#[test]
fn saved_title_persists_without_rewriting_unchanged_and_reads_old_records() {
    let office = Office::new();
    let root = office.root.path();
    let config = root.join("config");
    fs::create_dir_all(&config).unwrap();
    // Old record without "title" field still loads cleanly.
    let old = json!({"version": 1, "sessions": {format!("claude:{SID}"): {
        "cwd": root, "model": "m", "effort": null,
        "permissions": {"permission_mode": "auto"}, "account": null, "account_dir": null,
        "isolation_id": null, "fallback_mode": null, "parent_workspace_id": WS,
        "parent_boss_key": PARENT}}});
    let file = office.saved_file();
    fs::write(&file, old.to_string()).unwrap();
    office.terminals.save_launches_in(&config);
    assert_eq!(
        office.terminals.saved_launch("claude", SID).unwrap().title,
        None
    );

    // Live derived session observes a CLI title -> saved once, reported in derived_titles,
    // and preserved if another beat runs before title lookup runs on a fresh process.
    let (id, session) = office.derived("claude");
    let launch = DerivedLaunchConfig {
        cwd: root.to_path_buf(),
        model: Some("m".to_owned()),
        effort: None,
        permissions: json!({"permission_mode": "auto"}),
    };
    office.with(id, |r| {
        let derived = r.derived.as_mut().unwrap();
        derived.launch_config = Some(launch);
        derived.life.title = Some("修派生标题".to_owned());
    });
    office.beat(Duration::ZERO);
    let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        saved["sessions"][format!("claude:{SID}")]["title"],
        "修派生标题"
    );
    assert_eq!(
        office
            .terminals
            .saved_titles()
            .get(&desk())
            .map(String::as_str),
        Some("修派生标题")
    );

    fs::remove_file(&file).unwrap();
    office.beat(Duration::from_secs(1));
    assert!(!file.exists(), "unchanged title must not rewrite");
    session.stop();
}
