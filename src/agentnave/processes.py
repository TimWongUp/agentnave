# pyright cannot resolve the Windows-only `winreg` module on the Linux/macOS CI; the registry
# lookup below is the only place it is used, and its unchecked values are narrowed by hand.
# pyright: reportAttributeAccessIssue=false, reportUnknownMemberType=false, reportUnknownVariableType=false, reportMissingImports=false
"""Platform subprocess supervision and process-tree cleanup."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import subprocess
import sys
import tempfile
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from agentnave.adapters.base import provider_bin_dirs
from agentnave.host import NATIVE_SESSION_ENV
from agentnave.windows_supervisor import server_identity


@dataclass(frozen=True, slots=True)
class ProviderCompletion:
    kind: str
    returncode: int | None = None
    message: str | None = None


@dataclass(frozen=True, slots=True)
class SupervisedProcess:
    process: asyncio.subprocess.Process
    completion: asyncio.Task[ProviderCompletion]


async def spawn_process(argv: tuple[str, ...], cwd: Path) -> SupervisedProcess:
    if os.name == "nt":
        return await _spawn_windows_process(argv, cwd)
    if os.name != "posix":
        raise OSError(f"AgentNave does not support process supervision on {os.name}")
    status_read, status_write = os.pipe()
    try:
        process = await asyncio.create_subprocess_exec(
            sys.executable,
            "-I",
            str(Path(__file__).with_name("supervisor.py")),
            str(status_write),
            *argv,
            cwd=cwd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            start_new_session=True,
            pass_fds=(status_write,),
            env=provider_environment(argv),
        )
    except BaseException:
        os.close(status_read)
        raise
    finally:
        os.close(status_write)
    completion = asyncio.create_task(_read_completion(status_read))
    return SupervisedProcess(process, completion)


def provider_environment(argv: tuple[str, ...]) -> dict[str, str]:
    """Inherited environment with PATH completed for the provider only.

    Hosts may pass a minimal PATH; `#!/usr/bin/env node` style CLIs then fail even
    when their absolute path was found. Put the executable's directory first, then the
    inherited PATH (so the host's toolchain choice wins), then the existing common
    install directories.
    """
    env = dict(os.environ)
    env.pop("AGENTNAVE_HOST_SESSION_ID", None)
    env.pop("AGENTNAVE_HOST_APP", None)
    for name in NATIVE_SESSION_ENV.values():
        env.pop(name, None)
    if os.name == "nt":
        env.update(_system_proxy(env))
        return env
    inherited = [entry for entry in env.get("PATH", "").split(os.pathsep) if entry]
    leading = [os.path.dirname(argv[0])] if argv and os.path.isabs(argv[0]) else []
    path: list[str] = []
    for entry in (*leading, *inherited, *provider_bin_dirs()):
        if entry not in path:
            path.append(entry)
    env["PATH"] = os.pathsep.join(path)
    return env


def _system_proxy(env: dict[str, str]) -> dict[str, str]:
    """The Windows system proxy, for CLIs that only read `HTTPS_PROXY`.

    Windows keeps its proxy in the registry, where only WinINET clients look. Go and Node
    CLIs connect directly instead and then wait out a network timeout: Antigravity lists no
    models at all behind a proxy until these variables are set.
    """
    if any(name in env for name in ("HTTPS_PROXY", "https_proxy")):
        return {}
    try:
        # `winreg` only exists on Windows; pyright on the Linux/macOS CI cannot resolve it, so
        # this block carries its own ignores rather than relaxing the file's type checks.
        import winreg  # pyright: ignore[reportMissingImports]

        key = winreg.OpenKey(  # pyright: ignore[reportAttributeAccessIssue, reportUnknownMemberType]
            winreg.HKEY_CURRENT_USER,  # pyright: ignore[reportAttributeAccessIssue]
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        )
        with key:
            enabled = winreg.QueryValueEx(key, "ProxyEnable")[0]  # pyright: ignore[reportAttributeAccessIssue, reportUnknownMemberType]
            if not enabled:
                return {}
            server = winreg.QueryValueEx(key, "ProxyServer")[0]  # pyright: ignore[reportAttributeAccessIssue, reportUnknownMemberType]
    except OSError:
        return {}
    if not isinstance(server, str) or not server:
        return {}
    parts = dict(part.split("=", 1) for part in server.split(";") if "=" in part)
    entry = parts.get("https") or parts.get("http") or server.split(";")[0]
    if "://" not in entry:
        entry = f"http://{entry}"
    return {"HTTPS_PROXY": entry, "HTTP_PROXY": entry}


async def terminate_process_tree(
    process: asyncio.subprocess.Process, grace_seconds: float = 2.0
) -> None:
    """Terminate a provider tree through its platform supervisor."""
    if process.returncode is not None:
        return
    if os.name == "nt":
        # Terminating the supervisor closes its kill-on-close Job Object. Windows
        # has no portable graceful tree signal equivalent to POSIX SIGTERM.
        process.terminate()
        await process.wait()
        return
    process_group = process.pid
    os.killpg(process_group, signal.SIGTERM)
    await asyncio.sleep(grace_seconds)
    with suppress(ProcessLookupError):
        os.killpg(process_group, signal.SIGKILL)
    await process.wait()


async def _spawn_windows_process(argv: tuple[str, ...], cwd: Path) -> SupervisedProcess:
    status_fd, status_name = tempfile.mkstemp(prefix="agentnave-", suffix=".json")
    os.close(status_fd)
    status_path = Path(status_name)
    ready_fd, ready_name = tempfile.mkstemp(prefix="agentnave-", suffix=".ready")
    os.close(ready_fd)
    ready_path = Path(ready_name)
    ready_path.unlink()
    try:
        process = await asyncio.create_subprocess_exec(
            # This stdlib-only helper needs a real process, not a venv redirector
            # whose PID would hide the supervisor from termination and parent waits.
            getattr(sys, "_base_executable", None) or sys.executable,
            "-I",
            str(Path(__file__).with_name("windows_supervisor.py")),
            str(status_path),
            str(ready_path),
            str(os.getpid()),
            str(server_identity()),
            *argv,
            cwd=cwd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            env=provider_environment(argv),
        )
    except BaseException:
        status_path.unlink(missing_ok=True)
        ready_path.unlink(missing_ok=True)
        raise
    completion = asyncio.create_task(_read_windows_completion(process, status_path))
    try:
        await _wait_windows_supervisor_ready(process, ready_path)
        return SupervisedProcess(process, completion)
    except BaseException:
        cleanup = asyncio.create_task(_cancel_windows_startup(process, completion, ready_path))
        # A second cancellation must not abandon ownership before Job assignment.
        while not cleanup.done():
            try:
                await asyncio.shield(cleanup)
            except asyncio.CancelledError:
                continue
        cleanup.result()
        raise
    finally:
        ready_path.unlink(missing_ok=True)


async def _cancel_windows_startup(
    process: asyncio.subprocess.Process,
    completion: asyncio.Task[ProviderCompletion],
    ready_path: Path,
) -> None:
    # The provider may still be suspended outside its Job. Wait for ownership first.
    await _wait_windows_supervisor_ready(process, ready_path)
    if process.returncode is None:
        with suppress(ProcessLookupError):
            process.terminate()
    if process.stdin is not None:
        process.stdin.close()
    await asyncio.gather(
        completion,
        _discard_output(process.stdout),
        _discard_output(process.stderr),
        return_exceptions=True,
    )


async def _discard_output(stream: asyncio.StreamReader | None) -> None:
    if stream is not None:
        while await stream.read(65_536):
            pass


async def _wait_windows_supervisor_ready(
    process: asyncio.subprocess.Process, ready_path: Path
) -> None:
    """Do not expose the supervisor until its provider belongs to the Job Object."""
    while process.returncode is None and not ready_path.exists():
        await asyncio.sleep(0.01)


async def _read_windows_completion(
    process: asyncio.subprocess.Process, status_path: Path
) -> ProviderCompletion:
    try:
        await process.wait()
        try:
            data = status_path.read_bytes()
        except OSError:
            return ProviderCompletion(
                "supervisor_error", message="failed to read supervisor status"
            )
        if len(data) > 16_384:
            return ProviderCompletion("supervisor_error", message="status message too large")
        return _parse_completion(bytearray(data))
    finally:
        status_path.unlink(missing_ok=True)


async def _read_completion(status_fd: int) -> ProviderCompletion:
    loop = asyncio.get_running_loop()
    data = bytearray()
    completion: asyncio.Future[ProviderCompletion] = loop.create_future()
    reader_registered = False

    def finish(result: ProviderCompletion) -> None:
        if completion.done():
            return
        loop.remove_reader(status_fd)
        completion.set_result(result)

    def read_ready() -> None:
        try:
            chunk = os.read(status_fd, 4096)
        except BlockingIOError:
            return
        except OSError:
            finish(
                ProviderCompletion("supervisor_error", message="failed to read supervisor status")
            )
            return
        if chunk:
            data.extend(chunk)
            if len(data) > 16_384:
                finish(ProviderCompletion("supervisor_error", message="status message too large"))
            elif b"\n" in data:
                finish(_parse_completion(data))
            return
        finish(_parse_completion(data))

    try:
        os.set_blocking(status_fd, False)
        loop.add_reader(status_fd, read_ready)
        reader_registered = True
        return await completion
    finally:
        if reader_registered:
            loop.remove_reader(status_fd)
        os.close(status_fd)


def _parse_completion(data: bytearray) -> ProviderCompletion:
    if not data:
        return ProviderCompletion("supervisor_error", message="supervisor exited without status")
    try:
        raw = cast(object, json.loads(bytes(data).split(b"\n", 1)[0]))
    except (json.JSONDecodeError, UnicodeDecodeError):
        return ProviderCompletion("supervisor_error", message="invalid supervisor status")
    if not isinstance(raw, dict):
        return ProviderCompletion("supervisor_error", message="invalid supervisor status")
    payload = cast(dict[object, object], raw)
    kind = payload.get("kind")
    returncode = payload.get("returncode")
    message = payload.get("message")
    return ProviderCompletion(
        kind if isinstance(kind, str) else "supervisor_error",
        returncode if isinstance(returncode, int) else None,
        message if isinstance(message, str) else None,
    )
