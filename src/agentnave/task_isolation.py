"""Isolated directory acquisition and hand-back for one private task (ADR-0085)."""

from __future__ import annotations

import logging
from dataclasses import dataclass, replace
from pathlib import Path

from agentnave.isolated import (
    WORKTREES_DIR_NAME,
    ClaimedIsolation,
    CreatedIsolation,
    IsolatedDirs,
    IsolationContext,
    IsolationLease,
    isolation_instructions,
)
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.isolated.repo import is_same_or_subpath
from agentnave.models import InvocationRequest
from agentnave.store import TaskRecord


@dataclass
class IsolationHold:
    """The isolated directory a task holds from before launch until its run settles."""

    request: InvocationRequest
    created: CreatedIsolation | None = None
    claimed: ClaimedIsolation | None = None
    info: TaskIsolationInfo | None = None
    warnings: tuple[str, ...] = ()
    consumed: bool = False  # the run owns the lease from here on
    owns_isolation: bool = False
    rebuilt: bool = False

    @property
    def lease(self) -> IsolationLease | None:
        if self.created is not None:
            return self.created.lease
        return self.claimed.lease if self.claimed is not None else None


def may_touch_isolated_dirs(cwd: Path, *, requested: bool) -> bool:
    """False when acquiring can be skipped without reading any registry or repository."""
    if requested:
        return True
    return any(part.lower() == WORKTREES_DIR_NAME for part in cwd.resolve().parts)


def acquire_isolation(
    isolated: IsolatedDirs,
    request: InvocationRequest,
    workspace_id: str | None,
    *,
    requested: bool,
    ref: str | None,
    resumed_task: TaskRecord | None = None,
) -> IsolationHold:
    """Create a new isolated directory or lease the one cwd already lies in (blocking)."""
    if requested:
        assert workspace_id is not None
        created = isolated.create(request.cwd, workspace_id=workspace_id, ref=ref)
        return hold_from_created(request, created, owns_isolation=True)
    notes: list[str] = []
    claimed = isolated.claim_for_cwd(request.cwd, notes)
    expected = resumed_isolation_id(request.cwd, resumed_task)
    if expected is not None and (claimed is None or claimed.id != expected):
        if claimed is not None:
            claimed.lease.release()
        raise ValueError(
            f"{request.cwd} is no longer isolated directory {expected}; the path was replaced "
            "after it was released. Start a new task with isolation instead."
        )
    if claimed is None:
        return IsolationHold(request, warnings=tuple(notes))
    owns = (
        resumed_task is not None
        and resumed_task.isolation_owned
        and resumed_task.isolation_id == claimed.id
    )
    source_root = (
        Path(resumed_task.isolation_source_root)
        if resumed_task is not None and resumed_task.isolation_source_root
        else claimed.main_root
    )
    fallback_base_oid = resumed_task.isolation_base_oid if resumed_task is not None else None
    return hold_from_claimed(
        request,
        claimed,
        notes,
        source_root=source_root,
        owns_isolation=owns,
        fallback_base_oid=fallback_base_oid,
    )


def hold_from_created(
    request: InvocationRequest,
    created: CreatedIsolation,
    *,
    owns_isolation: bool = True,
    rebuilt: bool = False,
) -> IsolationHold:
    context = IsolationContext(
        id=created.id,
        worktree_path=created.worktree_path,
        branch=created.branch,
        main_root=created.main_root,
    )
    info = TaskIsolationInfo(
        id=created.id,
        path=str(created.worktree_path),
        cwd=str(created.execution_cwd),
        branch=created.branch,
        repo_common_dir=created.repo_common_dir,
        base_oid=created.base_oid,
        warnings=created.warnings,
        main_root=created.main_root,
        source_root=created.source_root or created.main_root,
    )
    return _hold(
        request,
        context,
        created.execution_cwd,
        info,
        created=created,
        owns_isolation=owns_isolation,
        rebuilt=rebuilt,
    )


def hold_from_claimed(
    request: InvocationRequest,
    claimed: ClaimedIsolation,
    notes: list[str],
    *,
    source_root: Path | None = None,
    owns_isolation: bool = False,
    fallback_base_oid: str | None = None,
) -> IsolationHold:
    context = IsolationContext(
        id=claimed.id,
        worktree_path=claimed.worktree_path,
        branch=claimed.branch,
        main_root=claimed.main_root,
    )
    info = TaskIsolationInfo(
        id=claimed.id,
        path=str(claimed.worktree_path),
        cwd=str(claimed.execution_cwd),
        branch=claimed.branch,
        repo_common_dir=claimed.repo_common_dir,
        base_oid=claimed.base_oid or fallback_base_oid,
        main_root=claimed.main_root,
        source_root=source_root or claimed.main_root,
    )
    hold = _hold(
        request,
        context,
        claimed.execution_cwd,
        info,
        claimed=claimed,
        owns_isolation=owns_isolation,
    )
    hold.warnings = tuple(notes)
    return hold


def _hold(
    request: InvocationRequest,
    context: IsolationContext,
    cwd: Path,
    info: TaskIsolationInfo,
    *,
    created: CreatedIsolation | None = None,
    claimed: ClaimedIsolation | None = None,
    owns_isolation: bool = False,
    rebuilt: bool = False,
) -> IsolationHold:
    text = isolation_instructions(context, cwd, read_only=request.read_only)
    info = replace(info, instructions_applied=text is not None)
    return IsolationHold(
        replace(request, cwd=cwd, extra_instructions=text),
        created=created,
        claimed=claimed,
        info=info,
        owns_isolation=owns_isolation,
        rebuilt=rebuilt,
    )


def resumed_isolation_id(cwd: Path, resumed_task: TaskRecord | None) -> str | None:
    """The isolated directory a resumed thread last ran in, when cwd still points into it."""
    if (
        resumed_task is None
        or resumed_task.isolation_id is None
        or resumed_task.isolation_path is None
        or not cwd.is_absolute()
    ):
        return None
    if not is_same_or_subpath(cwd, Path(resumed_task.isolation_path)):
        return None
    return resumed_task.isolation_id


def needs_resumed_rebuild(cwd: Path, resumed_task: TaskRecord | None) -> bool:
    """True when a resumed task targets an isolated path that is currently missing on disk."""
    if (
        resumed_task is None
        or not cwd.is_absolute()
        or cwd.is_dir()
        or resumed_task.isolation_id is None
        or resumed_task.isolation_path is None
    ):
        return False
    return is_same_or_subpath(cwd, Path(resumed_task.isolation_path))


def rebuild_resumed_isolation(
    isolated: IsolatedDirs,
    cwd: Path,
    resumed_task: TaskRecord,
    workspace_id: str | None,
) -> CreatedIsolation | ClaimedIsolation:
    """Validate and rebuild a missing isolated directory for its own resumed session thread."""
    if not resumed_task.isolation_owned:
        raise ValueError(
            f"Isolated directory {resumed_task.isolation_id} ({resumed_task.isolation_path}) "
            "belonged to another thread and is gone; this thread only ran inside it. "
            "Start a new task with isolation or another cwd instead."
        )
    if not resumed_task.isolation_auto_released:
        raise ValueError(
            f"Isolated directory {resumed_task.isolation_id} ({resumed_task.isolation_path}) "
            "is no longer present and was not auto-released; start a new task with isolation instead."
        )
    if (
        not resumed_task.isolation_id
        or not resumed_task.isolation_path
        or not resumed_task.isolation_base_oid
        or not resumed_task.isolation_main_root
    ):
        raise ValueError(
            f"Isolated directory {resumed_task.isolation_id} is missing saved rebuild metadata; "
            "start a new task with isolation instead."
        )
    if workspace_id is None:
        raise ValueError(
            "Rebuilding an auto-released isolated directory needs a known manager session; "
            "pass manager_session_id or configure AGENTNAVE_HOST_SESSION_ID"
        )
    return isolated.rebuild_auto_released(
        isolation_id=resumed_task.isolation_id,
        worktree_path=resumed_task.isolation_path,
        cwd=cwd,
        base_oid=resumed_task.isolation_base_oid,
        main_root=resumed_task.isolation_main_root,
        source_root=resumed_task.isolation_source_root,
        workspace_id=workspace_id,
    )


def hold_from_acquired(
    request: InvocationRequest,
    acquired: CreatedIsolation | ClaimedIsolation,
    resumed_task: TaskRecord,
) -> IsolationHold:
    """Wrap a rebuilt/claimed resumed isolated directory into an IsolationHold with prompt instructions."""
    if isinstance(acquired, CreatedIsolation):
        return hold_from_created(request, acquired, owns_isolation=True, rebuilt=True)
    source_root = (
        Path(resumed_task.isolation_source_root)
        if resumed_task.isolation_source_root
        else acquired.main_root
    )
    return hold_from_claimed(
        request,
        acquired,
        [],
        source_root=source_root,
        # Only an owner thread rebuilds, so a concurrent rebuild's claim stays its own.
        owns_isolation=resumed_task.isolation_owned and resumed_task.isolation_id == acquired.id,
        fallback_base_oid=resumed_task.isolation_base_oid,
    )


def release_acquired(isolated: IsolatedDirs, acquired: CreatedIsolation | ClaimedIsolation) -> None:
    """Roll back an acquired directory/lease before an IsolationHold was built."""
    log = logging.getLogger(__name__)
    if isinstance(acquired, CreatedIsolation):
        try:
            isolated.abort_created(acquired)
        except Exception:
            log.exception("Rolling back isolated directory %s failed", acquired.id)
    else:
        try:
            acquired.lease.release()
        except Exception:
            log.exception("Releasing lease on isolated directory %s failed", acquired.id)


def release_hold(isolated: IsolatedDirs, hold: IsolationHold) -> None:
    """Give back a hold that never reached a running task; each step stands alone."""
    if hold.consumed:
        return
    log = logging.getLogger(__name__)
    if hold.created is not None:
        try:
            isolated.abort_created(hold.created)
        except Exception:
            log.exception("Rolling back isolated directory %s failed", hold.created.id)
    elif hold.claimed is not None:
        try:
            hold.claimed.lease.release()
        except Exception:
            log.exception("Releasing lease on isolated directory %s failed", hold.claimed.id)
