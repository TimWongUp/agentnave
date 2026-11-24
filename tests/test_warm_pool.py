"""Processes kept after a turn for the next resume of the same session (ADR-0120)."""

from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path

import pytest
from test_terminal_execution import setup_tray, wait_for_file

from agentnave.adapters.base import PreparedCommand
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationResult, InvocationStatus

# Speaks the keep-alive contract: one JSON turn per stdin line, Codex exec events per turn.
_TURNS = """
import json, os, signal, sys, time
if "slow-exit" in sys.argv:
    signal.signal(signal.SIGTERM, lambda *_: (time.sleep(1), sys.exit(0)))
count, session = 0, None
for raw in sys.stdin:
    turn = json.loads(raw)
    count += 1
    session = turn.get("session_id") or session or f"thread-{os.getpid()}"
    print(json.dumps({"type": "thread.started", "thread_id": session}), flush=True)
    if turn["prompt"] == "slow":
        time.sleep(60)
    if turn["prompt"] == "fail":
        print(json.dumps({"type": "turn.failed", "error": {"message": "boom"}}), flush=True)
        continue
    text = f"{os.getpid()}:{count}"
    item = {"type": "agent_message", "text": text}
    print(json.dumps({"type": "item.completed", "item": item}), flush=True)
    print(json.dumps({"type": "turn.completed"}), flush=True)
"""


def _turn(tmp_path: Path, prompt: str, session_id: str | None, *variant: str) -> PreparedCommand:
    line = json.dumps({"session_id": session_id, "prompt": prompt}).encode() + b"\n"
    return PreparedCommand(
        (sys.executable, "-c", _TURNS, *variant), tmp_path, line, keep_alive=True
    )


async def _run(
    manager: InvocationManager,
    tmp_path: Path,
    prompt: str = "work",
    session_id: str | None = None,
    *variant: str,
) -> InvocationResult:
    request = InvocationRequest("codex", prompt, tmp_path, session_id=session_id)
    invocation_id = manager.start(request, prepared=_turn(tmp_path, prompt, session_id, *variant))
    result = await manager.wait(invocation_id, 30)
    assert result is not None
    return result


def _line(session_id: str | None, prompt: str) -> bytes:
    return json.dumps({"session_id": session_id, "prompt": prompt}).encode() + b"\n"


async def _run_prepared(
    manager: InvocationManager, request: InvocationRequest, prepared: PreparedCommand
) -> InvocationResult:
    result = await manager.wait(manager.start(request, prepared=prepared), 30)
    assert result is not None
    return result


def _pid(result: InvocationResult) -> int:
    assert result.status is InvocationStatus.SUCCEEDED, result
    return int(result.output.split(":")[0])


def _turn_number(result: InvocationResult) -> int:
    return int(result.output.split(":")[1])


async def _gone(pid: int) -> bool:
    """Whether the process has exited, waiting up to five seconds (POSIX only)."""
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        await asyncio.sleep(0.05)
    return False


posix_only = pytest.mark.skipif(os.name != "posix", reason="POSIX process liveness check")


@pytest.mark.asyncio
async def test_resume_within_keep_period_reuses_the_process(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        assert first.session_id is not None
        # Kept, but no longer a run: nothing counts it against the running limit.
        assert manager.active_ids() == set()
        second = await _run(manager, tmp_path, "again", first.session_id)
        assert (_pid(second), _turn_number(second)) == (_pid(first), 2)
        assert second.session_id == first.session_id
        third = await _run(manager, tmp_path, "more", first.session_id)
        assert (_pid(third), _turn_number(third)) == (_pid(first), 3)
    finally:
        await manager.shutdown()
    if os.name == "posix":
        assert await _gone(_pid(first))


@pytest.mark.asyncio
async def test_changed_launch_inputs_start_a_new_process(tmp_path: Path) -> None:
    config = tmp_path / "config.toml"
    config.write_text("a")

    def inputs(_env: Mapping[str, str]) -> object:
        return config.read_text()

    manager = InvocationManager()
    try:
        kept = replace(_turn(tmp_path, "work", None), launch_inputs=inputs)
        # The same launch reuses the process while its inputs stay the same ...
        first = await _run_prepared(manager, InvocationRequest("codex", "work", tmp_path), kept)
        same = replace(kept, stdin=_line(first.session_id, "again"))
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        assert _pid(await _run_prepared(manager, request, same)) == _pid(first)
        # ... and a changed config file starts a new one.
        config.write_text("b")
        changed = await _run_prepared(manager, request, same)
        assert _pid(changed) != _pid(first)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_launch_inputs_are_read_once_for_choosing_and_launching(tmp_path: Path) -> None:
    readings = iter(["a", "a", "b"])

    def inputs(_env: Mapping[str, str]) -> object:
        return next(readings)

    manager = InvocationManager()
    try:
        kept = replace(_turn(tmp_path, "work", None), launch_inputs=inputs)
        first = await _run_prepared(manager, InvocationRequest("codex", "work", tmp_path), kept)
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        unkept = replace(_turn(tmp_path, "again", first.session_id, "unkept"), keep_alive=False)
        resume = replace(kept, stdin=_line(first.session_id, "again"), unkept=unkept)
        # One reading serves both the choice and the launch: a later one cannot end the kept
        # process the choice took ...
        assert _pid(await _run_prepared(manager, request, resume)) == _pid(first)
        # ... and a reading that differs from the kept process's runs the one-off command.
        other = await _run_prepared(manager, request, resume)
        assert _pid(other) != _pid(first) and _turn_number(other) == 1
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_session_continued_elsewhere_is_not_served_by_the_kept_process(
    tmp_path: Path,
) -> None:
    log = tmp_path / "rollout.jsonl"
    log.write_text("turn 1")

    def state(_env: Mapping[str, str], _session_id: str) -> object:
        return log.read_text()

    manager = InvocationManager()
    try:
        kept = replace(_turn(tmp_path, "work", None), session_state=state)
        first = await _run_prepared(manager, InvocationRequest("codex", "work", tmp_path), kept)
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        unkept = replace(_turn(tmp_path, "again", first.session_id, "unkept"), keep_alive=False)
        resume = replace(kept, stdin=_line(first.session_id, "again"), unkept=unkept)
        # Another service's one-off run wrote to the session since this one kept its process.
        log.write_text("turn 1, turn 2 elsewhere")
        resumed = await _run_prepared(manager, request, resume)
        assert _pid(resumed) != _pid(first) and _turn_number(resumed) == 1
        if os.name == "posix":
            assert await _gone(_pid(first))
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_resume_without_a_kept_process_runs_the_unkept_command(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        unkept = replace(_turn(tmp_path, "again", first.session_id, "unkept"), keep_alive=False)
        resume = replace(_turn(tmp_path, "again", first.session_id), unkept=unkept)
        # A kept process serves the resume ...
        assert _pid(await _run_prepared(manager, request, resume)) == _pid(first)
        # ... and with none kept for the session, the one-off command runs, unkept.
        other = InvocationRequest("codex", "again", tmp_path, session_id="never-kept")
        cold = replace(resume, unkept=replace(unkept, stdin=_line("never-kept", "again")))
        result = await _run_prepared(manager, other, cold)
        assert _pid(result) != _pid(first) and _turn_number(result) == 1
        if os.name == "posix":
            assert await _gone(_pid(result))
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_kept_process_ends_when_the_keep_period_runs_out(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(InvocationManager, "keep_seconds", 0.3)
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        await asyncio.sleep(0.6)
        if os.name == "posix":
            assert await _gone(_pid(first))
        resumed = await _run(manager, tmp_path, "again", first.session_id)
        assert _pid(resumed) != _pid(first)
        assert _turn_number(resumed) == 1
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_resume_started_differently_ends_the_kept_process(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        other = await _run(manager, tmp_path, "again", first.session_id, "other-launch")
        assert _pid(other) != _pid(first)
        if os.name == "posix":
            assert await _gone(_pid(first))
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_more_kept_than_the_limit_ends_the_least_recently_used(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(InvocationManager, "max_kept", 2)
    manager = InvocationManager()
    try:
        oldest = await _run(manager, tmp_path)
        middle = await _run(manager, tmp_path)
        # Using the oldest again makes the middle one the least recently used.
        reused = await _run(manager, tmp_path, "again", oldest.session_id)
        assert _pid(reused) == _pid(oldest)
        newest = await _run(manager, tmp_path)
        if os.name == "posix":
            assert await _gone(_pid(middle))
        assert _pid(await _run(manager, tmp_path, "x", middle.session_id)) != _pid(middle)
        assert _pid(await _run(manager, tmp_path, "x", newest.session_id)) == _pid(newest)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_cancelled_or_failed_turn_does_not_keep_the_process(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        assert first.session_id is not None
        request = InvocationRequest("codex", "slow", tmp_path, session_id=first.session_id)
        slow = manager.start(request, prepared=_turn(tmp_path, "slow", first.session_id))
        await asyncio.sleep(0.3)
        cancelled = await manager.cancel(slow)
        assert cancelled.status is InvocationStatus.CANCELLED
        if os.name == "posix":
            assert await _gone(_pid(first))
        resumed = await _run(manager, tmp_path, "again", first.session_id)
        failed = await _run(manager, tmp_path, "fail", resumed.session_id)
        assert failed.status is InvocationStatus.FAILED
        after = await _run(manager, tmp_path, "again", resumed.session_id)
        assert _pid(after) != _pid(resumed)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@posix_only
async def test_shutdown_ends_kept_processes(tmp_path: Path) -> None:
    manager = InvocationManager()
    first = await _run(manager, tmp_path)
    second = await _run(manager, tmp_path)
    await manager.shutdown()
    assert await _gone(_pid(first))
    assert await _gone(_pid(second))


@pytest.mark.asyncio
async def test_unkept_resume_of_a_kept_session_ends_the_kept_process(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        unkept = replace(_turn(tmp_path, "again", first.session_id), keep_alive=False)
        result = await manager.wait(manager.start(request, prepared=unkept), 30)
        assert result is not None and _pid(result) != _pid(first)
        if os.name == "posix":
            assert await _gone(_pid(first))
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_isolated_run_does_not_keep_the_process(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        request = InvocationRequest("codex", "work", tmp_path)
        invocation_id = manager.start(
            request, prepared=_turn(tmp_path, "work", None), isolated=True
        )
        first = await manager.wait(invocation_id, 30)
        assert first is not None and first.session_id is not None
        if os.name == "posix":
            assert await _gone(_pid(first))
        resumed = await _run(manager, tmp_path, "again", first.session_id)
        assert _pid(resumed) != _pid(first)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@posix_only
async def test_resume_waits_for_the_sessions_ending_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(InvocationManager, "keep_seconds", 0.2)
    manager = InvocationManager()
    try:
        # The kept process takes a second to exit once it is told to.
        first = await _run(manager, tmp_path, "work", None, "slow-exit")
        await asyncio.sleep(0.4)  # its keep period ran out: it is ending now
        resumed = await _run(manager, tmp_path, "again", first.session_id, "slow-exit")
        assert _pid(resumed) != _pid(first)
        try:
            os.kill(_pid(first), 0)
        except ProcessLookupError:
            pass
        else:
            pytest.fail("the new process started while the old one was still ending")
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@posix_only
async def test_terminal_resume_ends_the_kept_process_first(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, _ = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    try:
        first = await _run(manager, tmp_path)
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        invocation_id = manager.start(
            request, prepared=PreparedCommand((), tmp_path), terminal=launch
        )
        # The tray is asked to open the session only once nothing else serves it.
        await wait_for_file(tmp_path / "spawn-requests" / f"{invocation_id}.json")
        with pytest.raises(ProcessLookupError):
            os.kill(_pid(first), 0)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@posix_only
async def test_terminal_resume_cancelled_while_the_kept_process_ends_starts_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, _ = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    try:
        # The kept process takes a second to exit, so the cancel lands while it ends.
        first = await _run(manager, tmp_path, "work", None, "slow-exit")
        request = InvocationRequest("codex", "again", tmp_path, session_id=first.session_id)
        invocation_id = manager.start(
            request, prepared=PreparedCommand((), tmp_path), terminal=launch
        )
        await asyncio.sleep(0.2)
        cancelled = await manager.cancel(invocation_id)
        assert cancelled.status is InvocationStatus.CANCELLED
        assert not (tmp_path / "spawn-requests" / f"{invocation_id}.json").exists()
    finally:
        await manager.shutdown()
