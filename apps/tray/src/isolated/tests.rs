use super::*;

fn registry(root: &Path, rows: &[(&str, &str, &str, &str)]) {
    let connection = Connection::open(root.join(NAME)).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE isolated_dirs (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, \
             worktree_path TEXT NOT NULL, branch_ref TEXT NOT NULL, state TEXT NOT NULL, \
             created_at TEXT NOT NULL DEFAULT '');",
        )
        .unwrap();
    for (index, (workspace, path, branch, state)) in rows.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO isolated_dirs (id, workspace_id, worktree_path, branch_ref, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![index.to_string(), workspace, path, branch, state],
            )
            .unwrap();
    }
}

#[test]
fn reads_registered_directories_and_skips_unfinished_ones() {
    let root = tempfile::tempdir().unwrap();
    assert!(read(root.path()).is_empty());
    registry(
        root.path(),
        &[
            ("w", "/r/a", "refs/heads/agentnave/a", "ready"),
            ("w", "/r/b", "refs/heads/agentnave/b", "creating"),
            ("w", "/r/c", "refs/heads/agentnave/c", "needs_attention"),
        ],
    );
    let dirs = read(root.path());
    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs[0].branch, "agentnave/a");
    assert!(dirs[0].ready && !dirs[1].ready);
    std::fs::write(root.path().join(NAME), b"not a database").unwrap();
    assert!(read(root.path()).is_empty());
}

#[test]
fn a_directory_is_in_use_only_while_a_running_call_works_inside_it() {
    let dir = |path: &str, ready| Dir {
        workspace: "w".into(),
        path: path.into(),
        branch: "agentnave/x".into(),
        ready,
        id: path.into(),
        created: String::new(),
    };
    let dirs = [
        dir("/r/a", true),
        dir("/r/b", true),
        dir("/r/c", false),
        dir("/r/ab", true),
    ];
    let running = vec!["/r/a/sub".to_owned(), "/r/c".to_owned()];
    let items = classify(&dirs, "w", &running, &[]);
    let status = |path: &str| items.iter().find(|i| i["path"] == path).unwrap()["status"].clone();
    assert_eq!(status("/r/a"), "using");
    assert_eq!(status("/r/b"), "idle");
    assert_eq!(status("/r/c"), "unfinished");
    assert_eq!(status("/r/ab"), "idle");
    assert_eq!(items[3]["status"], "using");
    assert!(classify(&dirs, "other", &running, &[]).is_empty());
}

#[test]
fn the_builder_is_found_by_the_registered_id() {
    let dir = Dir {
        workspace: "w".into(),
        path: "/r/a".into(),
        branch: "agentnave/a".into(),
        ready: true,
        id: "wt-1".into(),
        created: "2026-11-12T00:00:00+00:00".into(),
    };
    let builder = Builder {
        id: "wt-1".into(),
        path: String::new(),
        desk: "thread:x".into(),
        text: "修登录".into(),
    };
    let items = classify(&[dir], "w", &[], &[builder]);
    assert_eq!(items[0]["task"]["text"], "修登录");
}
