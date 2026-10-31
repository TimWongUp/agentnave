"""Lifetime lease for App-owned version directories; standard library only."""

from __future__ import annotations

import contextlib
import os
import sys
from collections.abc import Iterator
from pathlib import Path

if os.name == "nt":
    import ctypes
    import ctypes.wintypes
    import msvcrt
else:
    import fcntl


@contextlib.contextmanager
def hold(prefix: Path | None = None) -> Iterator[None]:
    """Keep this relay's original environment, including its future children, installed."""
    env = prefix or Path(sys.prefix)
    root = env.parent.parent
    if env.name != "agentnave" or env.parent.name != "tools" or root.parent.name != "service":
        yield
        return
    # Old/manual installations without this protocol remain uncollectable. The App creates
    # the lock before publishing the marker; once marked, a missing lock must fail closed.
    if not (root / "gc-v1").is_file():
        yield
        return
    with (root.parent / ".leases" / f"{root.name}.lock").open("r+b") as lock:
        if os.name == "nt":

            class Overlapped(ctypes.Structure):
                _fields_ = [
                    ("Internal", ctypes.c_size_t),
                    ("InternalHigh", ctypes.c_size_t),
                    ("Offset", ctypes.wintypes.DWORD),
                    ("OffsetHigh", ctypes.wintypes.DWORD),
                    ("hEvent", ctypes.wintypes.HANDLE),
                ]

            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            acquire = kernel.LockFileEx
            acquire.argtypes = [
                ctypes.wintypes.HANDLE,
                ctypes.wintypes.DWORD,
                ctypes.wintypes.DWORD,
                ctypes.wintypes.DWORD,
                ctypes.wintypes.DWORD,
                ctypes.POINTER(Overlapped),
            ]
            acquire.restype = ctypes.wintypes.BOOL
            overlapped = Overlapped()
            # Shared, nonblocking: a collector holding the exclusive lock wins this startup.
            if not acquire(
                msvcrt.get_osfhandle(lock.fileno()), 1, 0, 1, 0, ctypes.byref(overlapped)
            ):
                raise ctypes.WinError(ctypes.get_last_error())
        else:
            fcntl.flock(lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
        # Closing the file releases the OS lock even after a crash. Never unlink the lock inode.
        yield
