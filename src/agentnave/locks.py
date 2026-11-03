"""Nonblocking OS file locks shared by the invocation core and the conversation store."""

from __future__ import annotations

import os
from contextlib import suppress
from pathlib import Path
from typing import IO


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
