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
        )
    except Exception:
        os.close(status_read)
        raise
    finally:
        os.close(status_write)
    completion = asyncio.create_task(_read_completion(status_read))
    return SupervisedProcess(process, completion)


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
    try:
        process = await asyncio.create_subprocess_exec(
            sys.executable,
            "-I",
            str(Path(__file__).with_name("windows_supervisor.py")),
            str(status_path),
            *argv,
            cwd=cwd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except Exception:
        status_path.unlink(missing_ok=True)
        raise
    completion = asyncio.create_task(_read_windows_completion(process, status_path))
    return SupervisedProcess(process, completion)


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
