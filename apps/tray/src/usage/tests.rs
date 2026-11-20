use super::*;
use serde_json::json;
use std::path::Path;

fn parse(
    provider: &str,
    path: &str,
    v: serde_json::Value,
    state: &mut serde_json::Value,
) -> Option<Event> {
    parsers::parse(provider, Path::new(path), v.to_string().as_bytes(), state)
}

#[test]
fn claude_revision_subagent_and_cache() {
    let mut state = json!({});
    let data = json!({"timestamp":"2026-11-20T00:00:00Z","sessionId":"child","message":{"id":"msg-1","model":"claude-opus-4-5","usage":{"input_tokens":100,"cache_read_input_tokens":20,"cache_creation_input_tokens":12,"cache_creation":{"ephemeral_5m_input_tokens":5,"ephemeral_1h_input_tokens":7},"output_tokens":9}}});
    let path = "/projects/proj/parent/subagents/agent-child.jsonl";
    let event = parse("claude", path, data.clone(), &mut state).unwrap();
    assert_eq!(
        (
            event.root.as_str(),
            event.fresh,
            event.read,
            event.write5,
            event.write1,
            event.output
        ),
        ("parent", 100, 20, 5, 7, 9)
    );
    let mut revised = data.clone();
    revised["message"]["usage"]["output_tokens"] = json!(12);
    assert_eq!(
        event.key,
        parse("claude", path, revised, &mut state).unwrap().key
    );
    let mut synthetic = data;
    synthetic["message"]["model"] = json!("<synthetic>");
    assert!(parse("claude", path, synthetic, &mut state).is_none());
}

#[test]
fn codex_normalization_duplicate_and_reset() {
    let mut state = json!({});
    let path = "/rollout.jsonl";
    parse(
        "codex",
        path,
        json!({"type":"session_meta","payload":{"id":"s"}}),
        &mut state,
    );
    parse(
        "codex",
        path,
        json!({"type":"turn_context","payload":{"model":"gpt-5.4"}}),
        &mut state,
    );
    let record = json!({"timestamp":"2026-11-20T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"cache_write_input_tokens":10,"output_tokens":20},"last_token_usage":{"input_tokens":100,"cached_input_tokens":30,"cache_write_input_tokens":10,"output_tokens":20}}}});
    let e = parse("codex", path, record.clone(), &mut state).unwrap();
    assert_eq!((e.fresh, e.read, e.write5, e.output), (60, 30, 10, 20));
    assert!(parse("codex", path, record.clone(), &mut state).is_none());
    let mut reset = record;
    reset["payload"]["info"]["total_token_usage"]["input_tokens"] = json!(40);
    reset["payload"]["info"]["total_token_usage"]["cached_input_tokens"] = json!(10);
    reset["payload"]["info"]["total_token_usage"]["cache_write_input_tokens"] = json!(0);
    reset["payload"]["info"]["last_token_usage"] =
        json!({"input_tokens":40,"cached_input_tokens":10,"output_tokens":4});
    assert_eq!(parse("codex", path, reset, &mut state).unwrap().fresh, 30);
}

#[test]
fn antigravity_step_identity_and_unknown_cache() {
    let mut state = json!({});
    let path = "/brain/sid/.system_generated/logs/transcript_full.jsonl";
    let row = json!({"source":"MODEL","step_index":3,"created_at":"2026-11-20T00:00:00Z","model":"gemini-2.5-pro","input_tokens":9,"output_tokens":2});
    let e = parse("antigravity", path, row.clone(), &mut state).unwrap();
    assert_eq!(e.key, "agy:sid:3");
    assert!(e.unknown);
    assert_eq!(e.fresh, 9);
    let mut newer = row;
    newer["cache_read_tokens"] = json!(5);
    let n = parse("antigravity", path, newer, &mut state).unwrap();
    assert_eq!(e.key, n.key);
    assert!(!n.unknown);
    let mut a = query::Aggregate::default();
    a.add(&e);
    a.add(&n);
    assert_eq!(a.rate(), Some(5.0 / 14.0));
}

#[test]
fn pricing_tiers_and_missing_price() {
    let mut e = Event {
        provider: "codex".into(),
        model: "openai/gpt-5.4[1m]".into(),
        fresh: 272_001,
        output: 1,
        ..Default::default()
    };
    let cost = pricing::cost(&e).unwrap();
    assert!((cost - (272_001.0 * 5.0 + 22.5) / 1_000_000.0).abs() < 1e-9);
    e.model = "unknown".into();
    assert!(pricing::cost(&e).is_none());
}

#[test]
fn compressed_log_is_incremental_and_preview_path_is_separate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = root.join("rollout-test.jsonl.zst");
    let now = chrono::Utc::now().to_rfc3339();
    let lines = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"s"}}),
        json!({"type":"turn_context","payload":{"model":"gpt-5.4"}}),
        json!({"timestamp":now,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":4}}}})
    );
    std::fs::write(
        &file,
        ruzstd::encoding::compress_to_vec(
            lines.as_bytes(),
            ruzstd::encoding::CompressionLevel::Uncompressed,
        ),
    )
    .unwrap();
    let mut db = db::open(root).unwrap();
    scan::file(&mut db, "codex", &file).unwrap();
    scan::file(&mut db, "codex", &file).unwrap();
    let rows = db::events(&db, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fresh, 80);
    assert_eq!(db::path(root, false), root.join("usage.db"));
    assert_ne!(db::path(root, true), root.join("usage.db"));
}

#[test]
fn attribution_prioritizes_earliest_window_and_excludes_external_resume() {
    let boss = query::boss_key("codex", "boss-session");
    let item = crate::data::snapshot::Item {
        id: "id".into(),
        title: String::new(),
        mode: "task".into(),
        host: Some("codex".into()),
        manager_session: Some(boss.clone()),
        status: String::new(),
        updated: String::new(),
    };
    let view = json!({"director":{"tasks":[{"thread":"abc","time":"2026-11-20T00:00:00Z","elapsed_ms":1000},{"thread":"abc","time":"2026-11-20T00:00:01Z","elapsed_ms":1000}]}});
    let snapshots = vec![crate::data::snapshot::Snapshot {
        item,
        view: std::sync::Arc::new(view),
        live: false,
        live_updated: None,
    }];
    let mut projects = std::collections::BTreeMap::new();
    projects.insert(
        boss,
        crate::projects::Project {
            key: "p".into(),
            label: "P".into(),
            path: "/p".into(),
        },
    );
    let (windows, bosses) = query::attribution(&snapshots, &projects);
    let ts = chrono::DateTime::parse_from_rfc3339("2026-11-20T00:00:01Z")
        .unwrap()
        .timestamp_millis();
    let mut e = Event {
        provider: "codex".into(),
        root: "boss-session".into(),
        thread: "abc".into(),
        ts,
        ..Default::default()
    };
    assert!(query::assign(&e, &windows, &bosses).unwrap().1);
    e.root = "outside".into();
    e.ts += 10_000;
    assert!(query::assign(&e, &windows, &bosses).is_none());
    e.root = "boss-session".into();
    assert!(!query::assign(&e, &windows, &bosses).unwrap().1);
}

#[test]
fn partial_lines_and_rewritten_heads_do_not_advance_or_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("log.jsonl");
    let time = chrono::Utc::now().to_rfc3339();
    let row = |output| {
        json!({"timestamp":time,"sessionId":"s","message":{"id":"m","model":"claude-opus-4-5","usage":{"input_tokens":10,"output_tokens":output}}}).to_string()
    };
    let first = row(1);
    std::fs::write(&path, &first).unwrap();
    let mut conn = db::open(root).unwrap();
    scan::file(&mut conn, "claude", &path).unwrap();
    assert!(db::events(&conn, 0, i64::MAX).unwrap().is_empty());
    std::fs::write(&path, format!("{first}\n")).unwrap();
    scan::file(&mut conn, "claude", &path).unwrap();
    assert_eq!(db::events(&conn, 0, i64::MAX).unwrap()[0].output, 1);
    std::fs::write(&path, format!("{}\n", row(8))).unwrap();
    scan::file(&mut conn, "claude", &path).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].output, 8);
}

#[test]
fn boss_hash_matches_service_and_claude_one_hour_price() {
    assert_eq!(
        query::boss_key("codex", "66666666-7777-8888-9999-aaaaaaaaaaaa"),
        "s-f0fbe2aac7cf2a31d1e053814f45f640"
    );
    let e = Event {
        provider: "claude".into(),
        model: "claude-opus-4-5".into(),
        write1: 1_000_000,
        ..Default::default()
    };
    let price = pricing::cost(&e).unwrap();
    let input = Event {
        write1: 0,
        fresh: 1_000_000,
        ..e
    };
    assert!((price - 2.0 * pricing::cost(&input).unwrap()).abs() < 1e-9);
}

#[test]
fn oversized_line_is_skipped_without_losing_neighbors() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("rollout-large.jsonl");
    let date = chrono::Utc::now().to_rfc3339();
    let first = json!({"timestamp":date,"sessionId":"s","message":{"id":"m1","model":"claude-opus-4-5","usage":{"input_tokens":10}}});
    let second = json!({"timestamp":date,"sessionId":"s","message":{"id":"m2","model":"claude-opus-4-5","usage":{"input_tokens":20}}});
    let mut bytes = format!("{first}\n").into_bytes();
    bytes.extend(vec![b'x'; 4 * 1024 * 1024 + 128]);
    bytes.push(b'\n');
    bytes.extend(format!("{second}\n").as_bytes());
    std::fs::write(&file, bytes).unwrap();
    let mut conn = db::open(temp.path()).unwrap();
    scan::file(&mut conn, "claude", &file).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().map(|r| r.fresh).sum::<i64>(), 30);
}
