"""Read POSIX process metadata for best-effort host identification."""

from __future__ import annotations

import ctypes
import errno
import os
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class _ProcessInfo:
    ppid: int
    uid: int


def _linux_info(pid: int) -> _ProcessInfo:
    root = Path(f"/proc/{pid}")
    # comm can contain spaces and parentheses; fields after the last ')' are fixed.
    fields = (root / "stat").read_text().rsplit(")", 1)[-1].split()
    status = (root / "status").read_text()
    uid = next(line.split()[2] for line in status.splitlines() if line.startswith("Uid:"))
    return _ProcessInfo(int(fields[1]), int(uid))


def _linux_argv(pid: int) -> Sequence[str]:
    return [
        os.fsdecode(arg) for arg in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if arg
    ]


class _ProcBsdShortInfo(ctypes.Structure):
    # Layout from macOS sys/proc_info.h, PROC_PIDT_SHORTBSDINFO.
    _fields_ = [
        ("pid", ctypes.c_uint32),
        ("ppid", ctypes.c_uint32),
        ("pgid", ctypes.c_uint32),
        ("status", ctypes.c_uint32),
        ("comm", ctypes.c_char * 16),
        ("flags", ctypes.c_uint32),
        ("uid", ctypes.c_uint32),
        ("gid", ctypes.c_uint32),
        ("ruid", ctypes.c_uint32),
        ("rgid", ctypes.c_uint32),
        ("svuid", ctypes.c_uint32),
        ("svgid", ctypes.c_uint32),
        ("rfu", ctypes.c_uint32),
    ]


def _macos_info(pid: int) -> _ProcessInfo:
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    libproc.proc_pidinfo.argtypes = (
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_int,
    )
    libproc.proc_pidinfo.restype = ctypes.c_int
    # The short flavor also answers for setuid ancestors such as `login` or launchd, whose full
    # PROC_PIDTBSDINFO is denied to us; both flavors report the effective uid compared below.
    info = _ProcBsdShortInfo()
    size = ctypes.sizeof(info)
    if libproc.proc_pidinfo(pid, 13, 0, ctypes.byref(info), size) != size:
        error = ctypes.get_errno() or errno.EIO
        raise OSError(error, os.strerror(error))
    return _ProcessInfo(info.ppid, info.uid)


def _macos_argv(pid: int) -> Sequence[str]:
    libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    libc.sysctl.argtypes = (
        ctypes.POINTER(ctypes.c_int),
        ctypes.c_uint,
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_size_t),
        ctypes.c_void_p,
        ctypes.c_size_t,
    )
    libc.sysctl.restype = ctypes.c_int
    mib = (ctypes.c_int * 3)(1, 49, pid)  # CTL_KERN, KERN_PROCARGS2
    size = ctypes.c_size_t()
    if libc.sysctl(mib, 3, None, ctypes.byref(size), None, 0) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    if not ctypes.sizeof(ctypes.c_int) < size.value <= 16 * 1024 * 1024:
        raise ValueError("invalid KERN_PROCARGS2 buffer size")
    buffer = ctypes.create_string_buffer(size.value)
    if libc.sysctl(mib, 3, buffer, ctypes.byref(size), None, 0) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    data = buffer.raw[: size.value]
    argc = int.from_bytes(data[:4], sys.byteorder, signed=True)
    # Executable path precedes argv, separated by NUL padding.
    start = data.index(b"\0", 4) + 1
    while start < len(data) and data[start] == 0:
        start += 1
    arguments = data[start:].split(b"\0")
    if argc < 1 or len(arguments) <= argc:
        raise ValueError("invalid KERN_PROCARGS2 argv")
    return [os.fsdecode(arg) for arg in arguments[:argc]]


def read_process_parent(pid: int) -> int:
    """Read a POSIX process's parent for host identification."""
    if sys.platform == "darwin":
        return _macos_info(pid).ppid
    if sys.platform.startswith("linux"):
        return _linux_info(pid).ppid
    raise OSError(f"unsupported process platform: {sys.platform}")


def read_process_argv(pid: int) -> Sequence[str]:
    """Read POSIX argv for host identification."""
    if sys.platform == "darwin":
        return _macos_argv(pid)
    if sys.platform.startswith("linux"):
        return _linux_argv(pid)
    raise OSError(f"unsupported process platform: {sys.platform}")
