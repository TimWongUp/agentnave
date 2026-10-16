"""Detect dispatched CLI descendants without subprocesses or third-party dependencies."""

from __future__ import annotations

import ctypes
import errno
import os
import re
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast

SUPERVISOR_ENV = "AGENTNAVE_SUPERVISOR"
_MAX_ANCESTORS = 256


@dataclass(frozen=True)
class NestingResult:
    status: Literal["not_nested", "nested", "unavailable"]
    reason: str = ""

    @property
    def instructions(self) -> str:
        if self.status == "nested":
            return (
                "此服务运行在 AgentNave 派出的 CLI 内，为保持单向编排已禁用编排工具。" + self.reason
            )
        if self.status == "unavailable":
            return f"无法确认是否运行在 AgentNave 子代理内（{self.reason}），已禁用编排工具。"
        return ""


@dataclass(frozen=True)
class _ProcessInfo:
    ppid: int
    uid: int


def _is_supervisor(argv: Sequence[str]) -> bool:
    if not argv or not re.fullmatch(
        r"(?:pythonw?|pypy)(?:\d+(?:\.\d+)*)?(?:\.exe)?", Path(argv[0]).name, re.IGNORECASE
    ):
        return False
    for argument in argv[1:]:
        # -c/-m execute code or a module, not a script positional argument.
        if argument in {"-c", "-m"}:
            return False
        if not argument.startswith("-"):
            script = Path(argument)
            return script.name in {"supervisor.py", "windows_supervisor.py"} and (
                script.parent.name == "agentnave"
            )
    return False


def _detect_posix(
    marker: int | None,
    parent_pid: int,
    uid: int,
    read_info: Callable[[int], _ProcessInfo],
    read_argv: Callable[[int], Sequence[str]],
) -> NestingResult:
    if marker is not None:
        try:
            info = read_info(marker)
            if info.uid == uid and _is_supervisor(read_argv(marker)):
                return NestingResult("nested", f"环境标记指向 supervisor PID {marker}。")
        except (FileNotFoundError, ProcessLookupError):
            pass  # A dead supervisor leaves a stale marker.
        except (OSError, ValueError) as exc:
            return NestingResult("unavailable", f"读取标记进程 {marker} 失败：{exc}")

    pid = parent_pid
    seen: set[int] = set()
    for _ in range(_MAX_ANCESTORS):
        if pid <= 1:
            return NestingResult("not_nested")
        if pid in seen:
            return NestingResult("unavailable", "祖先进程链存在循环")
        seen.add(pid)
        try:
            info = read_info(pid)
            # Supervisors have the same owner; foreign argv may be inaccessible.
            if info.uid == uid and _is_supervisor(read_argv(pid)):
                return NestingResult("nested", f"祖先进程中存在 supervisor PID {pid}。")
        except (OSError, ValueError) as exc:
            return NestingResult("unavailable", f"读取祖先进程 {pid} 失败：{exc}")
        pid = info.ppid
    return NestingResult("unavailable", "祖先进程链超过检测层数上限")


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


class _ProcBsdInfo(ctypes.Structure):
    # Layout from macOS sys/proc_info.h, PROC_PIDTBSDINFO.
    _fields_ = [
        ("flags", ctypes.c_uint32),
        ("status", ctypes.c_uint32),
        ("xstatus", ctypes.c_uint32),
        ("pid", ctypes.c_uint32),
        ("ppid", ctypes.c_uint32),
        ("uid", ctypes.c_uint32),
        ("gid", ctypes.c_uint32),
        ("ruid", ctypes.c_uint32),
        ("rgid", ctypes.c_uint32),
        ("svuid", ctypes.c_uint32),
        ("svgid", ctypes.c_uint32),
        ("reserved", ctypes.c_uint32),
        ("comm", ctypes.c_char * 16),
        ("name", ctypes.c_char * 32),
        ("nfiles", ctypes.c_uint32),
        ("pgid", ctypes.c_uint32),
        ("pjobc", ctypes.c_uint32),
        ("tdev", ctypes.c_uint32),
        ("tpgid", ctypes.c_uint32),
        ("nice", ctypes.c_int32),
        ("start_sec", ctypes.c_uint64),
        ("start_usec", ctypes.c_uint64),
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
    info = _ProcBsdInfo()
    size = ctypes.sizeof(info)
    if libproc.proc_pidinfo(pid, 3, 0, ctypes.byref(info), size) != size:
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


def _windows_alive(pid: int) -> bool:
    load_library = cast(
        Callable[..., ctypes.CDLL],
        ctypes.WinDLL,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
    )
    kernel32 = load_library("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = (ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32)
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.WaitForSingleObject.argtypes = (ctypes.c_void_p, ctypes.c_uint32)
    kernel32.WaitForSingleObject.restype = ctypes.c_uint32
    kernel32.CloseHandle.argtypes = (ctypes.c_void_p,)
    kernel32.CloseHandle.restype = ctypes.c_int
    handle = kernel32.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE
    if not handle:
        get_last_error = cast(  # pyright: ignore[reportUnnecessaryCast]
            Callable[[], int],
            ctypes.get_last_error,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
        )
        error = get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: process no longer exists.
            return False
        raise OSError(error, "OpenProcess failed")
    try:
        result = kernel32.WaitForSingleObject(handle, 0)
        if result not in (0, 258):  # WAIT_OBJECT_0, WAIT_TIMEOUT
            raise OSError("WaitForSingleObject failed")
        return result == 258
    finally:
        kernel32.CloseHandle(handle)


def read_process_parent(pid: int) -> int:
    """Read a POSIX process's parent for host identification."""
    if sys.platform == "darwin":
        return _macos_info(pid).ppid
    if sys.platform.startswith("linux"):
        return _linux_info(pid).ppid
    raise OSError(f"unsupported process platform: {sys.platform}")


def read_process_argv(pid: int) -> Sequence[str]:
    """Read POSIX argv using the same platform readers as nesting detection."""
    if sys.platform == "darwin":
        return _macos_argv(pid)
    if sys.platform.startswith("linux"):
        return _linux_argv(pid)
    raise OSError(f"unsupported process platform: {sys.platform}")


def detect_nesting() -> NestingResult:
    marker: int | None = None
    try:
        value = int(os.environ.get(SUPERVISOR_ENV, ""))
        if 1 < value <= 0x7FFFFFFF:
            marker = value
    except ValueError:
        pass  # Invalid markers are stale, not evidence of nesting.
    try:
        if sys.platform == "win32":
            if marker is not None and _windows_alive(marker):
                return NestingResult("nested", f"环境标记指向存活的 supervisor PID {marker}。")
            return NestingResult("not_nested")
        if sys.platform == "darwin":
            read_info, read_argv = _macos_info, _macos_argv
        elif sys.platform.startswith("linux"):
            read_info, read_argv = _linux_info, _linux_argv
        else:
            return NestingResult("unavailable", f"不支持的平台：{sys.platform}")
        return _detect_posix(marker, os.getppid(), os.geteuid(), read_info, read_argv)
    except (OSError, ValueError) as exc:
        return NestingResult("unavailable", str(exc))
