"""Nonblocking OS file locks shared by the invocation core and the conversation store."""

from __future__ import annotations

import os
from pathlib import Path
from typing import IO


def lock_file(path: Path, busy_message: str) -> IO[bytes]:
    """Acquire a nonblocking OS lock; close the returned handle to release it."""
    lock = path.open("a+b")
    try:
        if os.name == "nt":
            import msvcrt

            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lock.close()
        raise ValueError(busy_message) from None
    return lock
