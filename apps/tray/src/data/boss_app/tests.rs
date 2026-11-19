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
        "archived_sessions/rollout-2026-10-01T00-00-00-{CODEX_SESSION}_{CLAUDE_SESSION}.jsonl.zst"
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

/// A terminal continues a chat by the ID its session record names, only when that ID's key is the
/// chat's own.
#[test]
fn a_chat_is_continued_by_the_id_whose_key_matches() {
    let id = "01a1ef3a-944a-7fc1-b936-694b4760a644";
    let key = boss_key("codex", id);
    let rollout =
        Path::new("/s/rollout-2026-11-18T23-15-56-01a1ef3a-944a-7fc1-b936-694b4760a644.jsonl");
    assert_eq!(
        native_session("codex", &key, Some(rollout))
            .map(|found| found.0)
            .as_deref(),
        Some(id)
    );
    assert_eq!(native_session("codex", "s-other", Some(rollout)), None);
    let claude = boss_key("claude", id);
    let transcript = PathBuf::from(format!("/p/x/{id}.jsonl"));
    assert_eq!(
        native_session("claude", &claude, Some(&transcript))
            .map(|found| found.0)
            .as_deref(),
        Some(id)
    );
    assert_eq!(native_session("grok", &claude, Some(&transcript)), None);
}

#[test]
fn a_chat_runs_again_in_the_folder_its_record_names() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("x.jsonl");
    fs::write(
        &record,
        "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/app\"}}\n",
    )
    .unwrap();
    assert_eq!(session_cwd(&record), Some(PathBuf::from("/work/app")));
}

fn titled(kind: &str, key: &str, text: &str) -> String {
    format!("{{\"type\":\"{kind}\",\"{key}\":\"{text}\",\"sessionId\":\"{CLAUDE_SESSION}\"}}\n")
}

/// A Claude chat is called by the name the user gave it, else by the one Claude wrote; empty
/// names count for nothing and the latest entry of each kind wins.
#[test]
fn a_claude_title_prefers_the_users_own_and_the_latest_entry() {
    let dir = tempfile::tempdir().unwrap();
    let auto = |text| titled("ai-title", "aiTitle", text);
    let custom = |text| titled("custom-title", "customTitle", text);
    let named = |text: &str, custom: bool| title::CliTitle {
        text: text.to_owned(),
        custom,
    };

    let write_to = |name: &str, text: String| {
        let file = dir.path().join(name);
        fs::write(&file, text).unwrap();
        title::claude(&file)
    };
    assert_eq!(write_to("none", "{\"type\":\"user\"}\n".to_owned()), None);
    assert_eq!(
        write_to("auto", [auto("first"), auto("second")].concat()),
        Some(named("second", false))
    );
    // A name the user gave wins over anything Claude wrote, before or after it.
    assert_eq!(
        write_to(
            "mine",
            [auto("a"), custom("mine"), auto("b"), custom("")].concat()
        ),
        Some(named("mine", true))
    );
    assert_eq!(
        write_to(
            "newer",
            [custom("mine"), custom("newer"), auto("b")].concat()
        ),
        Some(named("newer", true))
    );
}

/// A long transcript is read from its end, and from its start only when the end names nothing.
#[test]
fn a_claude_title_far_from_the_end_is_still_found_at_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("long.jsonl");
    let filler = format!("{{\"type\":\"user\",\"text\":\"{}\"}}\n", "x".repeat(1000));
    let body = filler.repeat(WINDOW as usize / filler.len() + 10);
    fs::write(
        &file,
        [titled("ai-title", "aiTitle", "early"), body.clone()].concat(),
    )
    .unwrap();
    assert_eq!(
        title::claude(&file).map(|t| t.text).as_deref(),
        Some("early")
    );
    // The same title written again near the end is the one a tail read sees.
    fs::write(
        &file,
        [
            titled("ai-title", "aiTitle", "early"),
            body,
            titled("ai-title", "aiTitle", "late"),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(
        title::claude(&file).map(|t| t.text).as_deref(),
        Some("late")
    );
}

#[test]
fn codex_names_come_from_its_session_index_and_the_latest_line_wins() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("session_index.jsonl"),
        &format!(
            "{{\"id\":\"{CODEX_SESSION}\",\"thread_name\":\"old\"}}\n\
             {{\"id\":\"other\",\"thread_name\":\"x\"}}\n\
             broken line\n\
             {{\"id\":\"{CODEX_SESSION}\",\"thread_name\":\"renamed\"}}\n"
        ),
    );
    let homes = vec![PathBuf::from("/missing"), dir.path().to_owned()];
    assert_eq!(
        title::codex(&homes, CODEX_SESSION).map(|t| (t.text, t.custom)),
        Some(("renamed".to_owned(), false))
    );
    assert_eq!(title::codex(&homes, "unknown"), None);
}

/// Each workspace gets its boss chat's CLI name: Claude's from the transcript, Codex's from the
/// index by the rollout's ID; a chat whose record is unreadable gets none.
#[test]
fn workspaces_are_titled_by_their_clis_names_for_the_chats() {
    let dir = tempfile::tempdir().unwrap();
    let (claude, codex) = (dir.path().join("claude"), dir.path().join("codex"));
    let connection = connection();
    let transcript = claude.join(format!("projects/p/{CLAUDE_SESSION}.jsonl"));
    write(&transcript, &titled("ai-title", "aiTitle", "Claude chat"));
    let rollout = codex.join(format!(
        "sessions/2026/11/19/rollout-2026-11-19T01-00-00-{CODEX_SESSION}.jsonl"
    ));
    write(
        &rollout,
        "{\"payload\":{\"originator\":\"codex_cli_rs\"}}\n",
    );
    write(
        &codex.join("session_index.jsonl"),
        &format!("{{\"id\":\"{CODEX_SESSION}\",\"thread_name\":\"Codex chat\"}}\n"),
    );
    let (claude_key, codex_key) = (
        boss_key("claude", CLAUDE_SESSION),
        boss_key("codex", CODEX_SESSION),
    );
    for (workspace, key, host) in [
        ("s-claude", &claude_key, "claude"),
        ("s-codex", &codex_key, "codex"),
    ] {
        connection
            .execute(
                "INSERT INTO workspaces VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![workspace, key, host],
            )
            .unwrap();
    }
    older_chat(
        &connection,
        "claude",
        "88888888-8888-8888-8888-888888888888",
    );
    let titles = BossApps::new().titles(&connection, || Homes {
        claude: vec![claude],
        codex: vec![codex],
        desktop: Vec::new(),
    });
    let text = |workspace: &str| {
        titles
            .iter()
            .find(|(id, _)| id == workspace)
            .map(|(_, title)| title.text.as_str())
    };
    assert_eq!(text("s-claude"), Some("Claude chat"));
    assert_eq!(text("s-codex"), Some("Codex chat"));
    assert_eq!(titles.len(), 2);
}

/// A terminal's Claude chat is found by the ID the terminal chose, in whichever project folder
/// Claude filed it; a Codex chat is found only when exactly one rollout in the terminal's folder
/// began after it started.
#[test]
fn a_terminals_chat_title_is_found_without_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let (claude, codex) = (dir.path().join("claude"), dir.path().join("codex"));
    let work = dir.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let homes = Homes {
        claude: vec![claude.clone()],
        codex: vec![codex.clone()],
        desktop: Vec::new(),
    };
    let started = std::time::SystemTime::now() - Duration::from_secs(5);
    let (found, none) = title::of_terminal(&homes, "claude", Some(CLAUDE_SESSION), &work, started);
    assert_eq!((found.as_deref(), none), (Some(CLAUDE_SESSION), None));
    write(
        &claude.join(format!("projects/any/{CLAUDE_SESSION}.jsonl")),
        &titled("ai-title", "aiTitle", "Fresh"),
    );
    let (_, named) = title::of_terminal(&homes, "claude", Some(CLAUDE_SESSION), &work, started);
    assert_eq!(named.map(|t| t.text).as_deref(), Some("Fresh"));

    // Codex: nothing yet, then one rollout in this folder (another folder's does not count).
    let day = chrono::Local::now().format("%Y/%m/%d").to_string();
    let rollout = |id: &str, folder: &Path| {
        let path = codex.join(format!(
            "sessions/{day}/rollout-2026-11-19T01-00-00-{id}.jsonl"
        ));
        write(
            &path,
            &format!("{{\"payload\":{{\"cwd\":\"{}\"}}}}\n", folder.display()),
        );
    };
    write(
        &codex.join("session_index.jsonl"),
        &format!("{{\"id\":\"{CODEX_SESSION}\",\"thread_name\":\"Codex fresh\"}}\n"),
    );
    assert_eq!(
        title::of_terminal(&homes, "codex", None, &work, started),
        (None, None)
    );
    rollout(
        "99999999-9999-9999-9999-999999999999",
        Path::new("/elsewhere"),
    );
    rollout(CODEX_SESSION, &work);
    let (id, named) = title::of_terminal(&homes, "codex", None, &work, started);
    assert_eq!(id.as_deref(), Some(CODEX_SESSION));
    assert_eq!(named.map(|t| t.text).as_deref(), Some("Codex fresh"));
    // A second candidate makes the match ambiguous, so none is taken.
    rollout("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", &work);
    assert_eq!(
        title::of_terminal(&homes, "codex", None, &work, started),
        (None, None)
    );
    // One that began before the terminal started is not its chat.
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    assert_eq!(
        title::of_terminal(&homes, "codex", None, &work, later),
        (None, None)
    );
    // Other CLIs have no title to read.
    assert_eq!(
        title::of_terminal(&homes, "grok", None, &work, started),
        (None, None)
    );
}
