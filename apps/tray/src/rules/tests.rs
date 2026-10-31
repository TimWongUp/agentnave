use super::*;

#[test]
fn one_row_per_file_names_every_app_that_reads_it() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let app = |id, label, root: &str| App {
        key: id,
        id,
        label,
        root: h.join(root),
    };
    let apps = [
        app("claude", "Claude Code", ".claude"),
        app("grok", "Grok", ".grok"),
        app("codex", "Codex", ".codex"),
        app("antigravity", "Antigravity", ".gemini/config"),
        app("pi", "Pi", ".pi/agent"),
        app("zcode", "ZCode", ".zcode"),
    ];
    for file in [
        ".claude/CLAUDE.md",
        ".codex/AGENTS.override.md",
        ".codex/AGENTS.md",
        ".gemini/AGENTS.md",
        ".pi/agent/CLAUDE.md",
    ] {
        std::fs::create_dir_all(h.join(file).parent().unwrap()).unwrap();
        std::fs::write(h.join(file), "rules").unwrap();
    }
    let listed = list(&apps, &[], h, &json!({"preset": "off"}));
    let rows = listed["files"].as_array().unwrap();
    let row = |file: &str| {
        rows.iter()
            .find(|row| Path::new(row["path"].as_str().unwrap()) == h.join(file))
            .cloned()
    };
    assert_eq!(
        row(".claude/CLAUDE.md").unwrap()["readers"],
        json!(["Claude Code", "Grok"])
    );
    // Codex and Pi read only the first that exists.
    assert!(row(".codex/AGENTS.override.md").is_some());
    assert!(row(".codex/AGENTS.md").is_none());
    assert_eq!(
        row(".pi/agent/CLAUDE.md").unwrap()["readers"],
        json!(["Pi"])
    );
    assert_eq!(
        row(".gemini/AGENTS.md").unwrap()["readers"],
        json!(["Antigravity"])
    );
    assert!(row(".gemini/GEMINI.md").is_none());
    // Grok already reads Claude's file; ZCode has none yet, so its usual one comes last.
    assert!(row(".grok/AGENTS.md").is_none());
    assert_eq!(
        Path::new(rows.last().unwrap()["path"].as_str().unwrap()),
        h.join(".zcode/AGENTS.md")
    );
    assert_eq!(rows.last().unwrap()["exists"], false);

    assert!(
        open(&apps, h, rows.last().unwrap()["path"].as_str().unwrap())
            .unwrap_err()
            .contains("未找到")
    );
    assert!(open(&apps, h, "/etc/hosts").is_err());
    assert!(!h.join(".zcode").exists()); // never created

    // Grok can be told not to read `~/.claude`; its own file counts under its exact name.
    std::fs::create_dir_all(h.join(".grok")).unwrap();
    std::fs::write(
        h.join(".grok/config.toml"),
        "[compat.claude]\nagents = false\n",
    )
    .unwrap();
    std::fs::write(h.join(".grok/Agents.md"), "rules").unwrap();
    let listed = list(&apps, &[], h, &json!({"preset": "off"}));
    let readers = |file: &str| {
        let rows = listed["files"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|row| Path::new(row["path"].as_str().unwrap()) == h.join(file));
        row.map(|row| row["readers"].clone())
    };
    assert_eq!(readers(".claude/CLAUDE.md"), Some(json!(["Claude Code"])));
    assert_eq!(readers(".grok/Agents.md"), Some(json!(["Grok"])));
    assert_eq!(readers(".grok/AGENTS.md"), None);
}

#[test]
fn the_block_goes_once_into_each_connected_app_and_leaves_the_rest_untouched() {
    let home = tempfile::tempdir().unwrap();
    let base = home.path().canonicalize().unwrap();
    let h = base.as_path();
    let app = |id, label, root: &str| App {
        key: id,
        id,
        label,
        root: h.join(root),
    };
    let claude = app("claude", "Claude Code", ".claude");
    let grok = app("grok", "Grok", ".grok");
    let codex = app("codex", "Codex", ".codex");
    let installed = [
        app("claude", "Claude Code", ".claude"),
        app("grok", "Grok", ".grok"),
        app("codex", "Codex", ".codex"),
    ];
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::create_dir_all(h.join(".grok")).unwrap();
    let own = "# 我的规则\n\n- 保留\n";
    std::fs::write(h.join(".claude/CLAUDE.md"), own).unwrap();
    // A block left in a file no connected app needs any more is taken out.
    std::fs::write(
        h.join(".grok/AGENTS.md"),
        format!("grok\n\n{}\n", block("旧")),
    )
    .unwrap();
    let backups = std::cell::RefCell::new(Vec::new());
    let backup = |path: &Path, _: &[u8]| {
        backups.borrow_mut().push(path.to_owned());
        Ok(())
    };
    let connected = [claude, grok, codex];
    let split = json!({"preset": "split"});
    let text = text_of(&split, "split").unwrap();
    assert!(sync(&installed, &connected, h, &split, &[], &backup).is_empty());
    let claude_md = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(
        claude_md,
        format!("# 我的规则\n\n- 保留\n\n{}\n", block(&text))
    );
    // Grok reads Claude's file too, so its own carries no second copy.
    assert_eq!(
        std::fs::read_to_string(h.join(".grok/AGENTS.md")).unwrap(),
        "grok\n"
    );
    assert!(
        std::fs::read_to_string(h.join(".codex/AGENTS.md"))
            .unwrap()
            .contains(START)
    );
    let listed = list(&installed, &connected, h, &split);
    let current: Vec<_> = listed["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["current"] == true)
        .map(|row| row["path"].clone())
        .collect();
    assert_eq!(
        current,
        [
            json!(h.join(".claude/CLAUDE.md")),
            json!(h.join(".codex/AGENTS.md"))
        ]
    );

    // Unchanged settings write nothing; turning it off restores every file as it was.
    backups.borrow_mut().clear();
    assert!(sync(&installed, &connected, h, &split, &[], &backup).is_empty());
    assert!(backups.borrow().is_empty());
    assert!(
        sync(
            &installed,
            &connected,
            h,
            &json!({"preset": "off"}),
            &[],
            &backup
        )
        .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap(),
        own
    );
    assert!(!h.join(".codex/AGENTS.md").exists());

    // Neither file exists yet: Grok would read the one created for Claude, so it gets no other.
    std::fs::remove_file(h.join(".claude/CLAUDE.md")).unwrap();
    std::fs::remove_file(h.join(".grok/AGENTS.md")).unwrap();
    assert!(sync(&installed, &connected, h, &split, &[], &backup).is_empty());
    assert!(h.join(".claude/CLAUDE.md").is_file());
    assert!(!h.join(".grok/AGENTS.md").exists());
    // A lone marker is left for the user rather than guessed at.
    let broken = format!("mine\n{START}\nold\n");
    std::fs::write(h.join(".codex/AGENTS.md"), &broken).unwrap();
    let failures = sync(&installed, &connected, h, &split, &[], &backup);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["path"], json!(h.join(".codex/AGENTS.md")));
    assert_eq!(
        std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap(),
        broken
    );

    // Adding and removing the block leaves the user's own spacing as it was.
    for own in ["我的规则\n\n\n", "我的规则\n", "我的规则\r\n"] {
        let added = splice(own, Some("x")).unwrap();
        assert_eq!(splice(&added, None).unwrap(), own);
    }
    let resaved = splice("我的规则\n", Some("x"))
        .unwrap()
        .replace('\n', "\r\n");
    assert_eq!(splice(&resaved, None).unwrap(), "我的规则\r\n");
    let alone = splice("", Some("x")).unwrap().replace('\n', "\r\n");
    assert_eq!(splice(&alone, None).unwrap(), "");
    // A file a moved host read before still loses its block.
    let moved = h.join("old-root/AGENTS.md");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::write(&moved, format!("old\n\n{}\n", block("旧"))).unwrap();
    sync(
        &installed,
        &connected,
        h,
        &json!({"preset": "off"}),
        std::slice::from_ref(&moved),
        &backup,
    );
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "old\n");

    assert!(check_custom("<!-- AGENTNAVE:END -->").is_err());
    assert_eq!(text_of(&json!({"custom": "  "}), "custom"), None);
}

#[test]
fn each_boss_gets_its_own_mode_and_apps_sharing_a_file_get_one() {
    let home = tempfile::tempdir().unwrap();
    let h = &home.path().canonicalize().unwrap();
    let app = |id, label, root: &str| App {
        key: id,
        id,
        label,
        root: h.join(root),
    };
    let apps = [
        app("claude", "Claude Code", ".claude"),
        app("codex", "Codex", ".codex"),
        app("grok", "Grok", ".grok"),
        app("zcode", "ZCode", ".zcode"),
    ];
    let backup = |_: &Path, _: &[u8]| Ok(());
    let document = json!({"version": 1, "preset": "off", "custom": "我的规则",
        "apps": {"claude": "split", "grok": "parallel", "codex": "custom"}});
    assert!(sync(&apps, &apps, h, &document, &[], &backup).is_empty());

    // Grok reads Claude's file, so it runs on the first app's mode rather than a second block.
    let claude = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude.contains(SHARED) && !claude.contains(PARALLEL));
    assert!(!h.join(".grok/AGENTS.md").exists());
    let codex = std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap();
    assert!(codex.contains("我的规则") && !codex.contains(SHARED));
    // ZCode has no mode of its own and follows the default.
    assert!(!h.join(".zcode/AGENTS.md").exists());

    let listed = list(&apps, &apps, h, &document);
    assert_eq!(
        listed["bosses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|boss| boss["labels"].clone())
            .collect::<Vec<_>>(),
        [
            json!(["Claude Code", "Grok"]),
            json!(["Codex"]),
            json!(["ZCode"])
        ]
    );
    for row in listed["files"].as_array().unwrap() {
        assert_eq!(row["target"], row["current"], "{row}");
    }

    let valid = crate::members::validate_orchestration;
    assert!(valid(&document).is_ok());
    assert!(valid(&json!({"version": 1, "preset": "off", "apps": {"codex": "fast"}})).is_err());
    assert!(valid(&json!({"version": 1, "preset": "off", "apps": {"codex": "custom"}})).is_err());

    // Turning one boss off leaves the other's block alone.
    let document = json!({"preset": "off", "custom": "我的规则", "apps": {"codex": "custom"}});
    assert!(sync(&apps, &apps, h, &document, &[], &backup).is_empty());
    assert!(!h.join(".claude/CLAUDE.md").exists());
    assert!(h.join(".codex/AGENTS.md").exists());

    // Claude joining Grok's file later does not undo the mode already set on Grok.
    let document = json!({"preset": "off", "apps": {"grok": "split"}});
    assert!(sync(&apps, &apps, h, &document, &[], &backup).is_empty());
    let claude = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude.contains(SHARED));
}
