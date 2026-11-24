"""Processes kept after a turn for the next resume of the same session (ADR-0120)."""

from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from dataclasses import replace
from pathlib import Path

import pytest
from fake_cli import write_fake_cli

from agentnave.adapters import codex
from agentnave.adapters.base import PreparedCommand
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationResult, InvocationStatus

# Speaks the keep-alive contract: one JSON turn per stdin line, Codex exec events per turn.
_TURNS = """
import json, os, sys, time
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


# A stand-in for `codex app-server`: logs what it is sent and answers like the real protocol.
_APP_SERVER = """
import json, os, sys
log = os.environ["FAKE_CODEX_LOG"]
def record(value):
    with open(log, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value) + "\\n")
def out(value):
    sys.stdout.write(json.dumps(value) + "\\n")
    sys.stdout.flush()
record({"argv": sys.argv[1:], "pid": os.getpid()})
thread, turns, pending = None, 0, None
for line in sys.stdin:
    message = json.loads(line)
    record(message)
    method = message.get("method")
    if method == "initialize":
        if os.environ.get("FAKE_CODEX_FAIL_INIT"):
            out({"id": message["id"], "error": {"code": -1, "message": "not signed in"}})
        else:
            out({"id": message["id"], "result": {}})
    elif method in ("thread/start", "thread/resume"):
        thread = message["params"].get("threadId") or f"thread-{os.getpid()}"
        out({"method": "thread/started", "params": {"thread": {"id": thread}}})
        out({"id": message["id"], "result": {"thread": {"id": thread}}})
    elif method == "turn/start":
        turns += 1
        turn = f"turn-{turns}"
        pending = (turn, message["params"])
        out({"id": message["id"], "result": {"turn": {"id": turn, "status": "inProgress"}}})
        out({"method": "turn/started", "params": {"threadId": thread, "turn": {"id": turn}}})
        out({"id": "ask-1", "method": "item/commandExecution/requestApproval",
             "params": {"threadId": thread, "turnId": turn, "itemId": "c1"}})
    elif "id" in message and "method" not in message and pending is not None:
        turn, params = pending
        base = {"threadId": thread, "turnId": turn}
        command = {"type": "commandExecution", "id": "c1", "command": "ls",
                   "aggregatedOutput": "x" * 1000}
        out({"method": "item/started", "params": {**base, "item": {**command, "status": "inProgress"}}})
        out({"method": "item/completed", "params": {**base, "item": {**command, "status": "declined"}}})
        out({"method": "item/completed", "params": {"threadId": "subagent", "turnId": "t",
             "item": {"type": "agentMessage", "id": "m0", "text": "not this thread"}}})
        out({"method": "thread/tokenUsage/updated", "params": {**base,
             "tokenUsage": {"last": {"inputTokens": 3, "cachedInputTokens": 1, "outputTokens": 2,
                                     "reasoningOutputTokens": 0}}}})
        text = f"{os.getpid()}:{turns}:{params.get('model')}:{params.get('effort')}"
        out({"method": "item/completed", "params": {**base,
             "item": {"type": "agentMessage", "id": "m1", "text": text}}})
        out({"method": "turn/completed", "params": {"threadId": thread,
             "turn": {"id": turn, "status": "completed", "items": []}}})
        pending = None
"""


def _fake_codex(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(bin_dir, "codex", _APP_SERVER)
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setattr(codex, "_WINDOWS_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    log = tmp_path / "app-server.jsonl"
    monkeypatch.setenv("FAKE_CODEX_LOG", str(log))
    return log


def _logged(log: Path) -> list[dict[str, object]]:
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]


async def _codex(
    manager: InvocationManager, tmp_path: Path, prompt: str, session_id: str | None = None
) -> InvocationResult:
    request = InvocationRequest(
        "codex",
        prompt,
        tmp_path,
        session_id=session_id,
        provider_options={"model": "gpt-test", "effort": "low", "sandbox": "read-only"},
    )
    result = await manager.wait(manager.start(request), 30)
    assert result is not None
    return result


@pytest.mark.asyncio
async def test_codex_turns_share_one_app_server_thread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = _fake_codex(tmp_path, monkeypatch)
    manager = InvocationManager()
    try:
        first = await _codex(manager, tmp_path, "first")
        assert first.status is InvocationStatus.SUCCEEDED, first
        pid, turn, model, effort = first.output.split(":")
        assert (turn, model, effort) == ("1", "gpt-test", "low")
        assert first.session_id == f"thread-{pid}"
        second = await _codex(manager, tmp_path, "second", first.session_id)
        assert second.status is InvocationStatus.SUCCEEDED, second
        assert second.output.split(":")[:2] == [pid, "2"]
        assert second.session_id == first.session_id
    finally:
        await manager.shutdown()

    logged = _logged(log)
    launches = [entry for entry in logged if "argv" in entry]
    assert len(launches) == 1
    assert launches[0]["argv"] == [
        "app-server",
        "--config",
        'approval_policy="never"',
        "--config",
        'sandbox_mode="read-only"',
    ]
    methods = [entry.get("method") for entry in logged if "argv" not in entry]
    assert methods.count("initialize") == methods.count("thread/start") == 1
    assert methods.count("turn/start") == 2 and "thread/resume" not in methods
    turn_start = next(entry for entry in logged if entry.get("method") == "turn/start")
    params = turn_start["params"]
    assert isinstance(params, dict)
    assert params["input"] == [
        {"type": "text", "text": params["input"][0]["text"], "text_elements": []}
    ]
    assert params["input"][0]["text"].endswith("first")
    # The approval request was refused, never approved.
    replies = [entry for entry in logged if entry.get("id") == "ask-1"]
    assert len(replies) == 2 and all("error" in reply for reply in replies)


@pytest.mark.asyncio
async def test_codex_cold_resume_loads_the_thread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = _fake_codex(tmp_path, monkeypatch)
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "resume", "thread-earlier")
        assert result.status is InvocationStatus.SUCCEEDED, result
        assert result.session_id == "thread-earlier"
    finally:
        await manager.shutdown()
    resume = next(entry for entry in _logged(log) if entry.get("method") == "thread/resume")
    assert resume["params"] == {
        "threadId": "thread-earlier",
        "excludeTurns": True,
        "cwd": str(tmp_path),
        "model": "gpt-test",
    }


@pytest.mark.asyncio
async def test_codex_handshake_failure_is_reported_not_retried_on_exec(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = _fake_codex(tmp_path, monkeypatch)
    monkeypatch.setenv("FAKE_CODEX_FAIL_INIT", "1")
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "first")
    finally:
        await manager.shutdown()
    assert result.status is InvocationStatus.FAILED
    assert result.error is not None
    assert "initialize failed: not signed in" in result.error.message
    assert len([entry for entry in _logged(log) if "argv" in entry]) == 1
