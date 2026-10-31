use super::*;

#[test]
fn reads_the_version_from_each_clis_own_format() {
    assert_eq!(
        version_of("codex-cli 0.161.0\n").as_deref(),
        Some("0.161.0")
    );
    assert_eq!(
        version_of("2.1.308 (Claude Code)").as_deref(),
        Some("2.1.308")
    );
    assert_eq!(
        version_of("grok 1.0.46 (2765805b9442)").as_deref(),
        Some("1.0.46")
    );
    assert_eq!(version_of("v1.4.7").as_deref(), Some("1.4.7"));
    assert_eq!(version_of("usage: tool [options]"), None);
    assert_eq!(
        version_of("\nmytool 3.2.0\nNode v22.14.0").as_deref(),
        Some("3.2.0")
    );
}

fn ids(models: &[Value]) -> Vec<&str> {
    models.iter().map(|m| m["id"].as_str().unwrap()).collect()
}

#[test]
fn codex_cache_lists_visible_models_by_priority_with_their_efforts() {
    let cache = json!({"models": [
        {"slug": "gpt-6-luna", "display_name": "GPT-6-Luna", "visibility": "list", "priority": 4,
         "default_reasoning_level": "medium",
         "supported_reasoning_levels": [{"effort": "medium"}, {"effort": "max"}]},
        {"slug": "hidden", "visibility": "hide", "priority": 1,
         "supported_reasoning_levels": [{"effort": "secret"}]},
        {"id": "legacy", "priority": 2, "supported_reasoning_levels": ["low", "ultra"]},
        {"slug": "gpt-6.1-sol", "display_name": "GPT-6.1-Sol", "visibility": "list", "priority": 1}
    ]});
    let (models, efforts) = parse_codex_cache(&cache.to_string()).unwrap();
    assert_eq!(ids(&models), ["gpt-6.1-sol", "legacy", "gpt-6-luna"]);
    assert_eq!(models[0], model("gpt-6.1-sol", "GPT-6.1-Sol", false));
    assert_eq!(models[1]["efforts"], json!(["low", "ultra"]));
    assert_eq!(models[2]["efforts"], json!(["medium", "max"]));
    assert_eq!(models[2]["effort"], "medium");
    assert_eq!(efforts, ["low", "medium", "max", "ultra"]);
    assert!(parse_codex_cache("{").is_err());
    assert!(parse_codex_cache("{}").is_err());
}

#[test]
fn each_model_runs_at_the_clis_setting_where_offered_else_its_own_level() {
    let levels = ["low".to_owned(), "high".to_owned()];
    let mut models = vec![
        json!({"id": "a", "efforts": ["low", "medium"], "effort": "low"}),
        json!({"id": "b", "effort": "high"}),
        json!({"id": "c", "efforts": ["max"], "effort": "high"}),
    ];
    model_efforts("codex", &mut models, &levels, |_| Some("medium".to_owned()));
    assert_eq!(models[0]["effort"], "medium");
    assert_eq!(models[1]["effort"], "high");
    assert!(models[2].get("effort").is_none());
    let mut pi = vec![
        json!({"id": "p/m", "efforts": ["off", "medium", "high"]}),
        json!({"id": "p/x", "efforts": ["off", "low", "high"]}),
        json!({"id": "p/y", "efforts": ["off", "low"]}),
    ];
    model_efforts("pi", &mut pi, &[], |id| {
        (id != "p/m").then(|| "medium".to_owned())
    });
    assert_eq!(pi[0]["effort"], "medium");
    assert_eq!(pi[1]["effort"], "high");
    assert_eq!(pi[2]["effort"], "low");
}

#[test]
fn dsh_dump_names_the_composed_default_model() {
    let dump = "# == base\n- id: agent-default-model\n  config:\n    provider: deepseek-official\n\
            \x20   model: deepseek-flash\n# == user\n- id: agent-default-model\n  config:\n\
            \x20   provider: magpie\n    model: 'gw/v4''s'\n- id: jobs\n  config:\n    model: other\n";
    assert_eq!(dsh_default(dump, "model").as_deref(), Some("gw/v4's"));
    assert_eq!(dsh_default(dump, "reasoningEffort"), None);
    assert_eq!(dsh_default("- id: jobs\n", "model"), None);
    assert_eq!(
        dsh_default(
            "- id: agent-default-model\n  config:\n    model: !!js String('deepseek-flash')\n",
            "model"
        ),
        None
    );
    assert_eq!(dsh_scalar("*default-model"), None);
    assert_eq!(
        dsh_scalar("'!!js literal'").as_deref(),
        Some("!!js literal")
    );
    let providers = "- id: llm-pi-ai\n  config:\n    providers:\n      gw:\n        models:\n\
            \x20         - id: opencode-go/v4.1\n            reasoningEfforts:\n\
            \x20             low: low\n              max: max\n            input:\n\
            \x20             - text\n\
            \x20         - id: \"codex/sol\"\n      gw2:\n        models:\n\
            \x20         - id: codex/sol\n          - id: opencode-go/v4.1\n            reasoningEfforts:\n\
            \x20             high: high\n          - id: grok/4\n    other:\n\
            \x20         - id: skipped\n- id: jobs\n";
    let declared = dsh_provider_models(providers);
    let ids: Vec<&str> = declared.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["opencode-go/v4.1", "codex/sol", "grok/4"]);
    assert_eq!(declared[0].1, ["low", "max"]);
    assert!(declared[1].1.is_empty());
}

#[test]
fn command_listings_are_parsed_from_their_fixed_formats() {
    let grok = "You are logged in with grok.com.\n\nDefault model: grok-4.7\n\n\
            Available models:\n  * grok-4.7 (default)\n  - grok-4.7-build-fast\n  - grok-4.6\n\nTip\n";
    let models = grok_models(grok);
    assert_eq!(
        ids(&models),
        ["grok-4.7", "grok-4.7-build-fast", "grok-4.6"]
    );
    assert_eq!(models[0]["default"], true);
    assert_eq!(models[1]["default"], false);

    let agy = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
            claude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n";
    let models = agy_models(agy);
    assert_eq!(
        ids(&models),
        ["gemini-3.8-flash-high", "claude-opus-4-6-thinking"]
    );
    assert_eq!(models[0]["label"], "Gemini 3.8 Flash (High)");

    let pi = "provider     model                 context  max-out  thinking  images\n\
            magpie       codex/gpt-5.6-luna    272K     128K     yes       yes\n\
            opencode-go  glm-5.3               1M       131.1K   yes       no\n";
    assert_eq!(
        ids(&pi_models(pi)),
        ["magpie/codex/gpt-5.6-luna", "opencode-go/glm-5.3"]
    );
    assert!(pi_models("No models available\n").is_empty());

    let codebuddy = "  --model <model>   Model for the current session. Please provide the \
            model ID. Currently supported: (hy4-preview, glm-5.3, kimi-k2.7)\n  --effort <level>\n";
    assert_eq!(
        ids(&codebuddy_models(codebuddy)),
        ["hy4-preview", "glm-5.3", "kimi-k2.7"]
    );
    assert!(codebuddy_models("Usage: codebuddy").is_empty());
    let wrapped = "  --model <model>   Please provide the model ID. Currently\n      \
            supported: (hy4-preview,\n      glm-5.3)\n";
    assert_eq!(ids(&codebuddy_models(wrapped)), ["hy4-preview", "glm-5.3"]);

    // The CLI's own default: what its settings name (by id or label), else what its listing
    // marks; never a guess from the list.
    let listed =
        |ids: &[&str]| -> Vec<Value> { ids.iter().map(|id| model(id, id, false)).collect() };
    let codex = listed(&["opencode-go/glm-5.3-flash", "gpt-6.1-sol", "gpt-6-luna"]);
    assert_eq!(native_model(&codex, None), None);
    assert_eq!(
        native_model(&codex, Some("gpt-6-luna")).as_deref(),
        Some("gpt-6-luna")
    );
    assert_eq!(native_model(&listed(&CLAUDE_ALIASES), None), None);
    assert_eq!(
        native_model(&grok_models(grok), None).as_deref(),
        Some("grok-4.7")
    );
    assert_eq!(
        native_model(&codebuddy_models(codebuddy), None).as_deref(),
        Some("hy4-preview")
    );
    let cache = json!({"models": {"grok-4.7": {"info": {"reasoning_efforts": [
            {"value": "xhigh", "default": false}, {"value": "high", "default": true}]}}}});
    assert_eq!(
        grok_default_effort(&cache, "grok-4.7").as_deref(),
        Some("high")
    );
    assert_eq!(grok_default_effort(&cache, "grok-4.6"), None);
    assert_eq!(grok_efforts(&cache, "grok-4.7").unwrap(), ["xhigh", "high"]);
    assert_eq!(grok_efforts(&cache, "grok-4.6"), None);

    // Claude's own picker and Pi's levels, as the CLIs answer over stdin.
    let initialize = json!({"type": "control_response", "response": {"response": {"models": [
            {"value": "default", "displayName": "Default", "supportedEffortLevels": ["low"]},
            {"value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5",
             "supportedEffortLevels": ["low", "max"]},
            {"value": "claude-haiku-4-5", "displayName": "Haiku 4.5"}]}}});
    let picker = claude_picker(&initialize);
    assert_eq!(ids(&picker), ["opus", "claude-haiku-4-5"]);
    assert_eq!(picker[0]["efforts"], json!(["low", "max"]));
    assert_eq!(picker[1]["efforts"], json!([]));
    assert_eq!(picker[0]["resolved"], "claude-opus-5-5");
    assert!(picker[1]["resolved"].is_null());
    assert_eq!(
        claude_name("claude-haiku-4-5-20251001").as_deref(),
        Some("claude-haiku-4-5")
    );
    assert_eq!(
        claude_name("anthropic/claude-opus-4.6").as_deref(),
        Some("claude-opus-4-6")
    );
    assert_eq!(claude_name("opus"), None);
    assert_eq!(
        claude_name("claude-opus-5-5[1m]").as_deref(),
        Some("claude-opus-5-5")
    );
    assert!(!claude_reads_top("opus"));
    assert!(claude_reads_top("claude-opus-5") && claude_reads_top("glm-5.3"));
    assert!(!claude_reads_top("claude-opus-5-5") && !claude_reads_top("claude-sonnet-6"));
    assert_eq!(pi_levels(&json!({"reasoning": false})), ["off"]);
    assert_eq!(
        pi_levels(&json!({"reasoning": true})),
        ["off", "minimal", "low", "medium", "high"]
    );
    assert_eq!(
        pi_levels(&json!({"reasoning": true, "thinkingLevelMap":
                {"off": null, "minimal": null, "medium": null, "max": "max"}})),
        ["low", "high", "max"]
    );
    let agy = agy_models(agy);
    assert_eq!(native_model(&agy, None), None);
    let label = agy[1]["label"].as_str().unwrap();
    assert_eq!(
        native_model(&agy, Some(label)),
        agy[1]["id"].as_str().map(str::to_owned)
    );
    assert_eq!(
        native_model(&[], Some("unlisted")).as_deref(),
        Some("unlisted")
    );
}

#[test]
fn published_defaults_keep_a_failed_probe_and_drop_what_is_gone() {
    let previous = json!({"version": 1, "clis": {
            "grok": {"model": "grok-4.6", "effort": "high"},
            "pi": {"model": "old/pi", "effort": null}}});
    let all = json!({
            "codex": {"installed": true, "error": null,
                      "native": {"model": "gpt-6.1-sol", "effort": "medium"},
                      "models": [{"id": "gpt-6.1-sol", "effort": "medium"}, {"id": "gpt-x"}]},
            "claude": {"installed": true, "error": null, "native": {"model": null, "effort": null}},
            "codebuddy": {"installed": true, "error": null, "native": {"model": null, "effort": null},
                          "models": [{"id": "glm-5.3", "effort": "high"}]},
            "grok": {"installed": true, "error": "超时", "native": {"model": null, "effort": null}},
            "pi": {"installed": false, "error": null, "native": {"model": null, "effort": null}}});
    assert_eq!(
        native_document(&all, &previous),
        json!({"version": 1, "clis": {
                "codex": {"model": "gpt-6.1-sol", "effort": "medium",
                          "efforts": {"gpt-6.1-sol": "medium"}},
                "codebuddy": {"model": null, "effort": null, "efforts": {"glm-5.3": "high"}},
                "grok": {"model": "grok-4.6", "effort": "high"}}})
    );
    let root = tempfile::tempdir().unwrap();
    publish_native(root.path(), &all);
    let written = std::fs::read_to_string(root.path().join(NATIVE_FILE)).unwrap();
    let modified = std::fs::metadata(root.path().join(NATIVE_FILE))
        .unwrap()
        .modified()
        .unwrap();
    publish_native(root.path(), &all);
    assert_eq!(
        std::fs::read_to_string(root.path().join(NATIVE_FILE)).unwrap(),
        written
    );
    assert_eq!(
        std::fs::metadata(root.path().join(NATIVE_FILE))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    publish_native(&root.path().join("missing"), &all);
    assert!(!root.path().join("missing").exists());
}

#[cfg(unix)]
#[test]
fn listing_commands_get_empty_stdin_a_completed_path_and_a_failure_reason() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("cli");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = fail ]; then echo boom >&2; exit 3; fi\n\
             read line || echo empty-stdin\necho \"$PATH\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = run(&script, &[]).unwrap();
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("empty-stdin"));
    // The CLI's own directory, then the inherited PATH ahead of the common directories.
    let path: Vec<String> = lines
        .next()
        .unwrap()
        .split(':')
        .map(str::to_owned)
        .collect();
    assert_eq!(path[0], directory.path().display().to_string());
    let inherited = std::env::var("PATH").unwrap_or_default();
    if let Some(first) = inherited
        .split(':')
        .find(|dir| *dir != path[0] && !dir.is_empty())
    {
        assert_eq!(path[1], first);
    }
    assert_eq!(run(&script, &["fail"]).unwrap_err(), "boom");

    // A failed DeepSeek dump still offers the catalog, but never invents a native
    // default or overwrites the last successfully detected model's settings.
    let dsh = directory.path().join("dsh");
    std::fs::write(&dsh, "#!/bin/sh\necho unavailable >&2\nexit 3\n").unwrap();
    std::fs::set_permissions(&dsh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let failed = probe_path("deepseek", &dsh);
    assert_eq!(failed["error"], "unavailable");
    assert!(failed["native"]["model"].is_null());
    assert_eq!(ids(failed["models"].as_array().unwrap()), DEEPSEEK_CATALOG);
    let previous = json!({"clis": {"deepseek": {"model": "gateway/pro"}}});
    let all = json!({"deepseek": failed});
    assert_eq!(
        native_document(&all, &previous)["clis"]["deepseek"]["model"],
        "gateway/pro"
    );

    std::fs::write(&dsh, "#!/bin/sh\necho '# no default model'\n").unwrap();
    let unknown = probe_path("deepseek", &dsh);
    assert!(unknown["error"].is_null());
    assert!(unknown["native"]["model"].is_null());

    // A grandchild keeping the pipe open after the CLI exited is stopped with its group.
    let lingering = directory.path().join("lingering");
    std::fs::write(
        &lingering,
        "#!/bin/sh
echo listed
sleep 30 &
",
    )
    .unwrap();
    std::fs::set_permissions(&lingering, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = Instant::now();
    assert_eq!(run(&lingering, &[]).unwrap(), "listed\n");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(windows)]
#[test]
fn the_uninstall_entry_locates_the_desktop_app() {
    // Not every machine has it installed; when it is, the registry must know where.
    let Some(dir) = install_dir("DeepSeek Harness") else {
        return;
    };
    assert!(dir.is_dir(), "{}", dir.display());
    assert!(
        dir.join("DeepSeek Harness.exe").is_file(),
        "{}",
        dir.display()
    );
}

#[cfg(windows)]
#[test]
fn the_bundled_cli_is_found_through_that_directory() {
    if install_dir("DeepSeek Harness").is_none() {
        return;
    }
    let path = desktop_binary("dsh").expect("bundled dsh");
    assert!(path.is_file(), "{}", path.display());
    assert!(path.to_string_lossy().ends_with("dsh.cmd"));
}

#[cfg(windows)]
#[test]
fn the_codex_desktop_apps_cli_is_found() {
    // Skip when the desktop app is not installed; when it is, both its stable plugin copy and its
    // content-hashed copies must resolve to a real file.
    let Some(path) = codex_bundled() else {
        return;
    };
    assert!(path.is_file(), "{}", path.display());
    assert!(path.to_string_lossy().ends_with("codex.exe"));
}
