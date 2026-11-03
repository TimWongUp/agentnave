use super::*;
use serde_json::json;

#[test]
fn old_database_displays_views_but_all_mutations_require_the_service_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "task",
        &json!({"title": "旧库"}),
        &json!({
            "version": 1, "summary": {"id": "task", "title": "旧库", "status": "succeeded"}
        }),
    );
    let connection = write(root).unwrap().unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 1; ALTER TABLE workspaces DROP COLUMN title; \
         ALTER TABLE workspaces DROP COLUMN cwd;",
        )
        .unwrap();
    let reader = crate::data::Reader::new(root.to_path_buf());
    assert_eq!(reader.overview().items[0].title, "旧库");
    let readonly = read(root).unwrap().unwrap();
    assert!(readonly.execute("DELETE FROM conversations", []).is_err());
    let expected = crate::preferences::general::text(
        "先重连宿主，让服务完成升级",
        "Reconnect the host first so the service can finish upgrading",
    );
    for result in [
        crate::manage::rename(root, "task", "新名"),
        crate::manage::delete(root, "task"),
        crate::manage::handover(root, "workspace"),
        crate::manage::forget(root, "workspace"),
        crate::manage::cleanup(root, &["task".to_owned()], i64::MAX).remove(0),
    ] {
        assert_eq!(result["status"], "error");
        assert_eq!(result["message"], expected);
    }
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    // No table/column repair and no conversation lock files are created by refused writes.
    assert!(connection.prepare("SELECT title FROM workspaces").is_err());
    assert!(!root.join("locks").exists());
    assert_eq!(reader.overview().items[0].title, "旧库");
}

#[test]
fn future_schema_is_rejected_for_reading_and_writing() {
    let dir = tempfile::tempdir().unwrap();
    let connection = create(dir.path());
    connection.pragma_update(None, "user_version", 3).unwrap();
    assert!(read(dir.path()).is_err());
    assert!(write(dir.path()).is_err());
}
