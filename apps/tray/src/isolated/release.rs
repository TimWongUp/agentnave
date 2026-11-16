//! Explicit deletion and interrupted-release recovery for isolated directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::git::{GitRunner, WORKTREES_DIR_NAME, never_checked_out, parse_worktrees, ref_exists};
use super::inspect::{
    STATUS_ARGS, inspect_record, verify_managed_branch_ref, verify_repo_identity,
    verify_worktree_container,
};
use super::leases::{
    acquire_repo_lock, has_active_leases, is_link, remove_lease_dir_locked, same_dir,
};
use super::state::{foreign_link_valid, remove_tree, worktree_link_valid};
use super::store::{Record, WorktreeStore, now_iso};

const REPO_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

fn outcome(id: &str, status: &str, reason: &str) -> Value {
    json!({"id": id, "status": status, "reason": reason, "message": reason})
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn remove_managed_branch(
    git: &GitRunner,
    record: &Record,
    main_root: &Path,
    expected_oid: &str,
) -> Result<Option<String>, String> {
    verify_managed_branch_ref(record)?;
    if !ref_exists(git, main_root, &record.branch_ref)? {
        return Ok(None);
    }
    let remaining = parse_worktrees(
        &git.run(
            main_root,
            &["worktree", "list", "--porcelain", "-z"],
            &[0],
            &[],
        )?
        .stdout,
    );
    if remaining
        .iter()
        .any(|w| w.branch_ref.as_deref() == Some(record.branch_ref.as_str()))
    {
        return Err("受管分支仍在另一个 worktree 中检出".to_owned());
    }
    let target = format!("{}^{{commit}}", record.branch_ref);
    let current = String::from_utf8_lossy(
        git.run(
            main_root,
            &["rev-parse", "--verify", "--end-of-options", &target],
            &[0],
            &[],
        )?
        .stdout
        .trim_ascii(),
    )
    .into_owned();
    if current != expected_oid {
        let short = &current[..current.len().min(12)];
        return Ok(Some(format!(
            "受管分支 {} 已保留：检查后分支移动到了 {short}，因此未删除。",
            record.branch()
        )));
    }
    git.run(
        main_root,
        &[
            "update-ref",
            "--no-deref",
            "-d",
            &record.branch_ref,
            expected_oid,
        ],
        &[0],
        &[],
    )?;
    Ok(None)
}

fn complete_deletion_locked(
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    main_root: &Path,
    common_dir: &Path,
    expected_oid: &str,
    remove_orphan: bool,
) -> Result<Option<String>, String> {
    if has_active_leases(common_dir, &record.id)? {
        return Err("存在活跃租约，不能完成删除".to_owned());
    }
    let wt_path = PathBuf::from(&record.worktree_path);
    if remove_orphan && (wt_path.exists() || is_link(&wt_path)) {
        remove_tree(&wt_path, &main_root.join(WORKTREES_DIR_NAME))?;
        git.run(main_root, &["worktree", "prune"], &[0], &[])?;
    } else {
        if !wt_path.exists() && !is_link(&wt_path) {
            git.run(main_root, &["worktree", "prune"], &[0], &[])?;
        }
        let worktrees = parse_worktrees(
            &git.run(
                main_root,
                &["worktree", "list", "--porcelain", "-z"],
                &[0],
                &[],
            )?
            .stdout,
        );
        let matches: Vec<_> = worktrees
            .iter()
            .filter(|w| {
                (w.path.exists() && wt_path.exists() && same_dir(&w.path, &wt_path))
                    || fs::canonicalize(&w.path).ok() == fs::canonicalize(&wt_path).ok()
                        && fs::canonicalize(&wt_path).is_ok()
            })
            .collect();
        if let Some(entry) = matches.first() {
            if entry.branch_ref.as_deref() != Some(record.branch_ref.as_str())
                || entry.head_oid.as_deref() != Some(expected_oid)
            {
                return Err("删除过程中 worktree 的分支或 HEAD 发生变化".to_owned());
            }
            if is_link(&wt_path) || is_link(&main_root.join(WORKTREES_DIR_NAME)) {
                return Err(format!("删除前发现不安全链接：{}", wt_path.display()));
            }
            let wt_str = wt_path.to_string_lossy();
            git.run(
                main_root,
                &["worktree", "remove", "--force", "--", wt_str.as_ref()],
                &[0],
                &[],
            )?;
        } else if wt_path.exists() || is_link(&wt_path) {
            return Err(format!(
                "路径 {} 仍存在但未在 Git worktree 中登记，拒绝递归删除",
                wt_path.display()
            ));
        }
    }
    store.update_phase(&record.id, "removing_branch", &now_iso())?;
    let note = remove_managed_branch(git, record, main_root, expected_oid)?;
    store.delete(&record.id, "deleting")?;
    remove_lease_dir_locked(common_dir, &record.id)?;
    Ok(note)
}

fn verify_rollback_worktree(
    git: &GitRunner,
    record: &Record,
    main_root: &Path,
    common_dir: &Path,
    wt_path: &Path,
) -> Result<(), String> {
    verify_worktree_container(wt_path, main_root, &record.id)?;
    if !wt_path.exists() && !is_link(wt_path) {
        git.run(main_root, &["worktree", "prune"], &[0], &[])?;
    }
    let worktrees = parse_worktrees(
        &git.run(
            main_root,
            &["worktree", "list", "--porcelain", "-z"],
            &[0],
            &[],
        )?
        .stdout,
    );
    let matches: Vec<_> = worktrees
        .iter()
        .filter(|w| w.path.exists() && wt_path.exists() && same_dir(&w.path, wt_path))
        .collect();
    if let Some(entry) = matches.first() {
        if entry.branch_ref.as_deref() != Some(record.branch_ref.as_str())
            || entry.head_oid.as_deref() != Some(record.base_oid.as_str())
        {
            return Err("未完成的 worktree 分支或 HEAD 已偏离起点".to_owned());
        }
        if wt_path.is_dir() {
            let linked = if record.worktree_git_dir.is_some() {
                worktree_link_valid(wt_path, record.worktree_git_dir.as_deref(), common_dir)
            } else {
                foreign_link_valid(wt_path)
            };
            if !linked {
                return Err("未完成的目录不是有效的 git worktree".to_owned());
            }
            if never_checked_out(git, wt_path)? {
                let has_other = fs::read_dir(wt_path)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .any(|e| e.file_name() != ".git");
                if has_other {
                    return Err("未检出的 worktree 目录里已有额外文件".to_owned());
                }
            } else if !git.run(wt_path, STATUS_ARGS, &[0], &[])?.stdout.is_empty() {
                return Err("未完成的 worktree 包含未提交的源码改动".to_owned());
            }
        }
        let wt_str = wt_path.to_string_lossy();
        git.run(
            main_root,
            &["worktree", "remove", "--force", "--", wt_str.as_ref()],
            &[0],
            &[],
        )?;
    } else if wt_path.exists() {
        return Err(format!("路径 {} 存在未登记目录", wt_path.display()));
    }
    Ok(())
}

fn rollback_creating_locked(git: &GitRunner, store: &WorktreeStore, record: &Record) -> Value {
    let attempt = || -> Result<(), String> {
        verify_managed_branch_ref(record)?;
        let (main_root, common_dir) = verify_repo_identity(git, record)?;
        if has_active_leases(&common_dir, &record.id)? {
            return Err("未完成创建的隔离目录仍有活跃租约".to_owned());
        }
        let wt_path = PathBuf::from(&record.worktree_path);
        if is_link(&wt_path) {
            return Err(format!("存在不安全链接：{}", wt_path.display()));
        }
        verify_rollback_worktree(git, record, &main_root, &common_dir, &wt_path)?;
        if ref_exists(git, &main_root, &record.branch_ref)? {
            let remaining = parse_worktrees(
                &git.run(
                    &main_root,
                    &["worktree", "list", "--porcelain", "-z"],
                    &[0],
                    &[],
                )?
                .stdout,
            );
            if remaining
                .iter()
                .any(|w| w.branch_ref.as_deref() == Some(record.branch_ref.as_str()))
            {
                return Err("受管分支仍在另一个 worktree 中检出".to_owned());
            }
            git.run(
                &main_root,
                &[
                    "update-ref",
                    "--no-deref",
                    "-d",
                    &record.branch_ref,
                    &record.base_oid,
                ],
                &[0],
                &[],
            )?;
        }
        store.delete(&record.id, "creating")?;
        remove_lease_dir_locked(&common_dir, &record.id)?;
        Ok(())
    };
    match attempt() {
        Ok(()) => json!({"id": record.id, "status": "deleted", "message": "已删除隔离目录"}),
        Err(detail) => {
            let _ = store.mark_attention(
                &record.id,
                "creating",
                "rollback_failed",
                &detail,
                &now_iso(),
            );
            outcome(&record.id, "needs_attention", &detail)
        }
    }
}

fn with_locked_record(
    root: &Path,
    isolation_id: &str,
    workspace_id: &str,
    op: impl FnOnce(&GitRunner, &WorktreeStore, &Record) -> Value,
) -> Value {
    let store = WorktreeStore::new(root);
    let initial = match store.get(isolation_id) {
        Ok(Some(rec)) if rec.workspace_id == workspace_id => rec,
        Ok(_) => return outcome(isolation_id, "not_found", "隔离目录已不存在"),
        Err(e) => return outcome(isolation_id, "blocked", &e),
    };
    let common_dir = PathBuf::from(&initial.repo_common_dir);
    if is_link(&common_dir) || !common_dir.is_dir() {
        return outcome(
            isolation_id,
            "blocked",
            &format!("Git 公共目录不可用：{}", common_dir.display()),
        );
    }
    let _lock = match acquire_repo_lock(&common_dir, REPO_LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(e) => return outcome(isolation_id, "blocked", &e),
    };
    let record = match store.get(isolation_id) {
        Ok(Some(rec)) if rec.workspace_id == workspace_id => rec,
        Ok(_) => return outcome(isolation_id, "not_found", "隔离目录已不存在"),
        Err(e) => return outcome(isolation_id, "blocked", &e),
    };
    match has_active_leases(&common_dir, isolation_id) {
        Ok(true) => return outcome(isolation_id, "in_use", "隔离目录正在被任务使用，不能删除"),
        Ok(false) => {}
        Err(e) => return outcome(isolation_id, "blocked", &e),
    }
    let git = match GitRunner::new() {
        Ok(git) => git,
        Err(e) => return outcome(isolation_id, "blocked", &e),
    };
    op(&git, &store, &record)
}

/// Inspect an isolated directory under `repo_lock`, persist its confirmation digest, and return
/// details for the confirmation dialog.
pub(crate) fn inspect(root: &Path, isolation_id: &str, workspace_id: &str) -> Value {
    with_locked_record(root, isolation_id, workspace_id, |git, store, record| {
        let (_, snap) = match inspect_record(git, store, record, workspace_id) {
            Ok(res) => res,
            Err(e) => return outcome(&record.id, "blocked", &e),
        };
        if let Err(e) = store.save_confirmation(&record.id, &snap.digest, &now_iso()) {
            return outcome(&record.id, "blocked", &e);
        }
        snap.to_confirm_json("confirm", record, None)
    })
}

/// Delete an isolated directory after verifying the confirmation digest under `repo_lock`.
pub(crate) fn delete(
    root: &Path,
    isolation_id: &str,
    workspace_id: &str,
    confirmation: &str,
) -> Value {
    with_locked_record(root, isolation_id, workspace_id, |git, store, record| {
        let (repo_dirs, snap) = match inspect_record(git, store, record, workspace_id) {
            Ok(res) => res,
            Err(e) => return outcome(&record.id, "blocked", &e),
        };
        let issued = record.confirmation_digest.as_deref();
        let valid = issued.is_some_and(|saved| {
            constant_time_eq(confirmation, saved) && constant_time_eq(confirmation, &snap.digest)
        });
        if !valid {
            if let Err(e) = store.save_confirmation(&record.id, &snap.digest, &now_iso()) {
                return outcome(&record.id, "blocked", &e);
            }
            return snap.to_confirm_json(
                "confirmation_required",
                record,
                Some("目录状态已变化或确认已过期，请核对最新情况后再次确认"),
            );
        }
        if snap.rollback_only {
            match store.resume_creating(&record.id, "rolling_back", &now_iso()) {
                Ok(true) => {
                    return match store.get(&record.id) {
                        Ok(Some(creating)) => rollback_creating_locked(git, store, &creating),
                        Ok(None) => {
                            json!({"id": record.id, "status": "deleted", "message": "已删除隔离目录"})
                        }
                        Err(e) => outcome(&record.id, "needs_attention", &e),
                    };
                }
                Ok(false) => {
                    return outcome(&record.id, "blocked", "隔离目录状态已变化，请重试");
                }
                Err(e) => return outcome(&record.id, "blocked", &e),
            }
        }
        let Some((main_root, common_dir)) = repo_dirs else {
            return outcome(&record.id, "blocked", "缺少仓库上下文");
        };
        let delete_oid = if snap.orphan {
            record
                .delete_oid
                .as_deref()
                .filter(|s| !s.is_empty())
                .or((!snap.branch_oid.is_empty()).then_some(snap.branch_oid.as_str()))
                .unwrap_or(&record.base_oid)
                .to_owned()
        } else {
            snap.branch_oid.clone()
        };
        match store.mark_deleting(
            &record.id,
            "removing_worktree",
            &delete_oid,
            &now_iso(),
            &record.state,
        ) {
            Ok(true) => {}
            Ok(false) => return outcome(&record.id, "blocked", "隔离目录状态已变化，请重试"),
            Err(e) => return outcome(&record.id, "blocked", &e),
        }
        match complete_deletion_locked(
            git,
            store,
            record,
            &main_root,
            &common_dir,
            &delete_oid,
            snap.orphan,
        ) {
            Ok(note) => json!({
                "id": record.id,
                "status": "deleted",
                "reason": note,
                "message": note.unwrap_or_else(|| "已删除隔离目录".to_owned()),
            }),
            Err(detail) => {
                let _ = store.mark_attention(
                    &record.id,
                    "deleting",
                    "delete_failed",
                    &detail,
                    &now_iso(),
                );
                outcome(&record.id, "needs_attention", &detail)
            }
        }
    })
}
