use super::*;
use serde_json::json;

fn installation(base: &Path, version: &str, leased: bool) -> std::path::PathBuf {
    let root = base.join("service").join(version);
    fs::create_dir_all(root.join("tools/agentnave/Lib/site-packages/agentnave-0.58.0.dist-info"))
        .unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/agentnave-mcp.exe"), b"launcher").unwrap();
    if leased {
        fs::create_dir_all(base.join("service/.leases")).unwrap();
        fs::write(base.join(format!("service/.leases/{version}.lock")), b"").unwrap();
        fs::write(root.join("gc-v1"), b"1").unwrap();
        let old = SystemTime::now() - GRACE - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(root.join("gc-v1"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    root
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let current = installation(dir.path(), "0.58.0", true);
    fs::write(
        dir.path().join("service/active.json"),
        json!({"version":"0.58.0", "bin":current.join("bin/agentnave-mcp.exe")}).to_string(),
    )
    .unwrap();
    installation(dir.path(), "0.57.0", true);
    dir
}

#[test]
fn keeps_active_rollback_configs_legacy_and_locked_environments() {
    let dir = fixture();
    let unreferenced = installation(dir.path(), "0.50.0", true);
    let pinned = installation(dir.path(), "0.51.0", true);
    let legacy = installation(dir.path(), "0.52.0", false);
    let running = installation(dir.path(), "0.53.0", true);
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .open(dir.path().join("service/.leases/0.53.0.lock"))
        .unwrap();
    lock.lock_shared().unwrap();
    prune(
        dir.path(),
        Some(vec![
            r#"command="C:\\data\\service\\0.51.0\\bin\\agentnave-mcp.exe""#.into(),
        ]),
    );
    assert!(!unreferenced.exists());
    for path in [
        pinned,
        legacy,
        running.clone(),
        dir.path().join("service/0.57.0"),
        dir.path().join("service/0.58.0"),
    ] {
        assert!(path.is_dir(), "{}", path.display());
    }
    drop(lock);
    prune(dir.path(), Some(Vec::new()));
    assert!(!running.exists());
}

#[test]
fn incomplete_inventory_grace_and_bad_pointer_prevent_deletion() {
    let dir = fixture();
    let old = installation(dir.path(), "0.50.0", true);
    prune(dir.path(), None);
    assert!(old.exists());
    fs::write(old.join("gc-v1"), b"1").unwrap();
    prune(dir.path(), Some(Vec::new()));
    assert!(old.exists());
    fs::write(dir.path().join("service/active.json"), b"{").unwrap();
    prune(dir.path(), Some(Vec::new()));
    assert!(old.exists());
}

#[cfg(unix)]
#[test]
fn never_follows_a_link_inside_an_environment() {
    let dir = fixture();
    let old = installation(dir.path(), "0.50.0", true);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), b"keep").unwrap();
    std::os::unix::fs::symlink(outside.path(), old.join("linked")).unwrap();
    prune(dir.path(), Some(Vec::new()));
    assert!(old.exists());
    assert!(outside.path().join("keep").is_file());
}
