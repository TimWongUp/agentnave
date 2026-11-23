//! Decide for one directory and act on it under the repository lock.

use std::path::Path;

use serde_json::Value;

use super::report::{Entry, Keep, Outcome, Rule};
use super::{DAY_MICROS, Env, GRACE_MICROS, Record};
use crate::git_pr::git::{count_commits_ahead, resolve_default_branch, resolve_remote_name};
use crate::isolated::git::GitRunner;
use crate::isolated::inspect::inspect_record;
use crate::isolated::release::{collapse_locked, remove_whole_locked, with_locked_record};
use crate::isolated::store::WorktreeStore;

/// A merged PR found before the lock is taken (the GitHub call can be slow); it only counts
/// while the directory is still at the commit it was found for.
struct PrHit {
    head_oid: String,
    number: u64,
}

/// What the cheap checks outside the lock found.
struct Facts {
    pr: Option<PrHit>,
    workspace_deleted: bool,
}

type Decision = (Outcome, Option<String>);

fn keep(why: Keep) -> Decision {
    (Outcome::Kept(why), None)
}

/// `None` when the directory vanished while the pass ran.
pub(super) fn clean_one(env: &Env, record: &Record) -> Option<Entry> {
    let kept = |why| Some(Entry::new(record, Outcome::Kept(why), None));
    if record.state != "ready" {
        return kept(Keep::NotReady);
    }
    if env.busy(record) {
        return kept(Keep::Busy);
    }
    if let Some(why) = in_use(env, record) {
        return kept(why);
    }
    let facts = Facts {
        pr: if env.rules.merged {
            merged_pr(env, record)
        } else {
            None
        },
        workspace_deleted: env.rules.workspace_deleted
            && env
                .inputs
                .workspaces
                .as_ref()
                .is_some_and(|known| !known.contains(&record.workspace_id)),
    };
    if !env.rules.unchanged && facts.pr.is_none() && !idle(env, record) && !facts.workspace_deleted
    {
        return kept(Keep::NoMatch);
    }
    let mut decided = None;
    let early = with_locked_record(
        env.root,
        &record.id,
        &record.workspace_id,
        |git, store, locked| {
            decided = Some(decide_locked(env, git, store, locked, &facts));
            Value::Null
        },
    );
    match decided {
        Some((outcome, detail)) => Some(Entry::new(record, outcome, detail)),
        None => early_exit(record, &early),
    }
}

/// A running task, an open terminal or a CLI there, or use within the grace period. Checked
/// before the slow `gh` call and again under the lock, since a terminal can open meanwhile.
fn in_use(env: &Env, record: &Record) -> Option<Keep> {
    if env.busy(record) {
        return Some(Keep::Busy);
    }
    (env.now - env.last_used(record) < GRACE_MICROS).then_some(Keep::Recent)
}

/// Unused for the chosen number of days. A history that could not be read in full may hide a
/// recent task, so it never counts as idle.
fn idle(env: &Env, record: &Record) -> bool {
    !env.inputs.history_unreadable
        && env
            .rules
            .idle_days
            .is_some_and(|days| env.now - env.last_used(record) >= i64::from(days) * DAY_MICROS)
}

/// The lock could not be taken or the directory is leased: with_locked_record answered itself.
fn early_exit(record: &Record, answer: &Value) -> Option<Entry> {
    let (why, detail) = match answer["status"].as_str() {
        Some("in_use") => (Keep::Leased, None),
        Some("blocked") => (Keep::Locked, answer["message"].as_str().map(str::to_owned)),
        _ => return None,
    };
    Some(Entry::new(record, Outcome::Kept(why), detail))
}

/// Under the lock: verify the directory afresh, pick the rule that applies and act.
fn decide_locked(
    env: &Env,
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    facts: &Facts,
) -> Decision {
    if record.state != "ready" {
        return keep(Keep::NotReady);
    }
    if let Some(why) = in_use(env, record) {
        return keep(why);
    }
    let (repo_dirs, snap) = match inspect_record(git, store, record, &record.workspace_id) {
        Ok(found) => found,
        Err(error) if error.contains("detached HEAD") => return keep(Keep::Detached),
        Err(error) => return (Outcome::Kept(Keep::Unverified), Some(error)),
    };
    if snap.orphan {
        return keep(Keep::Orphan);
    }
    if snap.uncommitted {
        return keep(Keep::Dirty);
    }
    if !snap.nested.is_empty() || snap.nested_truncated {
        return keep(Keep::Nested);
    }
    let Some((main_root, common_dir)) = repo_dirs.clone() else {
        return keep(Keep::Unverified);
    };
    let merged = facts
        .pr
        .as_ref()
        .filter(|hit| hit.head_oid == snap.head_oid);
    let integrated = env.rules.unchanged
        && integrated_into_default(git, &main_root, record.branch(), &snap.head_oid) == Some(true);
    // The first two rules find the work already in the default branch (or its merged PR), so
    // branch and registry row go too; the others keep the branch for a later resume.
    let (rule, whole) = if merged.is_some() {
        (Rule::Merged, true)
    } else if integrated {
        (Rule::Unchanged, true)
    } else if idle(env, record) {
        (Rule::Idle, false)
    } else if facts.workspace_deleted {
        (Rule::WorkspaceDeleted, false)
    } else if facts.pr.is_some() {
        return keep(Keep::Changed);
    } else {
        return keep(Keep::NoMatch);
    };
    let detail = merged.map(|hit| format!("#{}", hit.number));
    let done = if whole {
        remove_whole_locked(git, store, record, &main_root, &common_dir, &snap.head_oid)
            .map(|()| (Outcome::Removed(rule), detail))
    } else {
        collapse_locked(git, store, record, &main_root, &common_dir, &snap.head_oid)
            .map(|()| (Outcome::Collapsed(rule), None))
    };
    done.unwrap_or_else(|error| failed(rule, &Value::String(error)))
}

fn failed(rule: Rule, message: &Value) -> Decision {
    (Outcome::Failed(rule), message.as_str().map(str::to_owned))
}

/// A merged PR for the directory's branch at its tip, from GitHub; none without a GitHub
/// remote, without `gh` or when the PR is not merged.
fn merged_pr(env: &Env, record: &Record) -> Option<PrHit> {
    let git = GitRunner::new().ok()?;
    let main_root = Path::new(&record.main_root);
    let target = format!("{}^{{commit}}", record.branch_ref);
    let tip = git
        .run(
            main_root,
            &["rev-parse", "--verify", "--end-of-options", &target],
            &[0],
            &[],
        )
        .ok()?;
    let head_oid = String::from_utf8_lossy(tip.stdout.trim_ascii()).into_owned();
    let summary = crate::git_pr::inspect_merged_pr_for_isolated(
        &git,
        env.gh,
        main_root,
        record.branch(),
        &head_oid,
    )?;
    Some(PrHit {
        head_oid,
        number: summary.number,
    })
}

/// Whether `head_oid` is already part of the repository's default branch.
fn integrated_into_default(
    git: &GitRunner,
    main_root: &Path,
    branch: &str,
    head_oid: &str,
) -> Option<bool> {
    let remote = resolve_remote_name(git, main_root, Some(branch));
    let (_, default_ref) = resolve_default_branch(git, main_root, &remote)?;
    count_commits_ahead(git, main_root, &default_ref, head_oid).map(|ahead| ahead == 0)
}
