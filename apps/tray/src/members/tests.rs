use super::*;
use crate::data::Reader;

fn document() -> Value {
    json!({
        "version": 1,
        "clis": {"codex": {"models": {
            "gpt-6.1-sol": {
                "efforts": ["medium", "high", "xhigh"],
                "effort": "high",
                "options": {"skip_git_repo_check": true, "budget": 2.5},
                "notes": "多文件实现"
            },
            "gpt-6-luna": {}
        }}},
        "modes": {
            "task": {"codex": {
                "models": ["gpt-6.1-sol"],
                "default": "gpt-6.1-sol",
                "overrides": {"gpt-6.1-sol": {"effort": "medium"}}
            }},
            "discussion": {}
        }
    })
}

#[test]
fn accepts_the_contract_example_and_rejects_structural_or_reference_errors() {
    validate(&document()).unwrap();
    validate(&empty()).unwrap();
    let mut deepseek = document();
    deepseek["clis"]["deepseek"] = json!({"models": {"deepseek-flash": {"effort": "max"}}});
    deepseek["modes"]["task"]["deepseek"] = json!({"models": ["deepseek-flash"]});
    validate(&deepseek).unwrap();
    // Review seats live in review.json and may name models no longer configured.
    let mut members_with_review = document();
    members_with_review["review"] = json!({});
    assert!(validate(&members_with_review).is_err());
    let mut review = json!({"version": 1, "seats": {
        "correctness-reviewer": {"provider": "deepseek", "model": "gone", "effort": "max"},
        "security-reviewer": {"provider": "codex", "model": "gpt-6.1-sol"}
    }});
    validate_review(&review).unwrap();
    for seat in [
        json!({"other-reviewer": {"provider": "codex", "model": "m"}}),
        json!({"security-reviewer": {"provider": "other", "model": "m"}}),
        json!({"security-reviewer": {"provider": "codex"}}),
        json!({"security-reviewer": {"provider": "codex", "model": " "}}),
        json!({"security-reviewer": {"provider": "codex", "model": "m", "effort": 1}}),
        json!({"security-reviewer": {"provider": "codex", "model": "m", "extra": 1}}),
    ] {
        review["seats"] = seat.clone();
        assert!(validate_review(&review).is_err(), "{seat}");
    }
    for key in ["version", "clis", "modes"] {
        let mut missing = empty();
        missing.as_object_mut().unwrap().remove(key);
        assert!(validate(&missing).is_err(), "{key}");
    }
    for pointer in ["/clis/codex", "/modes/task/codex"] {
        let mut missing = document();
        let entry = missing.pointer_mut(pointer).unwrap();
        entry.as_object_mut().unwrap().remove("models");
        assert!(validate(&missing).is_err(), "{pointer}");
    }
    let cases: Vec<(&str, Value)> = vec![
        ("/extra", json!(true)),
        ("/version", json!(2)),
        ("/clis/other", json!({"models": {}})),
        ("/clis/codex/extra", json!({})),
        ("/clis/codex/models/gpt-6-luna/unknown", json!(1)),
        (
            "/clis/codex/models/gpt-6-luna/options",
            json!({"model": "x"}),
        ),
        ("/clis/codex/models/gpt-6-luna/options", json!({"x": []})),
        ("/clis/codex/models/gpt-6.1-sol/effort", json!("low")),
        ("/clis/codex/models/gpt-6.1-sol/efforts", json!(["high", 1])),
        ("/modes/review", json!({})),
        ("/modes/discussion/grok", json!({"models": ["grok-4.7"]})),
        (
            "/modes/task/codex/models",
            json!(["gpt-6.1-sol", "missing"]),
        ),
        ("/modes/task/codex/default", json!("gpt-6-luna")),
        (
            "/modes/task/codex/overrides",
            json!({"gpt-6.1-sol": {"effort": "low"}}),
        ),
        ("/modes/task/codex/overrides", json!({"missing": {}})),
        ("/clis/codex/models/ ", json!({})),
        ("/modes/task/codex/extra", json!(1)),
    ];
    for (pointer, value) in cases {
        let mut bad = document();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        bad.pointer_mut(parent).unwrap()[key] = value;
        assert!(validate(&bad).is_err(), "{pointer}");
    }
    // An override may replace the allowed efforts so its default fits again.
    let mut widened = document();
    widened["modes"]["task"]["codex"]["overrides"]["gpt-6.1-sol"] =
        json!({"efforts": ["low"], "effort": "low"});
    validate(&widened).unwrap();
    // An override only needs a configured model, as on the server; duplicates are tolerated.
    let mut lenient = document();
    lenient["modes"]["task"]["codex"]["overrides"]["gpt-6-luna"] = json!({"effort": ""});
    lenient["modes"]["task"]["codex"]["models"] = json!(["gpt-6.1-sol", "gpt-6.1-sol"]);
    validate(&lenient).unwrap();
    // With no listed models, the default only has to be configured.
    let mut open = document();
    open["modes"]["task"]["codex"] = json!({"models": [], "default": "gpt-6-luna"});
    validate(&open).unwrap();
    // A CLI enabled with any model needs no shared model configuration.
    open["modes"]["discussion"]["grok"] = json!({"models": []});
    validate(&open).unwrap();
    // An unrestricted mode keeps its default models under `defaults`.
    let mut kept = document();
    kept["modes"].as_object_mut().unwrap().remove("discussion");
    kept["defaults"] = json!({"discussion": {"codex": "gpt-6-luna"}});
    validate(&kept).unwrap();
    for (mode, model) in [("discussion", "missing"), ("task", "gpt-6-luna")] {
        let mut bad = kept.clone();
        bad["defaults"] = json!({mode: {"codex": model}});
        assert!(validate(&bad).is_err(), "{mode}");
    }
}

#[test]
fn saves_atomically_through_the_post_route_only() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let reader = Reader::new(root.clone());
    let body = serde_json::to_vec(&document()).unwrap();
    assert_eq!(
        reader
            .request("GET", "/app/members", Some("1"), &body)
            .status,
        405
    );
    assert_eq!(
        reader.request("POST", "/app/members", None, &body).status,
        403
    );
    assert_eq!(
        reader
            .request("POST", "/app/members", Some("1"), b"{")
            .status,
        400
    );
    let mut bad = document();
    bad["modes"]["task"]["codex"]["default"] = json!("missing");
    let response = reader.request(
        "POST",
        "/app/members",
        Some("1"),
        &serde_json::to_vec(&bad).unwrap(),
    );
    assert_eq!(response.status, 400);
    assert!(!root.exists());
    assert_eq!(
        reader
            .request("POST", "/app/members", Some("1"), &body)
            .status,
        200
    );
    assert_eq!(read(&root).unwrap(), Some(document()));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    // Rename cannot replace a directory; the old destination and no temporary file remain.
    fs::remove_file(root.join(FILE)).unwrap();
    fs::create_dir(root.join(FILE)).unwrap();
    assert!(save(&root, &document()).is_err());
    assert!(root.join(FILE).is_dir());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn review_seats_save_to_their_own_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let reader = Reader::new(root.clone());
    assert_eq!(read_review(&root).unwrap()["seats"], json!({}));
    let seats = json!({"version": 1, "seats": {
        "correctness-reviewer": {"provider": "codex", "model": "gpt-6.1-sol"}
    }});
    let post = |body: &Value| {
        reader
            .request(
                "POST",
                "/app/review",
                Some("1"),
                &serde_json::to_vec(body).unwrap(),
            )
            .status
    };
    assert_eq!(post(&json!({"version": 1, "seats": {"x": {}}})), 400);
    assert_eq!(post(&seats), 200);
    assert_eq!(read_review(&root).unwrap(), seats);
    assert!(!root.join(FILE).exists());
    fs::write(root.join(REVIEW_FILE), b"{").unwrap();
    assert!(read_review(&root).is_err());
}

#[test]
fn permission_tiers_save_to_their_own_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let reader = Reader::new(root.clone());
    assert_eq!(read_permissions(&root).unwrap(), None);
    let post = |body: &Value| {
        reader
            .request(
                "POST",
                "/app/permissions",
                Some("1"),
                &serde_json::to_vec(body).unwrap(),
            )
            .status
    };
    for clis in [
        json!({"pi": "auto"}),
        json!({"other": "auto"}),
        json!({"codex": "yolo"}),
        json!({"codex": "native"}),
        json!({"codex": true}),
    ] {
        assert_eq!(post(&json!({"version": 1, "clis": clis})), 400);
    }
    let tiers = json!({"version": 1, "clis": {"codex": "auto", "claude": "full"}});
    assert_eq!(post(&tiers), 200);
    assert_eq!(read_permissions(&root).unwrap(), Some(tiers));
    assert!(!root.join(FILE).exists());
    fs::write(root.join(PERMISSIONS_FILE), b"{").unwrap();
    assert!(read_permissions(&root).is_err());
}

#[test]
fn cli_options_save_to_their_own_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let reader = Reader::new(root.clone());
    assert_eq!(read_cli_options(&root).unwrap(), None);
    let post = |body: &Value| {
        reader
            .request(
                "POST",
                "/app/cli-options",
                Some("1"),
                &serde_json::to_vec(body).unwrap(),
            )
            .status
    };
    for clis in [
        json!({"other": {}}),
        json!({"codex": true}),
        json!({"codex": {"model": "m"}}),
        json!({"codex": {"sandbox": "read-only"}}),
        json!({"codex": {"skip_git_repo_check": [true]}}),
    ] {
        assert_eq!(post(&json!({"version": 1, "clis": clis})), 400);
    }
    let options = json!({"version": 1, "clis": {"codex": {"skip_git_repo_check": true}}});
    assert_eq!(post(&options), 200);
    assert_eq!(read_cli_options(&root).unwrap(), Some(options));
    assert!(!root.join(FILE).exists());
    fs::write(root.join(CLI_OPTIONS_FILE), b"{").unwrap();
    assert!(read_cli_options(&root).is_err());
}

#[test]
fn broken_file_is_reported_and_left_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert_eq!(document_state(root), (empty(), Value::Null));
    fs::write(root.join(FILE), b"{").unwrap();
    let (document, error) = document_state(root);
    assert_eq!(document, empty());
    assert!(error.as_str().unwrap().contains("损坏"));
    assert_eq!(fs::read(root.join(FILE)).unwrap(), b"{");
}

#[test]
fn a_missing_document_stays_missing_so_every_cli_keeps_its_own_default() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert_eq!(document_state(root), (empty(), Value::Null));
    assert!(!root.join(FILE).exists());
}
