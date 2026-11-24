from __future__ import annotations

import asyncio
import json
import os
import signal
import sys
import time
from contextlib import suppress
from pathlib import Path
from typing import cast

import pytest
from fake_cli import write_fake_cli

from agentnave import processes, supervisor


@pytest.mark.asyncio
async def test_provider_does_not_inherit_host_listener(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    inherited = {
        "SERVER__PORT": "51153",
        "SERVER__HOST": "127.0.0.1",
        "HERDR_PANE_ID": "pane-42",
        "HERDR_SESSION": "sess-9",
        "CODEBUDDY_CONFIG_DIR": str(tmp_path),
    }
    for name, value in inherited.items():
        monkeypatch.setenv(name, value)
    script = (
        "import json,os; print(json.dumps({name: os.environ.get(name) "
        f"for name in {(*inherited, 'AGENTNAVE_DISPATCHED')!r}}}))"
    )
    supervised = await processes.spawn_process((sys.executable, "-c", script), tmp_path)
    try:
        completion = await asyncio.wait_for(supervised.completion, 10)
    finally:
        await processes.terminate_process_tree(supervised.process)
    stdout, stderr = await supervised.process.communicate()

    assert completion.returncode == 0, stderr
    assert json.loads(stdout) == {
        "SERVER__PORT": None,
        "SERVER__HOST": None,
        "HERDR_PANE_ID": None,
        "HERDR_SESSION": None,
        "CODEBUDDY_CONFIG_DIR": str(tmp_path),
        # POSIX names the supervisor's own session, which the provider tree shares.
        "AGENTNAVE_DISPATCHED": "1" if os.name == "nt" else str(supervised.process.pid),
    }
    assert {name: os.environ[name] for name in inherited} == inherited


@pytest.mark.skipif(os.name != "posix", reason="POSIX status pipe")
@pytest.mark.asyncio
async def test_cancelled_spawn_closes_both_status_pipe_ends(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    opened: list[int] = []
    pipe = os.pipe
    entered = asyncio.Event()

    def track_pipe() -> tuple[int, int]:
        pair = pipe()
        opened.extend(pair)
        return pair

    async def spawn(*_args: object, **_kwargs: object) -> None:
        entered.set()
        await asyncio.Event().wait()

    monkeypatch.setattr(processes.os, "pipe", track_pipe)
    monkeypatch.setattr(processes.asyncio, "create_subprocess_exec", spawn)
    task = asyncio.create_task(processes.spawn_process(("unused",), tmp_path))
    await entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    remaining: list[int] = []
    for fd in opened:
        try:
            os.fstat(fd)
        except OSError:
            continue
        remaining.append(fd)
        os.close(fd)
    assert remaining == []


@pytest.mark.asyncio
async def test_cancelled_windows_startup_reaps_supervisor_and_tempfiles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class PendingProcess:
        returncode: int | None = None
        stdin = None
        stdout = None
        stderr = None

        def terminate(self) -> None:
            self.returncode = 1

        async def wait(self) -> int:
            while self.returncode is None:
                await asyncio.sleep(0)
            return self.returncode

    process = PendingProcess()
    entered = asyncio.Event()
    ready: Path | None = None

    async def spawn(*args: object, **_kwargs: object) -> asyncio.subprocess.Process:
        nonlocal ready
        ready = Path(str(args[4]))
        entered.set()
        return cast(asyncio.subprocess.Process, process)

    monkeypatch.setattr(processes, "server_identity", lambda: 1)
    monkeypatch.setattr(processes.tempfile, "tempdir", str(tmp_path))
    monkeypatch.setattr(processes.asyncio, "create_subprocess_exec", spawn)
    task = asyncio.create_task(processes._spawn_windows_process(("unused",), tmp_path))  # pyright: ignore[reportPrivateUsage]
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    assert process.returncode is None  # cannot kill it before the provider belongs to the Job
    task.cancel()  # repeated cancellation must not abandon the same ownership transfer
    await asyncio.sleep(0)
    assert not task.done()
    assert ready is not None
    ready.touch()
    try:
        with pytest.raises(asyncio.CancelledError):
            await task
        assert process.returncode == 1
        assert not list(tmp_path.glob("agentnave-*"))
    finally:
        process.terminate()
        await asyncio.sleep(0)


@pytest.mark.asyncio
async def test_cancelled_windows_startup_drains_full_output_pipes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sys

    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys,time; sys.stdout.buffer.write(b'x' * 1000000); sys.stdout.flush(); time.sleep(30)",
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    entered = asyncio.Event()
    ready: Path | None = None

    async def spawn(*args: object, **_kwargs: object) -> asyncio.subprocess.Process:
        nonlocal ready
        ready = Path(str(args[4]))
        entered.set()
        return process

    monkeypatch.setattr(processes, "server_identity", lambda: 1)
    monkeypatch.setattr(processes.tempfile, "tempdir", str(tmp_path))
    monkeypatch.setattr(processes.asyncio, "create_subprocess_exec", spawn)
    task = asyncio.create_task(processes._spawn_windows_process(("unused",), tmp_path))  # pyright: ignore[reportPrivateUsage]
    try:
        await entered.wait()
        await asyncio.sleep(0.2)  # allow output to fill the pipe before ownership is acknowledged
        task.cancel()
        assert ready is not None
        ready.touch()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(asyncio.shield(task), 5)
        assert process.returncode is not None
        assert not list(tmp_path.glob("agentnave-*"))
    finally:
        if process.returncode is None:
            process.kill()
        await process.communicate()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.skipif(os.name != "posix", reason="POSIX process-group grace cleanup")
@pytest.mark.asyncio
async def test_terminate_process_tree_stops_early_when_cli_exits_on_sigterm(
    tmp_path: Path,
) -> None:
    cli = write_fake_cli(
        tmp_path,
        "fake-cli",
        "import time\nprint('ready', flush=True)\ntime.sleep(60)\n",
    )
    supervised = await processes.spawn_process((str(cli),), tmp_path)
    try:
        assert supervised.process.stdout is not None
        assert await asyncio.wait_for(supervised.process.stdout.readline(), 5) == b"ready\n"
        started = time.monotonic()
        await processes.terminate_process_tree(
            supervised.process, grace_seconds=5.0, completion=supervised.completion
        )
        elapsed = time.monotonic() - started
        assert elapsed < 2.0
        assert supervised.process.returncode is not None
        assert supervised.completion.done()
    finally:
        await processes.terminate_process_tree(
            supervised.process, grace_seconds=0.01, completion=supervised.completion
        )
        await asyncio.gather(supervised.completion, return_exceptions=True)


@pytest.mark.skipif(os.name != "posix", reason="POSIX process-group grace cleanup")
@pytest.mark.asyncio
async def test_terminate_process_tree_waits_grace_and_sigkills_sigterm_ignoring_grandchild(
    tmp_path: Path,
) -> None:
    grandchild_code = (
        "import os, signal, time\n"
        "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
        "print(os.getpid(), flush=True)\n"
        "time.sleep(60)\n"
    )
    cli = write_fake_cli(
        tmp_path,
        "fake-cli",
        (
            "import signal, subprocess, sys\n"
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
            f"child = subprocess.Popen([sys.executable, '-c', {grandchild_code!r}])\n"
            "raise SystemExit(child.wait())\n"
        ),
    )
    supervised = await processes.spawn_process((str(cli),), tmp_path)
    grandchild_pid: int | None = None
    try:
        assert supervised.process.stdout is not None
        line = await asyncio.wait_for(supervised.process.stdout.readline(), 5)
        grandchild_pid = int(line.decode().strip())
        started = time.monotonic()
        await processes.terminate_process_tree(
            supervised.process, grace_seconds=0.5, completion=supervised.completion
        )
        elapsed = time.monotonic() - started
        assert 0.45 <= elapsed < 3.0
        assert supervised.process.returncode is not None
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            try:
                os.kill(grandchild_pid, 0)
            except ProcessLookupError:
                break
            await asyncio.sleep(0.01)
        else:
            pytest.fail(f"grandchild {grandchild_pid} survived SIGKILL")
    finally:
        if grandchild_pid is not None and sys.platform != "win32":
            with suppress(ProcessLookupError):
                os.kill(grandchild_pid, signal.SIGKILL)
        await processes.terminate_process_tree(
            supervised.process, grace_seconds=0.01, completion=supervised.completion
        )
        await asyncio.gather(supervised.completion, return_exceptions=True)


def test_supervisor_status_to_a_killed_server_does_not_stop_the_supervisor() -> None:
    # The server was SIGKILLed: its end of the status pipe is gone. The supervisor must go on
    # to end the process group instead of dying on the write.
    read, write = os.pipe()
    os.close(read)
    supervisor._write_status(write, {"kind": "exit", "returncode": 0})  # pyright: ignore[reportPrivateUsage]
    with pytest.raises(OSError):
        os.close(write)  # already closed by the call
