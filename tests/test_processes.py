from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import cast

import pytest

from agentnave import processes


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

    monkeypatch.setattr(processes.tempfile, "tempdir", str(tmp_path))
    monkeypatch.setattr(processes.asyncio, "create_subprocess_exec", spawn)
    task = asyncio.create_task(processes._spawn_windows_process(("unused",), tmp_path))  # pyright: ignore[reportPrivateUsage]
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    assert process.returncode is None  # cannot kill it before the provider belongs to the Job
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
