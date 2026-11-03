use super::*;
use crate::data::Reader;
use serde_json::Value;

#[test]
fn storage_routes_read_size_and_validate_preview_and_open() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("seats")).unwrap();
    fs::write(root.path().join("seats/file"), b"12345").unwrap();
    let reader = Reader::new(root.path().to_owned());
    let response = reader.request("GET", "/app/storage", None, b"");
    assert_eq!(response.status, 200);
    let payload: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(payload["path"], root.path().to_string_lossy().as_ref());
    assert_eq!(payload["bytes"], 5);
    assert_eq!(payload["retention_days"], Value::Null);
    assert!(!root.path().join("agentnave.db").exists());
    for days in [30, 90, 180] {
        let response = reader.request(
            "GET",
            &format!("/app/storage/preview?days={days}"),
            None,
            b"",
        );
        assert_eq!(response.status, 200);
        let payload: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(payload["workspaces"], 0);
    }
    for path in [
        "/app/storage/preview",
        "/app/storage/preview?days=31",
        "/app/storage/preview?days=30.0",
    ] {
        assert_eq!(reader.request("GET", path, None, b"").status, 400);
    }
    assert_eq!(
        reader.request("GET", "/app/storage/open", None, b"").status,
        405
    );
    assert_eq!(
        reader
            .request("POST", "/app/storage/open", None, b"")
            .status,
        403
    );
    assert_eq!(
        reader
            .request("POST", "/app/storage", Some("1"), b"")
            .status,
        405
    );
}

#[cfg(unix)]
#[test]
fn size_never_follows_symlinks() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file"), b"12345").unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();
    assert_eq!(bytes(root.path()).unwrap(), 5);
}
