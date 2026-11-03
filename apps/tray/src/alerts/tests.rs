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
        let mut notices = Vec::new();
        monitor.deliver(
            self.reader.alert_observations().unwrap(),
            options,
            |notice| {
                notices.push(notice.clone());
                Ok(())
            },
        );
        notices
    }

    fn begin(&self, monitor: &mut Monitor, id: &str, director: &Value, options: &Options) {
        let mut running = director.clone();
        for task in running["tasks"].as_array_mut().into_iter().flatten() {
            task["status"] = json!("running");
        }
        if let Some(round) = running.get_mut("round") {
            round["status"] = json!("running");
            for seat in round["seats"].as_array_mut().into_iter().flatten() {
                seat["status"] = json!("running");
                seat["finished"] = Value::Null;
                seat["review"] = Value::Null;
            }
        }
        self.save(id, &running, &[]);
        assert!(self.observe(monitor, options).is_empty());
    }
}

fn options() -> Options {
    Options::from(
        json!({"general": {"language": "zh"}, "alerts": {"notify": "system"}})
            .as_object()
            .unwrap(),
    )
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
    assert!(f.observe(&mut monitor, &options()).is_empty());
    let mut next = task("succeeded", 600_000);
    next["tasks"][0]["id"] = json!("next");
    f.save("new-task", &next, &[]);
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
        f.begin(&mut monitor, id, &task("succeeded", ms), &options());
        f.save(id, &task("succeeded", ms), &[]);
        assert_eq!(f.observe(&mut monitor, &options()).len(), expected);
    }
    let mut instant = options();
    instant.threshold_ms = 0;
    f.begin(&mut monitor, "instant", &task("succeeded", 0), &instant);
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
fn a_serial_review_pause_waits_for_the_entire_round_before_checking_the_threshold() {
    let f = Fixture::new();
    let mut show = show(&[seat("a", "running", None), seat("b", "not_started", None)]);
    f.save("room", &show, &[]);
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    show["round"]["seats"][0] = seat("a", "succeeded", Some("2026-10-15T00:01:00Z"));
    show["round"]["seats"][0]["review"] = json!("pending");
    f.save("room", &show, &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
    show["round"]["seats"][0]["review"] = json!("publish");
    show["round"]["seats"][1] = seat("b", "succeeded", Some("2026-10-15T00:20:00Z"));
    f.save("room", &show, &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn cancelled_or_interrupted_meetings_do_not_report_completion_or_problems() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    let mut meeting = show(&[seat("a", "cancelled", Some("2026-10-15T00:20:00Z"))]);
    f.begin(&mut monitor, "cancelled", &meeting, &opts);
    f.begin(&mut monitor, "interrupted", &meeting, &opts);
    meeting["round"]["status"] = json!("cancelled");
    f.save("cancelled", &meeting, &[]);
    meeting["round"]["status"] = json!("interrupted");
    meeting["round"]["seats"][0]["status"] = json!("interrupted");
    f.save("interrupted", &meeting, &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
}

#[test]
fn round_end_never_repeats_a_seat_failure_or_ignores_the_problem_switch() {
    let f = Fixture::new();
    let meeting = show(&[
        seat("a", "failed", Some("2026-10-15T00:20:00Z")),
        seat("b", "succeeded", Some("2026-10-15T00:20:00Z")),
    ]);
    let mut monitor = Monitor::default();
    let mut opts = options();
    f.observe(&mut monitor, &opts);
    f.begin(&mut monitor, "room", &meeting, &opts);
    f.save("room", &meeting, &[]);
    let notices = f.observe(&mut monitor, &opts);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].body, "本轮结束 · 20分0秒");
    opts.problems = true;
    f.begin(&mut monitor, "room-2", &meeting, &opts);
    f.save("room-2", &meeting, &[]);
    let notices = f.observe(&mut monitor, &opts);
    assert_eq!(
        notices
            .iter()
            .filter(|n| n.body.starts_with("失败"))
            .count(),
        1
    );
    assert_eq!(
        notices
            .iter()
            .filter(|n| n.body.starts_with("本轮结束"))
            .count(),
        1
    );
}

#[test]
fn retrying_a_failed_seat_can_notify_about_the_new_attempt_once() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    let mut meeting = show(&[
        seat("a", "failed", Some("2026-10-15T00:01:00Z")),
        seat("b", "running", None),
    ]);
    f.begin(&mut monitor, "room", &meeting, &opts);
    f.save("room", &meeting, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
    meeting["round"]["seats"][0] = seat("a", "running", None);
    f.save("room", &meeting, &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    meeting["round"]["seats"][0] = seat("a", "failed", Some("2026-10-15T00:02:00Z"));
    f.save("room", &meeting, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    // A complete retry can also occur between polls without exposing running state.
    meeting["round"]["seats"][0]["finished"] = json!("2026-10-15T00:03:00Z");
    f.save("room", &meeting, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
}

#[test]
fn launch_errors_without_finish_times_rearm_after_an_observed_retry() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    let mut meeting = show(&[seat("a", "failed", None), seat("b", "running", None)]);
    f.begin(&mut monitor, "room", &meeting, &opts);
    f.save("room", &meeting, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    meeting["round"]["seats"][0]["status"] = json!("running");
    f.save("room", &meeting, &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    meeting["round"]["seats"][0]["status"] = json!("failed");
    f.save("room", &meeting, &[]);
    assert_eq!(f.observe(&mut monitor, &opts).len(), 1);
}

#[test]
fn an_entire_round_pending_review_notifies_once_before_and_after_publication() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    f.observe(&mut monitor, &options());
    let mut meeting = show(&[seat("a", "succeeded", Some("2026-10-15T00:20:00Z"))]);
    meeting["round"]["status"] = json!("review");
    meeting["round"]["seats"][0]["review"] = json!("pending");
    f.begin(&mut monitor, "room", &meeting, &options());
    f.save("room", &meeting, &[]);
    assert_eq!(
        f.observe(&mut monitor, &options())[0].body,
        "待审核 · 20分0秒"
    );
    meeting["round"]["status"] = json!("finished");
    meeting["round"]["seats"][0]["review"] = json!("publish");
    f.save("room", &meeting, &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn failed_delivery_is_attempted_once_and_does_not_replay_when_permission_is_granted() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    f.save("task", &task("running", 0), &[]);
    f.observe(&mut monitor, &options());
    f.save("task", &task("succeeded", 600_000), &[]);
    let mut attempts = 0;
    monitor.deliver(f.reader.alert_observations().unwrap(), &options(), |_| {
        attempts += 1;
        Err("permission denied".into())
    });
    monitor.deliver(f.reader.alert_observations().unwrap(), &options(), |_| {
        attempts += 1;
        Ok(())
    });
    assert_eq!(attempts, 1);
}

#[test]
fn transient_view_read_failure_recovers_without_a_new_revision_and_uses_a_local_baseline() {
    let f = Fixture::new();
    f.save("task", &task("succeeded", 600_000), &[]);
    let db = crate::db::write(f.dir.path()).unwrap().unwrap();
    let original: String = db
        .query_row(
            "SELECT body FROM views WHERE conversation_id = 'task'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    db.execute(
        "UPDATE views SET body = '{bad' WHERE conversation_id = 'task'",
        [],
    )
    .unwrap();
    assert_eq!(f.reader.overview().unavailable.len(), 1);
    let mut monitor = Monitor::default();
    assert!(f.observe(&mut monitor, &options()).is_empty());
    db.execute(
        "UPDATE views SET body = ?1 WHERE conversation_id = 'task'",
        [&original],
    )
    .unwrap();
    assert!(f.reader.overview().unavailable.is_empty());
    assert!(f.observe(&mut monitor, &options()).is_empty());
    let mut next = task("succeeded", 600_000);
    next["tasks"][0]["id"] = json!("next");
    f.save("task", &next, &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
}

#[test]
fn one_permanently_damaged_view_does_not_block_other_conversations() {
    let f = Fixture::new();
    f.save("broken", &task("succeeded", 600_000), &[]);
    f.save("healthy", &task("running", 0), &[]);
    let db = crate::db::write(f.dir.path()).unwrap().unwrap();
    db.execute(
        "UPDATE views SET body = '{bad' WHERE conversation_id = 'broken'",
        [],
    )
    .unwrap();
    let mut monitor = Monitor::default();
    assert!(f.observe(&mut monitor, &options()).is_empty());
    f.save("healthy", &task("succeeded", 600_000), &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
    f.save("broken", &task("succeeded", 600_000), &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
}

#[test]
fn system_language_uses_the_same_os_locale_resolution_as_the_interface() {
    let opts = Options::from(
        json!({"general": {"language": "system"}})
            .as_object()
            .unwrap(),
    );
    assert_eq!(
        opts.english,
        crate::preferences::general::resolved_language(
            "system",
            &sys_locale::get_locale().unwrap_or_default()
        ) == "en"
    );
    assert!(!options().english);
}

#[test]
fn recovering_conversation_uses_its_own_baseline_while_others_keep_working() {
    let f = Fixture::new();
    f.save("task", &task("succeeded", 600_000), &[]);
    f.save("broken", &task("succeeded", 600_000), &[]);
    let db = crate::db::write(f.dir.path()).unwrap().unwrap();
    db.execute(
        "UPDATE views SET body = '{bad', rev = rev + 1 WHERE conversation_id = 'broken'",
        [],
    )
    .unwrap();
    let mut monitor = Monitor::default();
    assert!(f.observe(&mut monitor, &options()).is_empty());
    f.save("broken", &task("succeeded", 600_000), &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
    let mut next = task("succeeded", 600_000);
    next["tasks"][0]["id"] = json!("next");
    f.save("task", &next, &[]);
    db.execute(
        r#"UPDATE views SET body = '{"version":1}', rev = rev + 1 WHERE conversation_id = 'broken'"#,
        [],
    )
    .unwrap();
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
    f.save("broken", &task("succeeded", 600_000), &[]);
    assert!(f.observe(&mut monitor, &options()).is_empty());
    f.save("broken", &next, &[]);
    assert_eq!(f.observe(&mut monitor, &options()).len(), 1);
}

#[test]
fn meetings_scope_also_filters_private_task_problems() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let mut opts = options();
    opts.meetings_only = true;
    opts.problems = true;
    f.observe(&mut monitor, &opts);
    f.begin(&mut monitor, "task", &task("succeeded", 600_000), &opts);
    f.begin(&mut monitor, "failed-task", &task("failed", 1), &opts);
    f.begin(
        &mut monitor,
        "room",
        &show(&[seat("a", "running", None), seat("b", "running", None)]),
        &opts,
    );
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
    let mut show = show(&[
        seat("a", "blocked", Some("2026-10-15T00:00:01Z")),
        seat("b", "running", None),
    ]);
    f.begin(&mut monitor, "task", &task("failed", 1), &opts);
    f.begin(&mut monitor, "room", &show, &opts);
    f.save("task", &task("failed", 1), &[]);
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
    f.begin(&mut monitor, "task", &task("failed", 1), &opts);
    f.save("task", &task("failed", 1), &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
    opts.problems = true;
    let mut show = show(&[seat("a", "succeeded", Some("2026-10-15T00:00:01Z"))]);
    show["round"]["seats"][0]["review"] = json!("pending");
    f.begin(&mut monitor, "room", &show, &opts);
    f.save("room", &show, &[]);
    assert!(f.observe(&mut monitor, &opts).is_empty());
}

#[test]
fn off_consumes_events_so_enabling_notifications_does_not_catch_up() {
    let f = Fixture::new();
    let mut monitor = Monitor::default();
    let disabled = Options::from(&Map::new());
    f.observe(&mut monitor, &disabled);
    f.begin(&mut monitor, "task", &task("succeeded", 600_000), &disabled);
    f.begin(&mut monitor, "failed-task", &task("failed", 1), &disabled);
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
    f.begin(&mut monitor, "task", &task("succeeded", 601_000), &opts);
    f.save("task", &task("succeeded", 601_000), &[]);
    assert_eq!(
        f.observe(&mut monitor, &opts),
        vec![Notice {
            title: "工作区".into(),
            body: "Completed · 10m 1s".into()
        }]
    );
}
