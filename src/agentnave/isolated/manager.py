"""High-level coordinator for service-managed isolated directories."""

from __future__ import annotations

import secrets
import threading
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from agentnave.isolated.auto_release import (
    plan_rebuild,
    rebuild_worktree_locked,
    try_auto_release,
)
from agentnave.isolated.copy import copy_worktreeinclude
from agentnave.isolated.leases import (
    IsolationLease,
    RepoLockUnavailable,
    acquire_lease_locked,
    has_active_leases,
    repo_lock,
)
from agentnave.isolated.release import (
    ChangeSummary,
    RecoveryOutcome,
    ReleaseResult,
    recover_record,
    release_isolated_dir,
    rollback_creating_locked,
    summarize_record_changes,
    summarize_unregistered,
)
from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    RepoContext,
    add_worktree_locked,
    check_tree_and_filters,
    collect_creation_warnings,
    control_char_in,
    decode_git_path,
    discover_repo,
    ensure_info_exclude_locked,
    is_link,
    is_same_or_subpath,
    isolated_worktree_root,
    parse_worktrees,
    ref_exists,
    resolve_mapped_cwd,
    resolve_target_commit,
    run_git,
    same_dir,
    validate_cwd,
)
from agentnave.isolated.store import (
    ID_PATTERN,
    IsolatedRecord,
    IsolationState,
    WorktreeStore,
    local_machine_id,
)
from agentnave.members import data_root
from agentnave.store import now


@dataclass(frozen=True, slots=True)
class CreatedIsolation:
    id: str
    worktree_path: Path
    execution_cwd: Path
    branch: str
    branch_ref: str
    main_root: Path
    repo_common_dir: Path
    base_oid: str
    warnings: tuple[str, ...]
    lease: IsolationLease
    source_root: Path | None = None


@dataclass(frozen=True, slots=True)
class ClaimedIsolation:
    id: str
    worktree_path: Path
    execution_cwd: Path
    branch: str
    main_root: Path
    repo_common_dir: Path
    lease: IsolationLease
    base_oid: str | None = None


@dataclass(frozen=True, slots=True)
class IsolatedListItem:
    id: str
    path: str
    cwd: str
    branch: str
    base_oid: str
    state: IsolationState
    in_use: bool
    created_at: str
    attention: str | None = None
    own_workspace: bool = True


def _validate_workspace_id(workspace_id: str) -> str:
    if not workspace_id.strip():
        raise IsolationError("workspace_id must be a non-empty string")
    return workspace_id.strip()


def _allocate_id_locked(store: WorktreeStore, main_root: Path) -> tuple[str, Path, str]:
    container = main_root / WORKTREES_DIR_NAME
    for _ in range(16):
        candidate = f"wt-{secrets.token_hex(6)}"
        wt_path = container / candidate
        branch_ref = f"refs/heads/agentnave/{candidate}"
        if store.get(candidate) is not None:
            continue
        if wt_path.exists() or is_link(wt_path):
            continue
        if ref_exists(main_root, branch_ref):
            continue
        return candidate, wt_path, branch_ref
    raise IsolationError("Could not allocate a unique isolated directory identifier")


class IsolatedDirs:
    """Service-internal API for creating, claiming, listing, releasing, and recovering worktrees."""

    def __init__(self, root: Path | None = None) -> None:
        self.root = (root if root is not None else data_root()).resolve()
        self.store = WorktreeStore(self.root)
        self._active_mutations = 0
        self._mutations_lock = threading.Lock()

    def close(self) -> None:
        self.store.close()

    def restart_safe(self) -> bool:
        """False while create, explicit release, or startup recovery is mutating repository state."""
        with self._mutations_lock:
            return self._active_mutations == 0

    @contextmanager
    def _mutating(self) -> Generator[None]:
        with self._mutations_lock:
            self._active_mutations += 1
        try:
            yield
        finally:
            with self._mutations_lock:
                self._active_mutations -= 1

    def create(
        self,
        cwd: Path | str,
        *,
        workspace_id: str,
        ref: str | None = None,
    ) -> CreatedIsolation:
        """Create a flat isolated worktree, copy .worktreeinclude files, and return a held lease."""
        ws_id = _validate_workspace_id(workspace_id)
        ctx = discover_repo(cwd)
        for path in (ctx.cwd, ctx.source_root, ctx.main_root):
            if control_char_in(str(path)):
                raise IsolationError(
                    "A repository path contains control characters (such as a newline); "
                    "isolated directories cannot be created for it"
                )
        base_oid = resolve_target_commit(ctx, ref)
        lfs_detected = check_tree_and_filters(ctx, base_oid)
        warnings = collect_creation_warnings(ctx, lfs_detected=lfs_detected)
        with self._mutating(), repo_lock(ctx.repo_common_dir):
            return self._create_locked(ctx, ws_id, base_oid, warnings)

    def _create_locked(
        self,
        ctx: RepoContext,
        workspace_id: str,
        base_oid: str,
        warnings: tuple[str, ...],
    ) -> CreatedIsolation:
        rechecked = discover_repo(ctx.cwd)
        if not same_dir(rechecked.repo_common_dir, ctx.repo_common_dir) or not same_dir(
            rechecked.main_root, ctx.main_root
        ):
            raise IsolationError("Repository identity changed before acquiring lock")
        ensure_info_exclude_locked(ctx.main_root)
        isolation_id, wt_path, branch_ref = _allocate_id_locked(self.store, ctx.main_root)
        branch_name = f"agentnave/{isolation_id}"
        provisional_cwd = (
            wt_path.joinpath(*ctx.subdir_prefix.parts) if ctx.subdir_prefix.parts else wt_path
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
        self.store.insert_creating(creating_record)
        lease: IsolationLease | None = None
        try:
            wt_git_dir = add_worktree_locked(ctx.main_root, branch_name, wt_path, base_oid)
            self.store.mark_worktree_added(
                isolation_id,
                str(wt_git_dir),
                phase="copying_include",
                updated_at=now(),
            )
            copy_worktreeinclude(ctx.source_root, wt_path)
            mapped_cwd = resolve_mapped_cwd(wt_path, ctx.subdir_prefix)
            lease = acquire_lease_locked(ctx.repo_common_dir, isolation_id)
            self.store.mark_ready(isolation_id, usage_generation=1, updated_at=now())
            return CreatedIsolation(
                id=isolation_id,
                worktree_path=wt_path.resolve(),
                execution_cwd=mapped_cwd,
                branch=branch_name,
                branch_ref=branch_ref,
                main_root=ctx.main_root,
                repo_common_dir=ctx.repo_common_dir,
                base_oid=base_oid,
                warnings=warnings,
                lease=lease,
                source_root=ctx.source_root,
            )
        except BaseException:
            if lease is not None:
                lease.release()
            latest = self.store.get(isolation_id) or creating_record
            rollback_creating_locked(self.store, latest)
            raise

    def find_by_path(self, path: Path | str) -> IsolatedRecord | None:
        """Return the registered IsolatedRecord containing path, or None."""
        raw = Path(path)
        if not raw.is_absolute():
            raise IsolationError("path must be an absolute path")
        resolved = raw.resolve()
        for record in self.store.all_records():
            if record.state == "creating":
                continue
            if is_same_or_subpath(resolved, Path(record.worktree_path)):
                return record
        return None

    def claim_for_cwd(
        self, cwd: Path | str, warnings: list[str] | None = None
    ) -> ClaimedIsolation | None:
        """Acquire a usage lease when cwd lies inside a managed isolated worktree.

        A cwd outside .agentnave-worktrees never reads the registry. When the registry cannot be
        read, the lease is taken by path alone and the reason is appended to warnings.
        """
        resolved = validate_cwd(cwd)
        if not any(part.lower() == WORKTREES_DIR_NAME for part in resolved.parts):
            return None
        try:
            record = self.find_by_path(resolved)
        except (OSError, ValueError) as exc:
            return self._claim_without_registry(resolved, exc, warnings)
        if record is not None:
            return self._claim_registered(record, resolved)
        return self._claim_cross_data_root(resolved)

    def _claim_without_registry(
        self, resolved: Path, cause: Exception, warnings: list[str] | None
    ) -> ClaimedIsolation | None:
        """Lease an isolated directory by path alone because the registry cannot be read.

        A directory laid out like an isolated one must be leased: without a lease another
        service could release it while a task runs in it, so failing to lease refuses the start.
        """
        if isolated_worktree_root(resolved) is None:
            return None
        try:
            claimed = self._claim_cross_data_root(resolved)
        except (IsolationError, OSError, ValueError) as exc:
            claimed = None
            cause = exc
        if claimed is None:
            raise IsolationError(
                f"Isolated directory {resolved} cannot be leased ({cause}); "
                "the registry is unreadable. Fix worktrees.db or choose another cwd."
            ) from cause
        if warnings is not None:
            warnings.append(
                f"Isolated directory registry is unreadable ({cause}); "
                "the directory was used without its registration."
            )
        return claimed

    def _claim_registered(self, record: IsolatedRecord, resolved_cwd: Path) -> ClaimedIsolation:
        common_dir = Path(record.repo_common_dir)
        with repo_lock(common_dir):
            return self._claim_registered_locked(record, resolved_cwd, common_dir)

    def _claim_registered_locked(
        self, record: IsolatedRecord, resolved_cwd: Path, common_dir: Path
    ) -> ClaimedIsolation:
        latest = self.store.get(record.id)
        if latest is None or latest.state != "ready":
            state = "deleted" if latest is None else latest.state
            raise IsolationError(
                f"Isolated directory {record.id} is not available for use (state: {state})"
            )
        wt_path = Path(latest.worktree_path)
        if is_link(wt_path) or not wt_path.is_dir():
            raise IsolationError(f"Isolated directory is missing or unsafe: {wt_path}")
        if not is_same_or_subpath(resolved_cwd, wt_path):
            raise IsolationError("cwd resolves outside the isolated worktree")
        try:
            updated = self.store.bump_usage(latest.id, updated_at=now())
        except OSError as exc:
            # Recording the use is what voids confirmation tokens already handed out, so a
            # claim that cannot record it must not succeed.
            raise IsolationError(
                f"Isolated directory {record.id} cannot be used right now: the registry "
                f"is not writable ({exc}). Retry shortly."
            ) from exc
        lease = acquire_lease_locked(common_dir, updated.id)
        return ClaimedIsolation(
            id=updated.id,
            worktree_path=wt_path.resolve(),
            execution_cwd=resolved_cwd,
            branch=updated.branch,
            main_root=Path(updated.main_root),
            repo_common_dir=common_dir,
            lease=lease,
            base_oid=updated.base_oid,
        )

    def _claim_cross_data_root(self, resolved_cwd: Path) -> ClaimedIsolation | None:
        try:
            ctx = discover_repo(resolved_cwd)
        except IsolationError:
            return None
        wt_root = ctx.source_root
        if not ID_PATTERN.fullmatch(wt_root.name):
            return None
        if wt_root.parent.name != WORKTREES_DIR_NAME or not same_dir(
            wt_root.parent.parent, ctx.main_root
        ):
            return None
        isolation_id = wt_root.name
        with repo_lock(ctx.repo_common_dir):
            worktrees = parse_worktrees(
                run_git(["worktree", "list", "--porcelain", "-z"], cwd=ctx.main_root).stdout
            )
            if not any(w.path.exists() and same_dir(w.path, wt_root) for w in worktrees):
                raise IsolationError(f"Isolated directory is no longer registered: {wt_root}")
            sym = run_git(["symbolic-ref", "-q", "HEAD"], cwd=wt_root, allowed_exit_codes=(0, 1))
            branch = (
                decode_git_path(sym.stdout.rstrip(b"\r\n")).removeprefix("refs/heads/")
                if sym.returncode == 0 and sym.stdout.strip()
                else f"agentnave/{isolation_id}"
            )
            lease = acquire_lease_locked(ctx.repo_common_dir, isolation_id)
            return ClaimedIsolation(
                id=isolation_id,
                worktree_path=wt_root,
                execution_cwd=resolved_cwd,
                branch=branch,
                main_root=ctx.main_root,
                repo_common_dir=ctx.repo_common_dir,
                lease=lease,
            )

    def list_for_workspace(self, workspace_id: str) -> list[IsolatedListItem]:
        """List non-transient isolated directories registered under workspace_id."""
        ws_id = _validate_workspace_id(workspace_id)
        return [
            self._list_item(record, own_workspace=True)
            for record in self.store.list_workspace(ws_id)
        ]

    def list_for_repo(self, cwd: Path | str, workspace_id: str) -> list[IsolatedListItem]:
        """List every non-transient isolated directory of cwd's repository, any workspace."""
        ws_id = _validate_workspace_id(workspace_id)
        ctx = discover_repo(cwd)
        return [
            self._list_item(record, own_workspace=record.workspace_id == ws_id)
            for record in self.store.list_repo(ctx.repo_identity)
        ]

    def _list_item(self, record: IsolatedRecord, *, own_workspace: bool) -> IsolatedListItem:
        return IsolatedListItem(
            id=record.id,
            path=record.worktree_path,
            cwd=record.execution_cwd,
            branch=record.branch,
            base_oid=record.base_oid,
            state=record.state,
            in_use=self._probe_in_use(record),
            created_at=record.created_at,
            attention=record.attention,
            own_workspace=own_workspace,
        )

    def _probe_in_use(self, record: IsolatedRecord) -> bool:
        common_dir = Path(record.repo_common_dir)
        if not common_dir.is_dir() or is_link(common_dir):
            return False
        try:
            return has_active_leases(common_dir, record.id, cleanup_stale=False)
        except (OSError, ValueError):
            return True

    def unreleased_count(self, workspace_id: str) -> int:
        """Count isolated directories owned by workspace_id that have not been released."""
        ws_id = _validate_workspace_id(workspace_id)
        return self.store.count_unreleased(ws_id)

    def release(
        self,
        isolation_id: str,
        *,
        workspace_id: str,
        confirmation: str | None = None,
    ) -> ReleaseResult:
        """Inspect and explicitly release an isolated directory.

        Directories of other workspaces always need a token from an earlier call first."""
        ws_id = _validate_workspace_id(workspace_id)
        if not ID_PATTERN.fullmatch(isolation_id):
            raise IsolationError(f"Invalid isolated directory id: {isolation_id!r}")
        if confirmation is not None and not confirmation.strip():
            raise IsolationError("confirmation must not be blank")
        with self._mutating():
            return release_isolated_dir(
                self.store,
                isolation_id,
                workspace_id=ws_id,
                confirmation=confirmation,
            )

    def abort_created(self, created: CreatedIsolation) -> None:
        """Release the initial lease and roll back a newly created directory before task launch."""
        try:
            with self._mutating(), repo_lock(created.repo_common_dir):
                created.lease.release()
                latest = self.store.get(created.id)
                if latest is not None:
                    rollback_creating_locked(self.store, latest)
        except RepoLockUnavailable:
            # Without the lock nothing can be removed safely. Hand the record to recovery as an
            # unfinished creation instead of leaving a ready directory nobody knows about.
            self.store.revert_to_creating(created.id, updated_at=now())
        finally:
            if not created.lease.closed:
                created.lease.release()

    def summarize_changes(
        self,
        isolation_id: str,
        *,
        fallback_path: str = "",
        fallback_branch: str | None = None,
    ) -> ChangeSummary:
        """Return cumulative change summary relative to base_oid without raising."""
        branch = fallback_branch or f"agentnave/{isolation_id}"
        try:
            record = self.store.get(isolation_id)
        except (OSError, ValueError):
            record = None
        if record is not None:
            return summarize_record_changes(record)
        if fallback_path:
            return summarize_unregistered(isolation_id, Path(fallback_path), branch)
        return ChangeSummary(
            id=isolation_id,
            path=fallback_path,
            branch=branch,
            summary_status="error",
            error=f"Unknown isolated directory: {isolation_id}",
        )

    def settle_task(
        self,
        isolation_id: str,
        *,
        workspace_id: str | None,
        lease: IsolationLease,
        auto_release: bool,
        fallback_path: str = "",
        fallback_branch: str | None = None,
    ) -> tuple[ChangeSummary, list[str]]:
        """Summarize changes, release lease, and auto-release if strictly untouched and unleased."""
        try:
            summary = self.summarize_changes(
                isolation_id,
                fallback_path=fallback_path,
                fallback_branch=fallback_branch,
            )
            if not auto_release or workspace_id is None:
                return summary, []
            ws_id = _validate_workspace_id(workspace_id)
            with self._mutating():
                return try_auto_release(
                    self.store,
                    isolation_id,
                    workspace_id=ws_id,
                    lease=lease,
                    summary=summary,
                )
        finally:
            if not lease.closed:
                lease.release()

    def rebuild_auto_released(
        self,
        *,
        isolation_id: str,
        worktree_path: Path | str,
        cwd: Path | str,
        base_oid: str,
        main_root: Path | str,
        source_root: Path | str | None,
        workspace_id: str,
    ) -> CreatedIsolation | ClaimedIsolation:
        """Rebuild an auto-released isolated directory in place, or claim if already ready."""
        ws_id = _validate_workspace_id(workspace_id)
        plan = plan_rebuild(
            isolation_id=isolation_id,
            worktree_path=Path(worktree_path),
            cwd=Path(cwd),
            base_oid=base_oid,
            main_root=Path(main_root),
            source_root=Path(source_root) if source_root is not None else None,
        )
        with self._mutating(), repo_lock(plan.ctx.repo_common_dir):
            existing = self.store.get(isolation_id)
            if existing is not None:
                if existing.state == "ready":
                    return self._claim_registered_locked(
                        existing, Path(cwd).resolve(), plan.ctx.repo_common_dir
                    )
                raise IsolationError(
                    f"Isolated directory {isolation_id} is not available for rebuild "
                    f"(state: {existing.state})"
                )
            rebuilt = rebuild_worktree_locked(
                self.store,
                plan,
                isolation_id=isolation_id,
                base_oid=base_oid,
                workspace_id=ws_id,
            )
            return CreatedIsolation(
                id=isolation_id,
                worktree_path=rebuilt.worktree_path,
                execution_cwd=rebuilt.execution_cwd,
                branch=rebuilt.branch,
                branch_ref=rebuilt.branch_ref,
                main_root=rebuilt.main_root,
                repo_common_dir=rebuilt.repo_common_dir,
                base_oid=rebuilt.base_oid,
                warnings=rebuilt.warnings,
                lease=rebuilt.lease,
                source_root=rebuilt.source_root,
            )

    def recover(self) -> list[RecoveryOutcome]:
        """Roll back unfinished `creating` rows when safe and resume `deleting` rows."""
        with self._mutating():
            pending = self.store.pending_recovery()
            return [recover_record(self.store, record) for record in pending]
