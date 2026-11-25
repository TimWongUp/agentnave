use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use portable_pty::CommandBuilder;
use serde_json::{Value, json};

use super::super::account::Active;
use super::super::codex_server;
use super::super::derived;
use super::super::session::Session;
use super::super::spawn_requests::{Coordinator, ReceiptState, RunReceipt};
use super::super::unread::{Item, RawBoss};
use super::super::{DerivedInfo, NewDerived, Running, Terminals, wakeup};
use super::idle::{Beat, IDLE_GRACE};
use super::{Relaunched, ResumeSlot};
use crate::data::boss_app::boss_key;

const SID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const WS: &str = "s-work";
const PARENT: &str = "s-parent";

mod bosses;
mod wake;

/// A fake CLI that reports the states typed to it.
fn cli() -> Arc<Session> {
    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "stty -echo; printf up; while read -r line; do printf '\\033]7501;state=%s\\007' \"$line\"; done",
    ]);
    command.cwd(std::env::temp_dir());
    let session = Session::start(command, 80, 24).unwrap();
    until("the fake CLI is up", || {
        session.recent_screen_text(64).contains("up")
    });
    session
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn report(session: &Session, state: &str) {
    let before = session.osc_snapshot().0;
    session.write(format!("{state}\n").as_bytes()).unwrap();
    until(state, || session.osc_snapshot().0 > before);
}

struct Office {
    root: tempfile::TempDir,
    terminals: Terminals,
    start: Instant,
    chats: HashMap<u64, Vec<String>>,
    inboxes: HashMap<String, RawBoss>,
    runs: Vec<RunReceipt>,
}

impl Office {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            terminals: Terminals::default(),
            start: Instant::now(),
            chats: HashMap::new(),
            inboxes: HashMap::new(),
            runs: Vec::new(),
        }
    }

    /// A derived terminal between turns, its CLI ready for input.
    fn derived(&self, provider: &str) -> (u64, Arc<Session>) {
        let session = cli();
        report(&session, "idle");
        (
            self.register(provider, Some(SID), session.clone(), None),
            session,
        )
    }

    fn register(
        &self,
        provider: &str,
        native: Option<&str>,
        session: Arc<Session>,
        invocation: Option<&str>,
    ) -> u64 {
        self.terminals
            .register_derived(self.spec(provider, native, session, invocation))
    }

    fn spec(
        &self,
        provider: &str,
        native: Option<&str>,
        session: Arc<Session>,
        invocation: Option<&str>,
    ) -> NewDerived {
        NewDerived {
            provider: provider.to_owned(),
            cwd: self.root.path().to_path_buf(),
            session,
            native: native.map(str::to_owned),
            account: None,
            codex: None,
            info: DerivedInfo {
                parent_boss_key: PARENT.to_owned(),
                parent_workspace_id: Some(WS.to_owned()),
                busy: invocation.is_some(),
                blocked_kind: None,
                isolation_id: None,
                _lease: None,
                _log_dir: None,
                _hook_dir: Some(tempfile::tempdir().unwrap()),
                codex_ready_file: None,
                life: Default::default(),
                active_invocation_id: invocation.map(str::to_owned),
                launch_config: None,
            },
        }
    }

    fn boss(&self, workspaces: &[&str]) -> (u64, Arc<Session>) {
        let session = cli();
        let id = self.terminals.reserve_id();
        self.terminals.lock().open.push(Running {
            id,
            provider: "claude".to_owned(),
            cwd: self.root.path().to_path_buf(),
            session: session.clone(),
            workspaces: workspaces.iter().map(|w| (*w).to_owned()).collect(),
            native: None,
            title_located: false,
            title_searched: None,
            started: SystemTime::now(),
            pending: 0,
            account: None,
            codex: None,
            derived: None,
        });
        (id, session)
    }

    /// One beat of the background scan, `after` the start.
    fn beat(&self, after: Duration) {
        self.terminals.collapse_idle(&Beat {
            root: self.root.path(),
            now: self.start + after,
            chats: &self.chats,
            inboxes: &self.inboxes,
            runs: self.runs.clone(),
        });
    }

    fn idle_for(&self, span: Duration) {
        self.beat(Duration::ZERO);
        self.beat(span);
    }

    fn rows(&self) -> Vec<Value> {
        let (_, open) = self.terminals.snapshot(self.root.path());
        open.into_iter().map(|(_, _, value)| value).collect()
    }

    fn row(&self, id: u64) -> Value {
        self.rows()
            .into_iter()
            .find(|row| row["id"] == id)
            .unwrap_or_default()
    }

    fn state(&self, id: u64) -> String {
        self.row(id)["derived_state"]
            .as_str()
            .unwrap_or("gone")
            .to_owned()
    }

    fn with<T>(&self, id: u64, act: impl FnOnce(&mut Running) -> T) -> T {
        let mut state = self.terminals.lock();
        act(state.open.iter_mut().find(|r| r.id == id).unwrap())
    }

    fn hook(&self, id: u64, event: Value) {
        let path = self.with(id, |running| {
            let hook_dir = running.derived.as_ref().unwrap()._hook_dir.as_ref();
            derived::claude_events_path(hook_dir.unwrap().path())
        });
        let mut events = fs::read_to_string(&path).unwrap_or_default();
        events.push_str(&format!("{event}\n"));
        fs::write(path, events).unwrap();
    }

    /// A turn of Claude's that ended with `tasks` in its `Stop`; None for an older CLI without them.
    fn claude_turn(&self, id: u64, session: &Session, tasks: Option<Value>) {
        report(session, "working");
        let mut stop = json!({"hook_event_name": "Stop", "session_id": SID, "prompt_id": "p"});
        if let Some(tasks) = tasks {
            stop["background_tasks"] = tasks;
        }
        self.hook(id, stop);
        report(session, "done");
    }

    fn receipt(&self, session_id: Option<&str>, invocation: &str) -> RunReceipt {
        RunReceipt {
            invocation_id: invocation.to_owned(),
            session_id: session_id.map(str::to_owned),
            provider: None,
        }
    }
}

fn unread(key: &str) -> RawBoss {
    RawBoss {
        items: vec![Item {
            key: key.to_owned(),
            kind: "task_end".to_owned(),
            role: None,
            status: None,
            delivered: false,
        }],
        has_unread: true,
        unread_count: 1,
        ..RawBoss::default()
    }
}

fn ended(session: &Session) {
    until("the CLI ends", || session.exit().is_some());
}

#[test]
fn a_derived_session_has_one_state_whoever_started_the_turn() {
    let mut office = Office::new();
    let session = cli();
    let id = office.register("claude", Some(SID), session.clone(), Some("inv-1"));
    assert_eq!(office.state(id), "starting");
    // Nothing but the user can answer a trust or permission box, at startup or in a turn.
    report(&session, "blocked");
    assert_eq!(office.state(id), "waiting");
    report(&session, "idle");
    assert_eq!(office.state(id), "running", "the dispatched turn is on");
    let since = office.row(id)["derived_since"].as_f64().unwrap();
    assert!((super::now_unix_secs() - since).abs() < 5.0);
    assert_eq!(office.row(id)["derived_since"], since);
    office.terminals.clear_derived_turn(id, Some(SID));
    assert_eq!(office.state(id), "idle");

    // Collected by `wait`, the entry is gone or marked delivered, and no longer in `items`.
    office
        .inboxes
        .insert(PARENT.to_owned(), unread("task_end:inv-1"));
    office.beat(Duration::ZERO);
    assert_eq!(office.state(id), "uncollected");
    office
        .inboxes
        .insert(PARENT.to_owned(), unread("task_end:inv-other"));
    office.beat(Duration::ZERO);
    assert_eq!(office.state(id), "idle");

    // The user's own turn.
    report(&session, "working");
    assert_eq!(office.state(id), "running");
    report(&session, "blocked");
    assert_eq!(office.state(id), "waiting");
    report(&session, "done");
    assert_eq!(office.state(id), "idle");
    assert_eq!(office.row(id)["has_draft"], false);
    session.user_input(b"draft", false).unwrap();
    assert_eq!(office.row(id)["has_draft"], true);
    assert_eq!(office.row(id)["resumable"], false);

    session.stop();
    ended(&session);
    assert_eq!(office.state(id), "dormant");
    assert_eq!(office.row(id)["resumable"], true);
    // Without its native session or its account it cannot come back.
    office.with(id, |running| running.native = None);
    assert_eq!(office.state(id), "exited");
    assert_eq!(office.row(id)["resumable"], false);
    office.with(id, |running| {
        running.native = Some(SID.to_owned());
        running.account = Some(Active {
            id: "gone".to_owned(),
            label: "Gone".to_owned(),
            dir: std::env::temp_dir(),
        });
    });
    assert_eq!(office.state(id), "exited");
}

#[test]
fn three_quiet_minutes_collapse_an_idle_session_and_keep_its_row() {
    for provider in ["claude", "antigravity"] {
        let mut office = Office::new();
        let (id, session) = office.derived(provider);
        office.claude_turn(id, &session, Some(json!([])));
        // Its result not being collected yet is no reason to keep it.
        office.with(id, |running| {
            running.derived.as_mut().unwrap().life.claimed_by("inv-1");
        });
        office
            .inboxes
            .insert(PARENT.to_owned(), unread("task_end:inv-1"));
        office.idle_for(IDLE_GRACE - Duration::from_secs(1));
        assert_eq!(office.state(id), "uncollected");
        office.beat(IDLE_GRACE);
        assert_eq!(office.state(id), "dormant", "{provider}");
        ended(&session);
        assert_eq!(office.row(id)["resumable"], true);
        assert_eq!(office.rows().len(), 1);
    }
}

#[test]
fn anything_unfinished_or_untold_keeps_the_process() {
    type Keep = fn(&mut Office, u64, &Arc<Session>);
    let cases: [(&str, &str, Keep); 13] = [
        ("draft", "claude", |_, _, s| {
            s.user_input(b"draft", false).unwrap();
        }),
        ("user's own turn", "claude", |_, _, s| report(s, "working")),
        ("permission box", "claude", |_, _, s| report(s, "blocked")),
        ("dispatched turn", "claude", |o, id, _| {
            o.terminals
                .try_claim_derived_turn(id, "inv-2", None)
                .unwrap();
        }),
        ("receipt of the session", "claude", |o, _, _| {
            o.runs = vec![o.receipt(Some(SID), "inv-2")];
        }),
        (
            "settlement the service has not confirmed",
            "claude",
            |o, id, _| {
                o.with(id, |r| {
                    r.derived.as_mut().unwrap().life.claimed_by("inv-1");
                });
                o.runs = vec![o.receipt(None, "inv-1")];
            },
        ),
        ("own work in flight", "claude", |o, _, _| {
            let busy = RawBoss {
                in_flight: 1,
                ..RawBoss::default()
            };
            o.inboxes.insert(boss_key("claude", SID), busy);
        }),
        ("own results to collect", "claude", |o, id, _| {
            o.chats.insert(id, vec!["s-own".to_owned()]);
            o.inboxes.insert("s-own".to_owned(), unread("task_end:x"));
        }),
        ("background task", "claude", |o, id, s| {
            let tasks = json!([{"id": "b1", "type": "shell", "status": "running"}]);
            o.claude_turn(id, s, Some(tasks));
        }),
        ("Stop without background_tasks", "claude", |o, id, s| {
            o.claude_turn(id, s, None);
        }),
        ("turn that ended without a Stop", "claude", |_, _, s| {
            report(s, "working");
            report(s, "idle");
        }),
        (
            "interrupted turn after a clear one",
            "claude",
            |o, id, s| {
                o.claude_turn(id, s, Some(json!([])));
                o.beat(Duration::ZERO);
                report(s, "working");
                report(s, "idle");
            },
        ),
        ("status that cannot be told", "antigravity", |_, _, s| {
            report(s, "clear");
        }),
    ];
    for (name, provider, keep) in cases {
        let mut office = Office::new();
        let (id, session) = office.derived(provider);
        keep(&mut office, id, &session);
        office.idle_for(IDLE_GRACE * 2);
        assert!(session.exit().is_none(), "{name}");
        assert!(!["dormant", "gone"].contains(&office.state(id).as_str()));
        assert!(
            !office.with(id, |r| r.derived.as_ref().unwrap().life.stopping()),
            "{name}"
        );
        session.stop();
    }
}

#[test]
fn looking_at_a_terminal_restarts_its_three_minutes() {
    let office = Office::new();
    let (id, session) = office.derived("claude");
    office.beat(Duration::ZERO);
    let looked = IDLE_GRACE - Duration::from_secs(5);
    office
        .terminals
        .lock()
        .shown
        .insert(id, office.start + looked);
    office.beat(looked);
    office.beat(IDLE_GRACE);
    // The report has lapsed by now; the minutes count from here.
    let left = looked + Duration::from_secs(10);
    office.beat(left);
    office.beat(left + IDLE_GRACE - Duration::from_secs(1));
    assert_eq!(office.state(id), "idle");
    office.beat(left + IDLE_GRACE);
    assert_eq!(office.state(id), "dormant");
    ended(&session);
    office.terminals.mark_shown(id);
    assert!(office.terminals.lock().is_shown(id, Instant::now()));
}

#[test]
fn a_redispatch_and_a_collapse_never_cross() {
    let office = Office::new();
    let (id, session) = office.derived("claude");
    office.beat(Duration::ZERO);
    // Claimed first: the collapse that was due is off.
    office
        .terminals
        .try_claim_derived_turn(id, "inv-2", None)
        .unwrap();
    office.beat(IDLE_GRACE);
    assert_eq!(office.state(id), "running");
    assert!(session.exit().is_none());
    office.terminals.clear_derived_turn(id, Some(SID));

    // Collapsed first: nothing is dispatched to the process on its way out, and the re-dispatch
    // waits for it and starts again in the same row.
    office.with(id, |running| drop(running.begin_stop()));
    assert!(
        office
            .terminals
            .try_claim_derived_turn(id, "inv-3", None)
            .is_err()
    );
    let stopper = {
        let session = session.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            session.stop();
        })
    };
    let slot = office.terminals.resume_slot("claude", SID).unwrap();
    stopper.join().unwrap();
    assert!(session.exit().is_some());
    assert!(matches!(slot, ResumeSlot::Dormant(_)));
    let revived = cli();
    let again = office.register("claude", Some(SID), revived.clone(), Some("inv-3"));
    drop(slot);
    assert_eq!(again, id);
    assert_eq!(office.rows().len(), 1);
    assert_eq!(office.state(id), "starting");
    assert!(office.row(id)["exit"].is_null());

    // A start that fails leaves the row collapsed instead of taking it away.
    let handle = office.terminals.derived_by_invocation("inv-3").unwrap();
    derived::cleanup_derived_startup(&office.terminals, &handle, false, Some(SID));
    ended(&revived);
    assert_eq!(office.state(id), "dormant");

    // Let go of without a process, the row can be taken again.
    drop(office.terminals.resume_slot("claude", SID).unwrap());
    assert!(matches!(
        office.terminals.resume_slot("claude", SID),
        Ok(ResumeSlot::Dormant(_))
    ));
}

#[test]
fn codex_is_collapsed_only_while_every_loaded_thread_is_freshly_known_idle() {
    let states = Arc::new(std::sync::Mutex::new(HashMap::from([
        (SID.to_owned(), json!({"type": "idle"})),
        ("sub".to_owned(), json!({"type": "active"})),
    ])));
    let dir = tempfile::tempdir().unwrap();
    let served = states.clone();
    let codex =
        codex_server::tests::connect_to_mock(&dir.path().join("c.sock"), move |msg, out| {
            let states = served.lock().unwrap();
            let result = match msg["method"].as_str() {
                Some("initialize") => json!({}),
                Some("thread/loaded/list") => json!({"data": states.keys().collect::<Vec<_>>()}),
                Some("thread/read") => {
                    let tid = msg["params"]["threadId"].as_str().unwrap();
                    json!({"thread": {"id": tid, "status": states[tid]}})
                }
                _ => return,
            };
            let reply = json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
            codex_server::tests::write_server_frame(out, &reply.to_string());
        });
    let office = Office::new();
    let session = cli();
    let mut spec = office.spec("codex", Some(SID), session.clone(), None);
    spec.codex = Some(codex.clone());
    let id = office.terminals.register_derived(spec);
    assert_eq!(office.state(id), "idle");
    // A subagent's thread works.
    office.idle_for(IDLE_GRACE);
    assert_eq!(office.state(id), "idle");
    // Known idle, but the user's turn has started since and was not pushed.
    states.lock().unwrap().remove("sub");
    codex.refresh_threads();
    office.beat(IDLE_GRACE);
    states
        .lock()
        .unwrap()
        .insert(SID.to_owned(), json!({"type": "active"}));
    office.beat(IDLE_GRACE * 2);
    assert_eq!(office.state(id), "running");
    assert!(session.exit().is_none());
    states.lock().unwrap().insert(
        SID.to_owned(),
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    codex.refresh_threads();
    assert_eq!(office.state(id), "waiting");
    states
        .lock()
        .unwrap()
        .insert(SID.to_owned(), json!({"type": "idle"}));
    codex.refresh_threads();
    office.beat(IDLE_GRACE * 3);
    office.beat(IDLE_GRACE * 4);
    assert_eq!(office.state(id), "dormant");
    ended(&session);
}

#[test]
fn stopping_a_derived_terminal_collapses_it_and_settles_its_turn() {
    let office = Office::new();
    let coordinator = Coordinator::new(
        office.root.path().to_path_buf(),
        std::process::id(),
        0.0,
        &["claude"],
    );
    coordinator.insert_test_receipt("req-stop", "inv-stop");
    wakeup::set_coordinator(coordinator.clone());
    let session = cli();
    report(&session, "idle");
    let id = office.register("claude", Some(SID), session.clone(), Some("inv-stop"));
    assert_eq!(office.terminals.stop(Some(id)).status, 200);
    let receipt = coordinator.run_receipt_by_invocation("inv-stop").unwrap();
    assert_eq!(receipt.state, ReceiptState::Finished);
    assert_eq!(receipt.terminal.unwrap().status, "closed");
    ended(&session);
    assert_eq!(office.state(id), "dormant");
    assert_eq!(office.terminals.stop(Some(999)).status, 404);
}
