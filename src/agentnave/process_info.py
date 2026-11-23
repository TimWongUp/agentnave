"""Read POSIX process metadata for best-effort host identification."""

from __future__ import annotations

import ctypes
import errno
import os
import sys
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol, cast


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


def _linux_executable(pid: int) -> str:
    return os.readlink(f"/proc/{pid}/exe")


_MACOS_PIDPATH_MAXSIZE = 4096


def _macos_executable(pid: int) -> str:
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    libproc.proc_pidpath.argtypes = (ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32)
    libproc.proc_pidpath.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(_MACOS_PIDPATH_MAXSIZE)
    length = int(libproc.proc_pidpath(pid, buffer, _MACOS_PIDPATH_MAXSIZE))
    if length <= 0:
        error = ctypes.get_errno() or errno.EIO
        raise OSError(error, os.strerror(error))
    return os.fsdecode(buffer.raw[:length])


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


def read_process_executable(pid: int) -> str:
    """Read a live POSIX process's real executable path, or raise if unavailable."""
    if sys.platform == "darwin":
        return _macos_executable(pid)
    if sys.platform.startswith("linux"):
        return _linux_executable(pid)
    raise OSError(f"unsupported process platform: {sys.platform}")


class _ProcBsdInfo(ctypes.Structure):
    # PROC_PIDTBSDINFO, from sys/proc_info.h.
    _fields_ = [
        ("header", ctypes.c_uint32 * 12),
        ("comm", ctypes.c_char * 16),
        ("name", ctypes.c_char * 32),
        ("tail", ctypes.c_uint32 * 6),
        ("start_sec", ctypes.c_uint64),
        ("start_usec", ctypes.c_uint64),
    ]


_WINDOWS_EPOCH_OFFSET = 11644473600.0
_WINDOWS_SYNCHRONIZE = 0x00100000
_WINDOWS_QUERY_LIMITED = 0x1000
_WINDOWS_WAIT_TIMEOUT = 258


class _WinFn(Protocol):
    argtypes: object
    restype: object

    def __call__(self, *args: object) -> object: ...


class _WinKernel32(Protocol):
    OpenProcess: _WinFn
    WaitForSingleObject: _WinFn
    GetProcessTimes: _WinFn
    CloseHandle: _WinFn


def _windows_started(pid: int) -> float:
    from ctypes import wintypes

    load_library = cast(
        Callable[..., object],
        ctypes.WinDLL,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
    )
    api = cast(_WinKernel32, load_library("kernel32", use_last_error=True))
    api.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    api.OpenProcess.restype = wintypes.HANDLE
    api.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    api.WaitForSingleObject.restype = wintypes.DWORD
    api.GetProcessTimes.argtypes = (
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    )
    api.GetProcessTimes.restype = wintypes.BOOL
    api.CloseHandle.argtypes = (wintypes.HANDLE,)
    api.CloseHandle.restype = wintypes.BOOL

    handle = cast(
        wintypes.HANDLE,
        api.OpenProcess(_WINDOWS_SYNCHRONIZE | _WINDOWS_QUERY_LIMITED, False, pid),
    )
    if not handle:
        raise OSError(f"cannot open process {pid}")
    try:
        if api.WaitForSingleObject(handle, 0) != _WINDOWS_WAIT_TIMEOUT:
            raise ProcessLookupError(f"process {pid} has exited")
        created, exited, kernel, user = (wintypes.FILETIME() for _ in range(4))
        if not api.GetProcessTimes(
            handle,
            ctypes.byref(created),
            ctypes.byref(exited),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            raise OSError(f"cannot read process times for {pid}")
        ticks = (created.dwHighDateTime << 32) | created.dwLowDateTime
        return ticks / 10_000_000 - _WINDOWS_EPOCH_OFFSET
    finally:
        api.CloseHandle(handle)


def read_process_started(pid: int) -> float:
    """Return a live process's start time in Unix seconds, or raise if unavailable."""
    if sys.platform == "darwin":
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        libproc.proc_pidinfo.argtypes = (
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_int,
        )
        libproc.proc_pidinfo.restype = ctypes.c_int
        info = _ProcBsdInfo()
        size = ctypes.sizeof(info)
        if libproc.proc_pidinfo(pid, 3, 0, ctypes.byref(info), size) != size:
            error = ctypes.get_errno() or errno.EIO
            raise OSError(error, os.strerror(error))
        return info.start_sec + info.start_usec / 1_000_000
    if sys.platform == "linux":
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[-1].split()
        # CLOCK_BOOTTIME includes suspend, as does /proc/<pid>/stat starttime.
        boot = time.time() - time.clock_gettime(time.CLOCK_BOOTTIME)
        return boot + int(fields[19]) / os.sysconf("SC_CLK_TCK")
    if sys.platform == "win32":
        return _windows_started(pid)
    raise OSError(f"unsupported process platform: {sys.platform}")
