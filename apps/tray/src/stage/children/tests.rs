use serde_json::json;

use super::{enrich_groups_value, enrich_requested_group, read_hierarchy};
use crate::data::boss_app::boss_key;
use crate::terminal::thread_desk_key;

#[test]
fn links_subagent_workspace_under_derived_boss_and_keeps_one_level() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::create(dir.path());
    let derived_sid = "44444444-4444-4444-8444-444444444444";
    let derived_bk = boss_key("claude", derived_sid);
    let derived_desk = thread_desk_key("claude", derived_sid);

    let grandchild_sid = "55555555-5555-4555-8555-555555555555";
    let grandchild_bk = boss_key("codex", grandchild_sid);
    let grandchild_desk = thread_desk_key("codex", grandchild_sid);

    db.execute(
        "INSERT INTO workspaces (id, owner, owner_host) VALUES \
         ('s-boss', 's-boss', 'claude'), \
         ('s-child', ?1, 'claude'), \
         ('s-grandchild', ?2, 'codex')",
        [derived_bk.as_str(), grandchild_bk.as_str()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO conversations (id, kind, title, created, host, host_source, manager_session, cwd, review, isolated, seats, sessions, rev) VALUES \
         ('c-boss', 'task', 'b', '2026-11-26T00:00:00Z', 'claude', 'env', 's-boss', NULL, 0, 0, '[]', '{}', 1), \
         ('c-child', 'task', 'c', '2026-11-26T00:00:00Z', 'claude', 'env', 's-child', NULL, 0, 0, '[]', '{}', 1)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO tasks (id, conversation_id, position, provider, session_id, status, body) VALUES \
         ('t-d1', 'c-boss', 0, 'claude', ?1, 'succeeded', ?2), \
         ('t-g1', 'c-child', 0, 'codex', ?3, 'succeeded', ?4)",
        rusqlite::params![
            derived_sid,
            json!({"execution_mode": "terminal", "provider": "claude", "session_id": derived_sid}).to_string(),
            grandchild_sid,
            json!({"execution_mode": "terminal", "provider": "codex", "session_id": grandchild_sid}).to_string(),
        ],
    )
    .unwrap();

    let hierarchy = read_hierarchy(Some(&db));
    let mut groups = vec![
        json!({"group": "s-boss", "derived": [{"key": derived_desk}]}),
        json!({"group": "s-child", "derived": [{"key": grandchild_desk}]}),
        json!({"group": "s-grandchild", "derived": []}),
    ];
    enrich_groups_value(&mut groups, &hierarchy);

    assert_eq!(groups[0]["derived"][0]["boss_key"], derived_bk);
    assert_eq!(groups[1]["owner"], derived_bk);
    assert_eq!(groups[1]["parent_workspace_id"], "s-boss");
    assert_eq!(groups[1]["parent_desk_key"], derived_desk);

    // One-level invariant: a child under an already nested workspace is NOT nested a third level.
    assert!(groups[2]["parent_workspace_id"].is_null());

    let mut detail = json!({"group": "s-child"});
    enrich_requested_group(&mut detail, "s-child", &hierarchy);
    assert_eq!(detail["owner"], derived_bk);
    assert_eq!(detail["parent_workspace_id"], "s-boss");
    assert_eq!(detail["parent_desk_key"], derived_desk);
}
