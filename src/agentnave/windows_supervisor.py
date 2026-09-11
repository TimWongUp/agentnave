"""Windows Job Object supervisor used by :mod:`agentnave.processes`."""

from __future__ import annotations

import ctypes
import json
import shutil
import subprocess
import sys
from collections.abc import Callable
from ctypes import wintypes
from pathlib import Path
from typing import Protocol, cast

_CREATE_SUSPENDED = 0x00000004
_INFINITE = 0xFFFFFFFF
_JOB_OBJECT_EXTENDED_LIMIT_INFORMATION = 9
_JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
_STARTF_USESTDHANDLES = 0x00000100
_STD_INPUT_HANDLE = -10
_STD_OUTPUT_HANDLE = -11
_STD_ERROR_HANDLE = -12


class _IoCounters(ctypes.Structure):
    _fields_ = [
        ("read_operation_count", ctypes.c_ulonglong),
        ("write_operation_count", ctypes.c_ulonglong),
        ("other_operation_count", ctypes.c_ulonglong),
        ("read_transfer_count", ctypes.c_ulonglong),
        ("write_transfer_count", ctypes.c_ulonglong),
        ("other_transfer_count", ctypes.c_ulonglong),
    ]


class _JobObjectBasicLimitInformation(ctypes.Structure):
    _fields_ = [
        ("per_process_user_time_limit", ctypes.c_longlong),
        ("per_job_user_time_limit", ctypes.c_longlong),
        ("limit_flags", wintypes.DWORD),
        ("minimum_working_set_size", ctypes.c_size_t),
        ("maximum_working_set_size", ctypes.c_size_t),
        ("active_process_limit", wintypes.DWORD),
        ("affinity", ctypes.c_size_t),
        ("priority_class", wintypes.DWORD),
        ("scheduling_class", wintypes.DWORD),
    ]


class _JobObjectExtendedLimitInformation(ctypes.Structure):
    _fields_ = [
        ("basic_limit_information", _JobObjectBasicLimitInformation),
        ("io_info", _IoCounters),
        ("process_memory_limit", ctypes.c_size_t),
        ("job_memory_limit", ctypes.c_size_t),
        ("peak_process_memory_used", ctypes.c_size_t),
        ("peak_job_memory_used", ctypes.c_size_t),
    ]


class _StartupInfo(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("reserved", wintypes.LPWSTR),
        ("desktop", wintypes.LPWSTR),
        ("title", wintypes.LPWSTR),
        ("x", wintypes.DWORD),
        ("y", wintypes.DWORD),
        ("x_size", wintypes.DWORD),
        ("y_size", wintypes.DWORD),
        ("x_count_chars", wintypes.DWORD),
        ("y_count_chars", wintypes.DWORD),
        ("fill_attribute", wintypes.DWORD),
        ("flags", wintypes.DWORD),
        ("show_window", wintypes.WORD),
        ("reserved2_size", wintypes.WORD),
        ("reserved2", ctypes.POINTER(ctypes.c_ubyte)),
        ("stdin", wintypes.HANDLE),
        ("stdout", wintypes.HANDLE),
        ("stderr", wintypes.HANDLE),
    ]


class _ProcessInformation(ctypes.Structure):
    _fields_ = [
        ("process", wintypes.HANDLE),
        ("thread", wintypes.HANDLE),
        ("process_id", wintypes.DWORD),
        ("thread_id", wintypes.DWORD),
    ]


class _WinFunction(Protocol):
    argtypes: object
    restype: object

    def __call__(self, *args: object) -> object: ...


class _Kernel32(Protocol):
    CreateJobObjectW: _WinFunction
    SetInformationJobObject: _WinFunction
    CreateProcessW: _WinFunction
    AssignProcessToJobObject: _WinFunction
    ResumeThread: _WinFunction
    WaitForSingleObject: _WinFunction
    GetExitCodeProcess: _WinFunction
    TerminateProcess: _WinFunction
    CloseHandle: _WinFunction
    GetStdHandle: _WinFunction


def _write_status(path: Path, payload: dict[str, object]) -> None:
    path.write_text(json.dumps(payload, ensure_ascii=False) + "\n", encoding="utf-8")


def _last_error() -> str:
    get_last_error = cast(  # pyright: ignore[reportUnnecessaryCast]
        Callable[[], int],
        ctypes.get_last_error,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
    )
    win_error = cast(
        Callable[[int], OSError],
        ctypes.WinError,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
    )
    return str(win_error(get_last_error()))[:512]


def _configure_api() -> _Kernel32:
    load_library = cast(
        Callable[..., object],
        ctypes.WinDLL,  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]
    )
    kernel32 = cast(_Kernel32, load_library("kernel32", use_last_error=True))
    kernel32.CreateJobObjectW.argtypes = (ctypes.c_void_p, wintypes.LPCWSTR)
    kernel32.CreateJobObjectW.restype = wintypes.HANDLE
    kernel32.SetInformationJobObject.argtypes = (
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    )
    kernel32.SetInformationJobObject.restype = wintypes.BOOL
    kernel32.CreateProcessW.argtypes = (
        wintypes.LPCWSTR,
        wintypes.LPWSTR,
        ctypes.c_void_p,
        ctypes.c_void_p,
        wintypes.BOOL,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.LPCWSTR,
        ctypes.POINTER(_StartupInfo),
        ctypes.POINTER(_ProcessInformation),
    )
    kernel32.CreateProcessW.restype = wintypes.BOOL
    kernel32.AssignProcessToJobObject.argtypes = (wintypes.HANDLE, wintypes.HANDLE)
    kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
    kernel32.ResumeThread.argtypes = (wintypes.HANDLE,)
    kernel32.ResumeThread.restype = wintypes.DWORD
    kernel32.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    kernel32.WaitForSingleObject.restype = wintypes.DWORD
    kernel32.GetExitCodeProcess.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
    kernel32.GetExitCodeProcess.restype = wintypes.BOOL
    kernel32.TerminateProcess.argtypes = (wintypes.HANDLE, wintypes.UINT)
    kernel32.TerminateProcess.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = (wintypes.HANDLE,)
    kernel32.CloseHandle.restype = wintypes.BOOL
    kernel32.GetStdHandle.argtypes = (wintypes.DWORD,)
    kernel32.GetStdHandle.restype = wintypes.HANDLE
    return kernel32


def _run_provider(argv: list[str]) -> tuple[str, int | None, str | None]:
    executable = shutil.which(argv[0])
    if executable is None:
        return "launch_error", None, f"provider executable not found: {argv[0]}"
    argv = [executable, *argv[1:]]
    kernel32 = _configure_api()
    job = cast(wintypes.HANDLE, kernel32.CreateJobObjectW(None, None))
    if not job:
        return "launch_error", None, _last_error()
    process_info = _ProcessInformation()
    try:
        limits = _JobObjectExtendedLimitInformation()
        limits.basic_limit_information.limit_flags = _JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not kernel32.SetInformationJobObject(
            job,
            _JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            ctypes.byref(limits),
            ctypes.sizeof(limits),
        ):
            return "launch_error", None, _last_error()

        startup = _StartupInfo()
        startup.cb = ctypes.sizeof(startup)
        startup.flags = _STARTF_USESTDHANDLES
        startup.stdin = cast(wintypes.HANDLE, kernel32.GetStdHandle(_STD_INPUT_HANDLE))
        startup.stdout = cast(wintypes.HANDLE, kernel32.GetStdHandle(_STD_OUTPUT_HANDLE))
        startup.stderr = cast(wintypes.HANDLE, kernel32.GetStdHandle(_STD_ERROR_HANDLE))
        command_line = ctypes.create_unicode_buffer(subprocess.list2cmdline(argv))
        if not kernel32.CreateProcessW(
            None,
            command_line,
            None,
            None,
            True,
            _CREATE_SUSPENDED,
            None,
            None,
            ctypes.byref(startup),
            ctypes.byref(process_info),
        ):
            return "launch_error", None, _last_error()
        try:
            if not kernel32.AssignProcessToJobObject(job, process_info.process):
                message = _last_error()
                kernel32.TerminateProcess(process_info.process, 1)
                return "launch_error", None, message
            if cast(int, kernel32.ResumeThread(process_info.thread)) == 0xFFFFFFFF:
                message = _last_error()
                kernel32.TerminateProcess(process_info.process, 1)
                return "launch_error", None, message
            kernel32.WaitForSingleObject(process_info.process, _INFINITE)
            returncode = wintypes.DWORD()
            if not kernel32.GetExitCodeProcess(process_info.process, ctypes.byref(returncode)):
                return "supervisor_error", None, _last_error()
            return "exit", returncode.value, None
        finally:
            if process_info.thread:
                kernel32.CloseHandle(process_info.thread)
            if process_info.process:
                kernel32.CloseHandle(process_info.process)
    finally:
        kernel32.CloseHandle(job)


def main() -> int:
    if sys.platform != "win32" or len(sys.argv) < 3:
        return 2
    status_path = Path(sys.argv[1])
    try:
        kind, returncode, message = _run_provider(sys.argv[2:])
        payload: dict[str, object] = {"kind": kind}
        if returncode is not None:
            payload["returncode"] = returncode
        if message is not None:
            payload["message"] = message
        _write_status(status_path, payload)
    except OSError as exc:
        _write_status(status_path, {"kind": "launch_error", "message": str(exc)[:512]})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
