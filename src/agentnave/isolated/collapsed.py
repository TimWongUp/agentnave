"""Release and rollback helpers for collapsed isolated directories (ADR-0115)."""

from __future__ import annotations

from pathlib import Path

from agentnave.isolated.branches import branch_tip, has_unique_commits
from agentnave.isolated.leases import has_active_leases, remove_lease_dir_locked
from agentnave.isolated.release import (
    InspectionSnapshot,
    ReleaseResult,
    compute_digest,
    finish_deletion,
    verify_confirmation,
    verify_repo_identity,
    verify_untouched_worktree_for_rollback,
)
from agentnave.isolated.repo import IsolationError, is_link, run_git
from agentnave.isolated.store import IsolatedRecord, IsolationState, WorktreeStore
from agentnave.store import now

REBUILDING_COLLAPSED_PHASES = ("rebuilding_collapsed", "rebuilding_copying_include")


def release_collapsed_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    workspace_id: str,
    confirmation: str | None,
) -> ReleaseResult:
    """Release a collapsed isolated directory by removing its managed branch and DB row."""
    try:
        main_root, common_dir = verify_repo_identity(record, require_base_oid=False)
        if has_active_leases(common_dir, record.id, cleanup_stale=True):
            raise IsolationError("Isolated directory is currently in use by an active lease")
        wt_path = Path(record.worktree_path)
        if wt_path.exists() or is_link(wt_path):
            raise IsolationError(
                f"Collapsed isolated directory still has a path on disk at {wt_path}"
            )
        run_git(["worktree", "prune"], cwd=main_root)
        snap = _inspect_collapsed_locked(record, workspace_id, main_root)
    except (IsolationError, OSError, ValueError) as exc:
        return ReleaseResult(id=record.id, status="blocked", reason=str(exc))

    foreign = record.workspace_id != workspace_id
    if failure := verify_confirmation(store, record, snap, confirmation, foreign=foreign):
        return failure

    delete_oid = snap.branch_oid or record.base_oid
    if not store.mark_deleting(
        record.id,
        phase="removing_branch",
        delete_oid=delete_oid,
        updated_at=now(),
        expected_state=record.state,
    ):
        return ReleaseResult(
            id=record.id, status="blocked", reason="Isolated directory changed state; retry"
        )
    return _run_collapsed_deletion(
        store,
        record,
        main_root,
        common_dir,
        delete_oid,
        unique_commits=snap.unique_commits,
    )


def _inspect_collapsed_locked(
    record: IsolatedRecord, workspace_id: str, main_root: Path
) -> InspectionSnapshot:
    tip = branch_tip(main_root, record.branch_ref)
    unique = bool(tip) and has_unique_commits(main_root, tip)
    digest = compute_digest(
        record.id,
        workspace_id,
        "collapsed",
        tip or "none",
        record.usage_generation,
        b"collapsed",
        tip.encode("ascii"),
        unique_commits=unique,
    )
    return InspectionSnapshot(
        head_oid="",
        branch_oid=tip,
        uncommitted=False,
        unique_commits=unique,
        digest=digest,
    )


def _run_collapsed_deletion(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_oid: str,
    *,
    unique_commits: bool,
) -> ReleaseResult:
    try:
        note = finish_deletion(store, record, main_root, common_dir, expected_oid)
    except (IsolationError, OSError, ValueError) as exc:
        detail = str(exc)
        store.mark_attention(
            record.id,
            expected_state="deleting",
            phase="delete_failed",
            reason=detail,
            updated_at=now(),
        )
        return ReleaseResult(
            id=record.id,
            status="needs_attention",
            unique_commits=unique_commits,
            reason=detail,
        )
    return ReleaseResult(
        id=record.id,
        status="deleted",
        unique_commits=unique_commits,
        reason=note,
    )


def rollback_rebuilding_collapsed_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    *,
    expected_state: IsolationState = "creating",
) -> None:
    """Roll back an unfinished collapsed rebuild without deleting its branch or DB row."""
    try:
        main_root, common_dir = verify_repo_identity(record, require_base_oid=False)
        if has_active_leases(common_dir, record.id, cleanup_stale=True):
            raise IsolationError("Active lease exists on unfinished isolated directory")
        wt_path = Path(record.worktree_path)
        if is_link(wt_path):
            raise IsolationError(f"Unsafe link at {wt_path}")
        tip = branch_tip(main_root, record.branch_ref)
        if verify_untouched_worktree_for_rollback(wt_path, record, main_root, common_dir, tip):
            run_git(["worktree", "remove", "--force", "--", str(wt_path)], cwd=main_root)
        else:
            run_git(["worktree", "prune"], cwd=main_root)
        store.revert_creating_to_collapsed(
            record.id, updated_at=now(), expected_state=expected_state
        )
        remove_lease_dir_locked(common_dir, record.id)
    except (IsolationError, OSError, ValueError) as exc:
        store.mark_attention(
            record.id,
            expected_state=expected_state,
            phase="rollback_failed",
            reason=str(exc),
            updated_at=now(),
        )
