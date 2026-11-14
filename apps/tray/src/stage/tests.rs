#[test]
fn service_member_identity_contract_is_preserved() {
    use serde_json::{Value, json};

    let fixture: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/member-names.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        for identity in case["identities"].as_array().unwrap() {
            let record = json!({
                "provider": identity["provider"],
                "model": identity["model"],
                "identity": identity,
            });
            let speaker = super::member_speaker(&record);
            assert_eq!(speaker["identity"], *identity, "{}", case["name"]);
            let target = super::recipient(identity["key"].as_str().unwrap(), &record);
            assert_eq!(target["identity"], *identity, "{}", case["name"]);
            assert_eq!(target["key"], identity["key"]);
            assert_eq!(target["handle"], identity["handle"]);
        }
    }
}

#[test]
fn project_name_skips_agent_worktrees() {
    assert_eq!(
        super::project_name("/Users/a/Code/app/").as_deref(),
        Some("app")
    );
    assert_eq!(
        super::project_name("/Users/a/Code/app/.claude/worktrees/fix-1").as_deref(),
        Some("app")
    );
    assert_eq!(
        super::project_name("/Users/a/.codex/worktrees/ab12/app").as_deref(),
        Some("app")
    );
    assert_eq!(super::project_name("/"), None);
}

use super::Desk;

#[test]
fn a_failed_desk_settles_after_half_an_hour_unless_it_runs_again() {
    let mut desk = Desk::default();
    let minutes = |n: i64| n * 60 * 1_000_000;
    desk.observe(0, "failed");
    assert!(!desk.failure_settled(minutes(30)));
    assert!(desk.failure_settled(minutes(31)));
    desk.observe(minutes(32), "succeeded");
    assert!(!desk.failure_settled(minutes(90)));
}

#[test]
fn a_desk_shows_the_reported_model_until_a_newer_call_asks_for_another() {
    let mut desk = Desk::default();
    desk.label(1, "opus", None, None);
    assert_eq!(desk.art_model(), "opus");
    desk.reported(2, &serde_json::json!({"reported_model": "claude-opus-5-5"}));
    assert_eq!(desk.art_model(), "claude-opus-5-5");
    desk.label(3, "sonnet", None, None);
    assert_eq!(desk.art_model(), "sonnet");
}

fn origin_snapshot(
    id: &str,
    mode: &str,
    view: serde_json::Value,
) -> crate::data::snapshot::Snapshot {
    use crate::data::snapshot::{Item, Snapshot};
    Snapshot {
        item: Item {
            id: id.to_owned(),
            title: String::new(),
            mode: mode.to_owned(),
            host: None,
            manager_session: Some("s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
            status: "succeeded".to_owned(),
            updated: "2026-11-12T00:00:00Z".to_owned(),
        },
        view: std::sync::Arc::new(serde_json::json!({"director": view})),
        live: false,
        live_updated: None,
    }
}

#[test]
fn a_workspace_begins_where_its_earliest_surviving_dispatch_worked() {
    use serde_json::json;
    let task = |id: &str, calls: serde_json::Value| {
        origin_snapshot(id, "task", json!({"cwd": "/latest", "tasks": calls}))
    };
    let snapshots = vec![
        task(
            "b",
            json!([{"cwd": "/second", "time": "2026-11-02T00:00:00Z"}]),
        ),
        task(
            "a",
            json!([
                {"cwd": "/late", "time": "2026-11-05T00:00:00Z"},
                {"time": "2026-11-01T00:00:00Z"},
                {"cwd": "/first", "time": "2026-11-03T00:00:00Z"},
            ]),
        ),
        // A meeting counts from its creation, with its own directory.
        origin_snapshot(
            "m",
            "show",
            json!({"cwd": "/meeting", "created": "2026-11-04T00:00:00Z"}),
        ),
    ];
    let cwds = super::origin_cwds(&snapshots, &Default::default());
    assert_eq!(cwds["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"], "/second");
    // The same time falls back to a stable order.
    let tie = vec![
        task("y", json!([{"cwd": "/y", "time": "2026-11-02T00:00:00Z"}])),
        task("x", json!([{"cwd": "/x", "time": "2026-11-02T00:00:00Z"}])),
    ];
    assert_eq!(
        super::origin_cwds(&tie, &Default::default())["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "/x"
    );
}

#[test]
fn a_handed_over_workspace_without_sessions_uses_the_handover_directory() {
    let mut handovers = std::collections::HashMap::new();
    handovers.insert(
        "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        super::Handover {
            title: Some("发版".to_owned()),
            cwd: Some("/work/handed".to_owned()),
            ..Default::default()
        },
    );
    let cwds = super::origin_cwds(&[], &handovers);
    assert_eq!(cwds["s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"], "/work/handed");
}

#[test]
fn a_running_seat_works_in_its_own_directory() {
    use serde_json::json;

    let director = json!({
        "cwd": "/r",
        "seats": [{"id": "a", "cwd": "/r/.agentnave-worktrees/a"}, {"id": "b"}, {"id": "c", "cwd": "/x"}],
        "round": {"status": "running", "seats": [
            {"seat_id": "a", "status": "running"},
            {"seat_id": "b", "status": "running"},
            {"seat_id": "c", "status": "succeeded"},
        ]},
    });
    let running = super::running_in(&director);
    assert_eq!(running, ["/r/.agentnave-worktrees/a", "/r"]);
    let dirs = [crate::isolated::Dir {
        workspace: "w".into(),
        path: "/r/.agentnave-worktrees/a".into(),
        branch: "agentnave/a".into(),
        ready: true,
        id: "a".into(),
        created: String::new(),
    }];
    let shown = crate::isolated::classify(&dirs, "w", &running, &[]);
    assert_eq!(shown[0]["status"], "using");
}

#[test]
fn panel_desks_include_idle_seats_and_order_by_latest_boss_prompt() {
    use serde_json::{Value, json};

    let snapshots = || {
        vec![
            origin_snapshot(
                "tasks",
                "task",
                json!({
                    "mode": "task",
                    "tasks": [
                        {"id": "e1", "provider": "codex", "model": "m", "thread": "early", "status": "succeeded", "time": "2026-11-12T00:30:00Z"},
                        {"id": "i1", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T02:00:00Z", "elapsed_ms": 14_400_000},
                        {"id": "i2", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T03:00:00Z"},
                        {"id": "i3", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T04:00:00Z"},
                        {"id": "i4", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T05:00:00Z"},
                        {"id": "o1", "provider": "codex", "model": "m", "thread": "other", "status": "running", "time": "2026-11-12T05:30:00Z", "elapsed_ms": 2_400_000},
                        {"id": "e2", "provider": "codex", "model": "m", "thread": "early", "status": "running", "time": "2026-11-12T06:00:00Z"},
                    ],
                }),
            ),
            origin_snapshot(
                "m",
                "show",
                json!({
                    "mode": "show",
                    "created": "2026-11-12T00:00:00Z",
                    "seats": [
                        {"id": "a", "provider": "claude", "model": "m"},
                        {"id": "b", "provider": "codex", "model": "m"},
                    ],
                    "messages": [
                        {"kind": "task", "speaker": "@host", "round": 1, "time": "2026-11-12T03:30:00Z"},
                        {"kind": "answer", "speaker": "b", "round": 1, "text": "ok", "time": "2026-11-12T03:45:00Z"},
                        {"kind": "task", "speaker": "@host", "round": 2, "time": "2026-11-12T05:45:00Z"},
                    ],
                    "round": {"number": 2, "status": "running", "seats": [{"seat_id": "a", "status": "running"}]},
                }),
            ),
        ]
    };
    let query = |path: &str| -> Value {
        let places = super::Places {
            dirs: &[],
            projects: &Default::default(),
        };
        let res = super::respond(
            path,
            snapshots(),
            true,
            8,
            &Default::default(),
            &Default::default(),
            &places,
        );
        serde_json::from_slice(&res.body).unwrap()
    };
    let stage = query("/app/stage");
    let desks = stage["managers"][0]["desks"].as_array().unwrap();
    let keys: Vec<_> = desks.iter().filter_map(|d| d["key"].as_str()).collect();
    assert_eq!(
        keys,
        [
            "thread:early",
            "seat:m:a",
            "thread:other",
            "task:tasks:i4",
            "task:tasks:i3",
            "seat:m:b",
            "task:tasks:i2",
            "task:tasks:i1",
        ]
    );
    let idle_seat = desks.iter().find(|d| d["key"] == "seat:m:b").unwrap();
    assert_eq!(idle_seat["last_prompted"], "2026-11-12T03:30:00.000000Z");

    let chat = query("/app/chats/s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(chat["desks"][0]["key"], "thread:other");
}
