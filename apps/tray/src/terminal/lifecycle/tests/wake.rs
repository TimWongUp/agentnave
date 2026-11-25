use super::*;
use crate::isolated::leases;

pub(super) fn relaunched(session: Arc<Session>) -> Relaunched {
    Relaunched {
        session,
        log_dir: None,
        hook_dir: Some(tempfile::tempdir().unwrap()),
        codex: None,
    }
}

pub(super) fn body(reply: &crate::response::Response) -> Value {
    serde_json::from_slice(&reply.body).unwrap()
}

#[test]
fn waking_starts_one_process_in_the_same_row_and_submits_nothing() {
    let office = Office::new();
    let (id, session) = office.derived("claude");
    office.with(id, |r| {
        r.derived.as_mut().unwrap().life.claimed_by("inv-1");
    });
    let root = office.root.path();
    let live = office
        .terminals
        .wake_in(root, Some(id), |_, _| None, |_| panic!("it runs already"));
    assert_eq!(body(&live)["session"]["derived_state"], "idle");

    session.stop();
    let started = AtomicUsize::new(0);
    let woken = cli();
    woken.record_writes();
    let wake = || {
        office.terminals.wake_in(
            root,
            Some(id),
            |_, _| None,
            |plan| {
                started.fetch_add(1, Ordering::SeqCst);
                assert_eq!(plan.native, SID);
                assert_eq!(plan.cwd, root);
                thread::sleep(Duration::from_millis(200));
                Ok(relaunched(woken.clone()))
            },
        )
    };
    let replies: Vec<_> = thread::scope(|scope| {
        let clicks: Vec<_> = (0..2).map(|_| scope.spawn(wake)).collect();
        clicks.into_iter().map(|c| c.join().unwrap()).collect()
    });
    assert_eq!(started.load(Ordering::SeqCst), 1);
    for reply in &replies {
        assert_eq!(reply.status, 200);
        assert_eq!(body(reply)["session"]["id"], id);
        assert_eq!(body(reply)["session"]["derived_state"], "starting");
    }
    assert_eq!(office.rows().len(), 1);
    assert!(woken.recorded_writes().is_empty());
    assert!(office.terminals.derived_by_invocation("inv-1").is_none());

    // Opened only to look, its process ends again after its fifteen minutes.
    report(&woken, "idle");
    office.idle_for(IDLE_GRACE);
    assert_eq!(office.state(id), "dormant");
    ended(&woken);
}

#[test]
fn what_cannot_be_woken_says_why_and_can_be_tried_again() {
    let office = Office::new();
    let root = office.root.path();
    let (id, session) = office.derived("claude");
    let (boss, boss_session) = office.boss(&[WS]);
    session.stop();
    ended(&session);
    let refused = |id: u64, launch: Result<(), &str>| {
        let reply = office.terminals.wake_in(
            root,
            Some(id),
            |_, _| None,
            |_| match launch {
                Ok(()) => panic!("nothing may start"),
                Err(error) => Err(error.to_owned()),
            },
        );
        (
            reply.status,
            body(&reply)["error"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(refused(999, Ok(())).0, 404);
    assert_eq!(refused(boss, Ok(())).0, 409);
    assert_eq!(
        refused(id, Err("这个 CLI 没有安装")),
        (409, "这个 CLI 没有安装".to_owned())
    );
    let cwd = root.join("wt");
    office.with(id, |r| {
        r.cwd = cwd.clone();
        r.derived.as_mut().unwrap().isolation_id = Some("wt-abcdef012345".to_owned());
    });
    assert_eq!(
        refused(id, Ok(())),
        (409, "隔离目录已释放，无法恢复".to_owned())
    );
    assert_eq!(office.state(id), "dormant");
    // Why it cannot come back is told when it is opened.
    assert_eq!(office.row(id)["resumable"], true);
    office.with(id, |r| {
        r.account = Some(Active {
            id: "gone".to_owned(),
            label: "Gone".to_owned(),
            dir: PathBuf::from("/nowhere"),
        });
    });
    let (status, error) = refused(id, Ok(()));
    assert_eq!(status, 409);
    assert!(error.contains("账号不可用"), "{error}");
    boss_session.stop();
}

#[test]
fn an_isolated_session_holds_its_lease_only_while_its_process_runs() {
    let office = Office::new();
    let repo = office.root.path().join("repo");
    let wt = office.root.path().join("wt");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let done = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(done.success());
    };
    git(&["init", "--quiet"]);
    git(&["commit", "--allow-empty", "--quiet", "-m", "init"]);
    git(&["worktree", "add", "--quiet", wt.to_str().unwrap()]);
    let common: &Path = &repo.join(".git");
    let iso = "wt-abcdef012345";
    let held = || leases::has_active_leases(common, iso).unwrap();
    let (id, session) = office.derived("claude");
    let lease = leases::acquire_lease(common, iso, Duration::from_secs(3)).unwrap();
    office.with(id, |r| r.cwd = wt.clone());
    office.terminals.attach_derived_lease(id, Some(iso), lease);

    office.idle_for(IDLE_GRACE);
    assert!(held(), "the process is only on its way out");
    ended(&session);
    office.beat(IDLE_GRACE);
    assert!(!held());

    let woken = cli();
    let reply = office.terminals.wake_in(
        office.root.path(),
        Some(id),
        |_, _| None,
        |_| {
            assert!(held(), "taken before the CLI starts in it");
            Ok(relaunched(woken.clone()))
        },
    );
    assert_eq!(reply.status, 200);
    office.beat(IDLE_GRACE);
    assert!(held());
    office.terminals.close(Some(id));
    assert!(!held());
    ended(&woken);
}
