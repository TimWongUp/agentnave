"""Safety inspection, confirmation digests, explicit release, and recovery."""

from __future__ import annotations

import hashlib
import hmac
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from agentnave.isolated.branches import (
    branch_tip,
    has_unique_commits,
    verify_managed_branch_ref,
)
from agentnave.isolated.leases import (
    RepoLockUnavailable,
    has_active_leases,
    remove_lease_dir_locked,
    repo_lock,
)
from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    decode_git_path,
    is_link,
    is_same_or_subpath,
    never_checked_out,
    parse_worktrees,
    ref_exists,
    run_git,
    same_dir,
)
from agentnave.isolated.store import (
    IsolatedRecord,
    WorktreeStore,
)
from agentnave.isolated.worktree_state import (
    foreign_link_valid,
    nested_repositories,
    remove_tree,
    tree_fingerprint,
    verify_worktree_container,
    verify_worktree_root,
    worktree_link_valid,
)
from agentnave.store import now

# Never look inside submodules or nested repositories: that would start Git there, with the
# nested repository's own configuration.
STATUS_ARGS = [
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=all",
]
_STATUS_ARGS = STATUS_ARGS
NESTED_NOTE = "Contains nested repositories; their contents were not checked"
ORPHAN_NOTE = (
    "The directory is no longer a git worktree; the remaining files will be deleted "
    "without inspection"
)

_IN_PROGRESS_MARKERS = (
    "MERGE_HEAD",
    "REBASE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "sequencer",
)

type ReleaseStatus = Literal[
    "deleted",
    "confirmation_required",
    "blocked",
    "needs_attention",
    "not_found",
]
type RecoveryStatus = Literal["rolled_back", "deleted", "needs_attention", "skipped"]


@dataclass(frozen=True, slots=True)
class ReleaseResult:
    id: str
    status: ReleaseStatus
    head_oid: str | None = None
    uncommitted: bool = False
    unique_commits: bool = False
    confirmation: str | None = None
    reason: str | None = None
    nested_repos: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class RecoveryOutcome:
    id: str
    previous_state: str
    status: RecoveryStatus
    reason: str | None = None


@dataclass(frozen=True, slots=True)
class InspectionSnapshot:
    head_oid: str
    branch_oid: str
    uncommitted: bool
    unique_commits: bool
    digest: str
    nested: tuple[str, ...] = ()
    orphan: bool = False


def porcelain_v2_paths(status_z: bytes) -> tuple[list[str], list[str]]:
    """Extract all reported relative paths and untracked paths from porcelain=v2 -z output."""
    tokens = status_z.split(b"\x00")
    if tokens and tokens[-1] == b"":
        tokens.pop()
    all_paths: list[str] = []
    untracked: list[str] = []
    idx = 0
    while idx < len(tokens):
        token = tokens[idx]
        if token.startswith(b"? "):
            path = decode_git_path(token[2:])
            all_paths.append(path)
            untracked.append(path)
            idx += 1
        elif token.startswith(b"1 "):
            parts = token.split(b" ", 8)
            if len(parts) == 9:
                all_paths.append(decode_git_path(parts[8]))
            idx += 1
        elif token.startswith(b"2 "):
            parts = token.split(b" ", 9)
            if len(parts) == 10:
                all_paths.append(decode_git_path(parts[9]))
            if idx + 1 < len(tokens):
                all_paths.append(decode_git_path(tokens[idx + 1]))
            idx += 2
        elif token.startswith(b"u "):
            parts = token.split(b" ", 10)
            if len(parts) == 11:
                all_paths.append(decode_git_path(parts[10]))
            idx += 1
        else:
            idx += 1
    return all_paths, untracked


def _stat_fingerprint(worktree_path: Path, paths: list[str]) -> bytes:
    """Metadata-only fingerprint of dirty/untracked entries without reading file contents."""
    lines: list[bytes] = []
    for rel in sorted(set(paths)):
        target = worktree_path / rel
        try:
            st = target.lstat()
        except FileNotFoundError:
            lines.append(f"{rel}\tmissing\n".encode())
        else:
            lines.append(
                f"{rel}\t{st.st_mode}:{st.st_size}:{st.st_mtime_ns}:{st.st_ctime_ns}\n".encode()
            )
    return b"".join(lines)


def compute_digest(
    isolation_id: str,
    workspace_id: str,
    head_oid: str,
    branch_oid: str,
    usage_generation: int,
    status_raw: bytes,
    stat_bytes: bytes,
    nested: tuple[str, ...] = (),
    *,
    unique_commits: bool,
) -> str:
    hasher = hashlib.sha256()
    for part in (
        isolation_id.encode("ascii"),
        workspace_id.encode("utf-8"),
        head_oid.encode("ascii"),
        branch_oid.encode("ascii"),
        str(usage_generation).encode("ascii"),
        status_raw,
        stat_bytes,
        "\n".join(nested).encode("utf-8", errors="surrogateescape"),
        b"1" if unique_commits else b"0",
    ):
        hasher.update(f"{len(part)}:".encode("ascii"))
        hasher.update(part)
        hasher.update(b";")
    return hasher.hexdigest()


def verify_repo_identity(
    record: IsolatedRecord, *, require_base_oid: bool = True
) -> tuple[Path, Path]:
    """Check the recorded repository, and that the record names only its own branch and path."""
    verify_managed_branch_ref(record)
    main_root = Path(record.main_root)
    common_dir = Path(record.repo_common_dir)
    if is_link(main_root) or not main_root.is_dir():
        raise IsolationError(f"Main worktree is missing or unsafe: {main_root}")
    if is_link(common_dir) or not common_dir.is_dir():
        raise IsolationError(f"Repository common-dir is missing or unsafe: {common_dir}")
    bare = run_git(["rev-parse", "--is-bare-repository"], cwd=main_root).stdout.strip()
    if bare == b"true":
        raise IsolationError("Repository became bare")
    actual_common = Path(
        decode_git_path(
            run_git(["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=main_root).line
        )
    )
    if not same_dir(common_dir, actual_common):
        raise IsolationError("Main worktree no longer belongs to the recorded repository")
    if require_base_oid:
        run_git(["cat-file", "-e", f"{record.base_oid}^{{commit}}"], cwd=main_root)
    verify_worktree_container(Path(record.worktree_path), main_root, record.id)
    return main_root, common_dir


def _verify_worktree_identity(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
) -> Path:
    wt_path = Path(record.worktree_path)
    if is_link(wt_path) or not wt_path.is_dir():
        raise IsolationError(f"Isolated worktree is missing or unsafe: {wt_path}")
    verify_worktree_root(wt_path)
    wt_common = Path(
        decode_git_path(
            run_git(["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=wt_path).line
        )
    )
    wt_git_dir = Path(
        decode_git_path(
            run_git(["rev-parse", "--path-format=absolute", "--git-dir"], cwd=wt_path).line
        )
    )
    if not same_dir(wt_common, common_dir):
        raise IsolationError("Isolated worktree points to a different Git common-dir")
    if record.worktree_git_dir is None or not same_dir(wt_git_dir, Path(record.worktree_git_dir)):
        raise IsolationError("Isolated worktree Git admin directory does not match registration")
    for marker in _IN_PROGRESS_MARKERS:
        marker_path = wt_git_dir / marker
        if marker_path.exists() or is_link(marker_path):
            raise IsolationError(f"Git operation in progress ({marker})")
    worktrees = parse_worktrees(
        run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
    )
    if not worktrees or not same_dir(worktrees[0].path, main_root):
        raise IsolationError("Main worktree identity changed in git worktree list")
    matches = [w for w in worktrees if w.path.exists() and same_dir(w.path, wt_path)]
    if len(matches) != 1:
        raise IsolationError("Isolated worktree is not uniquely registered in Git")
    for entry in worktrees:
        if entry is matches[0]:
            continue
        if entry.branch_ref == record.branch_ref:
            raise IsolationError("Managed branch is checked out in another worktree")
        if is_same_or_subpath(entry.path, wt_path):
            raise IsolationError("Another Git worktree exists inside this isolated directory")
    for other in store.all_records():
        if other.id != record.id and is_same_or_subpath(Path(other.worktree_path), wt_path):
            raise IsolationError(
                f"Another registered isolated directory ({other.id}) is inside {wt_path}"
            )
    return wt_path


def _check_hidden_index_flags(wt_path: Path) -> None:
    out = run_git(["ls-files", "-v", "-z"], cwd=wt_path).stdout
    for item in out.split(b"\x00"):
        if not item:
            continue
        tag = item[0:1]
        if b"a" <= tag <= b"z":
            raise IsolationError("Worktree has assume-unchanged index entries")
        if tag in (b"S", b"s"):
            raise IsolationError("Worktree has skip-worktree index entries")


def inspect_ready_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    workspace_id: str,
) -> tuple[Path, Path, Path, InspectionSnapshot]:
    main_root, common_dir = verify_repo_identity(record)
    if has_active_leases(common_dir, record.id, cleanup_stale=True):
        raise IsolationError("Isolated directory is currently in use by an active lease")
    wt_path = _verify_worktree_identity(store, record, main_root, common_dir)
    if not worktree_link_valid(wt_path, record.worktree_git_dir, common_dir):
        raise IsolationError("Isolated directory is no longer a valid git worktree of this record")
    sym_res = run_git(["symbolic-ref", "-q", "HEAD"], cwd=wt_path, allowed_exit_codes=(0, 1))
    actual_ref = decode_git_path(sym_res.line) if sym_res.returncode == 0 else ""
    if actual_ref != record.branch_ref:
        raise IsolationError(
            f"Worktree HEAD is not on managed branch {record.branch} (found {actual_ref or 'detached HEAD'})"
        )
    head_oid = (
        run_git(["rev-parse", "--verify", "HEAD^{commit}"], cwd=wt_path)
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    branch_oid = (
        run_git(
            ["rev-parse", "--verify", "--end-of-options", f"{record.branch_ref}^{{commit}}"],
            cwd=main_root,
        )
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    if head_oid != branch_oid:
        raise IsolationError("Managed branch tip does not match worktree HEAD")
    _check_hidden_index_flags(wt_path)
    status_raw = run_git(_STATUS_ARGS, cwd=wt_path).stdout
    nested = tuple(nested_repositories(wt_path, status_raw))
    unique_commits = has_unique_commits(wt_path, head_oid)
    paths, _ = porcelain_v2_paths(status_raw)
    stat_bytes = _stat_fingerprint(wt_path, paths)
    # The token is bound to the workspace asking, so one workspace's token never confirms
    # another's release.
    digest = compute_digest(
        record.id,
        workspace_id,
        head_oid,
        branch_oid,
        record.usage_generation,
        status_raw,
        stat_bytes,
        nested,
        unique_commits=unique_commits,
    )
    snapshot = InspectionSnapshot(
        head_oid=head_oid,
        branch_oid=branch_oid,
        uncommitted=bool(status_raw),
        unique_commits=unique_commits,
        digest=digest,
        nested=nested,
    )
    return main_root, common_dir, wt_path, snapshot


def complete_deletion_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_oid: str,
    *,
    remove_orphan: bool = False,
    force: bool = True,
) -> str | None:
    """Remove worktree, managed branch, and DB row; raises IsolationError on partial failure.

    Returns a note when the managed branch was kept because it moved after the release check.
    remove_orphan deletes a directory that is no longer a git worktree (after confirmation).
    force=False lets git refuse a worktree that gained source changes after the last check.
    """
    if has_active_leases(common_dir, record.id, cleanup_stale=True):
        raise IsolationError("Active lease prevents completing deletion")
    wt_path = Path(record.worktree_path)
    if remove_orphan and (wt_path.exists() or is_link(wt_path)):
        remove_tree(wt_path, main_root / WORKTREES_DIR_NAME)
        run_git(["worktree", "prune"], cwd=main_root)
        return finish_deletion(store, record, main_root, common_dir, expected_oid)
    if not wt_path.exists() and not is_link(wt_path):
        run_git(["worktree", "prune"], cwd=main_root)
    worktrees = parse_worktrees(
        run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
    )
    wt_matches = [
        w
        for w in worktrees
        if (w.path.exists() and wt_path.exists() and same_dir(w.path, wt_path))
        or w.path.resolve() == wt_path.resolve()
    ]
    if wt_matches:
        entry = wt_matches[0]
        if entry.branch_ref != record.branch_ref or entry.head_oid != expected_oid:
            raise IsolationError("Worktree branch or HEAD changed during deletion")
        if is_link(wt_path) or is_link(main_root / WORKTREES_DIR_NAME):
            raise IsolationError(f"Unsafe link encountered before removing {wt_path}")
        run_git(
            ["worktree", "remove", *(["--force"] if force else []), "--", str(wt_path)],
            cwd=main_root,
            filters_from=() if force else (wt_path,),
        )
    elif wt_path.exists() or is_link(wt_path):
        raise IsolationError(f"Unregistered path remains at {wt_path}; refusing recursive removal")

    return finish_deletion(store, record, main_root, common_dir, expected_oid)


def finish_deletion(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_oid: str,
) -> str | None:
    store.update_phase(record.id, phase="removing_branch", updated_at=now())
    note = _remove_managed_branch(record, main_root, expected_oid)
    store.delete(record.id, expected_state="deleting")
    remove_lease_dir_locked(common_dir, record.id)
    return note


def _remove_managed_branch(
    record: IsolatedRecord, main_root: Path, expected_oid: str
) -> str | None:
    if not ref_exists(main_root, record.branch_ref):
        return None
    remaining = parse_worktrees(
        run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
    )
    if any(w.branch_ref == record.branch_ref for w in remaining):
        raise IsolationError("Managed branch is still checked out in a worktree")
    current = (
        run_git(
            ["rev-parse", "--verify", "--end-of-options", f"{record.branch_ref}^{{commit}}"],
            cwd=main_root,
        )
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    if current != expected_oid:
        return (
            f"Branch {record.branch} was kept: it moved to {current[:12]} after the release "
            "check, so it was not deleted."
        )
    # --no-deref: a task may have turned the managed branch into a link to another branch.
    run_git(["update-ref", "--no-deref", "-d", record.branch_ref, expected_oid], cwd=main_root)
    return None


def release_isolated_dir(
    store: WorktreeStore,
    isolation_id: str,
    *,
    workspace_id: str,
    confirmation: str | None = None,
) -> ReleaseResult:
    """Explicitly inspect and release an isolated directory under its repository lock.

    A directory registered under another workspace can be released too, but only after a
    confirmation token issued for its current state.
    """
    initial = store.get(isolation_id)
    if initial is None:
        return ReleaseResult(
            id=isolation_id,
            status="not_found",
            reason="Isolated directory is not registered",
        )
    common_dir = Path(initial.repo_common_dir)
    if is_link(common_dir) or not common_dir.is_dir():
        return ReleaseResult(
            id=isolation_id,
            status="blocked",
            reason=f"Repository common-dir is unavailable: {common_dir}",
        )
    try:
        with repo_lock(common_dir):
            return _release_locked(store, isolation_id, workspace_id, confirmation)
    except RepoLockUnavailable as exc:
        return ReleaseResult(id=isolation_id, status="blocked", reason=str(exc))


def _release_locked(
    store: WorktreeStore,
    isolation_id: str,
    workspace_id: str,
    confirmation: str | None,
) -> ReleaseResult:
    record = store.get(isolation_id)
    if record is None:
        return ReleaseResult(
            id=isolation_id,
            status="not_found",
            reason="Isolated directory is not registered",
        )
    if record.state in ("needs_attention", "deleting"):
        # Under the repository lock a deleting record is a leftover, not a deletion in progress.
        return _retry_attention_locked(store, record, workspace_id, confirmation)
    if record.state == "collapsed":
        from agentnave.isolated.collapsed import release_collapsed_locked

        return release_collapsed_locked(store, record, workspace_id, confirmation)
    if record.state != "ready":
        return ReleaseResult(
            id=isolation_id,
            status="blocked",
            reason=f"Isolated directory is in state {record.state}",
        )
    return _inspect_and_delete(store, record, workspace_id, confirmation)


def _inspect_and_delete(
    store: WorktreeStore,
    record: IsolatedRecord,
    workspace_id: str,
    confirmation: str | None,
) -> ReleaseResult:
    """The one release path for a directory that still exists: inspect, confirm, delete."""
    orphan = False
    try:
        main_root, verified_common = verify_repo_identity(record)
        wt_path = Path(record.worktree_path)
        orphan = (
            wt_path.is_dir()
            and not is_link(wt_path)
            and not worktree_link_valid(wt_path, record.worktree_git_dir, verified_common)
        )
        if orphan:
            snap = _inspect_orphan_locked(record, workspace_id, main_root, verified_common)
        else:
            main_root, verified_common, _wt_path, snap = inspect_ready_locked(
                store, record, workspace_id
            )
    except (IsolationError, OSError, ValueError) as exc:
        return ReleaseResult(id=record.id, status="blocked", reason=str(exc))

    foreign = record.workspace_id != workspace_id
    if failure := verify_confirmation(store, record, snap, confirmation, foreign=foreign):
        return failure

    # A directory that is no longer a worktree keeps the oid its deletion was first confirmed at.
    delete_oid = (
        (record.delete_oid or snap.branch_oid or record.base_oid) if orphan else snap.branch_oid
    )
    if not store.mark_deleting(
        record.id,
        phase="removing_worktree",
        delete_oid=delete_oid,
        updated_at=now(),
        expected_state=record.state,
    ):
        return ReleaseResult(
            id=record.id, status="blocked", reason="Isolated directory changed state; retry"
        )
    return _run_deletion(
        store,
        record,
        main_root,
        verified_common,
        delete_oid,
        head_oid=snap.head_oid or None,
        uncommitted=snap.uncommitted,
        unique_commits=snap.unique_commits,
        remove_orphan=orphan,
    )


def _inspect_orphan_locked(
    record: IsolatedRecord, workspace_id: str, main_root: Path, common_dir: Path
) -> InspectionSnapshot:
    """Snapshot a leftover directory that is no longer a git worktree (no Git runs inside it)."""
    if has_active_leases(common_dir, record.id, cleanup_stale=True):
        raise IsolationError("Isolated directory is currently in use by an active lease")
    wt_path = Path(record.worktree_path)
    tip = branch_tip(main_root, record.branch_ref)
    unique = bool(tip) and has_unique_commits(main_root, tip)
    digest = compute_digest(
        record.id,
        workspace_id,
        "orphan",
        tip or "none",
        record.usage_generation,
        b"orphan",
        tree_fingerprint(wt_path),
        unique_commits=unique,
    )
    return InspectionSnapshot(
        head_oid="",
        branch_oid=tip,
        uncommitted=True,
        unique_commits=unique,
        digest=digest,
        orphan=True,
    )


def _run_deletion(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_oid: str,
    *,
    head_oid: str | None = None,
    uncommitted: bool = False,
    unique_commits: bool = False,
    remove_orphan: bool = False,
) -> ReleaseResult:
    try:
        note = complete_deletion_locked(
            store, record, main_root, common_dir, expected_oid, remove_orphan=remove_orphan
        )
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
            head_oid=head_oid,
            uncommitted=uncommitted,
            unique_commits=unique_commits,
            reason=detail,
        )
    return ReleaseResult(
        id=record.id,
        status="deleted",
        head_oid=head_oid,
        uncommitted=uncommitted,
        unique_commits=unique_commits,
        reason=note,
    )


def _retry_attention_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    workspace_id: str,
    confirmation: str | None,
) -> ReleaseResult:
    """Re-check a record left in needs_attention and finish its interrupted deletion or rollback.

    While the directory still exists it goes through the same inspection and confirmation as a
    normal release; an already removed directory is finished without asking unless its branch
    holds unmerged commits.
    """
    stuck = ReleaseResult(
        id=record.id,
        status="needs_attention",
        reason=record.attention or "Isolated directory requires manual attention",
    )
    if not record.delete_oid:
        from agentnave.isolated.collapsed import (
            REBUILDING_COLLAPSED_PHASES,
            release_collapsed_locked,
            rollback_rebuilding_collapsed_locked,
        )

        if record.phase in REBUILDING_COLLAPSED_PHASES:
            if not store.resume_creating(
                record.id, phase="rebuilding_rolling_back", updated_at=now()
            ):
                return stuck
            latest = store.get(record.id)
            if latest is not None:
                rollback_rebuilding_collapsed_locked(store, latest)
            after = store.get(record.id)
            if after is None:
                return ReleaseResult(id=record.id, status="deleted")
            if after.state == "collapsed":
                return release_collapsed_locked(store, after, workspace_id, confirmation)
            return ReleaseResult(id=record.id, status="needs_attention", reason=after.attention)
        if not store.resume_creating(record.id, phase="rolling_back", updated_at=now()):
            return stuck
        latest = store.get(record.id)
        if latest is not None:
            rollback_creating_locked(store, latest)
        after = store.get(record.id)
        if after is None:
            return ReleaseResult(id=record.id, status="deleted")
        return ReleaseResult(id=record.id, status="needs_attention", reason=after.attention)
    wt_path = Path(record.worktree_path)
    if wt_path.exists() or is_link(wt_path):
        return _inspect_and_delete(store, record, workspace_id, confirmation)
    try:
        main_root, common_dir = verify_repo_identity(record, require_base_oid=False)
        if has_active_leases(common_dir, record.id, cleanup_stale=True):
            raise IsolationError("Isolated directory is currently in use by an active lease")
        snap = _inspect_missing_locked(record, workspace_id, main_root)
    except (IsolationError, OSError, ValueError) as exc:
        detail = str(exc)
        store.mark_attention(
            record.id,
            expected_state=record.state,
            phase="delete_failed",
            reason=detail,
            updated_at=now(),
        )
        return ReleaseResult(id=record.id, status="needs_attention", reason=detail)
    foreign = record.workspace_id != workspace_id
    if failure := verify_confirmation(store, record, snap, confirmation, foreign=foreign):
        return failure
    if not store.mark_deleting(
        record.id,
        phase="removing_worktree",
        delete_oid=record.delete_oid,
        updated_at=now(),
        expected_state=record.state,
    ):
        return stuck
    return _run_deletion(
        store,
        record,
        main_root,
        common_dir,
        record.delete_oid,
        unique_commits=snap.unique_commits,
    )


def _inspect_missing_locked(
    record: IsolatedRecord, workspace_id: str, main_root: Path
) -> InspectionSnapshot:
    """Snapshot a leftover whose directory is gone: only its branch can still hold work."""
    expected_oid = record.delete_oid or ""
    tip = branch_tip(main_root, record.branch_ref)
    unique = bool(tip) and has_unique_commits(main_root, tip)
    digest = compute_digest(
        record.id,
        workspace_id,
        "missing",
        expected_oid,
        record.usage_generation,
        b"missing",
        tip.encode("ascii"),
        unique_commits=unique,
    )
    return InspectionSnapshot(
        head_oid="",
        branch_oid=expected_oid,
        uncommitted=False,
        unique_commits=unique,
        digest=digest,
    )


def verify_confirmation(
    store: WorktreeStore,
    record: IsolatedRecord,
    snap: InspectionSnapshot,
    confirmation: str | None,
    *,
    foreign: bool,
) -> ReleaseResult | None:
    """Pass only with a token this service issued earlier for exactly the current state."""
    if not (snap.uncommitted or snap.unique_commits or foreign or snap.nested or snap.orphan):
        return None
    issued = record.confirmation_digest
    if (
        confirmation is not None
        and issued is not None
        and hmac.compare_digest(confirmation.encode(), issued.encode())
        and hmac.compare_digest(confirmation.encode(), snap.digest.encode())
    ):
        return None
    store.save_confirmation(record.id, snap.digest, updated_at=now())
    reasons: list[str] = []
    if snap.orphan:
        reasons.append(ORPHAN_NOTE)
    if snap.nested:
        reasons.append(NESTED_NOTE)
    if confirmation is not None:
        reasons.append("Confirmation token expired or does not match current worktree state")
    elif foreign:
        reasons.append("Directory belongs to another workspace; confirm to release it")
    elif snap.uncommitted or snap.unique_commits:
        reasons.append("Confirmation required for uncommitted files or unmerged commits")
    return ReleaseResult(
        id=record.id,
        status="confirmation_required",
        head_oid=snap.head_oid or None,
        uncommitted=snap.uncommitted,
        unique_commits=snap.unique_commits,
        confirmation=snap.digest,
        reason="; ".join(reasons),
        nested_repos=snap.nested,
    )


def verify_untouched_worktree_for_rollback(
    wt_path: Path,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_head: str,
) -> bool:
    """Verify wt_path is an untouched worktree of record; return True if registered in Git."""
    worktrees = parse_worktrees(
        run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
    )
    wt_matches = [
        w
        for w in worktrees
        if (w.path.exists() and wt_path.exists() and same_dir(w.path, wt_path))
        or w.path.resolve() == wt_path.resolve()
    ]
    if not wt_matches:
        if wt_path.exists():
            raise IsolationError(f"Unregistered path exists at {wt_path}")
        return False
    entry = wt_matches[0]
    if entry.branch_ref != record.branch_ref or entry.head_oid != expected_head:
        raise IsolationError("Unfinished worktree HEAD or branch moved from base")
    if wt_path.is_dir():
        linked = (
            worktree_link_valid(wt_path, record.worktree_git_dir, common_dir)
            if record.worktree_git_dir is not None
            else foreign_link_valid(wt_path)
        )
        if not linked:
            raise IsolationError("Unfinished directory is not a valid git worktree")
        verify_worktree_root(wt_path)
        if never_checked_out(wt_path):
            # Interrupted between adding and checking out. Without an index git reports
            # every file as deleted, so only a directory holding nothing but its .git
            # link is proven untouched; anything else may be someone's work.
            if any(item.name != ".git" for item in wt_path.iterdir()):
                raise IsolationError("Unfinished worktree was never checked out but holds files")
        else:
            _check_hidden_index_flags(wt_path)
            status_raw = run_git(_STATUS_ARGS, cwd=wt_path).stdout
            if status_raw:
                raise IsolationError("Unfinished worktree contains uncommitted source changes")
            if nested := nested_repositories(wt_path, status_raw):
                raise IsolationError(
                    f"Unfinished worktree contains nested repositories: {', '.join(nested)}"
                )
    return True


def rollback_creating_locked(store: WorktreeStore, record: IsolatedRecord) -> None:
    """Roll back an unfinished creation when proven untouched; else mark needs_attention."""
    from agentnave.isolated.collapsed import (
        REBUILDING_COLLAPSED_PHASES,
        rollback_rebuilding_collapsed_locked,
    )

    if record.phase in REBUILDING_COLLAPSED_PHASES:
        rollback_rebuilding_collapsed_locked(store, record)
        return
    try:
        main_root, common_dir = verify_repo_identity(record)
        if has_active_leases(common_dir, record.id, cleanup_stale=True):
            raise IsolationError("Active lease exists on unfinished isolated directory")
        wt_path = Path(record.worktree_path)
        if is_link(wt_path):
            raise IsolationError(f"Unsafe link at {wt_path}")
        if verify_untouched_worktree_for_rollback(
            wt_path, record, main_root, common_dir, record.base_oid
        ):
            run_git(["worktree", "remove", "--force", "--", str(wt_path)], cwd=main_root)

        if ref_exists(main_root, record.branch_ref):
            remaining = parse_worktrees(
                run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
            )
            if any(w.branch_ref == record.branch_ref for w in remaining):
                raise IsolationError("Managed branch is checked out in another worktree")
            run_git(
                ["update-ref", "--no-deref", "-d", record.branch_ref, record.base_oid],
                cwd=main_root,
            )
        store.delete(record.id, expected_state=record.state)
        remove_lease_dir_locked(common_dir, record.id)
    except (IsolationError, OSError, ValueError) as exc:
        store.mark_attention(
            record.id,
            expected_state=record.state,
            phase="rollback_failed",
            reason=str(exc),
            updated_at=now(),
        )


def recover_record(store: WorktreeStore, record: IsolatedRecord) -> RecoveryOutcome:
    """Recover one interrupted `creating` or `deleting` record at server startup.

    Without the repository lock nothing is written: the record keeps its state for next time.
    """
    common_dir = Path(record.repo_common_dir)
    if is_link(common_dir) or not common_dir.is_dir():
        from agentnave.isolated.collapsed import REBUILDING_COLLAPSED_PHASES

        reason = f"Repository common-dir is unavailable: {common_dir}"
        phase = (
            "rebuilding_recovery_failed"
            if record.phase in REBUILDING_COLLAPSED_PHASES
            else "recovery_failed"
        )
        store.mark_attention(
            record.id,
            expected_state=record.state,
            phase=phase,
            reason=reason,
            updated_at=now(),
        )
        return RecoveryOutcome(record.id, record.state, "needs_attention", reason)
    try:
        with repo_lock(common_dir):
            return _recover_locked(store, record)
    except (IsolationError, OSError, ValueError) as exc:
        return RecoveryOutcome(record.id, record.state, "skipped", str(exc))


def _recover_locked(store: WorktreeStore, record: IsolatedRecord) -> RecoveryOutcome:
    latest = store.get(record.id)
    if latest is None:
        return RecoveryOutcome(
            record.id,
            record.state,
            "rolled_back" if record.state == "creating" else "deleted",
        )
    if latest.state == "creating":
        from agentnave.isolated.collapsed import (
            REBUILDING_COLLAPSED_PHASES,
            rollback_rebuilding_collapsed_locked,
        )

        if latest.phase in REBUILDING_COLLAPSED_PHASES:
            rollback_rebuilding_collapsed_locked(store, latest)
            after = store.get(record.id)
            if after is not None and after.state == "collapsed":
                return RecoveryOutcome(record.id, "creating", "rolled_back")
            return RecoveryOutcome(
                record.id,
                "creating",
                "needs_attention",
                after.attention if after is not None else "Collapsed rebuild vanished",
            )
        rollback_creating_locked(store, latest)
        after = store.get(record.id)
        if after is None:
            return RecoveryOutcome(record.id, "creating", "rolled_back")
        return RecoveryOutcome(record.id, "creating", "needs_attention", after.attention)
    if latest.state == "deleting":
        return _recover_deleting_locked(store, latest)
    if latest.state in ("ready", "collapsed"):
        return RecoveryOutcome(
            record.id,
            latest.state,
            "skipped",
            f"Directory is already {latest.state}",
        )
    return RecoveryOutcome(
        record.id,
        latest.state,
        "needs_attention",
        latest.attention or f"Unexpected state {latest.state}",
    )


def _recover_deleting_locked(store: WorktreeStore, latest: IsolatedRecord) -> RecoveryOutcome:
    def fail(reason: str) -> RecoveryOutcome:
        store.mark_attention(
            latest.id,
            expected_state="deleting",
            phase="recovery_failed",
            reason=reason,
            updated_at=now(),
        )
        return RecoveryOutcome(latest.id, "deleting", "needs_attention", reason)

    wt_path = Path(latest.worktree_path)
    if wt_path.exists() or is_link(wt_path):
        # The directory may have changed since the deletion was confirmed: never delete it
        # here. A release call checks it again like any other.
        return fail("Directory still exists after an interrupted release; release it again")
    if not latest.delete_oid:
        return fail("Deleting record is missing expected branch OID")
    try:
        main_root, verified_common = verify_repo_identity(latest, require_base_oid=False)
        tip = branch_tip(main_root, latest.branch_ref)
        if tip and has_unique_commits(main_root, tip):
            # Another branch that held these commits may have gone since the confirmation.
            return fail("Managed branch holds unmerged commits; release it again to confirm")
        note = complete_deletion_locked(
            store, latest, main_root, verified_common, latest.delete_oid
        )
    except (IsolationError, OSError, ValueError) as exc:
        return fail(str(exc))
    return RecoveryOutcome(latest.id, "deleting", "deleted", note)
