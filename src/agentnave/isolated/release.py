"""Safety inspection, confirmation digests, explicit release, summaries, and recovery."""

from __future__ import annotations

import hashlib
import hmac
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

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
    parse_worktrees,
    ref_exists,
    run_git,
    same_dir,
)
from agentnave.isolated.store import (
    MANAGED_BRANCH_PREFIX,
    IsolatedRecord,
    WorktreeStore,
)
from agentnave.store import now

MAX_SUMMARY_FILES = 200

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
type SummaryStatus = Literal["complete", "no_baseline", "error"]
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


@dataclass(frozen=True, slots=True)
class ChangeSummary:
    id: str
    path: str
    branch: str
    summary_status: SummaryStatus
    head_oid: str | None = None
    actual_branch: str | None = None
    changed_files_from_base: tuple[str, ...] = ()
    truncated: bool = False
    uncommitted: bool | None = None
    error: str | None = None


@dataclass(frozen=True, slots=True)
class RecoveryOutcome:
    id: str
    previous_state: str
    status: RecoveryStatus
    reason: str | None = None


@dataclass(frozen=True, slots=True)
class _InspectionSnapshot:
    head_oid: str
    branch_oid: str
    uncommitted: bool
    unique_commits: bool
    digest: str


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


def _compute_digest(
    isolation_id: str,
    workspace_id: str,
    head_oid: str,
    branch_oid: str,
    usage_generation: int,
    status_raw: bytes,
    stat_bytes: bytes,
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
    ):
        hasher.update(f"{len(part)}:".encode("ascii"))
        hasher.update(part)
        hasher.update(b";")
    return hasher.hexdigest()


def _verify_repo_identity(record: IsolatedRecord) -> tuple[Path, Path]:
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
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=main_root
            ).stdout.rstrip(b"\r\n")
        )
    )
    if not same_dir(common_dir, actual_common):
        raise IsolationError("Main worktree no longer belongs to the recorded repository")
    run_git(["cat-file", "-e", f"{record.base_oid}^{{commit}}"], cwd=main_root)
    container = main_root / WORKTREES_DIR_NAME
    if is_link(container):
        raise IsolationError(f"Unsafe link at {container}")
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
    if not same_dir(wt_path.parent, main_root / WORKTREES_DIR_NAME):
        raise IsolationError("Isolated worktree is not directly under .agentnave-worktrees")
    wt_common = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=wt_path
            ).stdout.rstrip(b"\r\n")
        )
    )
    wt_git_dir = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-dir"], cwd=wt_path
            ).stdout.rstrip(b"\r\n")
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


def _inspect_ready_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    workspace_id: str,
) -> tuple[Path, Path, Path, _InspectionSnapshot]:
    main_root, common_dir = _verify_repo_identity(record)
    if has_active_leases(common_dir, record.id, cleanup_stale=True):
        raise IsolationError("Isolated directory is currently in use by an active lease")
    wt_path = _verify_worktree_identity(store, record, main_root, common_dir)
    sym_res = run_git(["symbolic-ref", "-q", "HEAD"], cwd=wt_path, allowed_exit_codes=(0, 1))
    actual_ref = decode_git_path(sym_res.stdout.rstrip(b"\r\n")) if sym_res.returncode == 0 else ""
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
    status_raw = run_git(
        ["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=none"],
        cwd=wt_path,
    ).stdout
    containing = (
        run_git(
            [
                "for-each-ref",
                f"--contains={head_oid}",
                "--format=%(refname)",
                "refs/heads/",
                "refs/remotes/",
            ],
            cwd=wt_path,
        )
        .stdout.decode("utf-8", errors="replace")
        .splitlines()
    )
    durable_refs = [ref for ref in containing if ref and not ref.startswith(MANAGED_BRANCH_PREFIX)]
    paths, _ = porcelain_v2_paths(status_raw)
    stat_bytes = _stat_fingerprint(wt_path, paths)
    # The token is bound to the workspace asking, so one workspace's token never confirms
    # another's release.
    digest = _compute_digest(
        record.id,
        workspace_id,
        head_oid,
        branch_oid,
        record.usage_generation,
        status_raw,
        stat_bytes,
    )
    snapshot = _InspectionSnapshot(
        head_oid=head_oid,
        branch_oid=branch_oid,
        uncommitted=bool(status_raw),
        unique_commits=not durable_refs,
        digest=digest,
    )
    return main_root, common_dir, wt_path, snapshot


def _complete_deletion_locked(
    store: WorktreeStore,
    record: IsolatedRecord,
    main_root: Path,
    common_dir: Path,
    expected_oid: str,
) -> str | None:
    """Remove worktree, managed branch, and DB row; raises IsolationError on partial failure.

    Returns a note when the managed branch was kept because it moved after the release check.
    """
    if has_active_leases(common_dir, record.id, cleanup_stale=True):
        raise IsolationError("Active lease prevents completing deletion")
    wt_path = Path(record.worktree_path)
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
        run_git(["worktree", "remove", "--force", "--", str(wt_path)], cwd=main_root)
    elif wt_path.exists() or is_link(wt_path):
        raise IsolationError(f"Unregistered path remains at {wt_path}; refusing recursive removal")

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
    run_git(["update-ref", "-d", record.branch_ref, expected_oid], cwd=main_root)
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
    try:
        main_root, verified_common, _wt_path, snap = _inspect_ready_locked(
            store, record, workspace_id
        )
    except (IsolationError, OSError, ValueError) as exc:
        return ReleaseResult(id=record.id, status="blocked", reason=str(exc))

    foreign = record.workspace_id != workspace_id
    if failure := _verify_confirmation(store, record, snap, confirmation, foreign=foreign):
        return failure

    if not store.mark_deleting(
        record.id,
        phase="removing_worktree",
        delete_oid=snap.branch_oid,
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
        snap.branch_oid,
        head_oid=snap.head_oid,
        uncommitted=snap.uncommitted,
        unique_commits=snap.unique_commits,
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
) -> ReleaseResult:
    try:
        note = _complete_deletion_locked(store, record, main_root, common_dir, expected_oid)
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
    normal release; only an already removed directory is finished without asking.
    """
    stuck = ReleaseResult(
        id=record.id,
        status="needs_attention",
        reason=record.attention or "Isolated directory requires manual attention",
    )
    if not record.delete_oid:
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
        main_root, common_dir = _verify_repo_identity(record)
    except (IsolationError, OSError, ValueError) as exc:
        return ReleaseResult(id=record.id, status="needs_attention", reason=str(exc))
    if not store.mark_deleting(
        record.id,
        phase="removing_worktree",
        delete_oid=record.delete_oid,
        updated_at=now(),
        expected_state=record.state,
    ):
        return stuck
    return _run_deletion(store, record, main_root, common_dir, record.delete_oid)


def _verify_confirmation(
    store: WorktreeStore,
    record: IsolatedRecord,
    snap: _InspectionSnapshot,
    confirmation: str | None,
    *,
    foreign: bool,
) -> ReleaseResult | None:
    """Pass only with a token this service issued earlier for exactly the current state."""
    if not (snap.uncommitted or snap.unique_commits or foreign):
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
    if confirmation is not None:
        reason = "Confirmation token expired or does not match current worktree state"
    elif foreign:
        reason = "Directory belongs to another workspace; confirm to release it"
    else:
        reason = "Confirmation required for uncommitted files or unmerged commits"
    return ReleaseResult(
        id=record.id,
        status="confirmation_required",
        head_oid=snap.head_oid,
        uncommitted=snap.uncommitted,
        unique_commits=snap.unique_commits,
        confirmation=snap.digest,
        reason=reason,
    )


def rollback_creating_locked(store: WorktreeStore, record: IsolatedRecord) -> None:
    """Roll back an unfinished creation when proven untouched; else mark needs_attention."""
    try:
        main_root, common_dir = _verify_repo_identity(record)
        if has_active_leases(common_dir, record.id, cleanup_stale=True):
            raise IsolationError("Active lease exists on unfinished isolated directory")
        wt_path = Path(record.worktree_path)
        if is_link(wt_path):
            raise IsolationError(f"Unsafe link at {wt_path}")
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
            if entry.branch_ref != record.branch_ref or entry.head_oid != record.base_oid:
                raise IsolationError("Unfinished worktree HEAD or branch moved from base")
            if wt_path.is_dir():
                dirty = run_git(
                    [
                        "status",
                        "--porcelain=v2",
                        "-z",
                        "--untracked-files=all",
                        "--ignore-submodules=none",
                    ],
                    cwd=wt_path,
                ).stdout
                if dirty:
                    raise IsolationError("Unfinished worktree contains uncommitted source changes")
            run_git(["worktree", "remove", "--force", "--", str(wt_path)], cwd=main_root)
        elif wt_path.exists():
            raise IsolationError(f"Unregistered path exists at {wt_path}")

        if ref_exists(main_root, record.branch_ref):
            remaining = parse_worktrees(
                run_git(["worktree", "list", "--porcelain", "-z"], cwd=main_root).stdout
            )
            if any(w.branch_ref == record.branch_ref for w in remaining):
                raise IsolationError("Managed branch is checked out in another worktree")
            run_git(["update-ref", "-d", record.branch_ref, record.base_oid], cwd=main_root)
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
        reason = f"Repository common-dir is unavailable: {common_dir}"
        store.mark_attention(
            record.id,
            expected_state=record.state,
            phase="recovery_failed",
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
        rollback_creating_locked(store, latest)
        after = store.get(record.id)
        if after is None:
            return RecoveryOutcome(record.id, "creating", "rolled_back")
        return RecoveryOutcome(record.id, "creating", "needs_attention", after.attention)
    if latest.state == "deleting":
        return _recover_deleting_locked(store, latest)
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

    if not latest.delete_oid:
        return fail("Deleting record is missing expected branch OID")
    try:
        main_root, verified_common = _verify_repo_identity(latest)
        note = _complete_deletion_locked(
            store, latest, main_root, verified_common, latest.delete_oid
        )
    except (IsolationError, OSError, ValueError) as exc:
        return fail(str(exc))
    return RecoveryOutcome(latest.id, "deleting", "deleted", note)


def _changed_paths(wt_path: Path, base_oid: str, status_raw: bytes) -> list[str]:
    """Files changed from base_oid: committed, staged, working tree, and untracked."""
    paths, _ = porcelain_v2_paths(status_raw)
    found = set(paths)
    diff_head = ["diff", "--no-ext-diff", "--no-textconv", "--name-only", "-z"]
    for revisions in ([base_oid, "HEAD"], ["--cached", base_oid], [base_oid]):
        raw = run_git([*diff_head, *revisions, "--"], cwd=wt_path).stdout
        found.update(decode_git_path(item) for item in raw.split(b"\x00") if item)
    return sorted(found)


def _read_head_and_branch(wt_path: Path) -> tuple[str, str | None]:
    head_oid = (
        run_git(["rev-parse", "--verify", "HEAD^{commit}"], cwd=wt_path)
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    sym_res = run_git(["symbolic-ref", "-q", "HEAD"], cwd=wt_path, allowed_exit_codes=(0, 1))
    actual_branch: str | None = None
    if sym_res.returncode == 0 and sym_res.stdout.strip():
        raw_ref = decode_git_path(sym_res.stdout.rstrip(b"\r\n"))
        actual_branch = raw_ref.removeprefix("refs/heads/")
    return head_oid, actual_branch


_STATUS_ARGS = [
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=none",
]


def summarize_record_changes(record: IsolatedRecord) -> ChangeSummary:
    """Collect cumulative changes from base_oid without raising to the caller."""
    wt_path = Path(record.worktree_path)
    try:
        if is_link(wt_path) or not wt_path.is_dir():
            raise IsolationError(f"Isolated worktree is missing or unsafe: {wt_path}")
        head_oid, actual_branch = _read_head_and_branch(wt_path)
        status_raw = run_git(_STATUS_ARGS, cwd=wt_path).stdout
        combined = _changed_paths(wt_path, record.base_oid, status_raw)
        return ChangeSummary(
            id=record.id,
            path=record.worktree_path,
            branch=record.branch,
            summary_status="complete",
            head_oid=head_oid,
            actual_branch=actual_branch,
            changed_files_from_base=tuple(combined[:MAX_SUMMARY_FILES]),
            truncated=len(combined) > MAX_SUMMARY_FILES,
            uncommitted=bool(status_raw),
        )
    except (IsolationError, OSError, ValueError) as exc:
        return ChangeSummary(
            id=record.id,
            path=record.worktree_path,
            branch=record.branch,
            summary_status="error",
            error=str(exc),
        )


def summarize_unregistered(isolation_id: str, wt_path: Path, branch: str) -> ChangeSummary:
    """Summary without a start commit, for a directory this data root has no registration of."""
    try:
        if is_link(wt_path) or not wt_path.is_dir():
            raise IsolationError(f"Isolated worktree is missing or unsafe: {wt_path}")
        head_oid, actual_branch = _read_head_and_branch(wt_path)
        status_raw = run_git(_STATUS_ARGS, cwd=wt_path).stdout
        return ChangeSummary(
            id=isolation_id,
            path=str(wt_path),
            branch=branch,
            summary_status="no_baseline",
            head_oid=head_oid,
            actual_branch=actual_branch,
            uncommitted=bool(status_raw),
        )
    except (IsolationError, OSError, ValueError) as exc:
        return ChangeSummary(
            id=isolation_id,
            path=str(wt_path),
            branch=branch,
            summary_status="error",
            error=str(exc),
        )
