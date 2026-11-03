"""Nonblocking OS file locks shared by the invocation core and the conversation store."""

from __future__ import annotations

import logging
import os
import re
import shutil
import stat
from collections.abc import Generator
from contextlib import ExitStack, contextmanager, suppress
from pathlib import Path
from typing import IO

_LOG = logging.getLogger(__name__)
_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")


def lock_file(path: Path, busy_message: str, *, create: bool = True) -> IO[bytes]:
    """Acquire a nonblocking OS lock; close the returned handle to release it."""
    for _ in range(5):
        lock = path.open("a+b" if create else "r+b")
        verified = False
        try:
            if os.name == "nt":
                import msvcrt

                if os.fstat(lock.fileno()).st_size == 0:
                    lock.write(b"0")
                    lock.flush()
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            opened = os.fstat(lock.fileno())
            try:
                current = path.stat()
            except FileNotFoundError:
                continue
            if (opened.st_dev, opened.st_ino) == (current.st_dev, current.st_ino):
                verified = True
                return lock
        except OSError:
            raise ValueError(busy_message) from None
        finally:
            # A cleaner may have unlinked this inode while we were opening it.
            # A lock on the detached inode does not protect the current pathname.
            if not verified:
                lock.close()
    raise ValueError(busy_message)


def release_and_remove(handle: IO[bytes], path: Path) -> None:
    """Release a disposable lock once, without unlinking a later holder on repeat calls."""
    if handle.closed:
        return
    if os.name == "nt":
        import msvcrt

        try:
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        finally:
            handle.close()
        # Python's Windows open does not share deletion with other openers.
        with suppress(OSError):
            path.unlink(missing_ok=True)
    else:
        try:
            path.unlink(missing_ok=True)
        finally:
            handle.close()


def conversation_lock_path(root: Path, id_: str) -> Path | None:
    if not _ID.fullmatch(id_):
        return None
    path = root / "locks" / f"{id_}.lock"
    try:
        if not stat.S_ISREG(path.lstat().st_mode):
            return None
    except FileNotFoundError:
        pass
    return path


def _make_removable(path: Path) -> None:
    """Let a seat tree be deleted: CLIs may leave read-only or unlistable directories, and on
    Windows read-only files. Symlinks and junctions are never followed or changed."""
    owner = stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR
    os.chmod(path, owner)
    for top, directories, files in os.walk(path):
        # Prune links and Windows junctions so the walk never reaches outside the seat.
        directories[:] = [
            name
            for name in directories
            if not os.path.islink(os.path.join(top, name))
            and not os.path.isjunction(os.path.join(top, name))
        ]
        for name in directories:
            os.chmod(os.path.join(top, name), owner)
        if os.name == "nt":
            for name in files:
                child = os.path.join(top, name)
                if not os.path.islink(child):
                    os.chmod(child, stat.S_IREAD | stat.S_IWRITE)


def contained_directory(directory: Path) -> bool:
    """A symlinked seats or locks directory leads outside the data root: never sweep it."""
    return not directory.is_symlink()


def remove_seat(root: Path, id_: str) -> bool:
    path = root / "seats" / id_
    if not contained_directory(path.parent):
        return False
    try:
        if path.is_symlink():
            path.unlink()
        elif path.is_junction():
            os.rmdir(path)  # removes the junction itself, never its target
        elif path.exists():
            _make_removable(path)
            shutil.rmtree(path)
        return True
    except Exception:
        _LOG.exception("History retention could not remove seat directory %s", path)
        return False


@contextmanager
def locked_conversations(root: Path, ids: list[str]) -> Generator[None]:
    """Hold the same OS locks as App and server writers, before a DB transaction."""
    if not contained_directory(root / "locks"):
        raise ValueError("Unsafe conversation lock directory")
    with ExitStack() as locks:
        for id_ in ids:
            path = conversation_lock_path(root, id_)
            if path is None:
                raise ValueError("Unsafe conversation lock")
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            locks.enter_context(lock_file(path, "Conversation is in use"))
        yield


def remove_seats(root: Path, ids: list[str]) -> None:
    """Reclaim deleted conversations' directories while their OS locks are still held."""
    for id_ in ids:
        remove_seat(root, id_)
