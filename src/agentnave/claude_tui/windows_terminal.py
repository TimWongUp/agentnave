# pyright: reportAttributeAccessIssue=false, reportUnknownMemberType=false, reportUnknownVariableType=false, reportUnknownArgumentType=false, reportUnknownParameterType=false
"""A Windows pseudoconsole hosted inside the invocation's existing Job Object."""

from __future__ import annotations

import ctypes
import os
import queue
import subprocess
import threading
from ctypes import wintypes

from agentnave.claude_tui.screen import Screen
from agentnave.claude_tui.terminal import child_environment
from agentnave.windows_supervisor import (
    _ProcessInformation,  # pyright: ignore[reportPrivateUsage]
    _StartupInfo,  # pyright: ignore[reportPrivateUsage]
)


class _Coord(ctypes.Structure):
    _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short)]


class _StartupInfoEx(ctypes.Structure):
    _fields_ = [("startup", _StartupInfo), ("attributes", ctypes.c_void_p)]


class _Process:
    def __init__(self, kernel: object, info: _ProcessInformation) -> None:
        self.kernel = kernel
        self.info = info
        self.pid = int(info.process_id)
        self.returncode: int | None = None

    def poll(self) -> int | None:
        if self.returncode is None:
            code = wintypes.DWORD()
            if not self.kernel.GetExitCodeProcess(self.info.process, ctypes.byref(code)):
                raise ctypes.WinError(ctypes.get_last_error())
            if code.value != 259:
                self.returncode = int(code.value)
        return self.returncode

    def close(self) -> None:
        if self.poll() is None:
            self.kernel.TerminateProcess(self.info.process, 1)
            self.kernel.WaitForSingleObject(self.info.process, 1000)
        self.kernel.CloseHandle(self.info.process)


def _kernel() -> object:
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    try:
        create_console = kernel.CreatePseudoConsole
    except AttributeError as exc:
        raise OSError("Windows ConPTY is unavailable") from exc
    handle = wintypes.HANDLE
    kernel.CreatePipe.argtypes = [
        ctypes.POINTER(handle),
        ctypes.POINTER(handle),
        ctypes.c_void_p,
        wintypes.DWORD,
    ]
    kernel.CreatePipe.restype = wintypes.BOOL
    create_console.argtypes = [
        _Coord,
        handle,
        handle,
        wintypes.DWORD,
        ctypes.POINTER(handle),
    ]
    create_console.restype = ctypes.c_long
    kernel.ClosePseudoConsole.argtypes = [handle]
    kernel.InitializeProcThreadAttributeList.argtypes = [
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    kernel.InitializeProcThreadAttributeList.restype = wintypes.BOOL
    kernel.UpdateProcThreadAttribute.argtypes = [
        ctypes.c_void_p,
        wintypes.DWORD,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_void_p,
    ]
    kernel.UpdateProcThreadAttribute.restype = wintypes.BOOL
    kernel.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
    kernel.CreateProcessW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.LPWSTR,
        ctypes.c_void_p,
        ctypes.c_void_p,
        wintypes.BOOL,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.LPCWSTR,
        ctypes.c_void_p,
        ctypes.POINTER(_ProcessInformation),
    ]
    kernel.CreateProcessW.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [handle]
    kernel.GetExitCodeProcess.argtypes = [handle, ctypes.POINTER(wintypes.DWORD)]
    kernel.GetExitCodeProcess.restype = wintypes.BOOL
    kernel.TerminateProcess.argtypes = [handle, wintypes.UINT]
    kernel.WaitForSingleObject.argtypes = [handle, wintypes.DWORD]
    return kernel


def _pipe(kernel: object) -> tuple[wintypes.HANDLE, wintypes.HANDLE]:
    read, write = wintypes.HANDLE(), wintypes.HANDLE()
    if not kernel.CreatePipe(ctypes.byref(read), ctypes.byref(write), None, 0):
        raise ctypes.WinError(ctypes.get_last_error())
    return read, write


def _spawn(
    kernel: object, console: wintypes.HANDLE, argv: list[str], env: dict[str, str]
) -> _Process:
    size = ctypes.c_size_t()
    kernel.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(size))
    storage = ctypes.create_string_buffer(size.value)
    attributes = ctypes.cast(storage, ctypes.c_void_p)
    if not kernel.InitializeProcThreadAttributeList(attributes, 1, 0, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not kernel.UpdateProcThreadAttribute(
            attributes, 0, 0x00020016, console, ctypes.sizeof(console), None, None
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        startup = _StartupInfoEx()
        startup.startup.cb = ctypes.sizeof(startup)
        # Prevent Windows from duplicating the helper's redirected stdio into the PTY child.
        startup.startup.flags = 0x00000100
        startup.attributes = attributes
        info = _ProcessInformation()
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline(argv))
        block = ctypes.create_unicode_buffer(
            "\0".join(
                f"{key}={value}"
                for key, value in sorted(env.items(), key=lambda item: item[0].upper())
            )
            + "\0\0"
        )
        if not kernel.CreateProcessW(
            None,
            command,
            None,
            None,
            False,
            0x00080000 | 0x00000400,
            ctypes.cast(block, ctypes.c_void_p),
            None,
            ctypes.byref(startup),
            ctypes.byref(info),
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        kernel.CloseHandle(info.thread)
        return _Process(kernel, info)
    finally:
        kernel.DeleteProcThreadAttributeList(attributes)


class WindowsTerminal:
    def __init__(
        self,
        executable: str,
        args: list[str],
        extra_env: dict[str, str] | None = None,
        screen: type[Screen] = Screen,
    ) -> None:
        import msvcrt

        self.screen = screen()
        self.outgoing = bytearray()
        self.kernel = _kernel()
        in_read, in_write = _pipe(self.kernel)
        try:
            out_read, out_write = _pipe(self.kernel)
        except BaseException:
            self.kernel.CloseHandle(in_read)
            self.kernel.CloseHandle(in_write)
            raise
        self.console = wintypes.HANDLE()
        try:
            result = self.kernel.CreatePseudoConsole(
                _Coord(200, 50), in_read, out_write, 0, ctypes.byref(self.console)
            )
            if result:
                raise OSError(f"CreatePseudoConsole failed: 0x{result & 0xFFFFFFFF:08x}")
            self.process = _spawn(
                self.kernel,
                self.console,
                [executable, *args],
                {**child_environment(executable), **(extra_env or {})},
            )
        except BaseException:
            if self.console:
                self.kernel.ClosePseudoConsole(self.console)
            self.kernel.CloseHandle(in_write)
            self.kernel.CloseHandle(out_read)
            raise
        finally:
            self.kernel.CloseHandle(in_read)
            self.kernel.CloseHandle(out_write)
        assert in_write.value is not None and out_read.value is not None
        self._input = msvcrt.open_osfhandle(in_write.value, os.O_WRONLY | os.O_BINARY)
        self._output = msvcrt.open_osfhandle(out_read.value, os.O_RDONLY | os.O_BINARY)
        self._received: queue.Queue[bytes] = queue.Queue()
        self._send: queue.Queue[bytes | None] = queue.Queue()
        self._written: queue.Queue[tuple[int, OSError | None]] = queue.Queue()
        self._writing = False
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._write, daemon=True).start()

    def _read(self) -> None:
        try:
            while data := os.read(self._output, 65536):
                self._received.put(data)
        except OSError:
            pass

    def _write(self) -> None:
        while (data := self._send.get()) is not None:
            count = len(data)
            try:
                while data:
                    data = data[os.write(self._input, data) :]
            except OSError as error:
                self._written.put((count, error))
                return
            self._written.put((count, None))

    def poll(self) -> str:
        if self._writing:
            try:
                count, error = self._written.get_nowait()
            except queue.Empty:
                pass
            else:
                if error:
                    raise error
                del self.outgoing[:count]
                self._writing = False
        if self.outgoing and not self._writing:
            self._send.put(bytes(self.outgoing))
            self._writing = True
        try:
            data = self._received.get(timeout=0.05)
        except queue.Empty:
            data = b""
        self.outgoing.extend(b"\x1b[1;1R" * self.screen.feed(data))
        return self.screen.text()

    def close(self) -> None:
        self.process.close()
        self._send.put(None)
        self.kernel.ClosePseudoConsole(self.console)
        os.close(self._input)
        os.close(self._output)
