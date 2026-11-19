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
