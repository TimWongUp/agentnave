use super::*;
use crate::data::Reader;
use rusqlite::Connection;
use std::fs;
use std::process::Command;

fn registry(root: &Path, rows: &[(&str, &str, &str, &str)]) {
    let connection = Connection::open(root.join(store::NAME)).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE isolated_dirs (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, \
             machine_id TEXT NOT NULL DEFAULT 'local', repo_common_dir TEXT NOT NULL DEFAULT '', \
             repo_identity TEXT NOT NULL DEFAULT '', main_root TEXT NOT NULL DEFAULT '', \
             worktree_path TEXT NOT NULL UNIQUE, worktree_git_dir TEXT, \
             execution_cwd TEXT NOT NULL DEFAULT '', branch_ref TEXT NOT NULL, \
             base_oid TEXT NOT NULL DEFAULT '', state TEXT NOT NULL, phase TEXT NOT NULL DEFAULT '', \
             usage_generation INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT '', \
             updated_at TEXT NOT NULL DEFAULT '', attention TEXT, confirmation_digest TEXT, \
             delete_oid TEXT); PRAGMA user_version = 1;",
        )
        .unwrap();
    for (index, (workspace, path, branch, state)) in rows.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO isolated_dirs (id, workspace_id, worktree_path, branch_ref, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![format!("wt-{index:012x}"), workspace, path, branch, state],
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
    // A registry of another version is not guessed at.
    Connection::open(root.path().join(store::NAME))
        .unwrap()
        .execute_batch("PRAGMA user_version = 2;")
        .unwrap();
    assert!(read(root.path()).is_empty());
    fs::write(root.path().join(store::NAME), b"not a database").unwrap();
    assert!(read(root.path()).is_empty());
}

#[test]
fn a_directory_is_in_use_only_while_a_running_call_works_inside_it() {
    let dir = |path: &str, ready| Dir {
        workspace: "w".into(),
        path: path.into(),
        branch: "agentnave/x".into(),
        ready,
        attention: None,
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
        attention: None,
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

/// The resolved path as the service records it: no Windows verbatim prefix, which git rejects.
fn real(path: &Path) -> std::path::PathBuf {
    let path = path.canonicalize().unwrap();
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        return std::path::PathBuf::from(rest);
    }
    path
}

fn git_cmd(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct RepoFixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    main_root: std::path::PathBuf,
    common_dir: std::path::PathBuf,
    wt_path: std::path::PathBuf,
    id: String,
    branch_ref: String,
    workspace: String,
}

fn setup_worktree_fixture(id: &str) -> RepoFixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    fs::create_dir_all(&root).unwrap();
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    git_cmd(&repo_dir, &["init", "-b", "main"]);
    fs::write(repo_dir.join(".gitignore"), "ignored/\n").unwrap();
    fs::write(repo_dir.join("README.md"), "hello\n").unwrap();
    git_cmd(&repo_dir, &["add", ".gitignore", "README.md"]);
    git_cmd(&repo_dir, &["commit", "-m", "initial"]);
    let main_root = real(&repo_dir);
    let common_dir = real(&std::path::PathBuf::from(git_cmd(
        &main_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )));
    let base_oid = git_cmd(&main_root, &["rev-parse", "HEAD"]);
    let branch_name = format!("agentnave/{id}");
    let branch_ref = format!("refs/heads/{branch_name}");
    let wt_path = main_root.join(".agentnave-worktrees").join(id);
    fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
    git_cmd(
        &main_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch_name,
            wt_path.to_str().unwrap(),
            &base_oid,
        ],
    );
    let wt_path = real(&wt_path);
    let wt_git_dir = git_cmd(
        &wt_path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    );
    let workspace = "s-0123456789abcdef0123456789abcdef".to_owned();
    let conn = rusqlite::Connection::open(root.join(store::NAME)).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS isolated_dirs (
            id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, machine_id TEXT NOT NULL,
            repo_common_dir TEXT NOT NULL, repo_identity TEXT NOT NULL, main_root TEXT NOT NULL,
            worktree_path TEXT NOT NULL UNIQUE, worktree_git_dir TEXT, execution_cwd TEXT NOT NULL,
            branch_ref TEXT NOT NULL, base_oid TEXT NOT NULL, state TEXT NOT NULL,
            phase TEXT NOT NULL, usage_generation INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            attention TEXT, confirmation_digest TEXT, delete_oid TEXT
        ); PRAGMA user_version = 1;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO isolated_dirs (
            id, workspace_id, machine_id, repo_common_dir, repo_identity, main_root,
            worktree_path, worktree_git_dir, execution_cwd, branch_ref, base_oid,
            state, phase, usage_generation, created_at, updated_at
        ) VALUES (?1, ?2, 'local', ?3, ?3, ?4, ?5, ?6, ?5, ?7, ?8, 'ready', 'ready', 1, '2026-11-16T00:00:00Z', '2026-11-16T00:00:00Z')",
        rusqlite::params![
            id,
            workspace,
            common_dir.to_str().unwrap(),
            main_root.to_str().unwrap(),
            wt_path.to_str().unwrap(),
            wt_git_dir,
            branch_ref,
            base_oid,
        ],
    )
    .unwrap();
    RepoFixture {
        _temp: temp,
        root,
        main_root,
        common_dir,
        wt_path,
        id: id.to_owned(),
        branch_ref,
        workspace,
    }
}

fn post(reader: &Reader, path: &str, body: Value) -> Value {
    let resp = reader.request("POST", path, Some("1"), body.to_string().as_bytes());
    assert_eq!(
        resp.status,
        200,
        "{path}: {}",
        String::from_utf8_lossy(&resp.body)
    );
    serde_json::from_slice(&resp.body).unwrap()
}

#[test]
fn clean_isolated_directory_is_deleted_along_with_branch_ignored_files_and_registry() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    // Put an ignored file inside the worktree and configure a hook that must never execute.
    fs::create_dir_all(fx.wt_path.join("ignored")).unwrap();
    fs::write(fx.wt_path.join("ignored/cache.bin"), b"cache").unwrap();
    let hooks_dir = fx.main_root.join(".evil-hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let marker = fx.main_root.join("hook-ran.txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = hooks_dir.join("post-checkout");
        fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_cmd(
        &fx.main_root,
        &["config", "core.hooksPath", hooks_dir.to_str().unwrap()],
    );

    let reader = Reader::new(fx.root.clone());
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");
    assert_eq!(inspected["uncommitted"], false);
    assert_eq!(inspected["unique_commits"], false);
    let token = inspected["confirmation"].as_str().unwrap().to_owned();

    let deleted = post(
        &reader,
        "/app/isolated/delete",
        json!({"id": fx.id, "workspace": fx.workspace, "confirmation": token}),
    );
    assert_eq!(deleted["status"], "deleted");
    assert!(!fx.wt_path.exists());
    assert!(!marker.exists(), "repo hooks must never run");
    assert!(read(&fx.root).is_empty());
    let check_ref = Command::new("git")
        .arg("-C")
        .arg(&fx.main_root)
        .args(["show-ref", "--verify", "--quiet", &fx.branch_ref])
        .status()
        .unwrap();
    assert!(!check_ref.success(), "managed branch should be deleted");
}

#[test]
fn active_lease_blocks_deletion_and_marks_card_using_while_unfinished_can_retry() {
    let fx = setup_worktree_fixture("wt-abcdef012345");
    let lease_dir = fx.common_dir.join("agentnave/leases").join(&fx.id);
    fs::create_dir_all(&lease_dir).unwrap();
    let lease_path = lease_dir.join("active.lock");
    let lease_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lease_path)
        .unwrap();
    lease_file.try_lock().unwrap();

    let dirs = read(&fx.root);
    assert_eq!(dirs.len(), 1);
    let items = classify(
        &dirs,
        &fx.workspace,
        &[fx.wt_path.to_string_lossy().into_owned()],
        &[],
    );
    assert_eq!(items[0]["status"], "using");
    assert_eq!(items[0]["in_use"], true);

    let reader = Reader::new(fx.root.clone());
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "in_use");
    let deleted = post(
        &reader,
        "/app/isolated/delete",
        json!({"id": fx.id, "workspace": fx.workspace, "confirmation": "any"}),
    );
    assert_eq!(deleted["status"], "in_use");
    assert!(fx.wt_path.exists());

    // Release lease and simulate a previous interrupted deletion left in needs_attention.
    drop(lease_file);
    fs::remove_file(&lease_path).unwrap();
    let tip = git_cmd(&fx.wt_path, &["rev-parse", "HEAD"]);
    Connection::open(fx.root.join(store::NAME))
        .unwrap()
        .execute(
            "UPDATE isolated_dirs SET state = 'needs_attention', attention = 'prev fail', delete_oid = ?1 WHERE id = ?2",
            rusqlite::params![tip, fx.id],
        )
        .unwrap();
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");
    assert_eq!(inspected["attention"], "prev fail");
    let deleted = post(
        &reader,
        "/app/isolated/delete",
        json!({
            "id": fx.id,
            "workspace": fx.workspace,
            "confirmation": inspected["confirmation"],
        }),
    );
    assert_eq!(deleted["status"], "deleted");
    assert!(!fx.wt_path.exists());
    assert!(read(&fx.root).is_empty());
}

#[test]
fn uncommitted_and_unmerged_changes_require_matching_confirmation_token() {
    let fx = setup_worktree_fixture("wt-112233445566");
    fs::write(fx.wt_path.join("README.md"), "committed change\n").unwrap();
    git_cmd(&fx.wt_path, &["add", "README.md"]);
    git_cmd(&fx.wt_path, &["commit", "-m", "feat: unmerged work"]);
    fs::write(fx.wt_path.join("dirty.txt"), "uncommitted\n").unwrap();

    let reader = Reader::new(fx.root.clone());
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");
    assert_eq!(inspected["uncommitted"], true);
    assert_eq!(inspected["unique_commits"], true);
    assert!(
        inspected["uncommitted_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "dirty.txt")
    );
    assert!(
        inspected["unmerged_commits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("feat: unmerged work"))
    );

    // Mutate worktree after inspection: old token must be rejected and return fresh confirmation.
    fs::write(fx.wt_path.join("extra.txt"), "changed later\n").unwrap();
    let stale = post(
        &reader,
        "/app/isolated/delete",
        json!({
            "id": fx.id,
            "workspace": fx.workspace,
            "confirmation": inspected["confirmation"],
        }),
    );
    assert_eq!(stale["status"], "confirmation_required");
    assert!(fx.wt_path.exists());

    let deleted = post(
        &reader,
        "/app/isolated/delete",
        json!({
            "id": fx.id,
            "workspace": fx.workspace,
            "confirmation": stale["confirmation"],
        }),
    );
    assert_eq!(deleted["status"], "deleted");
    assert!(!fx.wt_path.exists());
    assert!(read(&fx.root).is_empty());
}

#[test]
fn symref_managed_branch_to_main_keeps_main_and_tampered_branch_ref_is_blocked() {
    let fx = setup_worktree_fixture("wt-778899aabbcc");
    // Turn the managed branch into a symbolic ref pointing to refs/heads/main
    // and break the worktree link so inspect takes the orphan deletion path
    // down to remove_managed_branch (`git update-ref --no-deref -d`).
    git_cmd(
        &fx.main_root,
        &["symbolic-ref", &fx.branch_ref, "refs/heads/main"],
    );
    fs::remove_file(fx.wt_path.join(".git")).unwrap();

    let reader = Reader::new(fx.root.clone());
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");

    let deleted = post(
        &reader,
        "/app/isolated/delete",
        json!({
            "id": fx.id,
            "workspace": fx.workspace,
            "confirmation": inspected["confirmation"],
        }),
    );
    assert_eq!(deleted["status"], "deleted");
    assert!(!fx.wt_path.exists());
    let main_exists = Command::new("git")
        .arg("-C")
        .arg(&fx.main_root)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/main"])
        .status()
        .unwrap();
    assert!(main_exists.success(), "refs/heads/main must still exist");
    let managed_exists = Command::new("git")
        .arg("-C")
        .arg(&fx.main_root)
        .args(["show-ref", "--verify", "--quiet", &fx.branch_ref])
        .status()
        .unwrap();
    assert!(!managed_exists.success(), "managed symref must be deleted");

    // A tampered branch_ref in worktrees.db must be blocked at inspect time.
    let fx2 = setup_worktree_fixture("wt-ccddee001122");
    Connection::open(fx2.root.join(store::NAME))
        .unwrap()
        .execute(
            "UPDATE isolated_dirs SET branch_ref = 'refs/heads/main' WHERE id = ?1",
            [&fx2.id],
        )
        .unwrap();
    let reader2 = Reader::new(fx2.root.clone());
    let blocked = post(
        &reader2,
        "/app/isolated/inspect",
        json!({"id": fx2.id, "workspace": fx2.workspace}),
    );
    assert_eq!(blocked["status"], "blocked");
}

#[test]
fn rollback_checks_container_prunes_missing_dir_and_blocks_deleting_without_oid() {
    // 1) deleting without delete_oid is blocked (not offered as rollback_only).
    let fx = setup_worktree_fixture("wt-001122334455");
    Connection::open(fx.root.join(store::NAME))
        .unwrap()
        .execute(
            "UPDATE isolated_dirs SET state = 'deleting', delete_oid = NULL WHERE id = ?1",
            [&fx.id],
        )
        .unwrap();
    let reader = Reader::new(fx.root.clone());
    let blocked = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(blocked["status"], "blocked");

    // 2) needs_attention without delete_oid pointing outside .agentnave-worktrees/<id> refuses removal.
    let other_wt = fx.main_root.parent().unwrap().join("other-linked");
    git_cmd(
        &fx.main_root,
        &[
            "worktree",
            "add",
            "-b",
            "other-branch",
            other_wt.to_str().unwrap(),
        ],
    );
    let other_wt_real = real(&other_wt);
    Connection::open(fx.root.join(store::NAME))
        .unwrap()
        .execute(
            "UPDATE isolated_dirs SET state = 'needs_attention', delete_oid = NULL, worktree_path = ?1 WHERE id = ?2",
            rusqlite::params![other_wt_real.to_str().unwrap(), fx.id],
        )
        .unwrap();
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");
    assert_eq!(inspected["rollback_only"], true);
    let refused = post(
        &reader,
        "/app/isolated/delete",
        json!({
            "id": fx.id,
            "workspace": fx.workspace,
            "confirmation": inspected["confirmation"],
        }),
    );
    assert_eq!(refused["status"], "needs_attention");
    assert!(
        other_wt_real.is_dir(),
        "unrelated worktree must not be removed"
    );

    // 3) needs_attention without delete_oid whose dir is missing on disk prunes stale git registration and deletes branch.
    let fx3 = setup_worktree_fixture("wt-556677889900");
    fs::remove_dir_all(&fx3.wt_path).unwrap();
    Connection::open(fx3.root.join(store::NAME))
        .unwrap()
        .execute(
            "UPDATE isolated_dirs SET state = 'needs_attention', delete_oid = NULL WHERE id = ?1",
            [&fx3.id],
        )
        .unwrap();
    let reader3 = Reader::new(fx3.root.clone());
    let inspected3 = post(
        &reader3,
        "/app/isolated/inspect",
        json!({"id": fx3.id, "workspace": fx3.workspace}),
    );
    assert_eq!(inspected3["status"], "confirm");
    assert_eq!(inspected3["rollback_only"], true);
    let deleted3 = post(
        &reader3,
        "/app/isolated/delete",
        json!({
            "id": fx3.id,
            "workspace": fx3.workspace,
            "confirmation": inspected3["confirmation"],
        }),
    );
    assert_eq!(deleted3["status"], "deleted");
    assert!(read(&fx3.root).is_empty());
    let managed_exists = Command::new("git")
        .arg("-C")
        .arg(&fx3.main_root)
        .args(["show-ref", "--verify", "--quiet", &fx3.branch_ref])
        .status()
        .unwrap();
    assert!(
        !managed_exists.success(),
        "managed branch should be removed after prune"
    );
}
