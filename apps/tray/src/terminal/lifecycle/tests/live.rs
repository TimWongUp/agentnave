//! Opt-in end-to-end run of the derived-session lifecycle (ADR-0124) against the real, logged-in
//! CLIs, one provider per test: dispatch, collapse, wake, a turn the user types, re-dispatch, end
//! then re-dispatch, and the boss's end. It goes through the tray's own code (`execute_run`,
//! `Terminals::wake_in`, `collapse_idle`, `close`), with the data root in a temp directory and the
//! fifteen-minute grace driven by an injected clock. Costs quota; each test runs a handful of turns
//! on the cheap model.
//!
//! `cargo test live_lifecycle -- --ignored --test-threads=1 --nocapture`

use std::collections::{BTreeSet, HashMap};
use std::process::Command;
use std::sync::Mutex;

use super::super::tick;
use super::*;
use crate::terminal::spawn_requests::{current_process_started_at, ensure_spawn_requests_dir};

/// The checkout the CLIs are already trusted in; the prompts touch nothing in it.
const CWD: &str = "/Users/tim/Code/agentnave";
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(180);
/// The wakeup coordinator is process-global, so the three providers take turns.
static SERIAL: Mutex<()> = Mutex::new(());

struct Provider {
    name: &'static str,
    model: &'static str,
    effort: Option<&'static str>,
}

#[test]
#[ignore = "真实 Claude CLI，消耗额度，需要显式授权"]
fn live_lifecycle_claude() {
    live_lifecycle(&Provider {
        name: "claude",
        model: "claude-haiku-5-5",
        effort: None,
    });
}

#[test]
#[ignore = "真实 Codex CLI，消耗额度，需要显式授权"]
fn live_lifecycle_codex() {
    live_lifecycle(&Provider {
        name: "codex",
        model: "gpt-6-luna",
        effort: Some("low"),
    });
}

#[test]
#[ignore = "真实 Antigravity CLI，消耗额度，需要显式授权"]
fn live_lifecycle_antigravity() {
    live_lifecycle(&Provider {
        name: "antigravity",
        model: "gemini-3.8-flash-low",
        effort: None,
    });
}

/// One office on a temp data root with a fake boss terminal, and every process the CLIs start.
struct Live {
    office: Office,
    coordinator: Coordinator,
    provider: &'static str,
    model: &'static str,
    effort: Option<&'static str>,
    elapsed: Duration,
    requests: u32,
    pids: BTreeSet<u32>,
}

impl Drop for Live {
    fn drop(&mut self) {
        let ids: Vec<u64> = self
            .office
            .rows()
            .iter()
            .filter_map(|r| r["id"].as_u64())
            .collect();
        for id in ids {
            self.office.terminals.close(Some(id));
        }
        thread::sleep(Duration::from_millis(500));
        for pid in self.pids.iter().copied().filter(|pid| alive(*pid)) {
            eprintln!("清理遗留进程 {pid}");
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

/// Every process under `root`, itself included.
fn tree(root: u32) -> Vec<u32> {
    let listing = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(|f| f.parse::<u32>().ok());
            Some((fields.next()??, fields.next()??))
        })
        .collect();
    let mut found = vec![root];
    let mut at = 0;
    while at < found.len() {
        let parent = found[at];
        found.extend(
            pairs
                .iter()
                .filter(|(_, ppid)| *ppid == parent)
                .map(|(pid, _)| *pid),
        );
        at += 1;
    }
    found
}

/// Running, not a zombie waiting to be reaped.
fn alive(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.trim().starts_with('Z')
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

impl Live {
    fn new(provider: &Provider) -> Self {
        let office = Office::new();
        let root = office.root.path();
        ensure_spawn_requests_dir(root);
        fs::create_dir_all(root.join("inbox")).unwrap();
        let coordinator = Coordinator::with_terminals(
            root.to_path_buf(),
            std::process::id(),
            current_process_started_at(),
            wakeup::PROVIDERS,
            office.terminals.clone(),
        );
        coordinator.publish().unwrap();
        wakeup::set_coordinator(coordinator.clone());
        Self {
            office,
            coordinator,
            provider: provider.name,
            model: provider.model,
            effort: provider.effort,
            elapsed: Duration::ZERO,
            requests: 0,
            pids: BTreeSet::new(),
        }
    }

    fn root(&self) -> &Path {
        self.office.root.path()
    }

    /// The one derived row.
    fn derived(&self) -> u64 {
        let rows = self.office.rows();
        let derived: Vec<_> = rows
            .iter()
            .filter(|r| r.get("derived_state").is_some())
            .collect();
        assert_eq!(derived.len(), 1, "one derived row expected: {rows:?}");
        derived[0]["id"].as_u64().unwrap()
    }

    fn session(&self, id: u64) -> Arc<Session> {
        self.office.with(id, |r| r.session.clone())
    }

    /// Remembers every process the row's CLI has started, to check later that none is left.
    fn track(&mut self, id: u64) -> Vec<u32> {
        let root = self.session(id).pid().expect("CLI pid");
        let found = tree(root);
        self.pids.extend(found.iter().copied());
        found
    }

    fn screen(&self, id: u64) -> String {
        self.session(id).recent_screen_text(4_000_000)
    }

    /// What the screen shows now, with the cursor moves a streaming TUI draws by applied.
    fn rendered(&self, id: u64) -> String {
        derived::rendered_screen(&self.session(id).raw_output_from(0).1)
    }

    /// Dispatches one turn the way the service does: a request file the tray takes in.
    fn dispatch(&mut self, resume: Option<&str>, text: &str) -> (String, Vec<String>) {
        self.requests += 1;
        let n = self.requests;
        let request = format!("{n:08x}-0000-4000-8000-{:012x}", std::process::id());
        let invocation = format!("inv-{n}-{}", std::process::id());
        let pid = self.coordinator.pid;
        let started = self.coordinator.started_at;
        let doc = json!({
            "version": 1,
            "request_id": request,
            "kind": "run",
            "service": {"pid": pid, "started_at": started},
            "target_tray": {"pid": pid, "started_at": started},
            "invocation_id": invocation,
            "parent": {"boss_key": PARENT, "workspace_id": WS},
            "session_id": resume,
            "config": {
                "provider": self.provider,
                "cwd": CWD,
                "model": self.model,
                "effort": self.effort,
                "permissions": {},
            },
            "task": {"text": text},
        });
        fs::write(
            self.root()
                .join("spawn-requests")
                .join(format!("{request}.json")),
            doc.to_string(),
        )
        .unwrap();
        self.coordinator.poll(&self.office.terminals);

        let mut seen: Vec<String> = Vec::new();
        let mut last_screen = String::new();
        let deadline = Instant::now() + DISPATCH_TIMEOUT;
        let receipt = loop {
            if let Some(id) = self.office.rows().iter().find_map(|r| {
                r["id"]
                    .as_u64()
                    .filter(|_| r.get("derived_state").is_some())
            }) {
                last_screen = self.screen(id);
            }
            self.coordinator.poll(&self.office.terminals);
            if let Some(state) = self.states().into_iter().next()
                && seen.last() != Some(&state)
            {
                seen.push(state);
            }
            let receipt = self.coordinator.run_receipt_by_invocation(&invocation);
            if let Some(done) = receipt.filter(|r| r.state == ReceiptState::Finished) {
                break done;
            }
            assert!(
                Instant::now() < deadline,
                "dispatch {n} did not settle; states {seen:?}"
            );
            thread::sleep(Duration::from_millis(100));
        };
        let outcome = receipt.terminal.as_ref().expect("terminal outcome");
        if outcome.status != "succeeded" {
            println!("last screen: {}", tail(&last_screen));
        }
        println!(
            "[{}] dispatch {n}: status={} reason={:?} session={:?} states={seen:?}",
            self.provider, outcome.status, outcome.reason, receipt.session_id
        );
        assert_eq!(outcome.status, "succeeded", "{outcome:?}");
        self.settle(&request, &invocation);
        (receipt.session_id.clone().expect("native session id"), seen)
    }

    /// What the service does once it has taken the result in: acknowledge it in its inbox file.
    fn settle(&self, request: &str, invocation: &str) {
        let pid = self.coordinator.pid;
        let tray = json!({"pid": pid, "started_at": self.coordinator.started_at});
        fs::write(
            self.root().join("inbox").join(format!("{pid}.json")),
            json!({"version": 1, "pid": pid, "settled_requests": {request: {"tray": tray}}})
                .to_string(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self
            .coordinator
            .run_receipt_by_invocation(invocation)
            .is_some()
        {
            assert!(Instant::now() < deadline, "the receipt was never settled");
            self.coordinator.poll(&self.office.terminals);
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// The derived rows' states, as the pages would read them.
    fn states(&self) -> Vec<String> {
        self.office
            .rows()
            .iter()
            .filter_map(|r| r["derived_state"].as_str().map(str::to_owned))
            .collect()
    }

    fn wait_state(&self, id: u64, wanted: &[&str], timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let state = self.office.state(id);
            if wanted.contains(&state.as_str()) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "state stayed {state:?}, wanted {wanted:?}; screen tail: {}",
                tail(&self.screen(id))
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// One beat of the background scan, `advance` later on the injected clock.
    fn tick(&mut self, advance: Duration) {
        self.elapsed += advance;
        let seen = wakeup::Observed {
            chats: HashMap::new(),
            inboxes: HashMap::new(),
        };
        tick(
            &self.office.terminals,
            &self.coordinator,
            self.root(),
            &seen,
            self.office.start + self.elapsed,
        );
    }

    fn wait_gone(&self, pids: &[u32], what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left: Vec<u32> = pids.iter().copied().filter(|p| alive(*p)).collect();
            if left.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: still running {left:?}");
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// Fifteen idle minutes on the injected clock end the process and keep the row.
    fn collapse(&mut self, id: u64, tree: &[u32]) {
        // The grace starts at the first beat that finds nothing holding it.
        let began = Instant::now();
        let deadline = began + Duration::from_secs(30);
        loop {
            self.tick(Duration::ZERO);
            if self
                .office
                .with(id, |r| r.derived.as_ref().unwrap().life.process.quiet_since)
                .is_some()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "never quiet: state {}; has_draft {}; osc {:?}; codex {}",
                self.office.state(id),
                self.session(id).has_draft(),
                self.session(id).osc_snapshot(),
                self.codex_threads(id)
            );
            thread::sleep(Duration::from_millis(250));
        }
        println!("[{}] quiet after {:?}", self.provider, began.elapsed());
        self.tick(IDLE_GRACE - Duration::from_secs(1));
        assert_eq!(
            self.office.state(id),
            "idle",
            "collapsed before its fifteen minutes"
        );
        assert!(self.session(id).exit().is_none());
        self.tick(Duration::from_secs(2));
        let session = self.session(id);
        let deadline = Instant::now() + Duration::from_secs(20);
        while session.exit().is_none() {
            assert!(
                Instant::now() < deadline,
                "not collapsed after fifteen idle minutes; state {}; has_draft {}; osc {:?}; codex {}",
                self.office.state(id),
                session.has_draft(),
                session.osc_snapshot(),
                self.codex_threads(id)
            );
            thread::sleep(Duration::from_millis(100));
        }
        self.wait_gone(tree, "after the collapse");
        assert_eq!(self.office.state(id), "dormant");
        assert_eq!(self.office.row(id)["resumable"], true);
    }

    /// The user opens the collapsed row: the same ID, nothing submitted, no screen to answer.
    fn wake(&mut self, id: u64) {
        let terminals = self.office.terminals.clone();
        let reply = self.office.terminals.wake_in(
            self.root(),
            Some(id),
            |_, _| None,
            move |plan| {
                let started = derived::relaunch(&terminals, plan)?;
                started.session.record_writes();
                Ok(started)
            },
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["session"]["id"], id);
        let mut seen = vec![
            body["session"]["derived_state"]
                .as_str()
                .unwrap()
                .to_owned(),
        ];
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut waiting: Option<Instant> = None;
        let mut waited = None;
        loop {
            let state = self.office.state(id);
            if seen.last() != Some(&state) {
                seen.push(state.clone());
            }
            if state != "waiting" && waited.is_none() {
                waited = waiting.map(|since| since.elapsed());
            }
            if state == "idle" {
                break;
            }
            // Antigravity reports blocked/auth for the seconds it signs in with the stored login.
            let signing_in = state == "waiting"
                && self.session(id).osc_snapshot().1.as_deref() == Some("blocked")
                && self.session(id).osc_snapshot().2.as_deref() == Some("auth");
            if signing_in {
                waiting.get_or_insert_with(Instant::now);
            }
            assert!(
                state == "starting" || signing_in,
                "unexpected {state:?} while waking: {seen:?}; osc {:?}; codex {}; screen: {}",
                self.session(id).osc_snapshot(),
                self.codex_threads(id),
                tail(&self.screen(id))
            );
            assert!(
                Instant::now() < deadline,
                "wake never reached idle: {seen:?}"
            );
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_secs(2));
        let session = self.session(id);
        let writes = session.recorded_writes();
        let screen = self.screen(id).to_lowercase();
        println!(
            "[{}] wake: states={seen:?} (waiting for {waited:?}) writes={writes:?}",
            self.provider
        );
        assert!(
            writes
                .iter()
                .all(|w| w.as_slice() == crate::terminal::status::REPLY),
            "the wake wrote to the CLI: {writes:?}"
        );
        match self.office.with(id, |r| r.codex.clone()) {
            // Codex's own screen reports working while it loads a thread; its turns are the
            // app-server's.
            Some(codex) => {
                let native = self.native(id);
                assert!(!matches!(
                    codex.thread_status(&native),
                    Some(codex_server::ThreadStatus::Active { .. })
                ));
                assert_eq!(
                    codex.completed_turns_for_test(&native),
                    0,
                    "the wake ran a turn"
                );
            }
            None => assert_eq!(
                session.osc_activity_seqs().0,
                0,
                "the woken CLI worked on a turn"
            ),
        }
        for asking in [
            "do you trust",
            "trust this folder",
            "trust this workspace",
            "yes, i trust",
        ] {
            assert!(!screen.contains(asking), "the woken CLI asks: {asking}");
        }
        assert_eq!(self.office.state(id), "idle");
    }

    /// The end of the per-process log Antigravity is given, for a failure message.
    fn log_tail(&self, id: u64) -> String {
        let path = self.office.with(id, |r| {
            r.derived
                .as_ref()
                .and_then(|d| d._log_dir.as_ref().map(|dir| dir.path().join("agy.log")))
        });
        let text = path
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }

    /// What the Codex client knows of its threads, for a failure message.
    fn codex_threads(&self, id: u64) -> String {
        let Some(codex) = self.office.with(id, |r| r.codex.clone()) else {
            return "none".to_owned();
        };
        let threads: Vec<String> = codex
            .loaded_thread_ids_for_test()
            .into_iter()
            .map(|tid| format!("{tid}={:?}", codex.thread_status_for_test(&tid)))
            .collect();
        format!(
            "connected={} threads={threads:?}",
            codex.is_connected_for_test()
        )
    }

    fn native(&self, id: u64) -> String {
        self.office
            .with(id, |r| r.native.clone())
            .expect("native session id")
    }

    /// A turn the user types: the text, then Enter on its own.
    fn user_turn(&mut self, id: u64, text: &str) -> Vec<String> {
        let session = self.session(id);
        session.user_input(text.as_bytes(), false).unwrap();
        thread::sleep(Duration::from_millis(800));
        session.user_input(b"\r", false).unwrap();
        let mut seen = vec![self.office.state(id)];
        let deadline = Instant::now() + DISPATCH_TIMEOUT;
        let mut worked = false;
        let mut ticks = 0;
        let began = Instant::now();
        let mut osc: Vec<(Duration, Option<String>)> = Vec::new();
        loop {
            let reported = session.osc_snapshot().1;
            if osc.last().map(|(_, s)| s) != Some(&reported) {
                osc.push((began.elapsed(), reported));
            }
            let state = self.office.state(id);
            if seen.last() != Some(&state) {
                seen.push(state.clone());
            }
            if state == "running" {
                worked = true;
                // Thirty minutes on the injected clock while it works: it must never be collapsed.
                self.tick(Duration::from_secs(30));
                ticks += 1;
                assert!(
                    self.session(id).exit().is_none(),
                    "collapsed in the middle of a turn"
                );
            } else if worked && state == "idle" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the user's turn never ended: {seen:?}"
            );
            thread::sleep(Duration::from_millis(200));
        }
        assert!(ticks > 0, "no beat fell in the turn: {seen:?}");
        println!("[{}] user turn osc: {osc:?}", self.provider);
        println!(
            "[{}] user turn: states={seen:?}, {ticks} beats of 30s while it ran",
            self.provider
        );
        seen
    }
}

fn tail(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(600)..].iter().collect()
}

fn live_lifecycle(provider: &Provider) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut live = Live::new(provider);
    let code = format!("ZEBRA{}", std::process::id() % 10_000);
    let lower = code.to_lowercase();
    let (boss, boss_session) = live.office.boss(&[WS]);
    let ask = |n: u32| {
        format!(
            "我之前让你记住的暗号是什么？只回复一行 a{n}=<暗号全部转成小写>，不要使用任何工具。"
        )
    };
    // The cheap models now and then end a turn without a word; a question is asked again up to
    // three times, and every retry is printed.
    let answered = |live: &Live, id: u64, n: u32| {
        let want = format!("a{n}={lower}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !normalized(&live.rendered(id)).contains(&want) {
            if Instant::now() >= deadline {
                println!(
                    "[{}] no answer {want:?} on screen: {}\nCLI log tail: {}",
                    provider.name,
                    tail(&live.rendered(id)),
                    live.log_tail(id)
                );
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
        true
    };

    // 1. First dispatch: it runs, settles successfully, and the row goes from running to idle.
    let (native, seen) = live.dispatch(
        None,
        &format!("请记住暗号 {code}。只回复一句话「已记住」，不要使用任何工具，不要读写文件。"),
    );
    assert!(
        seen.iter().any(|s| s == "running"),
        "never seen running: {seen:?}"
    );
    let id = live.derived();
    live.wait_state(id, &["idle", "uncollected"], Duration::from_secs(30));
    assert_eq!(live.native(id), native);
    assert_eq!(live.office.row(id)["parent_terminal_id"], boss);
    let tree1 = live.track(id);
    println!("[{}] 1 ok: native={native} tree={tree1:?}", provider.name);

    // 2. Fifteen idle minutes (injected) end the whole process tree and keep the row.
    live.collapse(id, &tree1);
    println!("[{}] 2 ok: collapsed, row {id} dormant", provider.name);

    // 3. Opening it starts the same session in the same row and submits nothing.
    live.wake(id);
    assert_eq!(live.derived(), id);
    let tree3 = live.track(id);
    println!("[{}] 3 ok: woken, tree={tree3:?}", provider.name);

    // 7. A turn the user types into the woken CLI: running, then idle, never collapsed meanwhile.
    //    For Codex this also needs the thread subscription made when the row was woken.
    let mut attempts = 0..3;
    assert!(
        attempts.any(|attempt| {
            let n = 10 + attempt;
            let seen = live.user_turn(id, &ask(n));
            assert!(seen.iter().any(|s| s == "running"), "{seen:?}");
            answered(&live, id, n)
        }),
        "the user's turn was never answered"
    );
    println!("[{}] 7 ok: user turn answered", provider.name);

    // 4. The boss dispatches to the same session; the live row takes it.
    assert!(
        (0..3).any(|attempt| {
            let (again, seen) = live.dispatch(Some(&native), &ask(20 + attempt));
            assert_eq!(again, native);
            assert!(seen.iter().any(|s| s == "running"), "{seen:?}");
            assert_eq!(live.derived(), id);
            answered(&live, id, 20 + attempt)
        }),
        "the boss's turn was never answered"
    );
    live.wait_state(id, &["idle", "uncollected"], Duration::from_secs(30));
    if let Some(codex) = live.office.with(id, |r| r.codex.clone()) {
        let turns = codex_completed_turns(&codex, &native);
        assert!(
            turns >= 2,
            "the resumed thread's subscription saw {turns} completed turns"
        );
    }
    println!("[{}] 4 ok: boss re-dispatch answered", provider.name);

    // 5. Ended by the user, the row goes; the boss still resumes the same session by its ID.
    let tree5 = live.track(id);
    assert_eq!(live.office.terminals.close(Some(id)).status, 200);
    live.wait_gone(&tree5, "after the user ended it");
    assert!(live.office.row(id).is_null(), "the ended row stays");
    let mut id = id;
    assert!(
        (0..3).any(|attempt| {
            let (after_end, _) = live.dispatch(Some(&native), &ask(30 + attempt));
            assert_eq!(after_end, native);
            id = live.derived();
            answered(&live, id, 30 + attempt)
        }),
        "the turn after the end was never answered"
    );
    live.wait_state(id, &["idle", "uncollected"], Duration::from_secs(30));
    println!("[{}] 5 ok: dispatched after end", provider.name);

    // 6. Ending the boss takes the derived process and its row with it.
    let tree6 = live.track(id);
    assert_eq!(live.office.terminals.close(Some(boss)).status, 200);
    live.wait_gone(&tree6, "after the boss ended");
    assert!(
        live.office.row(id).is_null(),
        "the derived row stays after its boss"
    );
    assert!(live.office.row(boss).is_null());
    ended(&boss_session);
    println!("[{}] 6 ok: boss cascade", provider.name);

    // Nothing of what was started is left.
    let leftovers: Vec<u32> = live.pids.iter().copied().filter(|p| alive(*p)).collect();
    assert!(leftovers.is_empty(), "left running: {leftovers:?}");
    println!("[{}] all processes gone: {:?}", provider.name, live.pids);
}

/// How many turns of the thread the app-server's notifications reported completed.
fn codex_completed_turns(codex: &codex_server::Client, native: &str) -> usize {
    codex.completed_turns_for_test(native)
}

#[test]
#[ignore = "真实 Claude CLI，消耗额度，需要显式授权"]
fn live_open_derived_claude() {
    live_open_derived(&Provider {
        name: "claude",
        model: "claude-haiku-5-5",
        effort: None,
    });
}

#[test]
#[ignore = "真实 Codex CLI，消耗额度，需要显式授权"]
fn live_open_derived_codex() {
    live_open_derived(&Provider {
        name: "codex",
        model: "gpt-6-luna",
        effort: Some("low"),
    });
}

#[test]
#[ignore = "真实 Antigravity CLI，消耗额度，需要显式授权"]
fn live_open_derived_antigravity() {
    live_open_derived(&Provider {
        name: "antigravity",
        model: "gemini-3.8-flash-low",
        effort: None,
    });
}

/// The App forgets every row (as on a restart); opening the desk takes the session up again from
/// the service history and the saved launch, and it still knows what it was told.
fn live_open_derived(provider: &Provider) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut live = Live::new(provider);
    let code = format!("OKAPI{}", std::process::id() % 10_000);
    let want = format!("a1={}", code.to_lowercase());
    let (_, boss_session) = live.office.boss(&[WS]);
    live.office
        .terminals
        .save_launches_in(&live.root().join("config"));
    let (native, _) = live.dispatch(
        None,
        &format!("请记住暗号 {code}。只回复一句话「已记住」，不要使用任何工具，不要读写文件。"),
    );
    let id = live.derived();
    live.wait_state(id, &["idle", "uncollected"], Duration::from_secs(30));
    live.tick(Duration::ZERO);
    assert!(
        live.office
            .terminals
            .saved_launch(provider.name, &native)
            .is_some()
    );
    let tree = live.track(id);

    let rows = std::mem::take(&mut live.office.terminals.lock().open);
    super::super::end_removed(rows);
    live.wait_gone(&tree, "after the rows were forgotten");
    super::open::history(
        live.root(),
        &[
            json!({"provider": provider.name, "session_id": native, "execution_mode": "terminal",
            "cwd": CWD, "model": provider.model, "effort": provider.effort,
            "time": "2026-11-25T01:00:00+00:00"}),
        ],
    );
    let terminals = live.office.terminals.clone();
    let reply = live.office.terminals.open_derived_in(
        live.root(),
        WS,
        &crate::terminal::thread_desk_key(provider.name, &native),
        |_, _| None,
        move |plan| derived::relaunch(&terminals, plan),
    );
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let opened = live.derived();
    assert_eq!(live.native(opened), native);
    live.wait_state(opened, &["idle"], Duration::from_secs(90));
    live.track(opened);
    // Shown as idle, it takes the boss's next turn.
    let (again, _) = live.dispatch(Some(&native), "只回复一句话「收到」，不要使用任何工具。");
    assert_eq!(again, native);
    assert_eq!(live.derived(), opened);
    live.wait_state(opened, &["idle", "uncollected"], Duration::from_secs(30));
    let answered = (0..3).any(|_| {
        live.user_turn(
            opened,
            "我之前让你记住的暗号是什么？只回复一行 a1=<暗号全部转成小写>，不要使用任何工具。",
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !normalized(&live.rendered(opened)).contains(&want) {
            if Instant::now() >= deadline {
                println!(
                    "[{}] no answer: {}",
                    provider.name,
                    tail(&live.rendered(opened))
                );
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
        true
    });
    assert!(answered, "the opened session did not remember the code");
    println!("[{}] opened from the desk and answered", provider.name);
    boss_session.stop();
}
