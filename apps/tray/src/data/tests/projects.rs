use std::collections::BTreeSet;

use super::*;

#[test]
fn manual_projects_routes_enforce_action_header_persist_and_invalidate_chat_list_cache() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config = root.join("tray-config");
    let reader = Reader::new(root.to_path_buf());

    // Action header is required for writes; scratch/missing folders are rejected.
    let unauthed = reader.request("POST", "/app/projects/add", None, br#"{"cwd":"/tmp"}"#);
    assert_eq!(unauthed.status, 403);
    let missing = reader.request(
        "POST",
        "/app/projects/add",
        Some("1"),
        br#"{"cwd":"/nonexistent/agentnave-missing-project"}"#,
    );
    assert_eq!(missing.status, 400);
    let scratch_body = json!({"cwd": root}).to_string();
    let scratch = reader.request(
        "POST",
        "/app/projects/add",
        Some("1"),
        scratch_body.as_bytes(),
    );
    assert_eq!(scratch.status, 400);

    // Seed a saved manual project and verify GET /app/projects and GET /app/chats expose it.
    let existing = BTreeSet::new();
    let sample = crate::projects::Project {
        key: "/work/sample-app".into(),
        label: "sample-app".into(),
        path: "~/Code/sample-app".into(),
    };
    let (_, added, _) = crate::projects::manual::add(&config, sample, &existing).unwrap();
    assert!(added);

    // Restarting the reader still loads the saved manual project.
    let restarted = Reader::new(root.to_path_buf());
    let listed = state(&restarted, "/app/projects");
    assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
    assert_eq!(listed["projects"][0]["key"], "/work/sample-app");
    assert_eq!(
        state(&restarted, "/app/chats")["projects"][0]["label"],
        "sample-app"
    );

    // Removing via POST invalidates the cached /app/chats response immediately.
    let no_header = restarted.request(
        "POST",
        "/app/projects/remove",
        None,
        br#"{"key":"/work/sample-app"}"#,
    );
    assert_eq!(no_header.status, 403);
    let removed = restarted.request(
        "POST",
        "/app/projects/remove",
        Some("1"),
        br#"{"key":"/work/sample-app"}"#,
    );
    assert_eq!(removed.status, 200);
    let removed_json: Value = serde_json::from_slice(&removed.body).unwrap();
    assert_eq!(removed_json["removed"], true);
    assert_eq!(state(&restarted, "/app/chats")["projects"], json!([]));
}

#[test]
fn sidebar_routes_enforce_action_header_validate_persist_and_invalidate_chat_list_cache() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut view = task_snapshot("a1", "succeeded", "2026-10-15T12:00:00Z");
    view["director"]["tasks"][0]["time"] = json!("2026-10-14T08:30:00Z");
    save(root, &view);
    let reader = Reader::new(root.to_path_buf());

    // Initial GET /app/chats exposes created and empty sidebar defaults (priming route cache).
    let initial = state(&reader, "/app/chats");
    assert_eq!(initial["groups"][0]["created"], "2026-10-14T08:30:00Z");
    assert_eq!(
        initial["sidebar"],
        json!({
            "project_aliases": {},
            "pinned_projects": [],
            "pinned_groups": [],
            "project_order": [],
        })
    );

    // Action header, HTTP method, and payload validation are enforced.
    for path in [
        "/app/projects/rename",
        "/app/projects/pin",
        "/app/projects/order",
        "/app/chats/pin",
    ] {
        assert_eq!(reader.request("GET", path, None, b"").status, 405, "{path}");
        assert_eq!(
            reader.request("POST", path, None, b"{}").status,
            403,
            "{path}"
        );
    }
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/projects/rename",
                Some("1"),
                br#"{"key":"other","alias":"x"}"#,
            )
            .status,
        400
    );
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/projects/pin",
                Some("1"),
                br#"{"key":"","pinned":true}"#,
            )
            .status,
        400
    );
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/projects/order",
                Some("1"),
                br#"{"order":"bad"}"#,
            )
            .status,
        400
    );
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/chats/pin",
                Some("1"),
                br#"{"group":"   ","pinned":true}"#,
            )
            .status,
        400
    );

    // Valid mutations write sidebar-state.json, invalidate /app/chats cache, and return sidebar.
    let res = reader.request(
        "POST",
        "/app/projects/rename",
        Some("1"),
        br#"{"key":"/work/app","alias":"My App\nLine2"}"#,
    );
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(
        body["sidebar"]["project_aliases"]["/work/app"],
        "My App Line2"
    );

    let res = reader.request(
        "POST",
        "/app/projects/pin",
        Some("1"),
        br#"{"key":"/work/app","pinned":true}"#,
    );
    assert_eq!(res.status, 200);
    let res = reader.request(
        "POST",
        "/app/projects/order",
        Some("1"),
        br#"{"order":["","other","/work/b","/work/app","/work/b"]}"#,
    );
    assert_eq!(res.status, 200);
    let res = reader.request(
        "POST",
        "/app/chats/pin",
        Some("1"),
        format!(r#"{{"group":"{MANAGER}","pinned":true}}"#).as_bytes(),
    );
    assert_eq!(res.status, 200);

    let expected = json!({
        "project_aliases": {"/work/app": "My App Line2"},
        "pinned_projects": ["/work/app"],
        "pinned_groups": [MANAGER],
        "project_order": ["/work/b", "/work/app"],
    });
    assert_eq!(state(&reader, "/app/chats")["sidebar"], expected);

    // Restarting Reader reads persisted sidebar-state.json (version 1).
    let restarted = Reader::new(root.to_path_buf());
    assert_eq!(state(&restarted, "/app/chats")["sidebar"], expected);
}
