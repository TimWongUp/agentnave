use super::*;
use crate::projects::Project;
use chrono::{Local, TimeZone};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const HOUR: i64 = buckets::HOUR;
const DAY: i64 = buckets::DAY;

fn sample_project(name: &str) -> Project {
    Project {
        key: format!("/work/{name}"),
        label: name.to_owned(),
        path: format!("~/work/{name}"),
    }
}

#[test]
fn three_layer_storage_revision_delta_dedupe_and_prune() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_at(&dir.path().join("usage.db")).unwrap();
    let now_ms = 1_800_000_000_000_i64;
    let proj_a = sample_project("alpha");
    let proj_b = sample_project("beta");

    // 1. Recent (<7d) event writes to usage_events, usage_hourly_buckets, usage_dedupe, usage_projects.
    let recent = Event {
        key: "claude:m1".into(),
        provider: "claude".into(),
        session: "s1".into(),
        root: "s1".into(),
        thread: "t1".into(),
        ts: now_ms - HOUR,
        model: "claude-opus-4-5".into(),
        fresh: 1_000,
        read: 500,
        output: 100,
        cwd: "/work/alpha".into(),
        calls: 1,
        ..Default::default()
    };
    db::save_event(&conn, &recent, &proj_a, "boss", now_ms).unwrap();
    assert_eq!(db::events(&conn, 0, i64::MAX).unwrap().len(), 1);
    let b1 = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(b1.len(), 1);
    assert_eq!(
        (b1[0].fresh_input, b1[0].output, b1[0].calls),
        (1_000, 100, 1)
    );

    // 2. Revision within 7d moves hour and updates tokens cleanly via delta (old bucket deleted!).
    let revised = Event {
        ts: now_ms - 10_000,
        output: 250,
        ..recent.clone()
    };
    db::save_event(&conn, &revised, &proj_a, "boss", now_ms).unwrap();
    let b2 = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(b2.len(), 1);
    assert_eq!(b2[0].hour_ts_utc, buckets::hour_ts_utc(revised.ts));
    assert_eq!(
        (b2[0].fresh_input, b2[0].output, b2[0].calls),
        (1_000, 250, 1)
    );

    // Stale revision (< old ts) is ignored.
    let stale = Event {
        ts: now_ms - 2 * HOUR,
        output: 1,
        ..recent
    };
    db::save_event(&conn, &stale, &proj_a, "boss", now_ms).unwrap();
    let b_stale = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(b_stale.len(), 1);
    assert_eq!(b_stale[0].output, 250);

    // 3. Historical (7d..92d) event skips usage_events, writes bucket + dedupe, and deduplicates repeats.
    let hist = Event {
        key: "codex:old-1".into(),
        provider: "codex".into(),
        session: "s2".into(),
        root: "s2".into(),
        thread: "t2".into(),
        ts: now_ms - 30 * DAY,
        model: "gpt-5.4".into(),
        fresh: 2_000,
        output: 400,
        cwd: "/work/beta".into(),
        calls: 1,
        ..Default::default()
    };
    db::save_event(&conn, &hist, &proj_b, "native", now_ms).unwrap();
    assert_eq!(db::events(&conn, 0, i64::MAX).unwrap().len(), 1); // still only recent event
    // Re-ingesting the same historical event key must NOT double-count!
    db::save_event(&conn, &hist, &proj_b, "native", now_ms).unwrap();
    let hist_buckets = buckets::query_buckets(&conn, 0, now_ms - 10 * DAY).unwrap();
    assert_eq!(hist_buckets.len(), 1);
    assert_eq!(
        (hist_buckets[0].fresh_input, hist_buckets[0].calls),
        (2_000, 1)
    );

    // 4. Prune advances time by 80 days: recent becomes >7d (pruned from usage_events, kept in buckets),
    // hist becomes 110d old (pruned from buckets, dedupe, and usage_projects).
    let later = now_ms + 80 * DAY;
    db::prune(&mut conn, later).unwrap();
    assert!(db::events(&conn, 0, i64::MAX).unwrap().is_empty());
    let remaining = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].provider, "claude");
    let projects = buckets::query_projects(&conn).unwrap();
    assert!(projects.contains_key("/work/alpha"));
    assert!(!projects.contains_key("/work/beta"));
}

#[test]
fn v1_to_v3_migration_resets_stale_cursors_and_backfills_cursorless_dbs() {
    let dir = tempfile::tempdir().unwrap();
    let now_ms = 1_800_000_000_000_i64;
    let recent_ts = now_ms - 2 * DAY;
    let old_ts = now_ms - 20 * DAY;

    let make_v1 = |db_path: &Path, with_cursor: bool| {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE source_cursors(
                source_path TEXT PRIMARY KEY, provider TEXT NOT NULL,
                file_size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
                byte_offset INTEGER NOT NULL, head_hash_4k TEXT NOT NULL,
                parser_state TEXT NOT NULL
            );
            CREATE TABLE usage_events(
                event_key TEXT PRIMARY KEY, provider TEXT NOT NULL,
                session_id TEXT NOT NULL, root_session_id TEXT NOT NULL,
                thread_hash TEXT NOT NULL, ts_ms INTEGER NOT NULL,
                model TEXT NOT NULL, fresh_input INTEGER NOT NULL,
                cache_read INTEGER NOT NULL, cache_write_5m INTEGER NOT NULL,
                cache_write_1h INTEGER NOT NULL, output INTEGER NOT NULL,
                cache_unknown INTEGER NOT NULL
            );
            PRAGMA user_version = 1;",
        )
        .unwrap();
        if with_cursor {
            conn.execute(
                "INSERT INTO source_cursors VALUES ('/logs/a.jsonl','claude',100,200,100,'abc','{}')",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO usage_events VALUES ('claude:recent','claude','s1','s1','t1',?1,'claude-opus-4-5',100,50,10,0,20,0)",
            [recent_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events VALUES ('claude:old','claude','s2','s2','t2',?1,'claude-opus-4-5',300,0,0,0,40,0)",
            [old_ts],
        )
        .unwrap();
    };

    // Case 1: cursor-bearing legacy DB resets once so the next scan cleanly rebuilds cwd & workbuddy.
    let db_with_cursor = dir.path().join("with-cursor.db");
    make_v1(&db_with_cursor, true);
    let conn = db::open_at_time(&db_with_cursor, now_ms).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    assert!(db::cursor(&conn, "/logs/a.jsonl").unwrap().is_none());
    assert!(
        buckets::query_buckets(&conn, 0, i64::MAX)
            .unwrap()
            .is_empty()
    );

    // Case 2: cursorless legacy DB backfills buckets & dedupe and prunes >7d raw events.
    let db_path = dir.path().join("cursorless.db");
    make_v1(&db_path, false);
    let conn = db::open_at_time(&db_path, now_ms).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);

    // Only recent (<7d) row stays in usage_events; both rows were backfilled into buckets & dedupe.
    let detail = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0].key, "claude:recent");

    let bucket_rows = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(bucket_rows.len(), 2);
    assert_eq!(bucket_rows.iter().map(|b| b.fresh_input).sum::<i64>(), 400);
    // Re-ingesting the pruned old event key must be deduplicated via usage_dedupe.
    let dup_old = Event {
        key: "claude:old".into(),
        provider: "claude".into(),
        ts: old_ts,
        model: "claude-opus-4-5".into(),
        fresh: 300,
        output: 40,
        calls: 1,
        ..Default::default()
    };
    db::save_event(&conn, &dup_old, &Project::other(), "native", now_ms).unwrap();
    let bucket_rows_after = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(
        bucket_rows_after.iter().map(|b| b.fresh_input).sum::<i64>(),
        400
    );
}

#[test]
fn discover_finds_t3_settings_multi_account_homes_desktop_workbuddy_kimi_minimax() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let t3 = home.join(".t3");
    let desktop = dir.path().join("Support/Claude");

    // Multi-account Claude & Codex dirs under $HOME (direct and 1 level down).
    std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
    std::fs::create_dir_all(home.join(".claude_work/projects")).unwrap();
    std::fs::create_dir_all(home.join("claude-profiles/alt/projects")).unwrap();
    std::fs::create_dir_all(home.join(".claude_t3/projects")).unwrap();
    std::fs::create_dir_all(home.join(".codex/sessions")).unwrap();
    std::fs::create_dir_all(home.join(".codex_alt/sessions")).unwrap();
    std::fs::create_dir_all(home.join(".codex_t3/sessions")).unwrap();
    // T3 Antigravity ACP brain.
    std::fs::create_dir_all(t3.join("userdata/providers/antigravity/prof1/antigravity-acp/brain"))
        .unwrap();
    // Claude Desktop Cowork sessions.
    std::fs::create_dir_all(
        desktop.join("local-agent-mode-sessions/acct/org/local_abc/.claude/projects"),
    )
    .unwrap();
    // WorkBuddy & WorkBuddy AI.
    std::fs::create_dir_all(home.join(".workbuddy/projects")).unwrap();
    std::fs::create_dir_all(home.join(".workbuddy-ai/projects")).unwrap();
    // Kimi & MiniMax.
    std::fs::create_dir_all(home.join(".kimi-code/sessions")).unwrap();
    std::fs::create_dir_all(home.join(".minimax/v2/sessions")).unwrap();
    // T3 settings.json with custom homePath (and dummy secret that must never be touched).
    std::fs::create_dir_all(t3.join("userdata")).unwrap();
    std::fs::write(
        t3.join("userdata/settings.json"),
        json!({
            "providerInstances": {
                "claudeAgent_personal": {
                    "driver": "claudeAgent",
                    "config": { "homePath": "~/.claude_t3", "apiKey": "must-not-read" }
                },
                "codex_work": {
                    "driver": "codex",
                    "config": { "homePath": "~/.codex_t3" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let no_env = |_: &str| None;
    let roots = discover::roots_in(
        &home,
        &t3,
        std::slice::from_ref(&desktop),
        &crate::data::boss_app::Homes::default(),
        &no_env,
    );

    let has = |provider: &str, path: &Path| {
        roots
            .iter()
            .any(|(p, r, _)| *p == provider && discover::canon_key(r) == discover::canon_key(path))
    };
    assert!(has("claude", &home.join(".claude/projects")));
    assert!(has("claude", &home.join(".claude_work/projects")));
    assert!(has("claude", &home.join("claude-profiles/alt/projects")));
    assert!(has("claude", &home.join(".claude_t3/projects")));
    assert!(has(
        "claude-desktop",
        &desktop.join("local-agent-mode-sessions/acct/org/local_abc/.claude/projects")
    ));
    assert!(has("codex", &home.join(".codex/sessions")));
    assert!(has("codex", &home.join(".codex_alt/sessions")));
    assert!(has("codex", &home.join(".codex_t3/sessions")));
    assert!(has(
        "antigravity",
        &t3.join("userdata/providers/antigravity/prof1/antigravity-acp/brain")
    ));
    assert!(has("workbuddy", &home.join(".workbuddy/projects")));
    assert!(has("workbuddy", &home.join(".workbuddy-ai/projects")));
    assert!(has("kimi", &home.join(".kimi-code/sessions")));
    assert!(has("minimax", &home.join(".minimax/v2/sessions")));

    // Hermes DBs (main + profiles).
    std::fs::create_dir_all(home.join(".hermes/profiles/dev")).unwrap();
    std::fs::write(home.join(".hermes/state.db"), b"").unwrap();
    std::fs::write(home.join(".hermes/profiles/dev/state.db"), b"").unwrap();
    let hermes = discover::hermes_dbs_in(&home, &no_env);
    assert_eq!(hermes.len(), 2);

    // OpenCode DB via env override and XDG fallback.
    let custom_oc = dir.path().join("custom/oc.db");
    let env_oc = |k: &str| (k == "OPENCODE_DB").then(|| custom_oc.to_string_lossy().into_owned());
    assert_eq!(discover::opencode_db_in(&home, &env_oc), custom_oc);
    assert_eq!(
        discover::opencode_db_in(&home, &no_env),
        home.join(".local/share/opencode/opencode.db")
    );
}

#[test]
fn hermes_opencode_workbuddy_kimi_minimax_and_prefilter_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let now_sec = (now_ms as f64) / 1000.0;

    // --- Hermes state.db + revision delta ---
    let hermes_path = dir.path().join("state.db");
    {
        let hdb = rusqlite::Connection::open(&hermes_path).unwrap();
        hdb.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT, git_repo_root TEXT, parent_session_id TEXT, started_at REAL);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL,
                 billing_provider TEXT DEFAULT '', api_call_count INTEGER DEFAULT 0,
                 input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
                 cache_read_tokens INTEGER DEFAULT 0, cache_write_tokens INTEGER DEFAULT 0,
                 reasoning_tokens INTEGER DEFAULT 0, first_seen REAL, last_seen REAL
             );",
        )
        .unwrap();
        hdb.execute(
            "INSERT INTO sessions VALUES ('hs1', '', '/repo/hermes-proj', NULL, ?1)",
            [now_sec - 60.0],
        )
        .unwrap();
        hdb.execute(
            "INSERT INTO session_model_usage VALUES ('hs1','MiniMax-M2.7','p1',3,1000,200,5000,100,50,?1,?2)",
            rusqlite::params![now_sec - 60.0, now_sec - 10.0],
        )
        .unwrap();
    }
    let mut conn = db::open_at_time(&dir.path().join("usage.db"), now_ms).unwrap();
    let mut resolve = |e: &Event| {
        if e.cwd == "/repo/hermes-proj" {
            (sample_project("hermes-proj"), "native")
        } else {
            (Project::other(), "native")
        }
    };
    hermes::scan_with(
        &mut conn,
        &hermes_path,
        Duration::from_secs(1),
        now_ms,
        &mut resolve,
    )
    .unwrap();
    let rows = db::events(&conn, 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].provider.as_str(),
            rows[0].fresh,
            rows[0].read,
            rows[0].write5,
            rows[0].output,
            rows[0].calls
        ),
        ("hermes", 1000, 5000, 100, 250, 3)
    );
    assert_eq!(rows[0].cwd, "/repo/hermes-proj");

    // Grow Hermes session -> smooth revision delta without double counting!
    {
        let hdb = rusqlite::Connection::open(&hermes_path).unwrap();
        hdb.execute(
            "UPDATE session_model_usage SET api_call_count = 5, input_tokens = 1500, output_tokens = 300, last_seen = ?1 WHERE session_id = 'hs1'",
            [now_sec],
        )
        .unwrap();
    }
    hermes::scan_with(
        &mut conn,
        &hermes_path,
        Duration::from_secs(1),
        now_ms,
        &mut resolve,
    )
    .unwrap();
    let hbuckets = buckets::query_buckets(&conn, 0, i64::MAX).unwrap();
    assert_eq!(hbuckets.len(), 1);
    assert_eq!(
        (
            hbuckets[0].calls,
            hbuckets[0].fresh_input,
            hbuckets[0].output
        ),
        (5, 1500, 350)
    );

    // --- OpenCode v1 & v2 ---
    let oc_path = dir.path().join("opencode.db");
    {
        let ocdb = rusqlite::Connection::open(&oc_path).unwrap();
        ocdb.execute_batch(
            "CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT);
             CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_updated INTEGER, data TEXT);
             CREATE TABLE session_v2(id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT);
             CREATE TABLE session_message(id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES ('s1', NULL, '/work/oc-v1');
             INSERT INTO session_v2 VALUES ('s2', NULL, '/work/oc-v2');",
        )
        .unwrap();
        ocdb.execute(
            "INSERT INTO message VALUES ('om1', 's1', ?1, ?2)",
            rusqlite::params![
                now_ms,
                json!({"role":"assistant","modelID":"gpt-5.4","time":{"completed":now_ms},"tokens":{"input":400,"output":50,"reasoning":25,"cache":{"read":300,"write":20}}}).to_string()
            ],
        )
        .unwrap();
        ocdb.execute(
            "INSERT INTO session_message VALUES ('om2', 's2', 'assistant', ?1, ?2)",
            rusqlite::params![
                now_ms,
                json!({"model":{"id":"claude-opus-4-5"},"time":{"completed":now_ms},"tokens":{"input":600,"output":80,"reasoning":10,"cache":{"read":100,"write":0}}}).to_string()
            ],
        )
        .unwrap();
    }
    let mut oc_conn = db::open_at_time(&dir.path().join("oc-usage.db"), now_ms).unwrap();
    let mut plain = |_: &Event| (Project::other(), "native");
    zcode::scan_provider(
        &mut oc_conn,
        &oc_path,
        "opencode",
        Duration::from_secs(1),
        now_ms,
        &mut plain,
    )
    .unwrap();
    let oc_rows = db::events(&oc_conn, 0, i64::MAX).unwrap();
    assert_eq!(oc_rows.len(), 2);
    assert_eq!(
        (
            oc_rows[0].provider.as_str(),
            oc_rows[0].fresh,
            oc_rows[0].read,
            oc_rows[0].output,
            oc_rows[0].cwd.as_str()
        ),
        ("opencode", 400, 300, 75, "/work/oc-v1")
    );
    assert_eq!(
        (
            oc_rows[1].provider.as_str(),
            oc_rows[1].fresh,
            oc_rows[1].output,
            oc_rows[1].cwd.as_str()
        ),
        ("opencode", 600, 90, "/work/oc-v2")
    );

    // --- WorkBuddy separate from CodeBuddy + cwd ---
    let mut wb_state = json!({});
    let wb_line = json!({"id":"l1","cwd":"/work/wb","timestamp":now_ms,"sessionId":"ws1","providerData":{"messageId":"wm1","model":"glm-5.3","usage":{"inputTokens":500,"outputTokens":20,"inputTokensDetails":[{"cached_tokens":200}]}}});
    let wb = parse(
        "workbuddy",
        "/wb/projects/p/ws1.jsonl",
        wb_line,
        &mut wb_state,
    )
    .unwrap();
    assert_eq!(
        (
            wb.provider.as_str(),
            wb.key.as_str(),
            wb.fresh,
            wb.read,
            wb.cwd.as_str()
        ),
        ("workbuddy", "workbuddy:wm1", 300, 200, "/work/wb")
    );

    // --- Kimi wire.jsonl + state.json cwd ---
    let kimi_session = dir.path().join("kimi/sessions/slug/ks1");
    std::fs::create_dir_all(kimi_session.join("agents/main")).unwrap();
    std::fs::write(
        kimi_session.join("state.json"),
        json!({"cwd":"/work/kimi-proj"}).to_string(),
    )
    .unwrap();
    let wire = kimi_session.join("agents/main/wire.jsonl");
    let mut kstate = json!({});
    let ke = parse(
        "kimi",
        wire.to_str().unwrap(),
        json!({"type":"usage.record","agentId":"main","model":"kimi-code/k3","usage":{"inputOther":200,"output":30,"inputCacheRead":50,"inputCacheCreation":10},"time":now_ms}),
        &mut kstate,
    )
    .unwrap();
    assert_eq!(
        (
            ke.provider.as_str(),
            ke.fresh,
            ke.read,
            ke.write5,
            ke.output,
            ke.cwd.as_str()
        ),
        ("kimi", 200, 50, 10, 30, "/work/kimi-proj")
    );

    // --- MiniMax messages.jsonl ---
    let mut mstate = json!({});
    let me = parse(
        "minimax",
        "/mm/v2/sessions/2026/11/20/10-00-00-000-session_bXZzXzEyMw/messages.jsonl",
        json!({"message_id":"mm-1","cwd":"/work/mm","message":{"role":"assistant","model":"MiniMax-M2.7","usage":{"input":120,"output":15,"cacheRead":40,"cacheWrite":5},"timestamp":now_ms}}),
        &mut mstate,
    )
    .unwrap();
    assert_eq!(
        (
            me.provider.as_str(),
            me.session.as_str(),
            me.key.as_str(),
            me.fresh,
            me.read,
            me.cwd.as_str()
        ),
        ("minimax", "mvs_123", "minimax:mm-1", 120, 40, "/work/mm")
    );

    // --- Claude Desktop entrypoint & byte pre-filter ---
    let mut cstate = json!({});
    let cd = parse(
        "claude",
        "/projects/p/s.jsonl",
        json!({"entrypoint":"claude-desktop","cwd":"/work/cd","timestamp":"2026-11-20T00:00:00Z","sessionId":"s","message":{"id":"cd-1","model":"claude-opus-4-5","usage":{"input_tokens":10,"output_tokens":5}}}),
        &mut cstate,
    )
    .unwrap();
    assert_eq!(
        (cd.provider.as_str(), cd.cwd.as_str()),
        ("claude-desktop", "/work/cd")
    );

    let big_irrelevant = format!(
        r#"{{"type":"tool_result","blob":"{}"}}"#,
        "A".repeat(12_000)
    );
    assert!(!parsers::may_contain_relevant(
        "claude",
        big_irrelevant.as_bytes()
    ));
    let big_relevant = format!(
        r#"{{"message":{{"usage":{{}}}},"blob":"{}"}}"#,
        "A".repeat(12_000)
    );
    assert!(parsers::may_contain_relevant(
        "claude",
        big_relevant.as_bytes()
    ));
}

#[test]
fn antigravity_batch_cwd_and_project_attribution_alignment() {
    let dir = tempfile::tempdir().unwrap();
    let agy_root = dir.path().join("agy");
    std::fs::create_dir_all(agy_root.join("brain/conv-1/.system_generated/logs")).unwrap();
    let sdb = rusqlite::Connection::open(agy_root.join("conversation_summaries.db")).unwrap();
    sdb.execute_batch(
        "CREATE TABLE conversation_summaries(conversation_id TEXT PRIMARY KEY, workspace_uris TEXT NOT NULL);",
    )
    .unwrap();
    let (uri, expected_cwd) = if cfg!(windows) {
        ("file:///C:/work/my%20app", r"C:\work\my app")
    } else {
        ("file:///work/my%20app", "/work/my app")
    };
    sdb.execute(
        "INSERT INTO conversation_summaries VALUES ('conv-1', ?1)",
        [format!("[\"{uri}\"]")],
    )
    .unwrap();
    drop(sdb);

    let cwds = parsers::antigravity::load_cwds(&agy_root);
    assert_eq!(cwds.get("conv-1").map(String::as_str), Some(expected_cwd));

    let mut state = parsers::initial();
    let line = json!({"source":"MODEL","step_index":1,"created_at":"2026-11-20T10:00:00Z","model":"gemini-2.5-pro","input_tokens":100,"cache_read_tokens":50,"output_tokens":10});
    let events = parsers::parse_with(
        "antigravity",
        &agy_root.join("brain/conv-1/.system_generated/logs/transcript_full.jsonl"),
        line.to_string().as_bytes(),
        &mut state,
        Some(&cwds),
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cwd, expected_cwd);

    // ProjectResolver merges native cwd, boss key, and worker window.
    let proj_map: HashMap<String, String> = HashMap::new();
    let _ = proj_map;
    let mut resolver = scan::ProjectResolver::with_classifier(&[], &[], |cwd| {
        if cwd.starts_with("/work/app") {
            sample_project("app")
        } else {
            Project::other()
        }
    });
    let ev = Event {
        cwd: "/work/app/sub".into(),
        ..events[0].clone()
    };
    let (proj, role) = resolver.resolve(&ev);
    assert_eq!((proj.key.as_str(), role), ("/work/app", "native"));
}

#[test]
fn query_ranges_granularities_custom_validation_and_prev_comparison() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("usage.db")).unwrap();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
    let now_ms = Local
        .from_local_datetime(&day.and_hms_opt(15, 30, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let today_10 = Local
        .from_local_datetime(&day.and_hms_opt(10, 15, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let yesterday_10 = today_10 - DAY;
    let yesterday_20 = Local
        .from_local_datetime(&day.pred_opt().unwrap().and_hms_opt(20, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let twenty_days_ago = today_10 - 20 * DAY;

    let proj = sample_project("agentnave");
    let make =
        |key: &str, provider: &str, model: &str, ts: i64, fresh: i64, read: i64, out: i64| Event {
            key: key.into(),
            provider: provider.into(),
            ts,
            model: model.into(),
            fresh,
            read,
            output: out,
            calls: 1,
            ..Default::default()
        };

    // Today 10:15 (claude boss) + Yesterday 10:15 (claude native, within same progress window!)
    // + Yesterday 20:00 (outside today's 15:30 progress window!) + 20 days ago (codex worker)
    db::save_event(
        &conn,
        &make("c1", "claude", "claude-opus-4-5", today_10, 1000, 400, 100),
        &proj,
        "boss",
        now_ms,
    )
    .unwrap();
    db::save_event(
        &conn,
        &make(
            "c2",
            "claude",
            "claude-opus-4-5",
            yesterday_10,
            500,
            200,
            50,
        ),
        &proj,
        "native",
        now_ms,
    )
    .unwrap();
    db::save_event(
        &conn,
        &make(
            "c3",
            "claude",
            "claude-opus-4-5",
            yesterday_20,
            9000,
            0,
            1000,
        ),
        &proj,
        "native",
        now_ms,
    )
    .unwrap();
    db::save_event(
        &conn,
        &make("x1", "codex", "gpt-5.4", twenty_days_ago, 2000, 1000, 200),
        &proj,
        "worker",
        now_ms,
    )
    .unwrap();

    // 1. `today`: 24 hour slots, prev_overall matches yesterday 00:00..16:00 ONLY (excludes 20:00!).
    let r_today = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "today",
            custom: None,
            granularity: None,
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_today["granularity"], "hour");
    assert_eq!(r_today["available_granularities"], json!(["hour"]));
    assert_eq!(r_today["overall"]["total_tokens"], 1500);
    assert_eq!(r_today["overall"]["net_tokens"], 1100);
    assert_eq!(r_today["prev_overall"]["total_tokens"], 750);
    assert_eq!(r_today["prev_overall"]["net_tokens"], 550);
    assert_eq!(
        r_today["providers"][0]["models"][0]["key"],
        "claude::claude-opus-4-5"
    );
    assert_eq!(r_today["providers"][0]["prev_total_tokens"], 750);
    assert_eq!(r_today["projects"][0]["total"]["total_tokens"], 1500);
    assert_eq!(r_today["projects"][0]["boss"]["total_tokens"], 1500);
    assert_eq!(r_today["projects"][0]["native"]["total_tokens"], 0);
    assert!(r_today["daily"][10]["by_model"]["claude::claude-opus-4-5"].is_object());

    // 2. `24h`: 24 non-future hour slots including yesterday 20:00 and today 10:15.
    let r_24h = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "24h",
            custom: None,
            granularity: None,
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_24h["daily"].as_array().unwrap().len(), 24);
    assert!(
        r_24h["daily"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["future"] == false)
    );
    assert_eq!(r_24h["overall"]["total_tokens"], 11500);

    // 3. `7d` with manual granularity=hour override vs default day.
    let r_7d_day = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "7d",
            custom: None,
            granularity: None,
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_7d_day["granularity"], "day");
    assert_eq!(r_7d_day["available_granularities"], json!(["hour", "day"]));
    assert_eq!(r_7d_day["daily"].as_array().unwrap().len(), 7);

    let r_7d_hour = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "7d",
            custom: None,
            granularity: Some("hour"),
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_7d_hour["granularity"], "hour");

    // 4. `30d`: default day, available [day, week]; prev_overall is null because min_ts_ms is 20d ago!
    let r_30d = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "30d",
            custom: None,
            granularity: None,
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_30d["granularity"], "day");
    assert_eq!(r_30d["available_granularities"], json!(["day", "week"]));
    assert!(r_30d["prev_overall"].is_null());
    assert!(r_30d["providers"][0]["prev_cost_usd"].is_null());
    // Project totals align with overall across boss + workers + native.
    assert_eq!(
        r_30d["projects"][0]["total"]["total_tokens"],
        r_30d["overall"]["total_tokens"]
    );
    assert_eq!(r_30d["projects"][0]["workers"]["total_tokens"], 3200);

    // 5. `90d`: default week, available [day, week], supports manual day override.
    let r_90d = query::respond_db(
        &conn,
        &query::QuerySpec {
            range: "90d",
            custom: None,
            granularity: None,
        },
        now_ms,
        vec![],
    )
    .unwrap();
    assert_eq!(r_90d["granularity"], "week");
    assert_eq!(r_90d["available_granularities"], json!(["day", "week"]));

    // 6. Query parser validation for `custom` & invalid params.
    let valid_custom = format!(
        "range=custom&from={}&to={}&granularity=hour",
        now_ms - 4 * HOUR,
        now_ms
    );
    assert!(parse_usage_query(Some(&valid_custom)).is_ok());
    assert!(parse_usage_query(Some("range=custom&from=2000&to=1000")).is_err());
    assert!(parse_usage_query(Some("range=custom")).is_err());
    assert!(parse_usage_query(Some("range=invalid")).is_err());
    assert!(parse_usage_query(Some("range=7d&granularity=minute")).is_err());
    let too_wide = format!("range=custom&from={}&to={}", now_ms - 95 * DAY, now_ms);
    assert!(parse_usage_query(Some(&too_wide)).is_err());
}
