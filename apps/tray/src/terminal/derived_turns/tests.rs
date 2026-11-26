use std::fs;
use std::path::Path;

use serde_json::{Value, json};

fn setup_derived_history(root: &Path, account: Option<(&str, &Path)>, session_id: &str) {
    let db = crate::db::create(root);
    db.execute(
        "INSERT INTO conversations (id, kind, title, created, host, host_source, \
         manager_session, cwd, review, isolated, seats, sessions, rev) VALUES ('c1', 'task', \
         't', '2026-11-26T00:00:00+00:00', 'claude', 'unknown', 's-ws1', NULL, 0, 0, '[]', '{}', 1)",
        [],
    )
    .unwrap();
    let task_body = json!({
        "provider": "claude",
        "session_id": session_id,
        "execution_mode": "terminal",
        "prompt": "boss task prompt",
        "account": account.map(|(id, _)| id),
        "time": "2026-11-26T10:00:00Z",
    });
    db.execute(
        "INSERT INTO tasks (id, conversation_id, position, provider, session_id, status, body) \
         VALUES ('t1', 'c1', 0, 'claude', ?1, 'succeeded', ?2)",
        rusqlite::params![session_id, task_body.to_string()],
    )
    .unwrap();
    if let Some((id, dir)) = account {
        fs::write(
            root.join("accounts.json"),
            serde_json::to_string(&json!({
                "version": 1,
                "accounts": [{
                    "id": id,
                    "provider": "claude",
                    "label": "Test",
                    "dir": dir,
                    "managed": false
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn derived_turns_in_reads_user_turns_and_surfaces_unreadable_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let acc_dir = root.join("acc");
    let proj_dir = acc_dir.join("projects").join("-repo");
    fs::create_dir_all(&proj_dir).unwrap();
    let sid = "11111111-1111-4111-8111-111111111111";
    let desk = crate::terminal::thread_desk_key("claude", sid);
    setup_derived_history(root, Some(("acc1", &acc_dir)), sid);

    let terminals = crate::terminal::Terminals::default();

    // 1. Missing native transcript returns clear error and empty turns.
    let missing = terminals.derived_turns_in(root, "s-ws1", &desk);
    assert_eq!(missing.status, 200);
    let missing_json: Value = serde_json::from_slice(&missing.body).unwrap();
    assert_eq!(missing_json["turns"], json!([]));
    assert!(
        missing_json["error"]
            .as_str()
            .unwrap()
            .contains("找不到原生会话日志")
    );

    // 2. Populated transcript filters out boss task and returns user turn.
    let log_path = proj_dir.join(format!("{sid}.jsonl"));
    let boss_row = json!({
        "type": "user", "promptId": "p1", "timestamp": "2026-11-26T10:00:01Z",
        "message": {"content": "boss task prompt"}
    });
    let boss_reply = json!({
        "type": "assistant", "timestamp": "2026-11-26T10:00:05Z",
        "message": {"model": "claude-opus-4-6", "content": [{"type": "text", "text": "done"}]}
    });
    let user_row = json!({
        "type": "user", "promptId": "p2", "timestamp": "2026-11-26T10:01:00Z",
        "message": {"content": "user follow up"}
    });
    let user_reply = json!({
        "type": "assistant", "timestamp": "2026-11-26T10:01:05Z",
        "message": {"model": "claude-opus-4-6", "content": [{"type": "text", "text": "ok"}]}
    });
    fs::write(
        &log_path,
        format!("{boss_row}\n{boss_reply}\n{user_row}\n{user_reply}\n"),
    )
    .unwrap();
    let ok_resp = terminals.derived_turns_in(root, "s-ws1", &desk);
    assert_eq!(ok_resp.status, 200);
    let ok_json: Value = serde_json::from_slice(&ok_resp.body).unwrap();
    assert!(ok_json["error"].is_null());
    assert_eq!(ok_json["turns"].as_array().unwrap().len(), 1);
    assert_eq!(ok_json["turns"][0]["user_text"], "user follow up");
    assert_eq!(ok_json["turns"][0]["assistant_text"], "ok");

    // 3. Unrecognized file content returns clear format error.
    fs::write(&log_path, "{\"foo\":\"bar\"}\n").unwrap();
    let bad_fmt = terminals.derived_turns_in(root, "s-ws1", &desk);
    let bad_json: Value = serde_json::from_slice(&bad_fmt.body).unwrap();
    assert_eq!(bad_json["turns"], json!([]));
    assert!(bad_json["error"].as_str().unwrap().contains("格式无法识别"));

    // 4. Removed account reports unavailable account error.
    fs::write(
        root.join("accounts.json"),
        serde_json::to_string(&json!({"version": 1, "accounts": []})).unwrap(),
    )
    .unwrap();
    let removed = terminals.derived_turns_in(root, "s-ws1", &desk);
    let removed_json: Value = serde_json::from_slice(&removed.body).unwrap();
    assert_eq!(removed_json["turns"], json!([]));
    assert!(
        removed_json["error"]
            .as_str()
            .unwrap()
            .contains("账号不可用")
    );
}
