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
    assert!(text.contains("只在用户要求时开会") && !text.contains("boss 可以开 AgentNave 会议室"));
    let meetings = json!({"preset": "split", "split_options": {"meetings": true}});
    let meetings = text_of(&meetings, "split").unwrap();
    assert!(
        meetings.contains("boss 可以开 AgentNave 会议室")
            && !meetings.contains("只在用户要求时开会")
    );
    let lead = &catalog::catalog()["switches"]["lead_only"];
    let lead_asked = lead["when_false"].as_str().unwrap();
    let lead_always = lead["when_true"].as_str().unwrap();
    let meetings_head = catalog::section("meetings_only_head");
    let meetings_tail = catalog::section("meetings_only_tail");
    let split_meetings_asked = catalog::section("meetings_when_asked");
    let split_meetings_own = catalog::section("meetings_on_tradeoffs");
    assert!(text.contains(lead_asked) && !text.contains(lead_always));
    let parallel = text_of(&json!({"preset": "parallel"}), "parallel").unwrap();
    assert!(parallel.contains("boss 可以开 AgentNave 会议室"));
    let quiet = json!({"preset": "parallel", "parallel_options": {"meetings": false}});
    assert!(
        text_of(&quiet, "parallel")
            .unwrap()
            .contains("只在用户要求时开会")
    );
    for mode in ["split", "parallel"] {
        let lead = text_of(&json!({"preset": mode, "lead_only": true}), mode).unwrap();
        assert!(lead.contains(lead_always) && !lead.contains(lead_asked));
    }
    let meetings_only_doc = json!({
        "preset": "meetings",
        "lead_only": true,
        "enabled_roles": ["explorer", "worker", "worker_writing"],
        "image_generation": {"provider": "codex", "model": "chat"}
    });
    let plain_meetings = format!("{meetings_head}{meetings_tail}");
    let meetings_only = text_of(&meetings_only_doc, "meetings").unwrap();
    assert_eq!(meetings_only, plain_meetings);
    assert_eq!(
        presets(&meetings_only_doc)["meetings"],
        json!(plain_meetings)
    );
    assert_eq!(presets(&meetings_only_doc)["meeting"], Value::Null);
    assert!(!meetings_only.contains("explorer-easy"));
    assert!(!meetings_only.contains("worker-writing-easy"));
    assert!(!meetings_only.contains("image-generator"));
    assert!(!meetings_only.contains(lead_asked) && !meetings_only.contains(lead_always));
    for mode in ["meetings", "split", "parallel"] {
        assert!(
            !text_of(&json!({"preset": mode}), mode)
                .unwrap()
                .contains("开会默认")
        );
    }
    let partial_seats = "\n- 开会默认 4 席：Codex CLI（provider=`codex`）、Antigravity CLI（provider=`antigravity`），其余由 boss 选；同一个 CLI 或模型可以占多席。用户点名的成员和人数优先，超过 6 席须用户指定。";
    let doc_partial = json!({
        "preset": "meetings",
        "custom": "用户自写规则",
        "enabled_roles": ["explorer"],
        "meeting": {
            "count": 4,
            "seats": [
                {"provider": "codex", "model": "gpt-6.1-sol", "effort": "high"},
                null,
                {"provider": "antigravity", "model": "gemini-3.8-flash"}
            ]
        }
    });
    assert_eq!(
        text_of(&doc_partial, "meetings").unwrap(),
        format!("{meetings_head}{partial_seats}{meetings_tail}")
    );
    assert!(text_of(&doc_partial, "split").unwrap().contains(&format!(
        "{split_meetings_asked}{partial_seats}\n- 通用探索"
    )));
    assert!(
        text_of(&doc_partial, "parallel")
            .unwrap()
            .contains(&format!("{split_meetings_own}{partial_seats}\n- 通用探索"))
    );
    assert!(
        !text_of(&doc_partial, "custom")
            .unwrap()
            .contains("开会默认")
    );
    let all_seats = "\n- 开会默认 2 席：Codex CLI（provider=`codex`）、Codex CLI（provider=`codex`）；同一个 CLI 或模型可以占多席。用户点名的成员和人数优先，超过 6 席须用户指定。";
    let doc_all = json!({
        "meeting": {
            "count": 2,
            "seats": [
                {"provider": "codex", "model": "gpt-6.1-sol", "effort": "high"},
                {"provider": "codex", "model": "gpt-6.1-sol"}
            ]
        }
    });
    for mode in ["meetings", "split", "parallel"] {
        let generated = text_of(&doc_all, mode).unwrap();
        assert!(generated.contains(all_seats) && !generated.contains("其余由 boss 选"));
    }
    let no_seats = "\n- 开会默认 5 席，成员由 boss 选；同一个 CLI 或模型可以占多席。用户点名的成员和人数优先，超过 6 席须用户指定。";
    for doc_none in [
        json!({"meeting": {"count": 5}}),
        json!({"meeting": {"count": 5, "seats": [null, null]}}),
    ] {
        for mode in ["meetings", "split", "parallel"] {
            assert!(text_of(&doc_none, mode).unwrap().contains(no_seats));
        }
    }
    assert!(sync(&installed, &connected, h, &split, &[], &backup, false).is_empty());
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
    assert!(sync(&installed, &connected, h, &split, &[], &backup, false).is_empty());
    assert!(backups.borrow().is_empty());
    assert!(
        sync(
            &installed,
            &connected,
            h,
            &json!({"preset": "off"}),
            &[],
            &backup,
            false
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
    assert!(sync(&installed, &connected, h, &split, &[], &backup, false).is_empty());
    assert!(h.join(".claude/CLAUDE.md").is_file());
    assert!(!h.join(".grok/AGENTS.md").exists());
    // A lone marker is left for the user rather than guessed at.
    let broken = format!("mine\n{START}\nold\n");
    std::fs::write(h.join(".codex/AGENTS.md"), &broken).unwrap();
    let failures = sync(&installed, &connected, h, &split, &[], &backup, false);
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
        false,
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
    assert!(sync(&apps, &apps, h, &document, &[], &backup, false).is_empty());

    // Grok reads Claude's file, so it runs on the first app's mode rather than a second block.
    let shared = catalog::section("shared");
    let parallel = catalog::section("parallel_writes");
    let claude = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude.contains(shared) && !claude.contains(parallel));
    assert!(!h.join(".grok/AGENTS.md").exists());
    let codex = std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap();
    assert!(codex.contains("我的规则") && !codex.contains(shared));
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
    let image = |seat| json!({"version": 1, "preset": "off", "image_generation": seat});
    assert!(
        valid(&image(
            json!({"provider": "codex", "model": "m", "effort": "high"})
        ))
        .is_ok()
    );
    assert!(valid(&image(json!({"easy": {"provider": "codex", "model": "m"}}))).is_err());
    assert!(valid(&image(json!({"provider": "codex", "model": " "}))).is_err());
    assert!(
        valid(&image(
            json!({"provider": "codex", "model": "m", "effort": 1})
        ))
        .is_err()
    );
    let meeting = |m| json!({"version": 1, "preset": "meetings", "meeting": m});
    let seat = json!({"provider": "codex", "model": "m", "effort": "high"});
    assert!(valid(&meeting(json!({"count": 2, "seats": [&seat, &seat]}))).is_ok());
    assert!(valid(&meeting(json!({"count": 6, "seats": [null, &seat]}))).is_ok());
    assert!(valid(&meeting(json!({"count": 3}))).is_ok());
    assert!(valid(&meeting(json!({"count": 1}))).is_err());
    assert!(valid(&meeting(json!({"count": 7}))).is_err());
    assert!(valid(&meeting(json!({"count": 2, "seats": [null, null, null]}))).is_err());
    assert!(valid(&meeting(json!({"seats": [null]}))).is_err());
    assert!(valid(&meeting(json!({"count": 3, "members": ["codex"]}))).is_err());
    assert!(
        valid(&meeting(
            json!({"count": 2, "seats": [{"provider": "unknown", "model": "m"}]})
        ))
        .is_err()
    );

    // Turning one boss off leaves the other's block alone.
    let document = json!({"preset": "off", "custom": "我的规则", "apps": {"codex": "custom"}});
    assert!(sync(&apps, &apps, h, &document, &[], &backup, false).is_empty());
    assert!(!h.join(".claude/CLAUDE.md").exists());
    assert!(h.join(".codex/AGENTS.md").exists());

    // Claude joining Grok's file later does not undo the mode already set on Grok.
    let document = json!({"preset": "off", "apps": {"grok": "split"}});
    assert!(sync(&apps, &apps, h, &document, &[], &backup, false).is_empty());
    let claude = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude.contains(shared));
}

#[test]
fn a_held_sync_keeps_the_old_block_until_the_service_catches_up() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let h = home.as_path();
    let app = |key: &'static str, root: &str| App {
        key,
        id: key,
        label: key,
        root: h.join(root),
    };
    let apps = [app("claude", ".claude"), app("codex", ".codex")];
    let backup = |_: &Path, _: &[u8]| Ok(());
    let split = json!({"preset": "split"});
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    let old = format!("{}\n", block("旧"));
    std::fs::write(h.join(".claude/CLAUDE.md"), &old).unwrap();
    std::fs::write(h.join(".codex/AGENTS.md"), &old).unwrap();

    let failures = sync(&apps, &apps, h, &split, &[], &backup, true);
    assert_eq!(failures.len(), 2);
    assert_eq!(
        std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap(),
        old
    );

    // Turning the block off does not wait: no tool argument depends on its absence.
    let off = json!({"preset": "off"});
    assert!(sync(&apps, &apps, h, &off, &[], &backup, true).is_empty());
    assert!(!h.join(".claude/CLAUDE.md").exists());

    // Once the service caught up the block goes in, and holding again reports nothing.
    assert!(sync(&apps, &apps, h, &split, &[], &backup, false).is_empty());
    assert!(sync(&apps, &apps, h, &split, &[], &backup, true).is_empty());
}

#[test]
fn role_switches_remove_generated_seats_without_losing_bindings_or_custom_text() {
    let mut document = json!({"preset":"split", "enabled_roles":["worker_writing"],
        "worker_writing":{"easy":{"provider":"claude","model":"haiku"}},
        "worker":{"hard":{"provider":"codex","model":"gpt"}}});
    let generated = text_of(&document, "split").unwrap();
    assert!(generated.contains("worker-writing-easy"));
    assert!(generated.contains("worker-writing-hard"));
    assert!(!generated.contains("`worker-hard`"));
    assert!(!generated.contains("worker-frontend"));
    assert!(!generated.contains("explorer-web"));
    assert!(generated.contains("同难度专项优先"));
    document["custom"] = json!(format!(
        "{generated}\n- 本偏好不决定何时评审，用户要求或其规则要求时才评审。评审时有 agentnave-review Skill 就按它定级、选工位，不另派 Explorer；没有时派 Explorer，不交给写这段代码的 Worker，尽量换模型。\n用户自写要求：保留引用来源。"
    ));
    document["enabled_roles"] = json!([]);
    let custom = text_of(&document, "custom").unwrap();
    assert!(!custom.contains("worker-writing"));
    assert!(!custom.contains("Explorer"));
    assert!(!custom.contains("Worker"));
    assert!(custom.contains("不可用时由 boss 自行评审"));
    assert!(custom.contains("用户自写要求：保留引用来源。"));
    assert_eq!(document["worker_writing"]["easy"]["model"], "haiku");
    document["image_generation"] = json!({"provider":"codex","model":"chat"});
    assert!(
        text_of(&document, "custom")
            .unwrap()
            .contains("image-generator")
    );
}

#[test]
fn image_rules_follow_the_optional_seat_and_clear_copied_custom_text() {
    let mut document = json!({"preset": "split"});
    for mode in ["split", "parallel"] {
        assert!(
            !text_of(&document, mode)
                .unwrap()
                .contains("image-generator")
        );
    }
    document["image_generation"] = json!({"provider": "codex", "model": "chat"});
    let generated = text_of(&document, "split").unwrap();
    assert_eq!(generated.matches("image-generator").count(), 1);
    document["custom"] = json!(generated);
    assert_eq!(
        text_of(&document, "custom")
            .unwrap()
            .matches("image-generator")
            .count(),
        1
    );
    document["image_generation"] = Value::Null;
    assert!(
        !text_of(&document, "custom")
            .unwrap()
            .contains("image-generator")
    );
    assert!(
        !presets(&document)["split"]
            .as_str()
            .unwrap()
            .contains("image-generator")
    );
    assert!(text_of(&document, "off").is_none());
}

#[test]
fn image_rule_is_hidden_from_its_own_cli_and_shared_readers() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let app = |id, root: &str| App {
        key: id,
        id,
        label: id,
        root: h.join(root),
    };
    let apps = [
        app("codex", ".codex"),
        app("claude", ".claude"),
        app("grok", ".grok"),
        app("workbuddy", ".workbuddy"),
    ];
    let mut document =
        json!({"preset": "split", "image_generation": {"provider": "codex", "model": "chat"}});
    let rows = wanted(&apps, h, &document);
    for (path, text) in &rows {
        assert_eq!(
            text.contains("image-generator"),
            !path.starts_with(h.join(".codex"))
        );
    }
    document["image_generation"]["provider"] = json!("claude");
    let rows = wanted(&apps, h, &document);
    assert!(
        rows.iter()
            .find(|(p, _)| p.starts_with(h.join(".claude")))
            .is_some_and(|(_, text)| !text.contains("image-generator"))
    );
    assert!(
        rows.iter()
            .find(|(p, _)| p.starts_with(h.join(".codex")))
            .unwrap()
            .1
            .contains("image-generator")
    );
    document["image_generation"]["provider"] = json!("codebuddy");
    assert!(
        wanted(&apps, h, &document)
            .iter()
            .find(|(p, _)| p.starts_with(h.join(".workbuddy")))
            .is_some_and(|(_, text)| !text.contains("image-generator"))
    );
}

#[test]
fn custom_copies_learn_that_no_worktree_means_serial_writes() {
    let shared = catalog::section("shared");
    let parallel = catalog::section("parallel_writes");
    let old = "前提是各单元互不依赖、各用独立 worktree，做不到就串行。\n计划确认后，互不依赖的写入步骤并行派给多个可写工位，boss 合入当前分支。";
    let migrated = roles::migrate_legacy(old);
    assert!(migrated.contains("项目规定不建 worktree（如要求直接在 main 修改）时也算做不到"));
    assert!(migrated.contains("各用独立的隔离目录，做不到就串行；项目规定不建 worktree"));
    assert!(migrated.contains("各用独立的隔离目录并行派给多个可写工位"));
    assert!(!migrated.contains("独立 worktree"));
    let user_text = "我自己写：各用独立 worktree 并行。";
    assert_eq!(roles::migrate_legacy(user_text), user_text);
    assert_eq!(roles::migrate_legacy(&migrated), migrated);
    assert_eq!(roles::migrate_legacy(shared), shared);
    assert_eq!(roles::migrate_legacy(parallel), parallel);

    let rules = catalog::catalog();
    assert_eq!(
        rules
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "enabled_roles",
            "meeting_defaults",
            "order",
            "sections",
            "switches"
        ]
    );
    assert_eq!(
        rules["order"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["meetings", "parallel", "split"]
    );
    let lead = &rules["switches"]["lead_only"];
    assert!(
        catalog::section(lead["replaces_in"].as_str().unwrap())
            .contains(lead["when_false"].as_str().unwrap())
    );
    // The settings page keeps its own copy in `PROVIDER_NAMES`; the two must not drift.
    let page = include_str!("../../ui/settings-state.js");
    let (_, rest) = page.split_once("const PROVIDER_NAMES = {").expect("table");
    let (body, _) = rest.split_once("};").expect("end of table");
    let shown: Vec<(&str, &str)> = body
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').split_once(':'))
        .map(|(id, name)| (id.trim(), name.trim().trim_matches('"')))
        .collect();
    assert_eq!(shown.len(), crate::providers::PROVIDERS.len());
    for (provider, name) in shown {
        assert_eq!(
            rules["meeting_defaults"]["provider_names"][provider].as_str(),
            Some(name)
        );
    }
}
