use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;

use super::cache::{CLOSED_PR_TTL, Cache, OPEN_PR_TTL, pr_ttl};
use super::gh::{
    GhClient, PrInfo, extract_github_pr_links, parse_github_remote_url, parse_pr_view_json,
    pr_merged_for_worktree, pr_priority_rank,
};
use super::git::{inspect_workspace_git, parse_porcelain_v2_branch, parse_shortstat};
use super::links::{extract_pr_links_from_files, terminal_log_files};
use super::{GitPr, IsolatedTarget, RefreshInput, TerminalTarget, WorkspaceTarget, is_cli_boss};
use crate::data::boss_app::{
    ANTIGRAVITY_CLI, ANTIGRAVITY_DESKTOP, CLAUDE_CLI, CLAUDE_DESKTOP, CODEX_CLI, CODEX_DESKTOP,
    Homes,
};
use crate::isolated::git::GitRunner;

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

#[cfg(unix)]
fn write_fake_gh(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-gh");
    let script = format!("#!/bin/sh\n{body}\n");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn cli_boss_filter_excludes_desktop_and_unknown_hosts() {
    assert!(is_cli_boss(Some("claude"), Some(CLAUDE_CLI)));
    assert!(is_cli_boss(Some("codex"), Some(CODEX_CLI)));
    assert!(is_cli_boss(Some("antigravity"), Some(ANTIGRAVITY_CLI)));
    assert!(is_cli_boss(Some("grok"), None));
    assert!(is_cli_boss(Some("codebuddy"), None));
    assert!(is_cli_boss(Some("pi"), None));

    assert!(!is_cli_boss(Some("claude"), Some(CLAUDE_DESKTOP)));
    assert!(!is_cli_boss(Some("claude"), None));
    assert!(!is_cli_boss(Some("codex"), Some(CODEX_DESKTOP)));
    assert!(!is_cli_boss(Some("codex"), None));
    assert!(!is_cli_boss(Some("antigravity"), Some(ANTIGRAVITY_DESKTOP)));
    assert!(!is_cli_boss(Some("antigravity"), None));
    assert!(!is_cli_boss(Some("workbuddy"), None));
    assert!(!is_cli_boss(Some("deepseek"), None));
    assert!(!is_cli_boss(Some("hermes"), None));
    assert!(!is_cli_boss(None, None));
}

#[test]
fn parses_porcelain_v2_and_shortstat() {
    let raw = [
        "# branch.oid abc123",
        "# branch.head feat/git-pr",
        "# branch.upstream origin/feat/git-pr",
        "# branch.ab +2 -1",
        "1 M. N... 100644 100644 100644 a b src/a.rs",
        "1 .M N... 100644 100644 100644 a b src/b.rs",
        "2 R. N... 100644 100644 100644 a b R100 src/new.rs",
        "src/old.rs",
        "? untracked.txt",
        "",
    ]
    .join("\0");
    let status = parse_porcelain_v2_branch(raw.as_bytes());
    assert_eq!(status.branch.as_deref(), Some("feat/git-pr"));
    assert_eq!(status.head_sha.as_deref(), Some("abc123"));
    assert_eq!(status.upstream.as_deref(), Some("origin/feat/git-pr"));
    assert_eq!(status.ahead, 2);
    assert_eq!(status.behind, 1);
    assert_eq!(status.staged_files, 2);
    assert_eq!(status.unstaged_files, 1);
    assert_eq!(status.untracked_files, 1);
    assert_eq!(status.changed_files, 4);

    assert_eq!(
        parse_shortstat(" 3 files changed, 42 insertions(+), 7 deletions(-)"),
        (42, 7)
    );
    assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)"), (1, 0));
    assert_eq!(parse_shortstat(" 1 file changed, 5 deletions(-)"), (0, 5));
    assert_eq!(parse_shortstat(""), (0, 0));
}

#[test]
fn inspects_real_temp_repo_and_worktree_with_neutralized_git() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-b", "main"]);
    git_cmd(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    fs::write(repo.join("a.txt"), "line1\nline2\n").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-m", "init"]);

    let wt = tmp.path().join("wt-1");
    git_cmd(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "agentnave/wt-1",
            wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    fs::write(wt.join("b.txt"), "one\ntwo\n").unwrap();
    git_cmd(&wt, &["add", "b.txt"]);
    git_cmd(&wt, &["commit", "-m", "feat"]);
    fs::write(wt.join("a.txt"), "line1\nline2-edited\nline3\n").unwrap();
    fs::write(wt.join("note.txt"), "untracked\n").unwrap();

    let runner = GitRunner::new().unwrap();
    let main_status = inspect_workspace_git(&runner, &repo).unwrap();
    assert!(main_status.is_git);
    assert!(!main_status.is_worktree);
    assert_eq!(main_status.branch.as_deref(), Some("main"));
    assert_eq!(main_status.default_branch.as_deref(), Some("main"));
    assert_eq!(main_status.ahead_of_default, Some(0));
    assert!(main_status.integrated_into_default);
    assert_eq!(main_status.changed_files, 0);
    assert_eq!(main_status.github_repo.as_deref(), Some("acme/demo"));

    let wt_status = inspect_workspace_git(&runner, &wt).unwrap();
    assert!(wt_status.is_git);
    assert!(wt_status.is_worktree);
    assert_eq!(wt_status.branch.as_deref(), Some("agentnave/wt-1"));
    assert_eq!(wt_status.default_branch.as_deref(), Some("main"));
    assert_eq!(wt_status.ahead_of_default, Some(1));
    assert!(!wt_status.integrated_into_default);
    assert_eq!(wt_status.changed_files, 2);
    assert_eq!(wt_status.unstaged_files, 1);
    assert_eq!(wt_status.untracked_files, 1);
    assert_eq!(wt_status.additions, 2);
    assert_eq!(wt_status.deletions, 1);
}

#[test]
fn parses_gh_pr_view_json_priority_and_merged_check() {
    let raw = json!({
        "title": "Fix login race",
        "number": 42,
        "state": "OPEN",
        "isDraft": false,
        "additions": 120,
        "deletions": 15,
        "statusCheckRollup": [
            {"name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://github.com/acme/demo/actions/runs/1"},
            {"name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "https://github.com/acme/demo/actions/runs/2"},
            {"context": "deploy", "state": "PENDING", "targetUrl": "https://ci.example/3"}
        ],
        "reviewDecision": "CHANGES_REQUESTED",
        "mergeable": "CONFLICTING",
        "updatedAt": "2026-11-23T04:00:00Z",
        "url": "https://github.com/acme/demo/pull/42",
        "headRefName": "agentnave/wt-1",
        "headRefOid": "deadbeef",
        "baseRefName": "main"
    });
    let pr = parse_pr_view_json(&raw, None).unwrap();
    assert_eq!(pr.repo, "acme/demo");
    assert_eq!(pr.number, 42);
    assert_eq!(pr.state, "open");
    assert_eq!(pr.additions, 120);
    assert_eq!(pr.deletions, 15);
    assert_eq!(pr.checks.state, "failing");
    assert_eq!(pr.checks.total, 3);
    assert_eq!(pr.checks.passed, 1);
    assert_eq!(pr.checks.failed, 1);
    assert_eq!(pr.checks.pending, 1);
    assert_eq!(
        pr.checks.checks_url.as_deref(),
        Some("https://github.com/acme/demo/actions/runs/2")
    );
    assert_eq!(pr.review_decision.as_deref(), Some("changes_requested"));
    assert_eq!(pr.mergeable, "conflicting");
    assert_eq!(pr_priority_rank(&pr), 5);
    assert!(!pr_merged_for_worktree(
        &pr,
        "agentnave/wt-1",
        Some("main"),
        "deadbeef",
        false
    ));

    let merged = PrInfo {
        state: "merged".to_owned(),
        ..pr.clone()
    };
    assert_eq!(pr_priority_rank(&merged), 2);
    // Squash merge: exact headSha + headRef + baseRef match even when not integrated in local git.
    assert!(pr_merged_for_worktree(
        &merged,
        "agentnave/wt-1",
        Some("main"),
        "deadbeef",
        false
    ));
    // Different HEAD commit after merge: must NOT count as merged.
    assert!(!pr_merged_for_worktree(
        &merged,
        "agentnave/wt-1",
        Some("main"),
        "cafebabe",
        false
    ));
    // Wrong base branch: must NOT count as merged.
    assert!(!pr_merged_for_worktree(
        &merged,
        "agentnave/wt-1",
        Some("release"),
        "deadbeef",
        false
    ));
    // Already integrated into default branch: counts as merged.
    assert!(pr_merged_for_worktree(
        &merged,
        "agentnave/wt-1",
        Some("main"),
        "cafebabe",
        true
    ));

    assert_eq!(pr_ttl(Some(&pr)), OPEN_PR_TTL);
    assert_eq!(pr_ttl(Some(&merged)), CLOSED_PR_TTL);
}

#[test]
fn extracts_github_remote_and_pr_links_from_native_logs() {
    assert_eq!(
        parse_github_remote_url("git@github.com:TimWongUp/agentnave.git").as_deref(),
        Some("TimWongUp/agentnave")
    );
    assert_eq!(
        parse_github_remote_url("https://github.com/TimWongUp/agentnave").as_deref(),
        Some("TimWongUp/agentnave")
    );
    assert_eq!(parse_github_remote_url("https://gitlab.com/a/b.git"), None);

    let links = extract_github_pr_links(
        "已创建 https://github.com/acme/demo/pull/12 和 http://github.com/acme/demo/pull/34/files",
    );
    assert_eq!(
        links,
        vec![("acme/demo".to_owned(), 12), ("acme/demo".to_owned(), 34)]
    );

    let tmp = tempfile::tempdir().unwrap();
    let claude_home = tmp.path().join(".claude");
    let proj = claude_home.join("projects/p1");
    fs::create_dir_all(&proj).unwrap();
    let sid = "11111111-2222-3333-4444-555555555555";
    let log = proj.join(format!("{sid}.jsonl"));
    fs::write(
        &log,
        "{\"text\":\"opened https://github.com/acme/demo/pull/7\"}\n{\"text\":\"merged https://github.com/acme/demo/pull/8 and https://github.com/acme/demo/pull/7\"}\n",
    )
    .unwrap();
    let homes = Homes {
        claude: vec![claude_home],
        ..Homes::default()
    };
    let found_files = terminal_log_files(&homes, "claude", Some(sid), None);
    assert_eq!(found_files, vec![log]);
    let mut cache = Cache::default();
    let extracted = extract_pr_links_from_files(&mut cache, &found_files);
    // Most recent unique links first.
    assert_eq!(
        extracted,
        vec![("acme/demo".to_owned(), 7), ("acme/demo".to_owned(), 8)]
    );
}

#[cfg(unix)]
#[test]
fn refresh_aggregates_prs_caches_by_state_and_handles_missing_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-b", "main"]);
    git_cmd(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    fs::write(repo.join("f.txt"), "hello\n").unwrap();
    git_cmd(&repo, &["add", "f.txt"]);
    git_cmd(&repo, &["commit", "-m", "init"]);

    let wt = tmp.path().join("wt-abc");
    git_cmd(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "agentnave/wt-abc",
            wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    fs::write(wt.join("f.txt"), "changed\n").unwrap();
    git_cmd(&wt, &["add", "f.txt"]);
    git_cmd(&wt, &["commit", "-m", "work"]);
    let wt_head = git_cmd(&wt, &["rev-parse", "HEAD"]);

    let counter = tmp.path().join("gh-calls.log");
    let fake_gh = write_fake_gh(
        tmp.path(),
        &format!(
            r#"echo "$@" >> "{counter}"
if [ "$1" = "auth" ]; then
  echo "Logged in to github.com account tester"
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  case "$3" in
    agentnave/wt-abc)
      cat <<EOF
{{"title":"Merged worktree PR","number":99,"state":"MERGED","isDraft":false,"additions":10,"deletions":2,"statusCheckRollup":[],"reviewDecision":"APPROVED","mergeable":"UNKNOWN","updatedAt":"2026-11-23T01:00:00Z","url":"https://github.com/acme/demo/pull/99","headRefName":"agentnave/wt-abc","headRefOid":"{wt_head}","baseRefName":"main"}}
EOF
      exit 0
      ;;
    55)
      cat <<EOF
{{"title":"Session linked PR","number":55,"state":"OPEN","isDraft":false,"additions":5,"deletions":1,"statusCheckRollup":[{{"name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}}],"reviewDecision":"REVIEW_REQUIRED","mergeable":"MERGEABLE","updatedAt":"2026-11-23T02:00:00Z","url":"https://github.com/acme/demo/pull/55","headRefName":"feat/linked","headRefOid":"111111","baseRefName":"main"}}
EOF
      exit 0
      ;;
  esac
fi
echo "no pull requests found" >&2
exit 1"#,
            counter = counter.display()
        ),
    );

    let claude_home = tmp.path().join(".claude");
    let proj = claude_home.join("projects/p");
    fs::create_dir_all(&proj).unwrap();
    let sid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    fs::write(
        proj.join(format!("{sid}.jsonl")),
        "{\"text\":\"See https://github.com/acme/demo/pull/55\"}\n",
    )
    .unwrap();

    let git_pr = GitPr::with_client(GhClient::with_program(fake_gh));
    let input = || RefreshInput {
        reason: "focus".to_owned(),
        homes: Homes {
            claude: vec![claude_home.clone()],
            ..Homes::default()
        },
        terminals: vec![
            TerminalTarget {
                id: 1,
                provider: "claude".to_owned(),
                cwd: repo.clone(),
                native: Some(sid.to_owned()),
                account_dir: None,
                workspaces: vec!["s-ws1".to_owned()],
                derived: false,
                parent_workspace_id: None,
            },
            TerminalTarget {
                id: 2,
                provider: "claude".to_owned(),
                cwd: wt.clone(),
                native: None,
                account_dir: None,
                workspaces: Vec::new(),
                derived: true,
                parent_workspace_id: Some("s-ws1".to_owned()),
            },
        ],
        workspaces: vec![WorkspaceTarget {
            id: "s-ws1".to_owned(),
            host: Some("claude".to_owned()),
            owner_key: None,
            owner_file: None,
            account_dir: None,
            cwd: Some(repo.clone()),
        }],
        isolated: vec![IsolatedTarget {
            id: "wt-abc".to_owned(),
            workspace: "s-ws1".to_owned(),
            path: wt.display().to_string(),
            branch: "agentnave/wt-abc".to_owned(),
        }],
    };

    let snap = git_pr.refresh(input());
    assert_eq!(snap["gh_status"], "ready");
    // Terminal 1 has PR #55 from native log.
    assert_eq!(snap["terminals"]["1"]["prs"].as_array().unwrap().len(), 1);
    assert_eq!(snap["terminals"]["1"]["prs"][0]["number"], 55);
    // Terminal 2 (derived in worktree) has PR #99 from its branch.
    assert_eq!(snap["terminals"]["2"]["prs"][0]["number"], 99);
    // Workspace aggregates both #55 (open, rank 4) and #99 (merged, rank 2).
    let ws_prs = snap["workspaces"]["s-ws1"]["prs"].as_array().unwrap();
    assert_eq!(ws_prs.len(), 2);
    assert_eq!(ws_prs[0]["number"], 55);
    assert_eq!(ws_prs[1]["number"], 99);
    // Isolated dir recognizes squash-merged PR #99.
    assert_eq!(snap["isolated"]["wt-abc"]["merged_pr"]["number"], 99);

    let first_calls = fs::read_to_string(&counter).unwrap().lines().count();
    // Second refresh hits tiered cache: zero extra `gh` calls!
    let _ = git_pr.refresh(input());
    let second_calls = fs::read_to_string(&counter).unwrap().lines().count();
    assert_eq!(first_calls, second_calls);

    // When `gh` is missing, Git status still works and PRs are hidden.
    let no_gh = GitPr::with_client(GhClient::with_program(tmp.path().join("does-not-exist")));
    let offline = no_gh.refresh(input());
    assert_eq!(offline["gh_status"], "not_installed");
    assert_eq!(offline["terminals"]["1"]["git"]["branch"], "main");
    assert!(
        offline["terminals"]["1"]["prs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(offline["isolated"]["wt-abc"]["pr"].is_null());
}

#[cfg(unix)]
#[test]
fn isolated_inspect_marks_squash_merged_pr_and_falls_back_when_gh_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    fs::create_dir_all(&root).unwrap();
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    git_cmd(&repo_dir, &["init", "-b", "main"]);
    git_cmd(
        &repo_dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    fs::write(repo_dir.join("README.md"), "hi\n").unwrap();
    git_cmd(&repo_dir, &["add", "README.md"]);
    git_cmd(&repo_dir, &["commit", "-m", "initial"]);

    let main_root = repo_dir.canonicalize().unwrap();
    let common_dir = PathBuf::from(git_cmd(
        &main_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
    .canonicalize()
    .unwrap();
    let base_oid = git_cmd(&main_root, &["rev-parse", "HEAD"]);
    let id = "wt-aabbccddeeff";
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
    let wt_path = wt_path.canonicalize().unwrap();
    fs::write(wt_path.join("feature.txt"), "squash me\n").unwrap();
    git_cmd(&wt_path, &["add", "feature.txt"]);
    git_cmd(&wt_path, &["commit", "-m", "feature commit"]);
    let head_oid = git_cmd(&wt_path, &["rev-parse", "HEAD"]);
    let wt_git_dir = git_cmd(
        &wt_path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    );
    let workspace = "s-0123456789abcdef0123456789abcdef";
    let conn = rusqlite::Connection::open(root.join("worktrees.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE isolated_dirs (
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
        "INSERT INTO isolated_dirs (id, workspace_id, machine_id, repo_common_dir, repo_identity, \
         main_root, worktree_path, worktree_git_dir, execution_cwd, branch_ref, base_oid, state, \
         phase, created_at, updated_at) VALUES (?1,?2,'local',?3,'id',?4,?5,?6,?5,?7,?8,'ready','ready','t','t')",
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
    drop(conn);

    let fake_gh = write_fake_gh(
        tmp.path(),
        &format!(
            r#"if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  cat <<EOF
{{"title":"Squash feature","number":123,"state":"MERGED","isDraft":false,"additions":1,"deletions":0,"statusCheckRollup":[],"reviewDecision":"APPROVED","mergeable":"UNKNOWN","updatedAt":"2026-11-23T03:00:00Z","url":"https://github.com/acme/demo/pull/123","headRefName":"{branch_name}","headRefOid":"{head_oid}","baseRefName":"main"}}
EOF
  exit 0
fi
exit 1"#
        ),
    );
    let with_pr =
        crate::isolated::inspect_with_gh(&root, id, workspace, &GhClient::with_program(fake_gh));
    assert_eq!(with_pr["status"], "confirm");
    assert_eq!(with_pr["unique_commits"], true);
    assert_eq!(with_pr["merged_pr"]["number"], 123);
    assert_eq!(
        with_pr["merged_pr"]["url"],
        "https://github.com/acme/demo/pull/123"
    );

    // Without gh, falls back cleanly to unique_commits without merged_pr.
    let without_pr = crate::isolated::inspect_with_gh(
        &root,
        id,
        workspace,
        &GhClient::with_program(tmp.path().join("missing-gh")),
    );
    assert_eq!(without_pr["status"], "confirm");
    assert_eq!(without_pr["unique_commits"], true);
    assert!(without_pr["merged_pr"].is_null());
    // Confirmation digest stays identical regardless of gh availability.
    assert_eq!(with_pr["confirmation"], without_pr["confirmation"]);
}

#[test]
fn cache_respects_open_vs_closed_ttls() {
    let mut cache = Cache::default();
    let t0 = Instant::now();
    let open = PrInfo {
        repo: "acme/demo".into(),
        number: 1,
        title: "Open".into(),
        state: "open".into(),
        is_draft: false,
        additions: 1,
        deletions: 0,
        checks: Default::default(),
        review_decision: None,
        mergeable: "mergeable".into(),
        updated_at: String::new(),
        url: "https://github.com/acme/demo/pull/1".into(),
        head_ref_name: "a".into(),
        head_ref_oid: "1".into(),
        base_ref_name: "main".into(),
    };
    let merged = PrInfo {
        number: 2,
        state: "merged".into(),
        ..open.clone()
    };
    cache.put_pr_by_number("acme/demo", 1, t0, Some(open));
    cache.put_pr_by_number("acme/demo", 2, t0, Some(merged));

    let after_65s = t0 + Duration::from_secs(65);
    assert!(cache.get_pr_by_number("acme/demo", 1, after_65s).is_none());
    assert!(cache.get_pr_by_number("acme/demo", 2, after_65s).is_some());

    let after_605s = t0 + Duration::from_secs(605);
    assert!(cache.get_pr_by_number("acme/demo", 2, after_605s).is_none());
}
