//! Automatic cleanup (ADR-0115 §6): each rule, each reason to keep, and the collapsed state.

use std::collections::HashSet;

use super::*;
use crate::git_pr::gh::GhClient;
use crate::isolated::cleanup::{Env, Inputs, Report, Rules, Trigger, run};

const DAY: i64 = 86_400_000_000;
const MINUTE: i64 = 60_000_000;
/// `updated_at` of the fixture's registry row.
const UPDATED: &str = "2026-11-16T00:00:00Z";

/// One cleanup pass with the fixture's outside world: the clock, the tray's knowledge of what
/// is running, and `gh`.
struct Pass {
    rules: Rules,
    now: i64,
    inputs: Inputs,
    terminal_busy: bool,
    gh: GhClient,
    /// When the worktree's HEAD reflog last changed; the registry time unless a test says.
    reflog_at: Option<i64>,
}

impl Pass {
    fn new(rules: Rules) -> Self {
        let updated = crate::time_format::timestamp(UPDATED).unwrap();
        Self {
            rules,
            now: updated + 2 * DAY,
            inputs: Inputs::default(),
            terminal_busy: false,
            gh: GhClient::system(),
            reflog_at: None,
        }
    }

    fn after_days(mut self, days: i64) -> Self {
        self.now = crate::time_format::timestamp(UPDATED).unwrap() + days * DAY;
        self
    }

    fn run(&self, fx: &RepoFixture) -> Report {
        // Commits made by the test touch the reflog at wall-clock time; pin it to the fake clock.
        let at = self
            .reflog_at
            .unwrap_or_else(|| crate::time_format::timestamp(UPDATED).unwrap());
        let reflog = fx.wt_path.exists().then(|| {
            let git_dir = git_cmd(
                &fx.wt_path,
                &["rev-parse", "--path-format=absolute", "--git-dir"],
            );
            std::path::Path::new(&git_dir).join("logs").join("HEAD")
        });
        if let Some(reflog) = reflog.filter(|path| path.exists()) {
            let when = std::time::UNIX_EPOCH
                + std::time::Duration::from_micros(u64::try_from(at).unwrap());
            fs::File::options()
                .write(true)
                .open(&reflog)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        let busy = |_: &store::Record| self.terminal_busy;
        let env = Env {
            root: &fx.root,
            gh: &self.gh,
            now: self.now,
            rules: self.rules,
            inputs: &self.inputs,
            terminal_busy: &busy,
        };
        run(&env, Trigger::Manual)
    }
}

fn unchanged() -> Rules {
    Rules {
        unchanged: true,
        ..Rules::default()
    }
}

fn idle(days: u32) -> Rules {
    Rules {
        idle_days: Some(days),
        ..Rules::default()
    }
}

/// `(outcome, rule or reason)` of one directory in the report.
fn verdict(report: &Report, id: &str) -> (String, String) {
    let entries = report.to_json()["entries"].clone();
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {entries}"))
        .clone();
    let why = entry["rule"]
        .as_str()
        .or(entry["reason"].as_str())
        .unwrap()
        .to_owned();
    (entry["outcome"].as_str().unwrap().to_owned(), why)
}

fn kept(why: &str) -> (String, String) {
    ("kept".to_owned(), why.to_owned())
}

fn branch_exists(fx: &RepoFixture) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(&fx.main_root)
        .args(["show-ref", "--verify", "--quiet", &fx.branch_ref])
        .status()
        .unwrap()
        .success()
}

fn commit_file(fx: &RepoFixture, name: &str) -> String {
    fs::write(fx.wt_path.join(name), "work\n").unwrap();
    git_cmd(&fx.wt_path, &["add", name]);
    git_cmd(&fx.wt_path, &["commit", "-m", "work"]);
    git_cmd(&fx.wt_path, &["rev-parse", "HEAD"])
}

fn sql(fx: &RepoFixture, statement: &str) {
    Connection::open(fx.root.join(store::NAME))
        .unwrap()
        .execute(statement, [])
        .unwrap();
}

#[test]
fn an_unchanged_directory_goes_with_its_branch_and_registry_row() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    // Ignored files go with the directory, as in a manual delete.
    fs::create_dir_all(fx.wt_path.join("ignored")).unwrap();
    fs::write(fx.wt_path.join("ignored/cache.bin"), b"cache").unwrap();

    let report = Pass::new(unchanged()).run(&fx);
    assert_eq!(
        verdict(&report, &fx.id),
        ("removed".into(), "unchanged".into())
    );
    assert!(!fx.wt_path.exists());
    assert!(!branch_exists(&fx));
    assert!(read(&fx.root).is_empty());
    let shown = report.to_json();
    assert_eq!(shown["trigger"], "manual");
    assert_eq!(
        shown["entries"][0]["branch"],
        format!("agentnave/{}", fx.id)
    );
}

#[test]
fn commits_beyond_the_default_branch_are_not_unchanged() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    commit_file(&fx, "feature.txt");
    let report = Pass::new(unchanged()).run(&fx);
    assert_eq!(verdict(&report, &fx.id), kept("no_match"));
    assert!(fx.wt_path.exists() && branch_exists(&fx));

    // Once the default branch has them, the same directory qualifies.
    git_cmd(&fx.main_root, &["merge", "--ff-only", &fx.branch_ref]);
    let report = Pass::new(unchanged()).run(&fx);
    assert_eq!(
        verdict(&report, &fx.id),
        ("removed".into(), "unchanged".into())
    );
}

/// A `gh` that reports the branch's PR as merged at `head_oid`.
#[cfg(unix)]
fn merged_gh(fx: &RepoFixture, head_oid: &str) -> GhClient {
    use std::os::unix::fs::PermissionsExt;
    let branch = fx.branch_ref.trim_start_matches("refs/heads/");
    let path = fx.root.join("fake-gh");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  cat <<EOF
{{"title":"Squash","number":42,"state":"MERGED","isDraft":false,"additions":1,"deletions":0,"statusCheckRollup":[],"reviewDecision":"APPROVED","mergeable":"UNKNOWN","updatedAt":"2026-11-23T03:00:00Z","url":"https://github.com/acme/demo/pull/42","headRefName":"{branch}","headRefOid":"{head_oid}","baseRefName":"main"}}
EOF
  exit 0
fi
exit 1
"#
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    GhClient::with_program(path)
}

#[cfg(unix)]
#[test]
fn a_squash_merged_pr_removes_the_directory_only_at_its_head() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    git_cmd(
        &fx.main_root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    let tip = commit_file(&fx, "feature.txt");
    let rules = Rules {
        merged: true,
        ..Rules::default()
    };

    // A commit after the merged head is new work: the directory stays.
    let later = commit_file(&fx, "more.txt");
    let mut pass = Pass::new(rules);
    pass.gh = merged_gh(&fx, &tip);
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    assert!(fx.wt_path.exists() && branch_exists(&fx));

    pass.gh = merged_gh(&fx, &later);
    let report = pass.run(&fx);
    assert_eq!(
        verdict(&report, &fx.id),
        ("removed".into(), "merged".into())
    );
    assert!(!fx.wt_path.exists());
    assert!(!branch_exists(&fx));
    assert!(read(&fx.root).is_empty());
}

#[test]
fn idle_collapses_the_directory_and_keeps_its_branch_and_row() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    let tip = commit_file(&fx, "feature.txt");

    let early = Pass::new(idle(14)).after_days(8).run(&fx);
    assert_eq!(verdict(&early, &fx.id), kept("no_match"));
    assert!(fx.wt_path.exists());

    let report = Pass::new(idle(7)).after_days(8).run(&fx);
    assert_eq!(
        verdict(&report, &fx.id),
        ("collapsed".into(), "idle".into())
    );
    assert!(!fx.wt_path.exists());
    assert_eq!(git_cmd(&fx.main_root, &["rev-parse", &fx.branch_ref]), tip);
    let row = store::WorktreeStore::new(&fx.root)
        .get(&fx.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.state.as_str(), row.phase.as_str()),
        ("collapsed", "collapsed")
    );
    assert!(row.worktree_git_dir.is_none());

    // The list keeps showing it, and a later pass leaves it alone.
    let dirs = read(&fx.root);
    assert!(dirs[0].collapsed && !dirs[0].ready);
    let items = classify(&dirs, &fx.workspace, &[], &[]);
    assert_eq!(items[0]["status"], "collapsed");
    assert!(
        Pass::new(idle(7)).after_days(30).run(&fx).to_json()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_collapsed_directory_is_deleted_by_hand_with_its_unmerged_commits_listed() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    commit_file(&fx, "feature.txt");
    Pass::new(idle(7)).after_days(8).run(&fx);

    let reader = Reader::new(fx.root.clone());
    let inspected = post(
        &reader,
        "/app/isolated/inspect",
        json!({"id": fx.id, "workspace": fx.workspace}),
    );
    assert_eq!(inspected["status"], "confirm");
    assert_eq!(inspected["state"], "collapsed");
    assert_eq!(inspected["missing_dir"], true);
    assert_eq!(inspected["unique_commits"], true);
    assert_eq!(inspected["unmerged_total"], 1);
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
    assert!(!branch_exists(&fx));
    assert!(read(&fx.root).is_empty());
}

#[test]
fn recent_use_by_a_task_postpones_idle_cleanup() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    let mut pass = Pass::new(idle(7)).after_days(8);
    pass.inputs.last_task.insert(fx.id.clone(), pass.now - DAY);
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    assert!(fx.wt_path.exists());
}

#[test]
fn a_deleted_workspace_collapses_its_directories_only_when_the_history_was_read() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    let rules = Rules {
        workspace_deleted: true,
        ..Rules::default()
    };
    let mut pass = Pass::new(rules);
    // History unreadable: nothing is judged deleted.
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    pass.inputs.workspaces = Some(HashSet::from([fx.workspace.clone()]));
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    pass.inputs.workspaces = Some(HashSet::from(["s-other".to_owned()]));
    assert_eq!(
        verdict(&pass.run(&fx), &fx.id),
        ("collapsed".into(), "workspace_deleted".into())
    );
    assert!(!fx.wt_path.exists() && branch_exists(&fx));
}

#[test]
fn no_rule_on_looks_at_nothing() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    let report = Pass::new(Rules::default()).after_days(400).run(&fx);
    assert!(report.to_json()["entries"].as_array().unwrap().is_empty());
    assert!(fx.wt_path.exists());
}

#[test]
fn local_changes_nested_repositories_and_detached_heads_stay() {
    let all = Rules {
        unchanged: true,
        idle_days: Some(7),
        workspace_deleted: true,
        ..Rules::default()
    };
    let mut pass = Pass::new(all).after_days(30);
    pass.inputs.workspaces = Some(HashSet::new());

    let edited = setup_worktree_fixture("wt-000000000001");
    fs::write(edited.wt_path.join("README.md"), "changed\n").unwrap();
    assert_eq!(verdict(&pass.run(&edited), &edited.id), kept("dirty"));

    let untracked = setup_worktree_fixture("wt-000000000002");
    fs::write(untracked.wt_path.join("new.txt"), "x\n").unwrap();
    assert_eq!(verdict(&pass.run(&untracked), &untracked.id), kept("dirty"));

    let nested = setup_worktree_fixture("wt-000000000003");
    let sub = nested.wt_path.join("packages/sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("lib.rs"), "pub fn ok() {}\n").unwrap();
    git_cmd(&nested.wt_path, &["add", "packages/sub/lib.rs"]);
    git_cmd(&nested.wt_path, &["commit", "-m", "tracked subdir"]);
    git_cmd(&sub, &["init"]);
    assert_eq!(verdict(&pass.run(&nested), &nested.id), kept("nested"));

    let detached = setup_worktree_fixture("wt-000000000004");
    git_cmd(&detached.wt_path, &["checkout", "--detach"]);
    assert_eq!(
        verdict(&pass.run(&detached), &detached.id),
        kept("detached")
    );

    for fx in [&edited, &untracked, &nested, &detached] {
        assert!(fx.wt_path.exists() && branch_exists(fx));
    }
}

#[test]
fn ignored_files_do_not_keep_a_directory() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    fs::create_dir_all(fx.wt_path.join("ignored")).unwrap();
    fs::write(fx.wt_path.join("ignored/build.bin"), b"cache").unwrap();
    let report = Pass::new(unchanged()).run(&fx);
    assert_eq!(
        verdict(&report, &fx.id),
        ("removed".into(), "unchanged".into())
    );
}

#[test]
fn working_leased_recent_and_unready_directories_stay() {
    let rules = Rules {
        unchanged: true,
        ..Rules::default()
    };

    let working = setup_worktree_fixture("wt-000000000001");
    let mut pass = Pass::new(rules);
    pass.terminal_busy = true;
    assert_eq!(verdict(&pass.run(&working), &working.id), kept("busy"));
    pass.terminal_busy = false;
    fs::create_dir_all(working.wt_path.join("src")).unwrap();
    pass.inputs.running.push(working.wt_path.join("src"));
    assert_eq!(verdict(&pass.run(&working), &working.id), kept("busy"));

    let recent = setup_worktree_fixture("wt-000000000002");
    let updated = crate::time_format::timestamp(UPDATED).unwrap();
    let mut pass = Pass::new(rules);
    pass.now = updated + 4 * MINUTE;
    assert_eq!(verdict(&pass.run(&recent), &recent.id), kept("recent"));
    // A task that touched it later counts as use as well.
    let mut pass = Pass::new(rules);
    pass.inputs
        .last_task
        .insert(recent.id.clone(), pass.now - MINUTE);
    assert_eq!(verdict(&pass.run(&recent), &recent.id), kept("recent"));

    let unready = setup_worktree_fixture("wt-000000000003");
    sql(
        &unready,
        "UPDATE isolated_dirs SET state = 'needs_attention'",
    );
    assert_eq!(
        verdict(&Pass::new(rules).run(&unready), &unready.id),
        kept("not_ready")
    );

    let leased = setup_worktree_fixture("wt-000000000004");
    let lease_dir = leased.common_dir.join("agentnave/leases").join(&leased.id);
    fs::create_dir_all(&lease_dir).unwrap();
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lease_dir.join("active.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    assert_eq!(
        verdict(&Pass::new(rules).run(&leased), &leased.id),
        kept("leased")
    );

    for fx in [&working, &recent, &unready, &leased] {
        assert!(fx.wt_path.exists() && branch_exists(fx));
    }
}

#[cfg(unix)]
mod merged_pr {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    const ORIGIN: &str = "https://github.com/acme/demo.git";

    fn merged_rules() -> Rules {
        Rules {
            merged: true,
            ..Rules::default()
        }
    }

    /// A `gh` that reports one merged PR for the managed branch with the given head commit.
    fn fake_gh(fx: &RepoFixture, head_oid: &str) -> GhClient {
        let path: PathBuf = fx.root.parent().unwrap().join("fake-gh");
        let branch = format!("agentnave/{}", fx.id);
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  cat <<EOF
{{"title":"Squash feature","number":123,"state":"MERGED","isDraft":false,"additions":1,"deletions":0,"statusCheckRollup":[],"reviewDecision":"APPROVED","mergeable":"UNKNOWN","updatedAt":"2026-11-23T03:00:00Z","url":"https://github.com/acme/demo/pull/123","headRefName":"{branch}","headRefOid":"{head_oid}","baseRefName":"main"}}
EOF
  exit 0
fi
exit 1
"#
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        GhClient::with_program(path)
    }

    fn with_remote(fx: &RepoFixture) -> String {
        git_cmd(&fx.main_root, &["remote", "add", "origin", ORIGIN]);
        commit_file(fx, "feature.txt")
    }

    #[test]
    fn a_squash_merged_pr_removes_the_directory_and_its_branch() {
        let fx = setup_worktree_fixture("wt-0123456789ab");
        let head = with_remote(&fx);
        let mut pass = Pass::new(merged_rules());
        pass.gh = fake_gh(&fx, &head);
        let report = pass.run(&fx);
        assert_eq!(
            verdict(&report, &fx.id),
            ("removed".into(), "merged".into())
        );
        assert_eq!(report.to_json()["entries"][0]["detail"], "#123");
        assert!(!fx.wt_path.exists() && !branch_exists(&fx));
        assert!(read(&fx.root).is_empty());
    }

    #[test]
    fn a_pr_that_no_longer_matches_the_directory_head_does_not_count() {
        let fx = setup_worktree_fixture("wt-0123456789ab");
        with_remote(&fx);
        let mut pass = Pass::new(merged_rules());
        pass.gh = fake_gh(&fx, &"0".repeat(40));
        assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
        assert!(fx.wt_path.exists() && branch_exists(&fx));
    }

    #[test]
    fn a_repository_without_a_remote_never_counts_as_merged() {
        let fx = setup_worktree_fixture("wt-0123456789ab");
        let head = commit_file(&fx, "feature.txt");
        let mut pass = Pass::new(merged_rules());
        pass.gh = fake_gh(&fx, &head);
        assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
        assert!(fx.wt_path.exists() && branch_exists(&fx));
    }

    #[test]
    fn a_missing_gh_leaves_the_directory_alone() {
        let fx = setup_worktree_fixture("wt-0123456789ab");
        with_remote(&fx);
        let mut pass = Pass::new(merged_rules());
        pass.gh = GhClient::with_program(fx.root.join("no-gh"));
        assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    }
}

#[test]
fn a_recent_commit_in_the_worktree_postpones_idle_cleanup() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    commit_file(&fx, "feature.txt");
    let mut pass = Pass::new(idle(14)).after_days(30);
    // A derived terminal committed a minute ago; neither the registry nor a task shows it.
    pass.reflog_at = Some(pass.now - MINUTE);
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("recent"));
    pass.reflog_at = Some(pass.now - 20 * DAY);
    assert_eq!(
        verdict(&pass.run(&fx), &fx.id),
        ("collapsed".into(), "idle".into())
    );
}

#[test]
fn an_incompletely_read_history_never_counts_as_idle() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    commit_file(&fx, "feature.txt");
    let mut pass = Pass::new(idle(14)).after_days(30);
    pass.inputs.history_unreadable = true;
    assert_eq!(verdict(&pass.run(&fx), &fx.id), kept("no_match"));
    assert!(fx.wt_path.exists());
}

#[test]
fn a_source_file_written_after_the_check_keeps_the_directory_ready() {
    let fx = setup_worktree_fixture("wt-0123456789ab");
    let head = git_cmd(&fx.wt_path, &["rev-parse", "HEAD"]);
    // Appears between the locked inspection and the removal.
    fs::write(fx.wt_path.join("late.rs"), "fn main() {}\n").unwrap();
    let git = crate::isolated::git::GitRunner::new().unwrap();
    let store = store::WorktreeStore::new(&fx.root);
    let record = store.get(&fx.id).unwrap().unwrap();
    let removed = crate::isolated::release::remove_whole_locked(
        &git,
        &store,
        &record,
        &fx.main_root,
        &fx.common_dir,
        &head,
    );
    assert!(
        removed.as_ref().unwrap_err().contains("untracked files"),
        "{removed:?}"
    );
    assert!(fx.wt_path.join("late.rs").exists());
    assert!(branch_exists(&fx));
    assert_eq!(store.get(&fx.id).unwrap().unwrap().state, "ready");
}
