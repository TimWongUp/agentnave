"""The Codex app-server bridge: one thread over turns, with permissions resolved per turn."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, cast

import pytest
from fake_cli import write_fake_cli

from agentnave.adapters import codex, codex_permissions
from agentnave.adapters.codex_app_server import cancelled_elicitation
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationResult, InvocationStatus, ProviderOption

# A stand-in for `codex app-server`: logs what it is sent and answers like the real protocol.
# config/read returns FAKE_CODEX_CONFIG (read on every call) under the --config overrides.
_APP_SERVER = """
import json, os, sys, time
log = os.environ["FAKE_CODEX_LOG"]
def record(value):
    with open(log, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value) + "\\n")
def out(value):
    sys.stdout.write(json.dumps(value) + "\\n")
    sys.stdout.flush()
def config():
    path = os.environ.get("FAKE_CODEX_CONFIG")
    values = json.load(open(path, encoding="utf-8")) if path and os.path.exists(path) else {}
    args = sys.argv[1:]
    for flag, setting in zip(args, args[1:]):
        if flag == "--config":
            key, value = setting.split("=", 1)
            values[key] = json.loads(value)
    return values
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
    elif method == "config/read":
        out({"id": message["id"], "result": {"config": config(), "origins": {}}})
    elif method in ("thread/start", "thread/resume"):
        thread = message["params"].get("threadId") or f"thread-{os.getpid()}"
        out({"method": "thread/started", "params": {"thread": {"id": thread}}})
        out({"id": message["id"], "result": {"thread": {"id": thread}}})
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
    monkeypatch.setenv("FAKE_CODEX_CONFIG", str(tmp_path / "config.json"))
    log = tmp_path / "app-server.jsonl"
    monkeypatch.setenv("FAKE_CODEX_LOG", str(log))
    return log


def _set_config(tmp_path: Path, values: dict[str, object]) -> None:
    (tmp_path / "config.json").write_text(json.dumps(values), encoding="utf-8")


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
        else {"model": "gpt-test", "effort": "low", "sandbox": "read-only"},
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
    assert [entry["argv"] for entry in launches] == [
        ["app-server", "--config", 'sandbox_mode="read-only"']
    ]
    methods = [entry.get("method") for entry in logged if "argv" not in entry]
    assert methods.count("initialize") == methods.count("thread/start") == 1
    assert methods.count("turn/start") == 2 and "thread/resume" not in methods
    # Every thread and turn request carries its permissions explicitly.
    allowed = {
        "approvalPolicy": "never",
        "approvalsReviewer": "user",
        "runtimeWorkspaceRoots": [str(tmp_path)],
    }
    (start,) = _sent(fake_codex, "thread/start")
    assert start == {"cwd": str(tmp_path), "model": "gpt-test", "sandbox": "read-only", **allowed}
    for turn_start in _sent(fake_codex, "turn/start"):
        assert {key: turn_start[key] for key in allowed} == allowed
        assert turn_start["sandboxPolicy"] == {"type": "readOnly"}
    (text_input,) = _sent(fake_codex, "turn/start")[0]["input"]
    assert text_input["type"] == "text" and text_input["text_elements"] == []
    assert cast(str, text_input["text"]).endswith("first")
    # The approval request was refused, never approved.
    replies = [entry for entry in logged if entry.get("id") == "ask-1"]
    assert len(replies) == 2 and all("error" in reply for reply in replies)


@pytest.mark.asyncio
async def test_codex_cold_resume_overrides_the_threads_saved_permissions(
    fake_codex: Path, tmp_path: Path
) -> None:
    extra = tmp_path / "extra"
    _set_config(
        tmp_path,
        {
            "approval_policy": "on-request",
            "sandbox_mode": "workspace-write",
            "sandbox_workspace_write": {"writable_roots": [str(extra)], "network_access": True},
        },
    )
    manager = InvocationManager()
    try:
        result = await _codex(manager, tmp_path, "resume", "thread-earlier", {"model": "m"})
        assert result.status is InvocationStatus.SUCCEEDED, result
        assert result.session_id == "thread-earlier"
    finally:
        await manager.shutdown()
    (resume,) = _sent(fake_codex, "thread/resume")
    assert resume == {
        "threadId": "thread-earlier",
        "excludeTurns": True,
        "cwd": str(tmp_path),
        "model": "m",
        # exec never asks for approval, whatever the config says, unless auto review decides.
        "approvalPolicy": "never",
        "approvalsReviewer": "user",
        "sandbox": "workspace-write",
        "runtimeWorkspaceRoots": [str(tmp_path), str(extra)],
    }
    (turn_start,) = _sent(fake_codex, "turn/start")
    assert turn_start["sandboxPolicy"] == {
        "type": "workspaceWrite",
        "writableRoots": [str(extra)],
        "networkAccess": True,
        "excludeTmpdirEnvVar": False,
        "excludeSlashTmp": False,
    }


@pytest.mark.asyncio
async def test_codex_kept_process_follows_a_config_change(fake_codex: Path, tmp_path: Path) -> None:
    _set_config(tmp_path, {"sandbox_mode": "workspace-write"})
    manager = InvocationManager()
    try:
        first = await _codex(manager, tmp_path, "first", options={})
        _set_config(tmp_path, {"sandbox_mode": "read-only"})
        second = await _codex(manager, tmp_path, "second", first.session_id, options={})
        assert second.output.split(":")[:2] == [first.output.split(":")[0], "2"]
    finally:
        await manager.shutdown()
    assert len(_sent(fake_codex, "config/read")) == 2
    policies = [turn["sandboxPolicy"]["type"] for turn in _sent(fake_codex, "turn/start")]
    assert policies == ["workspaceWrite", "readOnly"]


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
        assert second.output.split(":")[0] != first.output.split(":")[0]
    finally:
        await manager.shutdown()
    assert len([entry for entry in _logged(fake_codex) if "argv" in entry]) == 2


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


def test_approval_follows_execs_headless_rule(tmp_path: Path) -> None:
    cwd = str(tmp_path)
    auto = {"approvals_reviewer": "auto_review", "approval_policy": "on-request"}
    assert codex_permissions.resolve(auto, cwd, "headless").approval_policy == "on-request"
    # The bypass flag keeps never even under auto review.
    assert codex_permissions.resolve(auto, cwd, "never").approval_policy == "never"
    user = {"approvals_reviewer": "user", "approval_policy": "on-request"}
    assert codex_permissions.resolve(user, cwd, "headless").approval_policy == "never"


def test_sandbox_defaults_follow_trust_and_profiles(tmp_path: Path) -> None:
    cwd = str(tmp_path)
    unknown = codex_permissions.resolve({}, cwd, "headless")
    assert (unknown.sandbox, unknown.profile) == ("read-only", None)
    trusted = {"projects": {cwd: {"trust_level": "trusted"}}}
    assert codex_permissions.resolve(trusted, cwd, "headless").sandbox == "workspace-write"
    profile = codex_permissions.resolve({"default_permissions": "dev"}, cwd, "headless")
    assert (
        profile.thread_params()["permissions"] == "dev" and "sandbox" not in profile.thread_params()
    )
    assert profile.turn_params()["permissions"] == "dev"
    assert "sandboxPolicy" not in profile.turn_params()


def test_elicitation_is_cancelled_as_non_interactive_for_tool_approvals() -> None:
    approval = {"mode": "form", "_meta": {"codex_approval_kind": "mcp_tool_call"}}
    assert cancelled_elicitation(approval) == {
        "action": "cancel",
        "content": None,
        "_meta": {"codex_approval_cancellation_reason": "non_interactive"},
    }
    assert cancelled_elicitation({"mode": "form"}) == {"action": "cancel", "content": None}
