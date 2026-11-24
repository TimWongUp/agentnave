"""The Codex app-server bridge: one thread over turns, sent what `codex exec` sends."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, cast

import pytest
from fake_cli import write_fake_cli

from agentnave.adapters import codex
from agentnave.adapters.codex_app_server import cancelled_elicitation, warning
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationResult, InvocationStatus, ProviderOption

# A stand-in for `codex app-server` (and `codex exec`): logs what it is sent and answers like the
# real protocol. The thread's resolved approval and reviewer come from its --config overrides.
_APP_SERVER = """
import json, os, sys, time
log = os.environ["FAKE_CODEX_LOG"]
def record(value):
    with open(log, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value) + "\\n")
def out(value):
    sys.stdout.write(json.dumps(value) + "\\n")
    sys.stdout.flush()
record({"argv": sys.argv[1:], "pid": os.getpid(), "cwd": os.getcwd()})
if sys.argv[1] == "exec":
    out({"type": "thread.started", "thread_id": sys.argv[-2]})
    out({"type": "item.completed", "item": {"type": "agent_message", "text": "exec"}})
    out({"type": "turn.completed"})
    sys.exit(0)
settings = dict(arg.split("=", 1) for arg in sys.argv[2:] if "=" in arg)
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
    elif method == "thread/start":
        thread = f"thread-{os.getpid()}"
        params = message["params"]
        approval = params.get("approvalPolicy") or json.loads(
            settings.get("approval_policy", '"on-request"'))
        reviewer = os.environ.get("FAKE_CODEX_REVIEWER") or json.loads(
            settings.get("approvals_reviewer", '"user"'))
        out({"method": "thread/started", "params": {"thread": {"id": thread}}})
        out({"id": message["id"], "result": {"thread": {"id": thread},
             "approvalPolicy": approval, "approvalsReviewer": reviewer}})
    elif method == "turn/start":
        turns += 1
        turn = f"turn-{turns}"
        pending = (turn, message["params"])
        started = {} if os.environ.get("FAKE_CODEX_NO_TURN_ID") else {"id": turn}
        out({"id": message["id"], "result": {"turn": started}})
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
        text = f"{os.getpid()}:{turns}:{params.get('model')}:{params.get('effort')}"
        out({"method": "item/completed", "params": {**base,
             "item": {"type": "agentMessage", "id": "m1", "text": text}}})
        out({"method": "turn/completed", "params": {"threadId": thread,
             "turn": {"id": turn, "status": "completed", "items": []}}})
        pending = None
        if os.environ.get("FAKE_CODEX_EXIT_IDLE"):
            time.sleep(0.3)  # gone while idle, well after the turn ended
            break
"""


@pytest.fixture
def fake_codex(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(bin_dir, "codex", _APP_SERVER)
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setattr(codex, "_WINDOWS_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    log = tmp_path / "app-server.jsonl"
    monkeypatch.setenv("FAKE_CODEX_LOG", str(log))
    return log


def _logged(log: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]


def _sent(log: Path, method: str) -> list[dict[str, Any]]:
    return [
        cast(dict[str, Any], entry["params"])
        for entry in _logged(log)
        if entry.get("method") == method
    ]


async def _codex(
    manager: InvocationManager,
    tmp_path: Path,
    prompt: str,
    session_id: str | None = None,
    options: dict[str, ProviderOption] | None = None,
) -> InvocationResult:
    request = InvocationRequest(
        "codex",
        prompt,
        tmp_path,
        session_id=session_id,
        provider_options=options
        if options is not None
        else {"model": "gpt-test", "effort": "low", "approve_for_me": True},
    )
    result = await manager.wait(manager.start(request), 30)
    assert result is not None
    return result


@pytest.mark.asyncio
async def test_codex_turns_share_one_app_server_thread(fake_codex: Path, tmp_path: Path) -> None:
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

    logged = _logged(fake_codex)
    launches = [entry for entry in logged if "argv" in entry]
    # Started in the task's directory with the config overrides `codex exec --approve-for-me` adds.
    assert [(entry["argv"], entry["cwd"]) for entry in launches] == [
        (
            [
                "app-server",
                "--config",
                'approvals_reviewer="auto_review"',
                "--config",
                'approval_policy="on-request"',
                "--config",
                'sandbox_mode="workspace-write"',
            ],
            os.path.realpath(tmp_path),
        )
    ]
    methods = [entry.get("method") for entry in logged if "argv" not in entry]
    assert methods.count("initialize") == methods.count("thread/start") == 1
    assert methods.count("turn/start") == 2 and "thread/resume" not in methods
    # As exec sends them: no cwd and no permissions on thread/start, which leaves them to the
    # config; every turn sends the cwd and the approval policy the thread resolved.
    (start,) = _sent(fake_codex, "thread/start")
    assert start == {
        "ephemeral": False,
        "historyMode": "paginated",
        "threadSource": "user",
        "model": "gpt-test",
    }
    turn_starts = _sent(fake_codex, "turn/start")
    for turn_start in turn_starts:
        assert set(turn_start) == {
            "threadId",
            "turnTrigger",
            "input",
            "cwd",
            "approvalPolicy",
            "model",
            "effort",
        }
        assert (turn_start["cwd"], turn_start["approvalPolicy"]) == (str(tmp_path), "on-request")
    (text_input,) = turn_starts[0]["input"]
    assert text_input["type"] == "text" and text_input["text_elements"] == []
    assert cast(str, text_input["text"]).endswith("first")
    # The approval request was refused, never approved.
    replies = [entry for entry in logged if entry.get("id") == "ask-1"]
    assert len(replies) == 2 and all("error" in reply for reply in replies)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("option", "reviewer", "thread", "approval"),
    [
        # exec asks for nothing unless the resolved reviewer is auto review.
        ("approve_for_me", "user", {}, "never"),
        # --dangerously-bypass-approvals-and-sandbox: exec's harness overrides, never kept.
        (
            "dangerously_bypass_approvals_and_sandbox",
            "auto_review",
            {"approvalPolicy": "never", "sandbox": "danger-full-access"},
            "never",
        ),
    ],
)
async def test_codex_threads_and_turns_carry_execs_permission_fields(
    fake_codex: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    option: str,
    reviewer: str,
    thread: dict[str, object],
    approval: str,
) -> None:
    monkeypatch.setenv("FAKE_CODEX_REVIEWER", reviewer)
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "work", options={option: True})
        assert result.status is InvocationStatus.SUCCEEDED, result
    finally:
        await manager.shutdown()
    (start,) = _sent(fake_codex, "thread/start")
    assert start == {
        "ephemeral": False,
        "historyMode": "paginated",
        "threadSource": "user",
        **thread,
    }
    (turn_start,) = _sent(fake_codex, "turn/start")
    assert "sandboxPolicy" not in turn_start and "approvalsReviewer" not in turn_start
    assert turn_start["approvalPolicy"] == approval


@pytest.mark.asyncio
async def test_codex_resume_without_a_kept_process_runs_exec_resume(
    fake_codex: Path, tmp_path: Path
) -> None:
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "resume", "thread-earlier")
    finally:
        await manager.shutdown()
    assert result.status is InvocationStatus.SUCCEEDED, result
    assert result.output == "exec"
    (launch,) = [entry["argv"] for entry in _logged(fake_codex) if "argv" in entry]
    assert launch[:2] == ["exec", "resume"] and launch[-2:] == ["thread-earlier", "-"]


@pytest.mark.asyncio
async def test_codex_app_server_exiting_between_turns_is_not_reused(
    fake_codex: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_CODEX_EXIT_IDLE", "1")
    manager = InvocationManager()
    try:
        first = await _codex(manager, tmp_path, "first")
        assert first.status is InvocationStatus.SUCCEEDED, first
        await asyncio.sleep(1.5)
        second = await _codex(manager, tmp_path, "second", first.session_id)
        assert second.status is InvocationStatus.SUCCEEDED, second
        assert second.output == "exec"
    finally:
        await manager.shutdown()
    launches = [entry["argv"][0] for entry in _logged(fake_codex) if "argv" in entry]
    assert launches == ["app-server", "exec"]


@pytest.mark.asyncio
async def test_codex_turn_without_id_fails(
    fake_codex: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_CODEX_NO_TURN_ID", "1")
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "first")
    finally:
        await manager.shutdown()
    assert result.status is InvocationStatus.FAILED
    assert result.error is not None and "no turn id" in result.error.message


@pytest.mark.asyncio
async def test_codex_handshake_failure_is_reported_not_retried_on_exec(
    fake_codex: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_CODEX_FAIL_INIT", "1")
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "first")
    finally:
        await manager.shutdown()
    assert result.status is InvocationStatus.FAILED
    assert result.error is not None
    assert "initialize failed: not signed in" in result.error.message
    assert len([entry for entry in _logged(fake_codex) if "argv" in entry]) == 1


def test_config_warnings_are_reported_as_exec_reports_them() -> None:
    config = {"method": "configWarning", "params": {"summary": "downgraded", "details": "why"}}
    assert warning(config, "t") == "downgraded (why)"
    assert warning({"method": "warning", "params": {"message": "m"}}, "t") == "m"
    assert (
        warning({"method": "warning", "params": {"threadId": "other", "message": "m"}}, "t") is None
    )


def test_elicitation_is_cancelled_as_non_interactive_for_tool_approvals() -> None:
    approval = {"mode": "form", "_meta": {"codex_approval_kind": "mcp_tool_call"}}
    assert cancelled_elicitation(approval) == {
        "action": "cancel",
        "content": None,
        "_meta": {"codex_approval_cancellation_reason": "non_interactive"},
    }
    assert cancelled_elicitation({"mode": "form"}) == {"action": "cancel", "content": None}
