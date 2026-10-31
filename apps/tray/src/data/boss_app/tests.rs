use super::*;

const CLAUDE_SESSION: &str = "11111111-2222-3333-4444-555555555555";
const CODEX_SESSION: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A zstd frame holding `data` in one raw block, as an archived rollout is packed.
fn packed(data: &[u8]) -> Vec<u8> {
    let block = (u32::try_from(data.len()).unwrap() << 3) | 1;
    let mut frame = vec![
        0x28,
        0xB5,
        0x2F,
        0xFD,
        0x20,
        u8::try_from(data.len()).unwrap(),
    ];
    frame.extend_from_slice(&block.to_le_bytes()[..3]);
    frame.extend_from_slice(data);
    frame
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn connection() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE conversations (manager_session TEXT, host TEXT);
             CREATE TABLE workspaces (id TEXT, owner TEXT, owner_host TEXT, owner_file TEXT);",
        )
        .unwrap();
    connection
}

fn older_chat(connection: &rusqlite::Connection, host: &str, session: &str) -> String {
    let key = boss_key(host, session);
    connection
        .execute(
            "INSERT INTO conversations VALUES (?1, ?2)",
            rusqlite::params![key, host],
        )
        .unwrap();
    key
}

#[test]
fn boss_keys_match_the_service() {
    // Python: "s-" + sha256(json.dumps([host, id], ensure_ascii=False).encode()).hexdigest()[:32]
    assert_eq!(
        boss_key("claude", CLAUDE_SESSION),
        "s-b05cc072347fe5d96ecec95864f12552"
    );
    assert_eq!(
        boss_key("codex", CODEX_SESSION),
        "s-f0fbe2aac7cf2a31d1e053814f45f640"
    );
}

#[test]
fn chats_without_a_saved_path_are_found_by_their_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let codex = dir.path().join("codex");
    let connection = connection();

    // Older chat; a session that changed folders leaves copies, and the newest one counts.
    let key = older_chat(&connection, "claude", CLAUDE_SESSION);
    let stale = claude.join(format!("projects/old/{CLAUDE_SESSION}.jsonl"));
    write(&stale, "{\"entrypoint\":\"cli\"}\n");
    let older = fs::metadata(&stale).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(older - std::time::Duration::from_secs(60))
        .unwrap();
    write(
        &claude.join(format!("projects/new/{CLAUDE_SESSION}.jsonl")),
        "{\"entrypoint\":\"claude-desktop\"}\n",
    );

    // A recorded workspace whose saved path has gone: Codex archived and packed the rollout.
    let codex_key = boss_key("codex", CODEX_SESSION);
    connection
        .execute(
            "INSERT INTO workspaces VALUES ('s-codex', ?1, 'codex', '/gone/rollout.jsonl')",
            [&codex_key],
        )
        .unwrap();
    let archived = codex.join(format!(
        "archived_sessions/rollout-2026-10-01T00-00-00-{CODEX_SESSION}.jsonl.zst"
    ));
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::write(
        &archived,
        packed(b"{\"payload\":{\"originator\":\"codex_cli_rs\"}}\n"),
    )
    .unwrap();

    let homes = || Homes {
        claude: vec![claude],
        codex: vec![Path::new("/missing").to_owned(), codex],
        desktop: Vec::new(),
    };
    assert_eq!(
        BossApps::new().of(&connection, homes),
        vec![(key, CLAUDE_DESKTOP), ("s-codex".to_owned(), CODEX_CLI)]
    );
}

#[test]
fn desktop_records_name_chats_whose_transcript_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Claude");
    write(
        &data.join("claude-code-sessions/account/org/local_x.json"),
        &format!(
            "{{\"sessionId\":\"local_x\",\"priorCliSessionIds\":[\"{CLAUDE_SESSION}\"],\"cliSessionId\":\"other\"}}"
        ),
    );
    let connection = connection();
    let key = older_chat(&connection, "claude", CLAUDE_SESSION);
    let unknown = older_chat(&connection, "claude", CODEX_SESSION);
    let cowork = "77777777-7777-7777-7777-777777777777";
    write(
        &data.join("local-agent-mode-sessions/account/org/local_y.json"),
        &format!("{{\"sessionId\":\"local_y\",\"cliSessionId\":\"{cowork}\"}}"),
    );
    let cowork = older_chat(&connection, "claude", cowork);
    let found = BossApps::new().of(&connection, || Homes {
        desktop: vec![data],
        ..Homes::default()
    });
    let mut known = vec![(key, CLAUDE_DESKTOP), (cowork, CLAUDE_DESKTOP)];
    known.sort_unstable();
    assert_eq!(found, known);
    assert!(!found.iter().any(|(workspace, _)| *workspace == unknown));
}

#[test]
fn only_known_codex_entries_are_named() {
    for (originator, app) in [
        ("Codex Desktop", Some(CODEX_DESKTOP)),
        ("codex_work_desktop", Some(CODEX_DESKTOP)),
        ("codex_cli_rs", Some(CODEX_CLI)),
        ("codex_exec", Some(CODEX_CLI)),
        ("codex_vscode", None),
    ] {
        assert_eq!(codex_app(originator), app, "{originator}");
    }
    assert_eq!(claude_app("claude-desktop-3p"), CLAUDE_DESKTOP);
    assert_eq!(claude_app("sdk-cli"), CLAUDE_CLI);
}

#[test]
fn a_startup_miss_is_retried_when_probed_roots_arrive() {
    let dir = tempfile::tempdir().unwrap();
    let codex = dir.path().join("codex-work");
    write(
        &codex.join(format!("sessions/2026/10/31/rollout-{CODEX_SESSION}.jsonl")),
        "{\"payload\":{\"originator\":\"Codex Desktop\"}}\n",
    );
    let connection = connection();
    let key = older_chat(&connection, "codex", CODEX_SESSION);
    let apps = BossApps::new();
    assert!(apps.of(&connection, Homes::default).is_empty());
    assert_eq!(
        apps.of(&connection, || Homes {
            codex: vec![codex],
            ..Homes::default()
        }),
        vec![(key, CODEX_DESKTOP)],
    );
}
