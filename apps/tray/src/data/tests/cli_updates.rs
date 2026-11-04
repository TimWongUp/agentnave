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
    assert_eq!(reader.respond("/settings/member-updates.js").status, 200);
}
