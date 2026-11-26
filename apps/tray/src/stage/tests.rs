#[test]
fn antigravity_boss_names_follow_the_app() {
    use crate::data::boss_app::{ANTIGRAVITY_CLI, ANTIGRAVITY_DESKTOP};
    assert_eq!(
        super::boss_label(Some("antigravity"), Some(ANTIGRAVITY_DESKTOP)),
        "Antigravity 桌面"
    );
    assert_eq!(
        super::boss_label(Some("antigravity"), Some(ANTIGRAVITY_CLI)),
        "Antigravity CLI"
    );
    assert_eq!(super::boss_label(Some("antigravity"), None), "Antigravity");
}

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
fn list_keeps_pending_workspaces_beyond_the_other_workspace_limit() {
    let mut groups: Vec<_> = (0..202)
        .map(|i| super::Group::new(format!("workspace-{i}")))
        .collect();
    groups[201].handover.pending = true;
    let response = super::list_response(groups, &[], &[], &Default::default(), None);
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    let listed = body["groups"].as_array().unwrap();
    assert_eq!(listed.len(), 201);
    assert!(
        listed
            .iter()
            .any(|g| g["group"] == "workspace-201" && g["handover"] == "pending")
    );
    assert!(!listed.iter().any(|g| g["group"] == "workspace-200"));
}

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
    let cwds = super::origin_cwds(&snapshots, &Default::default(), &Default::default());
    assert_eq!(cwds["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"], "/second");
    // The same time falls back to a stable order.
    let tie = vec![
        task("y", json!([{"cwd": "/y", "time": "2026-11-02T00:00:00Z"}])),
        task("x", json!([{"cwd": "/x", "time": "2026-11-02T00:00:00Z"}])),
    ];
    assert_eq!(
        super::origin_cwds(&tie, &Default::default(), &Default::default())["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "/x"
    );
    // The boss session's own directory beats an earlier dispatch directory.
    let boss_cwds = std::collections::HashMap::from([(
        "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        "/boss/agentnave".to_owned(),
    )]);
    let overridden = super::origin_cwds(&snapshots, &Default::default(), &boss_cwds);
    assert_eq!(
        overridden["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "/boss/agentnave"
    );
    // A blank boss directory is ignored so the earliest dispatch still serves as fallback.
    let blank_boss = std::collections::HashMap::from([(
        "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        "   ".to_owned(),
    )]);
    assert_eq!(
        super::origin_cwds(&snapshots, &Default::default(), &blank_boss)["s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "/second"
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
    let boss_cwds = std::collections::HashMap::from([(
        "s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        "/work/ignored-boss".to_owned(),
    )]);
    let cwds = super::origin_cwds(&[], &handovers, &boss_cwds);
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
        collapsed: false,
        attention: None,
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
                        {"id": "e1", "provider": "codex", "model": "m", "thread": "early", "status": "succeeded", "time": "2026-11-12T00:30:00Z", "execution_mode": "background"},
                        {"id": "i1", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T02:00:00Z", "elapsed_ms": 14_400_000},
                        {"id": "i2", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T03:00:00Z"},
                        {"id": "i3", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T04:00:00Z"},
                        {"id": "i4", "provider": "codex", "model": "m", "status": "succeeded", "time": "2026-11-12T05:00:00Z"},
                        {"id": "o1", "provider": "codex", "model": "m", "thread": "other", "status": "running", "time": "2026-11-12T05:30:00Z", "elapsed_ms": 2_400_000, "execution_mode": "background"},
                        {"id": "e2", "provider": "codex", "model": "m", "thread": "early", "status": "running", "time": "2026-11-12T06:00:00Z", "execution_mode": "terminal"},
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
            origins: &Default::default(),
            projects: &Default::default(),
            links: &[],
            manual_projects: &[],
            sidebar: &Default::default(),
            titles: &Default::default(),
            renamed: &Default::default(),
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
    // A thread desk is derived by its latest call; calls without a mode and seats never are.
    let derived = |key: &str| desks.iter().find(|d| d["key"] == key).unwrap()["derived"].clone();
    assert_eq!(derived("thread:early"), true);
    assert_eq!(derived("thread:other"), false);
    assert_eq!(derived("task:tasks:i1"), false);
    assert_eq!(derived("seat:m:a"), false);

    let chat = query("/app/chats/s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(chat["desks"][0]["key"], "thread:other");
    // No CLI name was read for this chat, and the title stays the conversation's own.
    assert!(chat["cli_title"].is_null());
    assert_eq!(chat["cli_title_custom"], false);

    let renamed = std::collections::BTreeSet::new();
    let titles = std::collections::HashMap::from([(
        "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        crate::data::boss_app::CliTitle {
            text: "Named in the CLI".to_owned(),
            custom: true,
        },
    )]);
    let places = super::Places {
        dirs: &[],
        origins: &Default::default(),
        projects: &Default::default(),
        links: &[],
        manual_projects: &[],
        sidebar: &Default::default(),
        titles: &titles,
        renamed: &renamed,
    };
    for path in [
        "/app/chats/s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "/app/chats",
    ] {
        let res = super::respond(
            path,
            snapshots(),
            true,
            8,
            &Default::default(),
            &Default::default(),
            &places,
        );
        let value: Value = serde_json::from_slice(&res.body).unwrap();
        let group = if path == "/app/chats" {
            value["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["group"] == "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap()
                .clone()
        } else {
            value
        };
        assert_eq!(group["cli_title"], "Named in the CLI", "{path}");
        assert_eq!(group["cli_title_custom"], true, "{path}");
        assert_eq!(group["display_title"], "Named in the CLI", "{path}");
        assert!(group["title"].is_string(), "{path}");
    }
}

/// An agent is shown by the first of these that exists: the name the user gave it in the App, the
/// one the user gave in the CLI, the CLI's own, then the title the chat was dispatched with.
#[test]
fn an_agents_display_title_follows_the_order_of_names() {
    use crate::data::boss_app::CliTitle;
    let cli = |text: &str, custom| {
        Some(CliTitle {
            text: text.to_owned(),
            custom,
        })
    };
    let group = |cli_title: Option<CliTitle>, renamed: bool| {
        let mut group = super::Group::new("s-x".to_owned());
        group.title = "boss title".to_owned();
        group.cli_title = cli_title;
        group.renamed = renamed;
        group
    };
    assert_eq!(group(None, false).display_title(), "boss title");
    assert_eq!(group(cli("auto", false), false).display_title(), "auto");
    assert_eq!(group(cli("mine", true), false).display_title(), "mine");
    // Renamed in the App beats both of the CLI's names.
    assert_eq!(group(cli("mine", true), true).display_title(), "boss title");
    assert_eq!(
        group(cli("auto", false), true).display_title(),
        "boss title"
    );
    let value = group(cli("auto", false), false).header();
    assert_eq!(value["display_title"], "auto");
    assert_eq!(value["title"], "boss title");
}

#[test]
fn untitled_dispatches_fall_back_to_first_prompt_line_before_project_name() {
    use serde_json::{Value, json};

    let make_snapshot = |id: &str, title: &str, prompt: &str, cwd: &str| {
        let mut s = origin_snapshot(
            id,
            "task",
            json!({
                "created": "2026-11-12T00:00:00Z",
                "tasks": [{"prompt": prompt, "cwd": cwd, "time": "2026-11-12T00:00:01Z"}],
            }),
        );
        s.item.title = title.to_owned();
        s
    };
    let places = super::Places {
        dirs: &[],
        origins: &Default::default(),
        projects: &Default::default(),
        links: &[],
        manual_projects: &[],
        sidebar: &Default::default(),
        titles: &Default::default(),
        renamed: &Default::default(),
    };
    let long_prompt = format!("\n  {}  \n第二行不该出现", "a".repeat(70));
    let res = super::respond(
        "/app/chats",
        vec![make_snapshot(
            "c1",
            "claude 任务",
            &long_prompt,
            "/work/proj",
        )],
        true,
        8,
        &Default::default(),
        &Default::default(),
        &places,
    );
    let body: Value = serde_json::from_slice(&res.body).unwrap();
    let expected = format!("{}…", "a".repeat(60));
    assert_eq!(body["groups"][0]["display_title"], expected);

    // Without any prompt text, it still falls back to the project name.
    let res_empty = super::respond(
        "/app/chats",
        vec![make_snapshot("c2", "claude 任务", "   \n  ", "/work/proj")],
        true,
        8,
        &Default::default(),
        &Default::default(),
        &places,
    );
    let body_empty: Value = serde_json::from_slice(&res_empty.body).unwrap();
    assert_eq!(body_empty["groups"][0]["display_title"], "proj");
}

#[test]
fn chat_list_includes_earliest_created_and_falls_back_to_updated() {
    use serde_json::{Value, json};

    let mut early_task = origin_snapshot(
        "c1",
        "task",
        json!({
            "tasks": [
                {"time": "2026-11-10T09:00:00Z", "prompt": "later"},
                {"time": "2026-11-09T08:00:00Z", "prompt": "earlier"},
            ],
        }),
    );
    early_task.item.updated = "2026-11-12T00:00:00Z".to_owned();

    let mut show = origin_snapshot(
        "c2",
        "show",
        json!({
            "created": "2026-11-08T10:00:00+08:00",
            "messages": [{"kind": "task", "time": "2026-11-10T00:00:00Z"}],
        }),
    );
    show.item.updated = "2026-11-12T01:00:00Z".to_owned();

    let mut fallback = origin_snapshot("c3", "task", json!({}));
    fallback.item.manager_session = Some("s-cccccccccccccccccccccccccccccccc".to_owned());
    fallback.item.updated = "2026-11-11T05:00:00Z".to_owned();

    let handovers = std::collections::HashMap::from([(
        "s-dddddddddddddddddddddddddddddddd".to_owned(),
        super::Handover {
            title: Some("交接".to_owned()),
            released: Some("2026-11-11T06:00:00Z".to_owned()),
            ..Default::default()
        },
    )]);
    let sidebar = crate::projects::SidebarState {
        pinned_groups: vec!["s-cccccccccccccccccccccccccccccccc".to_owned()],
        ..Default::default()
    };
    let places = super::Places {
        dirs: &[],
        origins: &Default::default(),
        projects: &Default::default(),
        links: &[],
        manual_projects: &[],
        sidebar: &sidebar,
        titles: &Default::default(),
        renamed: &Default::default(),
    };
    let res = super::respond(
        "/app/chats",
        vec![early_task, show, fallback],
        true,
        8,
        &handovers,
        &Default::default(),
        &places,
    );
    let body: Value = serde_json::from_slice(&res.body).unwrap();
    let group = |id: &str| {
        body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["group"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        group("s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")["created"],
        "2026-11-08T10:00:00+08:00"
    );
    assert_eq!(
        group("s-cccccccccccccccccccccccccccccccc")["created"],
        "2026-11-11T05:00:00Z"
    );
    assert_eq!(
        group("s-dddddddddddddddddddddddddddddddd")["created"],
        "2026-11-11T06:00:00Z"
    );
    assert_eq!(
        body["sidebar"]["pinned_groups"],
        json!(["s-cccccccccccccccccccccccccccccccc"])
    );
}

#[test]
fn chat_list_includes_derived_desks_sorted_by_first_seen_and_office_draws_all_derived() {
    use serde_json::{Value, json};

    let snapshots = || {
        vec![origin_snapshot(
            "c1",
            "task",
            json!({
                "mode": "task",
                "tasks": [
                    {
                        "id": "r1",
                        "provider": "codex",
                        "model": "m-reg-1",
                        "status": "succeeded",
                        "time": "2026-11-12T01:00:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r2",
                        "provider": "codex",
                        "model": "m-reg-2",
                        "status": "failed",
                        "time": "2020-01-01T00:00:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r3",
                        "provider": "codex",
                        "model": "m-reg-3",
                        "status": "failed",
                        "time": "2020-01-01T00:01:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r4",
                        "provider": "codex",
                        "model": "m-reg-4",
                        "status": "failed",
                        "time": "2020-01-01T00:02:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r5",
                        "provider": "codex",
                        "model": "m-reg-5",
                        "status": "failed",
                        "time": "2020-01-01T00:03:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r6",
                        "provider": "codex",
                        "model": "m-reg-6",
                        "status": "failed",
                        "time": "2020-01-01T00:04:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "r7",
                        "provider": "codex",
                        "model": "m-reg-7",
                        "status": "failed",
                        "time": "2020-01-01T00:05:00Z",
                        "execution_mode": "background"
                    },
                    {
                        "id": "d2",
                        "provider": "codex",
                        "model": "m-drv-2",
                        "role": "Later",
                        "thread": "drv-2",
                        "status": "failed",
                        "time": "2020-01-01T00:00:00Z",
                        "execution_mode": "terminal",
                        "identity": {"provider": "codex", "model": "m-drv-2", "model_short": "m-drv-2", "handle": "@m-drv-2"}
                    },
                    {
                        "id": "d0",
                        "provider": "codex",
                        "model": "m-never-started",
                        "status": "failed",
                        "time": "2020-01-01T00:00:30Z",
                        "execution_mode": "terminal"
                    },
                    {
                        "id": "d1",
                        "provider": "claude",
                        "model": "m-drv-1",
                        "role": "Earlier",
                        "thread": "drv-1",
                        "status": "succeeded",
                        "time": "2019-12-31T23:00:00Z",
                        "execution_mode": "terminal",
                        "identity": {"provider": "claude", "model": "m-drv-1", "model_short": "m-drv-1", "handle": "@m-drv-1"}
                    }
                ],
            }),
        )]
    };
    let places = super::Places {
        dirs: &[],
        origins: &Default::default(),
        projects: &Default::default(),
        links: &[],
        manual_projects: &[],
        sidebar: &Default::default(),
        titles: &Default::default(),
        renamed: &Default::default(),
    };

    let list_res = super::respond(
        "/app/chats",
        snapshots(),
        true,
        1,
        &Default::default(),
        &Default::default(),
        &places,
    );
    let list_body: Value = serde_json::from_slice(&list_res.body).unwrap();
    let derived = list_body["groups"][0]["derived"].as_array().unwrap();
    let keys: Vec<_> = derived.iter().filter_map(|d| d["key"].as_str()).collect();
    assert_eq!(keys, ["thread:drv-1", "thread:drv-2"]);
    assert_eq!(derived[0]["provider"], "claude");
    assert_eq!(derived[0]["model"], "m-drv-1");
    assert_eq!(derived[0]["role"], "Earlier");
    assert_eq!(derived[0]["identity"]["handle"], "@m-drv-1");
    assert!(!derived[0]["first_seen"].as_str().unwrap().is_empty());
    assert!(!derived[0]["last_active"].as_str().unwrap().is_empty());

    // Office (panel=false, limit=1): a derived call that never got a session settles like any other; both derived desks stay drawn despite limit=1 and settled failure;
    // regular limit=1 still draws r1 and optimizes settled failure r2.
    let chat_res = super::respond(
        "/app/chats/s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        snapshots(),
        true,
        1,
        &Default::default(),
        &Default::default(),
        &places,
    );
    let chat_body: Value = serde_json::from_slice(&chat_res.body).unwrap();
    let drawn: Vec<_> = chat_body["desks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["key"].as_str())
        .collect();
    assert!(drawn.contains(&"task:c1:r1"));
    assert!(drawn.contains(&"thread:drv-1"));
    assert!(drawn.contains(&"thread:drv-2"));
    assert_eq!(drawn.len(), 3);
    assert_eq!(chat_body["optimized"]["count"], 7);
    assert_eq!(chat_body["optimized"]["desks"][0]["key"], "task:c1:r7");

    // Panel (panel=true, PANEL_IDLE_DESKS=8): cap still applies across all desks (regular + derived).
    let stage_res = super::respond(
        "/app/stage",
        snapshots(),
        true,
        1,
        &Default::default(),
        &Default::default(),
        &places,
    );
    let stage_body: Value = serde_json::from_slice(&stage_res.body).unwrap();
    assert_eq!(
        stage_body["managers"][0]["desks"].as_array().unwrap().len(),
        8
    );
    assert_eq!(stage_body["managers"][0]["optimized"]["count"], 2);
}
