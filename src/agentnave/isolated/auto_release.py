"""Auto-release of untouched isolated worktrees and in-place rebuild on resume (ADR-0092)."""

from __future__ import annotations

import os
from dataclasses import dataclass, replace
from pathlib import Path, PurePosixPath

from agentnave.isolated.copy import copy_worktreeinclude
from agentnave.isolated.leases import (
    IsolationLease,
    RepoLockUnavailable,
    acquire_lease_locked,
    repo_lock,
)
from agentnave.isolated.release import (
    ChangeSummary,
    complete_deletion_locked,
    inspect_ready_locked,
    rollback_creating_locked,
    summarize_record_changes,
)
from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    RepoContext,
    add_worktree_locked,
    check_tree_and_filters,
    control_char_in,
    discover_repo,
    ensure_info_exclude_locked,
    has_active_hooks,
    is_link,
    ref_exists,
    resolve_mapped_cwd,
    run_git,
)
from agentnave.isolated.store import (
    ID_PATTERN,
    IsolatedRecord,
    WorktreeStore,
    local_machine_id,
)
from agentnave.store import now


@dataclass(frozen=True, slots=True)
class RebuildPlan:
    ctx: RepoContext
    wt_path: Path
    subdir_prefix: PurePosixPath
    include_source: Path
    warnings: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class RebuiltWorktree:
    worktree_path: Path
    execution_cwd: Path
    branch: str
    branch_ref: str
    main_root: Path
    source_root: Path
    repo_common_dir: Path
    base_oid: str
    warnings: tuple[str, ...]
    lease: IsolationLease


def _auto_release_warning(isolation_id: str, exc: object) -> str:
    return (
        f"Could not auto-release untouched isolated directory {isolation_id}: {exc}. "
        "It was kept; use isolated_dirs to inspect or release it later."
    )


def try_auto_release(
    store: WorktreeStore,
    isolation_id: str,
    *,
    workspace_id: str,
    lease: IsolationLease,
    summary: ChangeSummary,
) -> tuple[ChangeSummary, list[str]]:
    """Delete an untouched, unleased isolated directory owned by workspace_id; retain on failure."""
    try:
        if (
            summary.summary_status != "complete"
            or summary.changed_files_from_base
            or summary.uncommitted
            or summary.nested_repos
        ):
            return summary, []
        try:
            record = store.get(isolation_id)
        except (OSError, ValueError) as exc:
            return summary, [_auto_release_warning(isolation_id, exc)]
        if record is None or record.state != "ready" or record.workspace_id != workspace_id:
            return summary, []
        if summary.head_oid != record.base_oid:
            return summary, []
        common_dir = Path(record.repo_common_dir)
        if is_link(common_dir) or not common_dir.is_dir():
            return summary, [
                _auto_release_warning(
                    isolation_id, f"Repository common-dir is unavailable: {common_dir}"
                )
            ]
        try:
            with repo_lock(common_dir):
                lease.release()
                return _auto_release_locked(store, isolation_id, workspace_id, summary)
        except (RepoLockUnavailable, OSError, ValueError) as exc:
            return summary, [_auto_release_warning(isolation_id, exc)]
    finally:
        if not lease.closed:
            lease.release()


def _auto_release_locked(
    store: WorktreeStore,
    isolation_id: str,
    workspace_id: str,
    summary: ChangeSummary,
) -> tuple[ChangeSummary, list[str]]:
    latest = store.get(isolation_id)
    if latest is None or latest.state != "ready" or latest.workspace_id != workspace_id:
        return summary, []
    try:
        main_root, verified_common, _wt_path, snap = inspect_ready_locked(
            store, latest, workspace_id
        )
    except IsolationError as exc:
        fresh = summarize_record_changes(latest)
        if fresh.summary_status == "error":
            return fresh, [_auto_release_warning(isolation_id, exc)]
        return fresh, []
    except (OSError, ValueError) as exc:
        return summary, [_auto_release_warning(isolation_id, exc)]

    if (
        snap.head_oid != latest.base_oid
        or snap.branch_oid != latest.base_oid
        or snap.uncommitted
        or snap.nested
    ):
        return summarize_record_changes(latest), []
    if not store.mark_deleting(
        latest.id,
        phase="removing_worktree",
        delete_oid=snap.branch_oid,
        updated_at=now(),
        expected_state="ready",
    ):
        return summarize_record_changes(latest), []
    try:
        note = complete_deletion_locked(store, latest, main_root, verified_common, snap.branch_oid)
    except (IsolationError, OSError, ValueError) as exc:
        store.mark_attention(
            latest.id,
            expected_state="deleting",
            phase="delete_failed",
            reason=str(exc),
            updated_at=now(),
        )
        return summary, [_auto_release_warning(isolation_id, exc)]
    warnings = [note] if note else []
    return replace(summary, auto_released=True), warnings


def plan_rebuild(
    *,
    isolation_id: str,
    worktree_path: Path,
    cwd: Path,
    base_oid: str,
    main_root: Path,
    source_root: Path | None,
) -> RebuildPlan:
    """Validate preconditions and collect non-locking repository checks before rebuilding."""
    if not ID_PATTERN.fullmatch(isolation_id):
        raise IsolationError(f"Invalid isolated directory id: {isolation_id!r}")
    if is_link(main_root) or not main_root.is_dir():
        raise IsolationError(f"Main worktree is missing or unsafe: {main_root}")
    ctx = discover_repo(main_root)
    for path in (cwd, ctx.main_root):
        if control_char_in(str(path)):
            raise IsolationError(
                "A repository path contains control characters (such as a newline); "
                "the isolated directory cannot be rebuilt"
            )
    wt_path = (ctx.main_root / WORKTREES_DIR_NAME / isolation_id).resolve()
    if os.path.normcase(str(wt_path)) != os.path.normcase(str(worktree_path.resolve())):
        raise IsolationError(
            f"Recorded worktree path {worktree_path} no longer matches repository layout {wt_path}"
        )
    cwd_resolved = cwd.resolve()
    if not cwd_resolved.is_relative_to(wt_path):
        raise IsolationError(f"Target cwd {cwd} is outside isolated worktree {wt_path}")
    rel = cwd_resolved.relative_to(wt_path)
    subdir_prefix = PurePosixPath(rel.as_posix()) if rel != Path(".") else PurePosixPath()

    try:
        run_git(
            ["cat-file", "-e", f"{base_oid}^{{commit}}"],
            cwd=ctx.main_root,
            extra_env={"GIT_NO_LAZY_FETCH": "1"},
        )
    except IsolationError as exc:
        raise IsolationError(
            f"Original commit {base_oid[:12]} for isolated directory {isolation_id} is no longer "
            "available in the repository; start a new thread with isolation instead."
        ) from exc

    lfs_detected = check_tree_and_filters(ctx, base_oid)
    warnings: list[str] = []
    hooks_skipped = has_active_hooks(ctx.main_root)
    if hooks_skipped and lfs_detected:
        warnings.append("Git hooks and LFS downloads were skipped; LFS files may contain pointers.")
    elif hooks_skipped:
        warnings.append("Git hooks were skipped when rebuilding the isolated directory.")
    elif lfs_detected:
        warnings.append("Git LFS downloads were skipped; LFS files may contain pointers.")

    include_source = ctx.main_root
    if source_root is not None and not control_char_in(str(source_root)):
        resolved_source = source_root.resolve()
        if not is_link(resolved_source) and resolved_source.is_dir():
            include_source = resolved_source
        elif os.path.normcase(str(resolved_source)) != os.path.normcase(str(ctx.main_root)):
            warnings.append(
                f"Original .worktreeinclude source ({source_root}) is no longer present; "
                "copied ignored files from main worktree instead."
            )
    return RebuildPlan(
        ctx=ctx,
        wt_path=wt_path,
        subdir_prefix=subdir_prefix,
        include_source=include_source,
        warnings=tuple(warnings),
    )


def rebuild_worktree_locked(
    store: WorktreeStore,
    plan: RebuildPlan,
    *,
    isolation_id: str,
    base_oid: str,
    workspace_id: str,
) -> RebuiltWorktree:
    """Create the worktree at the recorded id, path, branch, and base_oid under repo_lock."""
    ctx = plan.ctx
    wt_path = plan.wt_path
    branch_name = f"agentnave/{isolation_id}"
    branch_ref = f"refs/heads/{branch_name}"
    if wt_path.exists() or is_link(wt_path):
        raise IsolationError(
            f"Cannot rebuild isolated directory {isolation_id}: path already exists at {wt_path}"
        )
    if ref_exists(ctx.main_root, branch_ref):
        raise IsolationError(
            f"Cannot rebuild isolated directory {isolation_id}: branch {branch_name} already exists"
        )
    ensure_info_exclude_locked(ctx.main_root)
    provisional_cwd = (
        wt_path.joinpath(*plan.subdir_prefix.parts) if plan.subdir_prefix.parts else wt_path
    )
    timestamp = now()
    creating_record = IsolatedRecord(
        id=isolation_id,
        workspace_id=workspace_id,
        machine_id=local_machine_id(),
        repo_common_dir=str(ctx.repo_common_dir),
        repo_identity=ctx.repo_identity,
        main_root=str(ctx.main_root),
        worktree_path=str(wt_path),
        worktree_git_dir=None,
        execution_cwd=str(provisional_cwd),
        branch_ref=branch_ref,
        base_oid=base_oid,
        state="creating",
        phase="adding_worktree",
        usage_generation=0,
        created_at=timestamp,
        updated_at=timestamp,
    )
    store.insert_creating(creating_record)
    lease: IsolationLease | None = None
    try:
        wt_git_dir = add_worktree_locked(ctx.main_root, branch_name, wt_path, base_oid)
        store.mark_worktree_added(
            isolation_id,
            str(wt_git_dir),
            phase="copying_include",
            updated_at=now(),
        )
        copy_worktreeinclude(plan.include_source, wt_path)
        mapped_cwd = resolve_mapped_cwd(wt_path, plan.subdir_prefix)
        lease = acquire_lease_locked(ctx.repo_common_dir, isolation_id)
        store.mark_ready(isolation_id, usage_generation=1, updated_at=now())
        return RebuiltWorktree(
            worktree_path=wt_path.resolve(),
            execution_cwd=mapped_cwd,
            branch=branch_name,
            branch_ref=branch_ref,
            main_root=ctx.main_root,
            source_root=plan.include_source,
            repo_common_dir=ctx.repo_common_dir,
            base_oid=base_oid,
            warnings=plan.warnings,
            lease=lease,
        )
    except BaseException:
        if lease is not None:
            lease.release()
        latest = store.get(isolation_id) or creating_record
        rollback_creating_locked(store, latest)
        raise
