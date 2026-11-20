use super::*;
use serde_json::json;
use std::path::Path;

#[test]
fn fork_copy_does_not_overwrite_original_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    let orig = Event {
        key: "claude:msg-1".into(),
        provider: "claude".into(),
        session: "orig-session".into(),
        root: "orig-root".into(),
        thread: "orig-thread".into(),
        ts: 1_000,
        model: "claude-opus-4-5".into(),
        fresh: 100,
        output: 10,
        ..Default::default()
    };
    db::save(&conn, &orig).unwrap();

    // Fork copy with a newer timestamp must NOT overwrite original attribution.
    let fork = Event {
        session: "fork-session".into(),
        root: "fork-root".into(),
        thread: "fork-thread".into(),
        ts: 2_000,
        output: 20,
        ..orig.clone()
    };
    db::save(&conn, &fork).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].session.as_str(),
            rows[0].root.as_str(),
            rows[0].thread.as_str(),
            rows[0].ts,
            rows[0].output
        ),
        ("orig-session", "orig-root", "orig-thread", 2_000, 20)
    );

    // Older timestamp does not regress tokens or timestamp.
    let stale = Event {
        ts: 500,
        output: 1,
        ..orig
    };
    db::save(&conn, &stale).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!((rows[0].ts, rows[0].output), (2_000, 20));

    // Empty attribution columns get filled on conflict update.
    let empty_attr = Event {
        key: "claude:msg-2".into(),
        provider: "claude".into(),
        ts: 1_000,
        model: "claude-opus-4-5".into(),
        fresh: 5,
        ..Default::default()
    };
    db::save(&conn, &empty_attr).unwrap();
    let filled = Event {
        session: "s2".into(),
        root: "r2".into(),
        thread: "t2".into(),
        ts: 1_000,
        output: 3,
        ..empty_attr
    };
    db::save(&conn, &filled).unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    let second = rows.iter().find(|r| r.key == "claude:msg-2").unwrap();
    assert_eq!(
        (
            second.session.as_str(),
            second.root.as_str(),
            second.thread.as_str(),
            second.output
        ),
        ("s2", "r2", "t2", 3)
    );
}

#[test]
fn claude_skips_zero_token_rows_and_grok_keeps_subsecond_precision() {
    let mut state = json!({});
    let zero = json!({"timestamp":"2026-11-20T00:00:00Z","sessionId":"s","message":{"id":"m0","model":"claude-opus-4-5","usage":{"input_tokens":0,"output_tokens":0}}});
    assert!(parse("claude", "/projects/p/s.jsonl", zero, &mut state).is_none());

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cwd/sid-sub");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("summary.json"),
        json!({"created_at":"2026-11-20T10:00:00.900Z","current_model_id":"grok-4.7","parent_session_id":"p","session_kind":"fork"}).to_string(),
    )
    .unwrap();
    let path = dir.join("updates.jsonl");
    let turn = |us: i64, id: &str| json!({"timestamp_us":us,"params":{"_meta":{"eventId":id},"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":10,"outputTokens":2}}}});
    let created_ms = chrono::DateTime::parse_from_rfc3339("2026-11-20T10:00:00.900Z")
        .unwrap()
        .timestamp_millis();
    let mut gstate = json!({});
    // Same second, 400ms earlier than created_at -> must be filtered out.
    assert!(
        parse_all(
            "grok",
            path.to_str().unwrap(),
            turn((created_ms - 400) * 1000, "e-before"),
            &mut gstate
        )
        .is_empty()
    );
    // 50ms after created_at -> kept.
    let kept = parse_all(
        "grok",
        path.to_str().unwrap(),
        turn((created_ms + 50) * 1000, "e-after"),
        &mut gstate,
    );
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].ts, created_ms + 50);
}

#[test]
fn pricing_bounds_frames_limit_and_small_json_guards() {
    let mut models = serde_json::Map::new();
    for i in 0..20 {
        models.insert(
            format!("valid-m-{i}"),
            json!({"cost":{"input":1,"output":2}}),
        );
    }
    models.insert(
        "too-expensive".into(),
        json!({"cost":{"input":1_000_000.1,"output":1}}),
    );
    models.insert(
        "max-allowed".into(),
        json!({"cost":{"input":1_000_000.0,"output":0.0}}),
    );
    let cache = pricing_refresh::extract(&json!({"openai":{"models":models}})).unwrap();
    assert!(cache["models"].get("too-expensive").is_none());
    assert!(cache["models"].get("max-allowed").is_some());
    let mut bad = cache;
    bad["models"]["valid-m-0"]["input"] = json!(1_000_001.0);
    assert!(!pricing_refresh::valid_cache(&bad));

    let temp = tempfile::tempdir().unwrap();
    let zst = temp.path().join("bomb.zst");
    let payload = vec![b'a'; 256];
    std::fs::write(
        &zst,
        ruzstd::encoding::compress_to_vec(
            payload.as_slice(),
            ruzstd::encoding::CompressionLevel::Uncompressed,
        ),
    )
    .unwrap();
    let mut ok_frames = frames::Frames::with_limit(std::fs::File::open(&zst).unwrap(), 256);
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut ok_frames, &mut buf).unwrap();
    assert_eq!(buf.len(), 256);
    let mut bomb_frames = frames::Frames::with_limit(std::fs::File::open(&zst).unwrap(), 100);
    buf.clear();
    assert!(std::io::Read::read_to_end(&mut bomb_frames, &mut buf).is_err());

    let big_json = temp.path().join("big.json");
    let mut huge = String::from("{\"k\":\"");
    huge.push_str(&"x".repeat(1024 * 1024 + 32));
    huge.push_str("\"}");
    std::fs::write(&big_json, huge).unwrap();
    assert!(read_small_json(&big_json).is_none());

    #[cfg(unix)]
    {
        let target = temp.path().join("real.json");
        std::fs::write(&target, b"{}").unwrap();
        let link = temp.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read_small_json(&link).is_none());
    }
}

#[test]
fn zcode_busy_lock_fails_without_advancing_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("db.sqlite");
    let now = chrono::Utc::now().timestamp_millis();
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
            CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT);
            CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_updated INTEGER, data TEXT);",
        )
        .unwrap();
    writer
        .execute(
            "INSERT INTO message VALUES ('m1', 's1', ?1, ?2)",
            rusqlite::params![
                now,
                json!({"role":"assistant","modelId":"glm-5.3","time":{"completed":now},"tokens":{"input":10,"output":2,"cache":{"read":0,"write":0}}}).to_string()
            ],
        )
        .unwrap();
    writer
        .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE")
        .unwrap();

    let mut conn = db::open_at(&temp.path().join("usage.db")).unwrap();
    let err = zcode::scan_with_timeout(&mut conn, &path, std::time::Duration::from_millis(50));
    assert!(err.is_err());
    assert!(db::events(&conn, 0, i64::MAX).unwrap().is_empty());
    assert!(
        db::cursor(&conn, &path.to_string_lossy())
            .unwrap()
            .is_none()
    );

    writer
        .execute_batch("COMMIT; PRAGMA locking_mode = NORMAL")
        .unwrap();
    drop(writer);
    zcode::scan_with_timeout(&mut conn, &path, std::time::Duration::from_millis(50)).unwrap();
    assert_eq!(db::events(&conn, 0, i64::MAX).unwrap().len(), 1);
}

#[test]
fn dst_midnight_gap_and_project_unpriced_and_thread_golden() {
    #[derive(Clone, Copy, Debug)]
    struct SpringForwardTz;
    impl chrono::Offset for SpringForwardTz {
        fn fix(&self) -> chrono::FixedOffset {
            chrono::FixedOffset::east_opt(0).unwrap()
        }
    }
    impl chrono::TimeZone for SpringForwardTz {
        type Offset = Self;
        fn from_offset(_: &Self::Offset) -> Self {
            Self
        }
        fn offset_from_local_date(
            &self,
            _: &chrono::NaiveDate,
        ) -> chrono::MappedLocalTime<Self::Offset> {
            chrono::MappedLocalTime::Single(*self)
        }
        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::MappedLocalTime<Self::Offset> {
            if local.time() < chrono::NaiveTime::from_hms_opt(1, 0, 0).unwrap() {
                chrono::MappedLocalTime::None
            } else {
                chrono::MappedLocalTime::Single(*self)
            }
        }
        fn offset_from_utc_date(&self, _: &chrono::NaiveDate) -> Self::Offset {
            *self
        }
        fn offset_from_utc_datetime(&self, _: &chrono::NaiveDateTime) -> Self::Offset {
            *self
        }
    }

    let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    let expected = date
        .and_hms_opt(1, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(query::start_in(&SpringForwardTz, date), expected);

    // Project by_provider exposes unpriced_tokens.
    let mut projects = std::collections::BTreeMap::new();
    let proj = crate::projects::Project {
        key: "p1".into(),
        label: "Proj".into(),
        path: "/tmp/p1".into(),
    };
    let unpriced = Event {
        provider: "claude".into(),
        model: "unknown-model".into(),
        fresh: 42,
        output: 8,
        ..Default::default()
    };
    query::add_project(&mut projects, &proj, false, &unpriced);
    let val = query::project_value(projects.into_values().next().unwrap());
    assert_eq!(val["by_provider"][0]["unpriced_tokens"], 50);

    // Python naming.thread golden assertion.
    assert_eq!(
        parsers::event(
            "codex",
            "s",
            "66666666-7777-8888-9999-aaaaaaaaaaaa",
            0,
            "m",
            "k".into()
        )
        .thread,
        "6dccf56517dc623f"
    );
    assert_eq!(
        parsers::event(
            "claude",
            "s",
            "11111111-2222-3333-4444-555555555555",
            0,
            "m",
            "k".into()
        )
        .thread,
        "5b1d8aef19b79168"
    );

    // Overflow saturation.
    let max_ev = Event {
        fresh: i64::MAX,
        output: 100,
        ..Default::default()
    };
    assert_eq!(max_ev.total(), i64::MAX);
    let mut agg = query::Aggregate::default();
    agg.add(&max_ev);
    agg.add(&max_ev);

    // Probed Homes take precedence in scan::roots.
    let mut homes = crate::data::boss_app::Homes::default();
    homes.claude.push("/custom/claude".into());
    homes.codex.push("/custom/codex".into());
    homes.codex.push("/custom/codex".into());
    homes.other.push(("pi".into(), "/custom/pi".into()));
    let discovered = scan::roots(&homes);
    assert_eq!(
        discovered
            .iter()
            .filter(|(p, _, _)| *p == "claude")
            .map(|(_, path, _)| path.clone())
            .collect::<Vec<_>>(),
        vec![std::path::PathBuf::from("/custom/claude/projects")]
    );
    assert_eq!(
        discovered.iter().filter(|(p, _, _)| *p == "codex").count(),
        2
    );
    assert!(
        discovered
            .iter()
            .any(|(p, path, _)| *p == "pi" && path == Path::new("/custom/pi/sessions"))
    );
}
