#[test]
fn custom_titles_are_not_confused_with_provider_defaults() {
    assert!(super::meaningful_title("review 任务"));
    assert!(!super::meaningful_title("codex 任务"));
    assert!(!super::meaningful_title(" "));
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

use super::{Desk, desk_label, desk_role, handle, model_with_effort};

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
fn a_desk_role_keeps_the_callers_responsibility() {
    let role = |role: &str| desk_role(&serde_json::json!({"role": role}));
    assert_eq!(role("worker-hard").as_deref(), Some("worker-hard"));
    assert_eq!(
        role("correctness-reviewer").as_deref(),
        Some("correctness-reviewer")
    );
    assert_eq!(role("接手方视角").as_deref(), Some("接手方视角"));
}

#[test]
fn a_desk_shows_the_reported_model_until_a_newer_call_asks_for_another() {
    let mut desk = Desk::default();
    desk.label(1, "opus", None, None);
    assert_eq!(desk.display_model(), "opus");
    desk.reported(2, &serde_json::json!({"reported_model": "claude-opus-5-5"}));
    assert_eq!(desk.display_model(), "claude-opus-5-5");
    desk.label(3, "sonnet", None, None);
    assert_eq!(desk.display_model(), "sonnet");
}

#[test]
fn handles_shorten_only_the_model_middle_by_unicode_characters() {
    assert_eq!(
        handle(
            "backend/abcdefghijklmno尾部模型",
            None,
            "pi",
            Some("review")
        ),
        "@backend/ab…klmno尾部模型·Pi·review"
    );
    assert_eq!(
        handle(
            "backend/abcdefghijklmno尾部模型",
            Some("high"),
            "pi",
            Some("review")
        ),
        "@backend/ab…klmno尾部模型 · high·Pi·review"
    );
    assert_eq!(
        desk_label(&model_with_effort("", Some("high")), "codex", None),
        "CLI 默认模型 · high · Codex"
    );
}
