#[test]
fn service_member_identity_contract_is_preserved() {
    use serde_json::{Value, json};

    let fixture: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/member-names.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        for identity in case["identities"].as_array().unwrap() {
            let record = json!({
                "provider": identity["provider"],
                "model": identity["model"],
                "identity": identity,
            });
            let speaker = super::member_speaker(&record);
            assert_eq!(speaker["identity"], *identity, "{}", case["name"]);
            let target = super::recipient(identity["key"].as_str().unwrap(), &record);
            assert_eq!(target["identity"], *identity, "{}", case["name"]);
            assert_eq!(target["key"], identity["key"]);
            assert_eq!(target["handle"], identity["handle"]);
        }
    }
}

#[test]
fn project_name_skips_agent_worktrees() {
    assert_eq!(
        super::project_name("/Users/a/Code/app/").as_deref(),
        Some("app")
    );
    assert_eq!(
        super::project_name("/Users/a/Code/app/.claude/worktrees/fix-1").as_deref(),
        Some("app")
    );
    assert_eq!(
        super::project_name("/Users/a/.codex/worktrees/ab12/app").as_deref(),
        Some("app")
    );
    assert_eq!(super::project_name("/"), None);
}

use super::Desk;

#[test]
fn a_failed_desk_settles_after_half_an_hour_unless_it_runs_again() {
    let mut desk = Desk::default();
    let minutes = |n: i64| n * 60 * 1_000_000;
    desk.observe(0, "failed");
    assert!(!desk.failure_settled(minutes(30)));
    assert!(desk.failure_settled(minutes(31)));
    desk.observe(minutes(32), "succeeded");
    assert!(!desk.failure_settled(minutes(90)));
}

#[test]
fn a_desk_shows_the_reported_model_until_a_newer_call_asks_for_another() {
    let mut desk = Desk::default();
    desk.label(1, "opus", None, None);
    assert_eq!(desk.art_model(), "opus");
    desk.reported(2, &serde_json::json!({"reported_model": "claude-opus-5-5"}));
    assert_eq!(desk.art_model(), "claude-opus-5-5");
    desk.label(3, "sonnet", None, None);
    assert_eq!(desk.art_model(), "sonnet");
}
