use super::*;

#[test]
fn cli_updates_routes_require_get_or_authorized_post() {
    let dir = tempfile::tempdir().unwrap();
    let reader = Reader::new(dir.path().to_owned());
    assert_eq!(
        reader
            .request("POST", "/app/cli-updates", Some("1"), b"{}")
            .status,
        405
    );
    assert_eq!(
        reader
            .request("GET", "/app/cli-updates/update", Some("1"), b"{}")
            .status,
        405
    );
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/cli-updates/update",
                None,
                b"{\"provider\":\"codex\"}"
            )
            .status,
        403
    );
    assert_eq!(
        reader
            .request("POST", "/app/cli-updates/update", Some("1"), b"not json")
            .status,
        400
    );
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/cli-updates/update",
                Some("1"),
                b"{\"provider\":\"unknown\"}"
            )
            .status,
        400
    );
    let commands = reader.request("GET", "/app/cli-install-commands", None, b"");
    assert_eq!(commands.status, 200);
    let commands: Value = serde_json::from_slice(&commands.body).unwrap();
    assert!(commands["codex"].is_array());
    assert!(commands["grok"].as_array().unwrap().is_empty());
    assert_eq!(
        reader
            .request("POST", "/app/cli-install-commands", Some("1"), b"{}")
            .status,
        405
    );
    assert_eq!(reader.respond("/settings/member-updates.js").status, 200);
}

#[test]
fn cli_update_target_rejects_desktop_unknown_and_other_providers() {
    let dir = tempfile::tempdir().unwrap();
    let reader = Reader::new(dir.path().to_owned());
    for body in [
        json!({"provider":"codex", "target":"desktop"}),
        json!({"provider":"codex", "target":"/tmp/codex"}),
        json!({"provider":"codex", "target":false}),
        json!({"provider":"codex", "target":null}),
        json!({"provider":"claude", "target":"standalone"}),
    ] {
        let response = reader.request(
            "POST",
            "/app/cli-updates/update",
            Some("1"),
            &serde_json::to_vec(&body).unwrap(),
        );
        assert_eq!(response.status, 400);
        let result: Value = serde_json::from_slice(&response.body).unwrap();
        assert!(
            result["error"].as_str().unwrap().contains("target"),
            "{result}"
        );
    }
}
