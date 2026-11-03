use super::*;

struct Fixture {
    dir: tempfile::TempDir,
    reader: Reader,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let reader = Reader::new(dir.path().into());
        Self { dir, reader }
    }

    fn save(&self, id: &str, director: &Value, rounds: &[Value]) {
        let view = json!({
            "version": 1,
            "summary": {"id": id, "title": "工作区", "mode": director["mode"],
                "status": "running", "updated": "2026-10-15T00:10:00Z"},
            "director": director,
        });
        crate::db::put(
            self.dir.path(),
            id,
            &json!({
                "kind": director["mode"], "title": "工作区", "rounds": rounds,
            }),
            &view,
        );
    }

    fn observe(&self, monitor: &mut Monitor, options: &Options) -> Vec<Notice> {
        monitor.observe(self.reader.alert_observations().unwrap(), options)
    }
}

fn options() -> Options {
    Options::from(json!({"alerts": {"notify": "system"}}).as_object().unwrap())
}

fn task(status: &str, ms: u64) -> Value {
    json!({"mode": "task", "tasks": [{"id": "task-1", "status": status,
        "time": "2026-10-15T00:00:00Z", "elapsed_ms": ms,
        "prompt": "secret prompt", "output": "secret answer"}]})
}

fn seat(id: &str, status: &str, finished: Option<&str>) -> Value {
    json!({"seat_id": id, "status": status, "finished": finished, "output": "secret"})
}

fn show(seats: &[Value]) -> Value {
    json!({"mode": "show", "messages": [{"kind": "task", "round": 1,
        "time": "2026-10-15T00:00:00Z", "text": "secret instructions"}],
        "round": {"number": 1, "status": "running", "seats": seats}})
}

#[test]
fn startup_baseline_does_not_replay_finished_tasks_or_meetings() {
    let f = Fixture::new();
    f.save("task", &task("succeeded", 600_000), &[]);
    f.save(
        "room",
        &show(&[seat("a", "succeeded", Some("2026-10-15T00:10:00Z"))]),
        &[],
    );
    let mut monitor = Monitor::default();
    assert!(f.observe(&mut monitor, &options()).is_empty());
    assert!(f.observe(&mut monitor, &options()).is_empty());
    f.save("new-task", &task("succeeded", 600_000), &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
}

#[test]
fn successful_tasks_require_the_threshold_inclusively() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    for (id, ms, expected) in [
        ("short", 599_999, 0),
        ("exact", 600_000, 1),
        ("long", 600_001, 1),
    ] {
        f.save(id, &task("succeeded", ms), &[]);
        assert_eq!(f.observe(&mut monitor, &options()).len(), expected);
    }
    let mut instant = options();
    instant.threshold_ms = 0;
    f.save("instant", &task("succeeded", 0), &[]);
    assert_eq!(f.observe(&mut monitor, &instant).len(), 1);
}

#[test]
fn a_seat_finishing_does_not_notify_until_the_whole_round_finishes_once() {
    let f = Fixture::new();
    let mut show = show(&[seat("a", "running", None), seat("b", "pending", None)]);
    f.save("room", &show, &[]);
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    show["round"]["seats"][0] = seat("a", "succeeded", Some("2026-10-15T00:06:00Z"));
    f.save("room", &show, &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
    show["round"]["seats"][1] = seat("b", "succeeded", Some("2026-10-15T00:10:00Z"));
    f.save("room", &show, &[]);
    let notices = f.observe(&mut monitor, &options());
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].body, "完成 · 10分0秒");
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn a_review_pause_ends_a_serial_round_and_does_not_notify_again_after_approval() {
    let f = Fixture::new();
    let mut show = show(&[seat("a", "running", None), seat("b", "not_started", None)]);
    f.save("room", &show, &[]);
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    show["round"]["seats"][0] = seat("a", "succeeded", Some("2026-10-15T00:10:00Z"));
    show["round"]["seats"][0]["review"] = json!("pending");
    f.save("room", &show, &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
    show["round"]["seats"][0]["review"] = json!("publish");
    show["round"]["seats"][1] = seat("b", "succeeded", Some("2026-10-15T00:20:00Z"));
    f.save("room", &show, &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn meetings_scope_also_filters_private_task_problems() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.meetings_only = true;
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    f.save("task", &task("succeeded", 600_000), &[]);
    f.save("failed-task", &task("failed", 1), &[]);
    f.save(
        "room",
        &show(&[
            seat("a", "blocked", Some("2026-10-15T00:00:01Z")),
            seat("b", "running", None),
        ]),
        &[],
    );
    let notices = f.observe(&mut monitor, &opts);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].body, "受阻 · 0分1秒");
}

#[test]
fn problems_notify_immediately_once_for_tasks_and_seats_without_the_threshold() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    f.save("task", &task("failed", 1), &[]);
    let mut show = show(&[
        seat("a", "blocked", Some("2026-10-15T00:00:01Z")),
        seat("b", "running", None),
    ]);
    f.save("room", &show, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 2);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    show["round"]["seats"][1] = seat("b", "failed", Some("2026-10-15T00:00:02Z"));
    f.save("room", &show, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
}

#[test]
fn problems_false_and_review_do_not_trigger_problem_notifications() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    f.observe(&mut monitor, &opts);
    f.save("task", &task("failed", 1), &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    opts.problems = true;
    let mut show = show(&[seat("a", "succeeded", Some("2026-10-15T00:00:01Z"))]);
    show["round"]["seats"][0]["review"] = json!("pending");
    f.save("room", &show, &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
}

#[test]
fn off_consumes_events_so_enabling_notifications_does_not_catch_up() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let disabled = Options::from(&Map::new());
    f.observe(&mut monitor, &disabled);
    f.save("task", &task("succeeded", 600_000), &[]);
    f.save("failed-task", &task("failed", 1), &[]);
    assert!(f.observe(&mut monitor, &disabled).is_empty());
    let mut opts = options();
    opts.problems = true;
    assert!(f.observe(&mut monitor, &opts).is_empty());
}

#[test]
fn database_keeps_a_round_that_finished_between_polls_before_another_round_started() {
    let f = Fixture::new();
    let mut show = show(&[seat("a", "running", None)]);
    f.save("room", &show, &[]);
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    let round = json!({"id": "r1", "number": 1, "status": "finished", "seats": [
        seat("a", "succeeded", Some("2026-10-15T00:10:00Z"))]});
    show["round"] =
        json!({"number": 2, "status": "running", "seats": [seat("a", "running", None)]});
    show["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"round": 2, "kind": "task", "time": "2026-10-15T01:00:00Z"}));
    f.save("room", &show, &[round]);
    let notices = f.observe(&mut monitor, &options());
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].body, "完成 · 10分0秒");
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn notices_follow_language_and_never_include_message_content() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let opts = Options::from(
        json!({"general": {"language": "en"}, "alerts": {"notify": "system"}})
            .as_object()
            .unwrap(),
    );
    f.observe(&mut monitor, &opts);
    f.save("task", &task("succeeded", 601_000), &[]);
    assert_eq!(
        f.observe(&mut monitor, &opts),
        vec![Notice {
            title: "工作区".into(),
            body: "Completed · 10m 1s".into()
        }]
    );
}
