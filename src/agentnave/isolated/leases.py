"""Repository-level serialization lock and per-worktree OS usage leases."""

from __future__ import annotations

import os
import stat
import time
from collections.abc import Collection, Generator
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import IO, Self
from uuid import uuid4

from agentnave.isolated.store import ID_PATTERN
from agentnave.locks import lock_file, release_and_remove

_LOCK_RETRY_SECONDS = 0.025


class RepoLockUnavailable(ValueError):
    """The repository lock could not be taken in time; nothing was done under it."""


def _is_link(path: Path) -> bool:
    return path.is_symlink() or os.path.isjunction(path)


def _check_safe_dir(path: Path) -> None:
    if _is_link(path):
        raise ValueError(f"Unsafe link in repository lock directory: {path}")
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISDIR(info.st_mode):
        raise ValueError(f"Expected directory for repository state: {path}")


def _check_safe_regular(path: Path) -> None:
    if _is_link(path):
        raise ValueError(f"Unsafe link at lock path: {path}")
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f"Expected regular lock file: {path}")


def _state_dir(repo_common_dir: Path) -> Path:
    if _is_link(repo_common_dir) or not repo_common_dir.is_dir():
        raise ValueError(f"Invalid Git common directory: {repo_common_dir}")
    directory = repo_common_dir / "agentnave"
    _check_safe_dir(directory)
    return directory


def _leases_dir(repo_common_dir: Path, isolation_id: str) -> Path:
    if not ID_PATTERN.fullmatch(isolation_id):
        raise ValueError(f"Invalid isolated directory id: {isolation_id!r}")
    base = _state_dir(repo_common_dir) / "leases"
    _check_safe_dir(base)
    target = base / isolation_id
    _check_safe_dir(target)
    return target


def _take_repo_lock(repo_common_dir: Path, timeout_seconds: float) -> IO[bytes]:
    busy = f"Repository isolated-directory lock is busy: {repo_common_dir}"
    deadline = time.monotonic() + max(0.0, timeout_seconds)
    try:
        state = _state_dir(repo_common_dir)
        state.mkdir(mode=0o700, parents=True, exist_ok=True)
        _check_safe_dir(state)
        lock_path = state / "lock"
        _check_safe_regular(lock_path)
    except (OSError, ValueError) as exc:
        raise RepoLockUnavailable(str(exc)) from exc
    while True:
        try:
            _check_safe_regular(lock_path)
            return lock_file(lock_path, busy)
        except ValueError as exc:
            if time.monotonic() >= deadline:
                raise RepoLockUnavailable(str(exc)) from exc
            time.sleep(_LOCK_RETRY_SECONDS)


@contextmanager
def repo_lock(
    repo_common_dir: Path,
    *,
    timeout_seconds: float = 10.0,
) -> Generator[None]:
    """Serialize create, claim, and release operations across processes for one repository."""
    handle = _take_repo_lock(repo_common_dir, timeout_seconds)
    try:
        yield
    finally:
        # Never unlink the repository lock file so concurrent waiters lock the same inode.
        handle.close()


@dataclass
class IsolationLease:
    """Process-held OS file lock keeping an isolated worktree from being released while in use."""

    isolation_id: str
    path: Path
    _handle: IO[bytes]

    @property
    def closed(self) -> bool:
        return self._handle.closed

    def release(self) -> None:
        release_and_remove(self._handle, self.path)

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        _exc_type: type[BaseException] | None,
        _exc: BaseException | None,
        _tb: TracebackType | None,
    ) -> None:
        self.release()


def acquire_lease_locked(repo_common_dir: Path, isolation_id: str) -> IsolationLease:
    """Create and hold a new lease file; caller must already hold repo_lock(repo_common_dir)."""
    directory = _leases_dir(repo_common_dir, isolation_id)
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    _check_safe_dir(directory)
    for _ in range(8):
        path = directory / f"{uuid4().hex}.lock"
        if path.exists() or _is_link(path):
            continue
        handle = lock_file(path, f"Isolated directory lease collision: {isolation_id}")
        return IsolationLease(isolation_id=isolation_id, path=path, _handle=handle)
    raise ValueError(f"Could not allocate lease file for {isolation_id}")


def has_active_leases(
    repo_common_dir: Path,
    isolation_id: str,
    *,
    cleanup_stale: bool = False,
) -> bool:
    """Return True if any live process holds a lease on isolation_id.

    Pass cleanup_stale=True only while holding repo_lock(repo_common_dir) so dead lease files
    left behind by crashed processes are reclaimed without racing active acquisitions.
    """
    directory = _leases_dir(repo_common_dir, isolation_id)
    if not directory.exists():
        return False
    for entry in sorted(directory.iterdir()):
        if entry.suffix != ".lock":
            raise ValueError(f"Unexpected entry in lease directory: {entry}")
        _check_safe_regular(entry)
        try:
            guard = lock_file(entry, "Isolated directory is in use", create=False)
        except FileNotFoundError:
            continue
        except ValueError:
            return True
        if cleanup_stale:
            release_and_remove(guard, entry)
        else:
            guard.close()
    return False


def remove_lease_dir_locked(repo_common_dir: Path, isolation_id: str) -> None:
    """Reclaim an empty lease directory once no active leases remain (under repo_lock)."""
    directory = _leases_dir(repo_common_dir, isolation_id)
    if not directory.exists():
        return
    if has_active_leases(repo_common_dir, isolation_id, cleanup_stale=True):
        raise ValueError(f"Isolated directory {isolation_id} still has an active lease")
    with suppress(OSError):
        directory.rmdir()


def other_active_leases(
    repo_common_dir: Path,
    isolation_id: str,
    *,
    exclude_paths: Collection[Path] = (),
) -> int:
    """Count active leases on isolation_id excluding known lease files of this process."""
    directory = _leases_dir(repo_common_dir, isolation_id)
    if not directory.exists():
        return 0
    excluded = {path.resolve() for path in exclude_paths}
    active = 0
    for entry in sorted(directory.iterdir()):
        if entry.suffix != ".lock":
            continue
        if entry.resolve() in excluded:
            continue
        _check_safe_regular(entry)
        try:
            guard = lock_file(entry, "Isolated directory is in use", create=False)
        except FileNotFoundError:
            continue
        except ValueError:
            active += 1
            continue
        guard.close()
    return active
