use super::*;
use serde_json::json;
use std::path::Path;

mod adr0129;
mod regression;

fn parse_all(
    provider: &str,
    path: &str,
    v: serde_json::Value,
    state: &mut serde_json::Value,
) -> Vec<Event> {
    parsers::parse(provider, Path::new(path), v.to_string().as_bytes(), state)
}

fn parse(
    provider: &str,
    path: &str,
    v: serde_json::Value,
    state: &mut serde_json::Value,
) -> Option<Event> {
    parse_all(provider, path, v, state).into_iter().next()
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
    let mut db = db::open_at(&root.join("usage.db")).unwrap();
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
fn workspace_splits_boss_workers_and_excludes_other_groups() {
    let group = query::boss_key("codex", "boss-session");
    let other = query::boss_key("codex", "other-session");
    let item = |session: &str| crate::data::snapshot::Item {
        id: session.into(),
        title: String::new(),
        mode: "task".into(),
        host: Some("codex".into()),
        manager_session: Some(session.into()),
        status: String::new(),
        updated: String::new(),
    };
    let view = |thread: &str| json!({"director":{"tasks":[{"thread":thread,"time":"2026-11-20T00:00:00Z","elapsed_ms":1000}]}});
    let snapshots = vec![
        crate::data::snapshot::Snapshot {
            item: item(&group),
            view: std::sync::Arc::new(view("abc")),
            live: false,
            live_updated: None,
        },
        crate::data::snapshot::Snapshot {
            item: item(&other),
            view: std::sync::Arc::new(view("zzz")),
            live: false,
            live_updated: None,
        },
    ];
    let start = chrono::DateTime::parse_from_rfc3339("2026-11-20T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let event = |root: &str, thread: &str, ts: i64, fresh: i64, output: i64| Event {
        provider: "codex".into(),
        root: root.into(),
        thread: thread.into(),
        ts,
        fresh,
        output,
        ..Default::default()
    };
    let events = [
        event("boss-session", "boss-thread", start, 100, 10),
        event("worker", "abc", start + 500, 50, 5),
        event("other-session", "other-thread", start, 900, 90),
        event("other-worker", "zzz", start + 500, 800, 80),
        event("outside", "abc", start + 10_000, 700, 70),
    ];
    let value = query::workspace_of(&snapshots, &events, &group);
    assert_eq!(value["group"], group);
    assert_eq!(value["has_data"], true);
    assert_eq!(value["boss"]["calls"], 1);
    assert_eq!(value["boss"]["total_tokens"], 110);
    assert_eq!(value["workers"]["calls"], 1);
    assert_eq!(value["workers"]["total_tokens"], 55);
    assert_eq!(value["total"]["calls"], 2);
    assert_eq!(value["total"]["total_tokens"], 165);
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
    let mut conn = db::open_at(&root.join("usage.db")).unwrap();
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
    for model in [
        "gemini-3.8-flash-medium",
        "gemini-4-hydrogen-high",
        "gemini-4-hydrogen-tiered-high",
        "gemini-4-neon-xhigh",
        "gpt-6.2-sol",
        "gpt-6.2-astra",
    ] {
        let item = Event {
            provider: "antigravity".into(),
            model: model.into(),
            fresh: 1_000_000,
            ..Default::default()
        };
        assert!(pricing::cost(&item).is_some(), "expected price for {model}");
    }
}

#[test]
fn antigravity_skips_zero_token_steps() {
    let path = std::path::Path::new("/tmp/brain/sid/.system_generated/logs/transcript_full.jsonl");
    let line = json!({"source":"MODEL","step_index":1,"created_at":"2026-11-20T10:00:00Z"});
    assert!(
        parsers::parse(
            "antigravity",
            path,
            line.to_string().as_bytes(),
            &mut parsers::initial()
        )
        .is_empty()
    );
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
    let mut conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    scan::file(&mut conn, "claude", &file).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().map(|r| r.fresh).sum::<i64>(), 30);
}

#[test]
fn pricing_extracts_context_tiers_and_preserves_bundled_models() {
    let mut models = serde_json::Map::new();
    for i in 0..20 {
        models.insert(
            format!("new-model-{i}"),
            json!({"cost":{"input":1,"output":2}}),
        );
    }
    models.insert(
        "new-model-0".into(),
        json!({"cost": {
            "input":1,"output":2,"cache_write":3,
            "tiers":[{"tier":{"type":"context","size":123456},"input":4,"cache_write":5}],
            "context_over_200k":{"input":99}
        }}),
    );
    models.insert(
        "new-model-1".into(),
        json!({"cost": {
            "input":1,"output":2,"context_over_200k":{"input":6,"output":7}
        }}),
    );
    let source = json!({"openai":{"models":models},"other":{"models":{"ignored":{"cost":{"input":1,"output":2}}}}});
    let cache = pricing_refresh::extract(&source).unwrap();
    let mut corrupt = cache.clone();
    corrupt["models"]["new-model-0"]["tiers"] = json!("invalid");
    assert!(!pricing_refresh::valid_cache(&corrupt));
    assert_eq!(cache["models"]["new-model-0"]["tiers"][0]["above"], 123456);
    assert_eq!(
        cache["models"]["new-model-0"]["tiers"][0]["cache_write_5m"],
        5.0
    );
    assert_eq!(cache["models"]["new-model-1"]["tiers"][0]["above"], 200000);
    assert!(cache["models"].get("ignored").is_none());
    let merged = pricing::merged(Some(&cache));
    assert_eq!(merged["models"]["new-model-0"]["input"], 1.0);
    assert!(merged["models"].get("claude-opus-4-5").is_some());
    let mut override_cache = cache.clone();
    override_cache["models"]["claude-opus-4-5"] = json!({"input": 42, "output": 43});
    assert_eq!(
        pricing::merged(Some(&override_cache))["models"]["claude-opus-4-5"]["input"],
        42
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pricing-cache.json");
    pricing_refresh::save(&path, source.to_string().as_bytes()).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved, cache);
    let event = Event {
        model: "new-model-0".into(),
        fresh: 1_000_000,
        ..Default::default()
    };
    assert_eq!(pricing::cost(&event), Some(4.0));
}

#[test]
fn invalid_pricing_does_not_replace_cache_and_preview_is_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let path = pricing_refresh::cache_path(dir.path(), false);
    std::fs::write(&path, b"original").unwrap();
    assert!(pricing_refresh::save(&path, b"{").is_err());
    assert!(
        pricing_refresh::save(
            &path,
            br#"{"openai":{"models":{"few":{"cost":{"input":1,"output":2}}}}}"#
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(
        pricing::merged(Some(&json!({"models": {"bad": {}}})))["models"]
            .get("claude-opus-4-5")
            .is_some()
    );
    assert_eq!(path, dir.path().join("pricing-cache.json"));
    assert_eq!(
        pricing_refresh::cache_path(dir.path(), true),
        crate::preferences::dir().join("pricing-cache.json")
    );
    assert_ne!(pricing_refresh::cache_path(dir.path(), true), path);
}

#[test]
fn pi_filters_fork_inheritance_and_keeps_input_uncached() {
    let mut state = json!({});
    let path = "/sessions/proj/2026-11-20T10-00-00-000Z_child.jsonl";
    let header = json!({"type":"session","id":"child","timestamp":"2026-11-20T10:00:00.000Z","parentSession":"parent.jsonl"});
    assert!(parse("pi", path, header, &mut state).is_none());
    let change = json!({"type":"model_change","id":"m1","timestamp":"2026-11-20T10:00:00.100Z","provider":"p","modelId":"glm-5.3"});
    assert!(parse("pi", path, change, &mut state).is_none());
    let message = |id: &str, at: &str, input: i64| json!({"type":"message","id":id,"timestamp":at,"message":{"role":"assistant","model":"glm-5.3","usage":{"input":input,"output":7,"cacheRead":30,"cacheWrite":5}}});
    let old = message("e1", "2026-11-20T09:59:59.000Z", 10);
    assert!(parse("pi", path, old, &mut state).is_none());
    let call = message("e2", "2026-11-20T10:00:05.250Z", 100);
    let e = parse("pi", path, call, &mut state).unwrap();
    assert_eq!(
        (e.fresh, e.read, e.write5, e.output, e.session.as_str()),
        (100, 30, 5, 7, "child")
    );
    assert!(e.key.starts_with("pi:e2:"));
    let usage = json!({"type":"usage","id":"u1","timestamp":"2026-11-20T10:00:06.000Z","provider":"p","model":"deepseek-v4-flash","usage":{"input":4,"output":1,"cacheRead":0,"cacheWrite":0}});
    assert_eq!(
        parse("pi", path, usage, &mut state).unwrap().model,
        "deepseek-v4-flash"
    );
    let empty = json!({"type":"compaction","id":"c1","timestamp":"2026-11-20T10:00:07.000Z","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}});
    assert!(parse("pi", path, empty, &mut state).is_none());
}

#[test]
fn codebuddy_subtracts_cache_and_replaces_by_message_id() {
    let path = "/projects/proj/parent/subagents/agent-1.jsonl";
    let line = |output: i64| json!({"id":"line","timestamp":1_795_000_000_000_i64,"sessionId":"child","providerData":{"messageId":"reply","model":"glm-5.3-flash","usage":{"inputTokens":1000,"outputTokens":output,"inputTokensDetails":[{"cached_tokens":300}],"cacheCreationTokens":100}}});
    let mut state = json!({});
    let first = parse("codebuddy", path, line(5), &mut state).unwrap();
    assert_eq!(
        (first.fresh, first.read, first.write5, first.root.as_str()),
        (600, 300, 100, "parent")
    );
    let last = parse("codebuddy", path, line(9), &mut state).unwrap();
    assert_eq!((first.key.as_str(), last.output), ("codebuddy:reply", 9));
    let temp = tempfile::tempdir().unwrap();
    let conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    db::save(&conn, &first).unwrap();
    db::save(&conn, &last).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!((rows.len(), rows[0].output), (1, 9));
}

fn zstd_frame(lines: &[serde_json::Value]) -> Vec<u8> {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    ruzstd::encoding::compress_to_vec(
        text.as_bytes(),
        ruzstd::encoding::CompressionLevel::Uncompressed,
    )
}

#[test]
fn deepseek_reads_zstd_frames_skips_seed_and_marks_unknown_cache() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("sessions/proj/session-b");
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("session.v3.jsonl.zstd");
    std::fs::write(
        &old,
        zstd_frame(&[json!({"type":"session","version":3,"id":"session-b"})]),
    )
    .unwrap();
    let file = dir.join("session.v4.jsonl.zstd");
    let now = chrono::Utc::now().timestamp_millis();
    let reply = |seq: i64, cache: bool| {
        let mut usage = json!({"inputTokens":50,"outputTokens":3});
        if cache {
            usage["cacheReadTokens"] = json!(40);
        }
        json!({"type":"assistant/message","seq":seq,"time":now,"data":{"usage":usage,"message":{"source":{"model":"deepseek-flash","provider":"x"}}}})
    };
    let mut bytes = zstd_frame(&[
        json!({"type":"session","version":4,"id":"session-b","isSeeded":true,"delegationDepth":1,"parentSession":"session-a"}),
        reply(0, true),
        json!({"type":"session/end-seed","seq":1,"time":now,"data":{"inherited":true}}),
    ]);
    bytes.extend(zstd_frame(&[reply(2, false)]));
    std::fs::write(&file, &bytes).unwrap();
    let mut conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    scan::file(&mut conn, "deepseek", &file).unwrap();
    bytes.extend(zstd_frame(&[reply(3, true)]));
    bytes.extend_from_slice(&[0x28, 0xb5, 0x2f, 0xfd, 1]);
    std::fs::write(&file, &bytes).unwrap();
    scan::file(&mut conn, "deepseek", &file).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].unknown && !rows[1].unknown);
    assert_eq!(
        (rows[1].fresh, rows[1].read, rows[1].root.as_str()),
        (50, 40, "session-a")
    );
    let mut found = Vec::new();
    scan::discover(&temp.path().join("sessions"), "deepseek", 3, &mut found);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, file);
}

#[test]
fn grok_expands_models_and_skips_fork_copy() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cwd/sid");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("summary.json"),
        json!({"created_at":"2026-11-20T10:00:00.900Z","current_model_id":"grok-4.7","parent_session_id":"p","session_kind":"fork"}).to_string(),
    )
    .unwrap();
    let path = dir.join("updates.jsonl");
    let turn = |seconds: i64, usage: serde_json::Value| json!({"timestamp":seconds,"params":{"_meta":{"eventId":"sid-1"},"update":{"sessionUpdate":"turn_completed","prompt_id":format!("p-{seconds}"),"usage":usage}}});
    let mut state = json!({});
    let two = json!({"inputTokens":300,"outputTokens":30,"modelUsage":{
        "grok-4.7-build":{"inputTokens":200,"outputTokens":20,"cachedReadTokens":150,"cacheCreationTokens":10},
        "grok-4.7-build-fast":{"inputTokens":100,"outputTokens":10,"cachedReadTokens":0}}});
    let copied = turn(1_795_168_800 - 3600, two.clone());
    assert!(parse_all("grok", path.to_str().unwrap(), copied, &mut state).is_empty());
    let events = parse_all(
        "grok",
        path.to_str().unwrap(),
        turn(1_795_168_800 + 5, two),
        &mut state,
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].key, "grok:sid:sid-1:grok-4.7-build");
    assert_eq!(
        (events[0].fresh, events[0].read, events[0].write5),
        (40, 150, 10)
    );
    assert_eq!(events[1].fresh, 100);
    let single = turn(
        1_795_168_900,
        json!({"inputTokens":9,"outputTokens":1,"cachedReadTokens":4}),
    );
    let e = parse_all("grok", path.to_str().unwrap(), single, &mut state);
    assert_eq!(
        (e[0].model.as_str(), e[0].fresh, e[0].read),
        ("grok-4.7", 5, 4)
    );
}

#[test]
fn zcode_reads_database_incrementally_without_copies_or_errors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("db.sqlite");
    let now = chrono::Utc::now().timestamp_millis();
    {
        let source = rusqlite::Connection::open(&path).unwrap();
        source
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT);
                CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_updated INTEGER, data TEXT);
                INSERT INTO session VALUES ('root', NULL), ('child', 'root');",
            )
            .unwrap();
        let put = |id: &str, session: &str, data: serde_json::Value| {
            source
                .execute(
                    "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![id, session, now, data.to_string()],
                )
                .unwrap();
        };
        let tokens =
            json!({"input":1000,"output":20,"reasoning":5,"cache":{"read":400,"write":100}});
        let reply = |extra: serde_json::Value| {
            let mut data = json!({"role":"assistant","modelId":"GLM-5.3","time":{"completed":now},"tokens":tokens});
            data.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            data
        };
        put("m1", "child", reply(json!({})));
        put(
            "m2",
            "child",
            reply(json!({"metadata":{"forkOrigin":{"messageId":"m1"}}})),
        );
        put(
            "m3",
            "root",
            reply(json!({"tokens":{"input":0,"output":0},"error":{"name":"APIError"}})),
        );
        put("m4", "root", json!({"role":"user","time":{"created":now}}));
    }
    let mut conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    zcode::scan(&mut conn, &path).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].key.as_str(),
            rows[0].fresh,
            rows[0].read,
            rows[0].write5,
            rows[0].output,
            rows[0].root.as_str()
        ),
        ("zcode:m1", 500, 400, 100, 20, "root")
    );
    let source = rusqlite::Connection::open(&path).unwrap();
    source
        .execute(
            "INSERT INTO message VALUES ('m5', 'root', ?1, ?2)",
            rusqlite::params![now + 1, json!({"role":"assistant","model":{"id":"glm-5.3"},"time":{"created":now},"tokens":{"input":10,"output":2,"cache":{"read":0,"write":0}}}).to_string()],
        )
        .unwrap();
    drop(source);
    zcode::scan(&mut conn, &path).unwrap();
    assert_eq!(db::events(&conn, 0, i64::MAX).unwrap().len(), 2);
}

#[test]
fn routing_prefixes_and_variant_suffixes_are_priced() {
    let cost = |model: &str| {
        pricing::cost(&Event {
            model: model.into(),
            fresh: 1_000_000,
            ..Default::default()
        })
    };
    let flash = cost("glm-5.3-flash").unwrap();
    assert_eq!(cost("magpie/trae-cn/glm-5.3-flash-max"), Some(flash));
    assert_eq!(cost("opencode-go/glm-5.3-flash"), Some(flash));
    assert_eq!(cost("x-ai/grok-4.7-build-fast"), cost("grok-4.7"));
    assert!(cost("trae-cn/Doubao-Seed-2.1-Turbo").is_some());
    assert!(cost("trae-cn/not-a-model").is_none());
}

#[test]
fn today_range_buckets_into_twenty_four_local_hours() {
    use chrono::{Local, TimeZone};
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("usage.db")).unwrap();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
    let afternoon = Local
        .from_local_datetime(&day.and_hms_opt(14, 35, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let spec = query::QuerySpec {
        range: "today",
        custom: None,
        granularity: None,
    };
    let resp = query::respond_db(&conn, &spec, afternoon, Vec::new()).unwrap();
    let daily = resp["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 24);
    assert_eq!(daily[0]["date"], "2026-11-20T00:00");
    assert_eq!(daily[14]["date"], "2026-11-20T14:00");
    assert_eq!(daily[14]["future"], false);
    assert_eq!(daily[15]["future"], true);
    assert_eq!(daily[23]["date"], "2026-11-20T23:00");
}
