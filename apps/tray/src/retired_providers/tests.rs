use serde_json::json;

#[test]
fn files_naming_a_retired_cli_read_without_it() {
    let (claude, devin) = (
        json!({"provider": "claude", "model": "opus"}),
        json!({"provider": "devin", "model": "swe"}),
    );
    let mut members = json!({
        "clis": {"devin": {}, "codex": {}},
        "modes": {"task": {"devin": {}, "codex": {}}},
        "defaults": {"discussion": {"devin": "swe"}},
    });
    super::members(&mut members);
    assert_eq!(
        members,
        json!({"clis": {"codex": {}}, "modes": {"task": {"codex": {}}}, "defaults": {"discussion": {}}})
    );

    let mut review = json!({"seats": {
        "security-reviewer": {"provider": "devin", "model": "swe", "fallbacks": [claude]},
        "performance-reviewer": {"provider": "devin", "model": "swe", "fallbacks": [devin]},
    }});
    super::review(&mut review);
    assert_eq!(
        review,
        json!({"seats": {"security-reviewer": {"provider": "claude", "model": "opus", "fallbacks": []}}})
    );

    let mut orchestration = json!({
        "explorer": {"easy": devin},
        "image_generation": devin,
        "meeting": {"count": 2, "seats": [devin, claude]},
    });
    super::orchestration(&mut orchestration);
    assert_eq!(
        orchestration,
        json!({"explorer": {}, "meeting": {"count": 2, "seats": [null, claude]}})
    );

    let mut decision = json!({"seats": [
        {"id": "a", "provider": "devin", "model": "swe", "fallbacks": [claude]},
        {"id": "b", "provider": "devin", "model": "swe"},
    ]});
    super::decision_seats(&mut decision);
    assert_eq!(
        decision,
        json!({"seats": [{"id": "a", "provider": "claude", "model": "opus", "fallbacks": []}]})
    );
}
