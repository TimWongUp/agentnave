from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, cast

import pytest

from agentnave import terminal_execution, terminal_requests
from agentnave.adapters.base import PreparedCommand
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationStatus
from agentnave.push_capability import TrayIdentity
from agentnave.terminal_execution import TerminalLaunch


async def wait_for_file(path: Path) -> None:
    for _ in range(100):
        if path.exists():
            return
        await asyncio.sleep(0.02)
    raise AssertionError(f"file not created: {path}")


def snapshot(path: Path, entries: dict[str, object]) -> None:
    path.write_text(json.dumps({"version": 1, "pid": 42, "requests": entries}))


def setup_tray(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[TerminalLaunch, Path]:
    folder = tmp_path / "push"
    folder.mkdir()
    path = folder / "42.json"
    snapshot(path, {})
    birth = path.stat().st_mtime - 1

    def started(_pid: int) -> float:
        return birth

    monkeypatch.setattr(terminal_requests, "read_process_started", started)
    monkeypatch.setattr(terminal_execution, "read_process_started", started)
    return TerminalLaunch(tmp_path, TrayIdentity(42, birth), "s-boss", "workspace", None), path


@pytest.mark.asyncio
async def test_terminal_lifecycle_uses_receipt_and_preserves_capacity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    request = InvocationRequest("codex", "task", tmp_path)

    def read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "answer"

    monkeypatch.setattr(terminal_execution, "read_turn", read_turn)
    invocation = manager.start(request, prepared=PreparedCommand((), tmp_path), terminal=launch)
    request_path = tmp_path / "spawn-requests" / f"{invocation}.json"
    await wait_for_file(request_path)
    assert invocation in manager.active_ids()
    value = json.loads(request_path.read_text())
    assert value["task"]["text"].endswith("task")
    assert value["target_tray"]["pid"] == 42
    item: dict[str, object] = {
        "invocation_id": invocation,
        "service_pid": os.getpid(),
        "state": "started",
        "session_id": "native",
        "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
    }
    snapshot(path, {invocation: item})
    request_path.unlink()
    await asyncio.sleep(0.2)
    with pytest.raises(ValueError, match="already has an active invocation"):
        manager.start(
            InvocationRequest("codex", "duplicate", tmp_path, session_id="native"),
            prepared=PreparedCommand((), tmp_path),
            terminal=launch,
        )
    item["state"] = "finished"
    item["terminal"] = {"status": "completed"}
    snapshot(path, {invocation: item})
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert result is not None and result.status is InvocationStatus.SUCCEEDED
    assert result.output == "answer"
    assert invocation not in manager.active_ids()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_shutdown_does_not_send_terminal_cancel(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    await manager.shutdown()
    result = await manager.wait(invocation)
    assert (
        result is not None and result.error is not None and result.error.code == "service_stopped"
    )
    assert len(list((tmp_path / "spawn-requests").glob("*.json"))) == 1
    assert path.exists()


@pytest.mark.asyncio
async def test_late_cancel_is_not_reported_as_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    item: dict[str, object] = {
        "invocation_id": invocation,
        "service_pid": os.getpid(),
        "state": "started",
        "session_id": "native",
        "anchor": {"turn_id": "turn"},
    }
    snapshot(path, {invocation: item})
    await asyncio.sleep(0.2)
    cancel = asyncio.create_task(manager.cancel(invocation))
    cancel_path: Path | None = None
    for _ in range(100):
        files = [p for p in (tmp_path / "spawn-requests").glob("*.json") if p.stem != invocation]
        if files:
            cancel_path = files[0]
            break
        await asyncio.sleep(0.02)
    assert cancel_path is not None
    cancel_doc = cast(dict[str, object], json.loads(cancel_path.read_text()))
    assert cancel_doc["expected_anchor"] == {"session_id": "native", "turn_id": "turn"}
    snapshot(
        path,
        {
            invocation: item,
            cancel_path.stem: {
                "invocation_id": invocation,
                "service_pid": os.getpid(),
                "state": "finished",
                "terminal": {"status": "not_safe_to_interrupt"},
            },
        },
    )
    with pytest.raises(ValueError, match="could not safely"):
        await asyncio.wait_for(cancel, 2)
    assert invocation in manager.active_ids()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_missing_anchor_fails_and_never_uses_other_turn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    snapshot(
        path,
        {
            invocation: {
                "invocation_id": invocation,
                "service_pid": os.getpid(),
                "state": "finished",
                "session_id": "native",
                "terminal": {"status": "completed"},
            }
        },
    )
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert result is not None and result.error is not None and result.error.code == "missing_anchor"
    await manager.shutdown()


@pytest.mark.asyncio
async def test_tray_loss_reports_delivery_unknown_without_resending(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    path.unlink()
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert (
        result is not None and result.error is not None and result.error.code == "delivery_unknown"
    )
    assert len(list((tmp_path / "spawn-requests").glob("*.json"))) == 1
    await manager.shutdown()


@pytest.mark.asyncio
async def test_terminal_schema_hidden_for_derived_connection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from mcp.server.mcpserver.exceptions import ToolError

    from agentnave import mcp_runtime
    from agentnave.mcp_server import mcp

    monkeypatch.setenv("AGENTNAVE_DERIVED_SESSION", "1")
    assert not mcp_runtime.terminal_available()
    tools = {tool.name: tool for tool in await mcp.list_tools()}
    assert "terminal" not in tools["start_agent"].input_schema["properties"]
    with pytest.raises(ToolError, match="terminal is not supported"):
        await mcp.call_tool("start_agent", {"prompt": "x", "terminal": True})


@pytest.mark.asyncio
async def test_failed_lease_handoff_finishes_without_running(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    settled: list[InvocationStatus] = []

    def settle(_invocation: str, status: InvocationStatus) -> None:
        settled.append(status)

    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
        on_settled=settle,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    snapshot(
        path,
        {
            invocation: {
                "invocation_id": invocation,
                "service_pid": os.getpid(),
                "state": "finished",
                "terminal": {"status": "failed", "reason": "lease acquisition failed"},
            }
        },
    )
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert result is not None and result.status == InvocationStatus.FAILED
    assert result.error is not None and "lease acquisition failed" in result.error.message
    assert settled == [InvocationStatus.FAILED]
    await manager.shutdown()


@pytest.mark.asyncio
async def test_terminal_schema_requires_matching_tray(monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave import mcp_runtime, push_capability
    from agentnave.mcp_server import mcp

    monkeypatch.delenv("AGENTNAVE_DERIVED_SESSION", raising=False)
    monkeypatch.setattr(mcp_runtime.sys, "platform", "darwin")

    def find_tray(_root: Path, _provider: str | None) -> TrayIdentity:
        return TrayIdentity(42, 1.0)

    def read_tray(_root: Path, _tray: TrayIdentity) -> dict[str, object]:
        return {"terminal_providers": ["codex"]}

    monkeypatch.setattr(push_capability, "find_tray", find_tray)
    monkeypatch.setattr(terminal_requests, "read_tray", read_tray)
    tools = {tool.name: tool for tool in await mcp.list_tools()}
    assert "terminal" in tools["start_agent"].input_schema["properties"]
    if "start_agents" in tools:
        assert "terminal" not in tools["start_agents"].input_schema["properties"]
    monkeypatch.setenv("AGENTNAVE_DERIVED_SESSION", "1")
    assert not mcp_runtime.terminal_available()


@pytest.mark.asyncio
async def test_conversation_persists_terminal_result_and_acknowledges(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import conversations as conversations_module
    from agentnave.conversations import Conversations
    from agentnave.members import CapacityError
    from agentnave.models import Host

    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    conversations = Conversations(manager, tmp_path, host=Host(name="codex"))

    def read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "answer"

    monkeypatch.setattr(terminal_execution, "read_turn", read_turn)
    monkeypatch.setattr(conversations_module, "read_max_running", lambda: 1)
    try:
        task = conversations.start_task(
            InvocationRequest("codex", "task", tmp_path),
            None,
            None,
            manager_session_id="boss",
            terminal_tray=launch.tray,
        )
        await wait_for_file(tmp_path / "spawn-requests" / f"{task.invocation_id}.json")
        assert conversations.capacity()[0] == 1
        with pytest.raises(CapacityError):
            conversations.start_task(
                InvocationRequest("codex", "second", tmp_path),
                None,
                None,
                manager_session_id="boss",
                terminal_tray=launch.tray,
            )
        snapshot(
            path,
            {
                task.invocation_id: {
                    "invocation_id": task.invocation_id,
                    "service_pid": os.getpid(),
                    "state": "finished",
                    "session_id": "native",
                    "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
                    "terminal": {"status": "completed"},
                }
            },
        )
        result = await asyncio.wait_for(manager.wait(task.invocation_id), 2)
        assert result is not None and result.output == "answer"
        await asyncio.gather(*conversations.collectors)
        saved = conversations.records[task.conversation_id].state.tasks[-1]
        assert saved.output == "answer" and saved.execution_mode == "terminal"
        assert manager.settled_requests == [
            {
                "tray_pid": 42,
                "tray_started_at": launch.tray.started_at,
                "request_id": task.invocation_id,
            }
        ]
        assert conversations.capacity()[0] == 0
    finally:
        await manager.shutdown()
        await conversations.close()


@pytest.mark.asyncio
async def test_mcp_terminal_dispatch_writes_run_request(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from mcp import Client

    from agentnave import mcp_runtime, mcp_tasks
    from agentnave.mcp_server import mcp
    from agentnave.models import Host

    root = tmp_path / "agentnave-data"
    root.mkdir(exist_ok=True)
    (root / "push").mkdir()
    path = root / "push/42.json"
    snapshot(path, {})
    started = path.stat().st_mtime - 1
    tray = TrayIdentity(42, started)

    def find_tray(_root: Path, _provider: str | None) -> TrayIdentity:
        return tray

    def read_capability(_root: Path, _tray: TrayIdentity) -> dict[str, object]:
        return {"terminal_providers": ["codex"]}

    def process_started(_pid: int) -> float:
        return started

    def read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "answer"

    monkeypatch.setattr(mcp_runtime, "HOST", Host(name="codex"))
    monkeypatch.setattr(mcp_runtime, "terminal_available", lambda: True)
    monkeypatch.setattr(mcp_tasks, "find_tray", find_tray)
    monkeypatch.setattr(mcp_tasks, "read_tray", read_capability)
    monkeypatch.setattr(terminal_requests, "read_process_started", process_started)
    monkeypatch.setattr(terminal_execution, "read_process_started", process_started)
    monkeypatch.setattr(terminal_execution, "read_turn", read_turn)
    async with Client(mcp) as client:
        response = await client.call_tool(
            "start_agent",
            {"provider": "codex", "cwd": str(tmp_path), "prompt": "hello", "terminal": True},
        )
        assert not response.is_error, response.content
        result = cast(dict[str, object], response.structured_content)
        invocation = str(result["invocation_id"])
        request_path = root / "spawn-requests" / f"{invocation}.json"
        await wait_for_file(request_path)
        request = json.loads(request_path.read_text())
        assert request["kind"] == "run" and request["config"]["provider"] == "codex"
        snapshot(
            path,
            {
                invocation: {
                    "invocation_id": invocation,
                    "service_pid": os.getpid(),
                    "state": "finished",
                    "session_id": "native",
                    "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
                    "terminal": {"status": "completed"},
                }
            },
        )
        completed = await client.call_tool("wait", {"ids": [invocation], "wait_seconds": 270})
        assert not completed.is_error
        background = await client.call_tool(
            "start_agent", {"session_id": "native", "prompt": "background", "terminal": False}
        )
        assert background.is_error and "background mode" in str(background.content)
        resumed = await client.call_tool(
            "start_agent", {"session_id": "native", "prompt": "continue"}
        )
        assert not resumed.is_error, resumed.content
        resumed_id = str(cast(dict[str, object], resumed.structured_content)["invocation_id"])
        resumed_path = root / "spawn-requests" / f"{resumed_id}.json"
        await wait_for_file(resumed_path)
        assert json.loads(resumed_path.read_text())["session_id"] == "native"


@pytest.mark.asyncio
async def test_settlement_ack_is_retained_until_tray_removes_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import mcp_runtime

    launch, path = setup_tray(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    snapshot(path, {"req": {"state": "finished"}})
    runtime = mcp_runtime.Runtime()
    try:
        runtime.manager.ack_terminal_request("req", launch.tray)
        await asyncio.sleep(0.05)
        payload = json.loads(runtime.inbox_snapshot.path.read_text())
        assert payload["settled_requests"][0]["request_id"] == "req"
        snapshot(path, {})
        runtime.refresh_inbox()
        assert not runtime.inbox_snapshot.path.exists()
        assert not runtime.manager.settled_requests
    finally:
        runtime.close_inbox()
        await runtime.retention.close()
        await runtime.manager.shutdown()
        await runtime.conversations.close()


@pytest.mark.asyncio
async def test_isolation_lease_handoff_failure_keeps_worktree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import subprocess

    from agentnave.conversations import Conversations
    from agentnave.models import Host

    launch, path = setup_tray(tmp_path, monkeypatch)
    repo = tmp_path / "repo"
    repo.mkdir()
    git = [
        "git",
        "-C",
        str(repo),
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ]
    subprocess.run([*git, "init", "-q"], check=True)
    subprocess.run([*git, "commit", "--allow-empty", "-m", "init", "-q"], check=True)
    manager = InvocationManager()
    conversations = Conversations(manager, tmp_path, host=Host(name="codex"))
    try:
        task = conversations.start_task(
            InvocationRequest("codex", "task", repo, read_only=True),
            None,
            None,
            manager_session_id="boss",
            isolation_requested=True,
            terminal_tray=launch.tray,
        )
        assert task.isolation is not None
        assert conversations.active_lease_paths
        await wait_for_file(tmp_path / "spawn-requests" / f"{task.invocation_id}.json")
        snapshot(
            path,
            {
                task.invocation_id: {
                    "invocation_id": task.invocation_id,
                    "service_pid": os.getpid(),
                    "state": "finished",
                    "terminal": {"status": "failed", "reason": "lease acquisition failed"},
                }
            },
        )
        result = await asyncio.wait_for(manager.wait(task.invocation_id), 5)
        assert result is not None and result.status == InvocationStatus.FAILED
        await asyncio.gather(*conversations.collectors)
        assert not conversations.active_lease_paths
        assert Path(task.isolation.path).exists()
    finally:
        await manager.shutdown()
        await conversations.close()


@pytest.mark.asyncio
async def test_successful_isolation_transfer_keeps_tray_lease(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import subprocess

    from agentnave.conversations import Conversations
    from agentnave.isolated.leases import acquire_lease_locked, has_active_leases, repo_lock
    from agentnave.models import Host

    launch, path = setup_tray(tmp_path, monkeypatch)
    repo = tmp_path / "repo"
    repo.mkdir()
    git = [
        "git",
        "-C",
        str(repo),
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ]
    subprocess.run([*git, "init", "-q"], check=True)
    subprocess.run([*git, "commit", "--allow-empty", "-m", "init", "-q"], check=True)
    manager = InvocationManager()
    conversations = Conversations(manager, tmp_path, host=Host(name="codex"))

    def read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "answer"

    monkeypatch.setattr(terminal_execution, "read_turn", read_turn)
    tray_lease = None
    try:
        task = conversations.start_task(
            InvocationRequest("codex", "task", repo, read_only=True),
            None,
            None,
            manager_session_id="boss",
            isolation_requested=True,
            terminal_tray=launch.tray,
        )
        info = task.isolation
        assert info is not None
        await wait_for_file(tmp_path / "spawn-requests" / f"{task.invocation_id}.json")
        with repo_lock(info.repo_common_dir):
            tray_lease = acquire_lease_locked(info.repo_common_dir, info.id)
        snapshot(
            path,
            {
                task.invocation_id: {
                    "invocation_id": task.invocation_id,
                    "service_pid": os.getpid(),
                    "state": "finished",
                    "session_id": "native",
                    "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
                    "terminal": {"status": "completed"},
                }
            },
        )
        result = await asyncio.wait_for(manager.wait(task.invocation_id), 5)
        assert result is not None and result.status == InvocationStatus.SUCCEEDED
        await asyncio.gather(*conversations.collectors)
        assert not conversations.active_lease_paths
        assert Path(info.path).exists()
        with repo_lock(info.repo_common_dir):
            assert has_active_leases(info.repo_common_dir, info.id)
    finally:
        if tray_lease is not None:
            tray_lease.release()
        await manager.shutdown()
        await conversations.close()


@pytest.mark.asyncio
async def test_cancel_interrupts_only_matching_terminal_turn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    item: dict[str, object] = {
        "invocation_id": invocation,
        "service_pid": os.getpid(),
        "state": "started",
        "session_id": "native",
        "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
    }
    snapshot(path, {invocation: item})
    await asyncio.sleep(0.2)
    cancel = asyncio.create_task(manager.cancel(invocation))
    files: list[Path] = []
    for _ in range(100):
        files = [p for p in (tmp_path / "spawn-requests").glob("*.json") if p.stem != invocation]
        if files:
            break
        await asyncio.sleep(0.02)
    assert files
    assert json.loads(files[0].read_text())["expected_anchor"] == {
        "session_id": "native",
        "turn_id": "turn",
        "client_user_message_id": "client",
    }
    item.update({"state": "finished", "terminal": {"status": "interrupted"}})
    snapshot(
        path,
        {
            invocation: item,
            files[0].stem: {
                "invocation_id": invocation,
                "service_pid": os.getpid(),
                "state": "finished",
                "terminal": {"status": "interrupted"},
            },
        },
    )
    result = await asyncio.wait_for(cancel, 2)
    assert result.status == InvocationStatus.CANCELLED
    assert any(ack["request_id"] == files[0].stem for ack in manager.settled_requests)
    await manager.shutdown()


@pytest.mark.asyncio
async def test_changed_turn_anchor_does_not_settle_another_turn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    item: dict[str, object] = {
        "invocation_id": invocation,
        "service_pid": os.getpid(),
        "state": "started",
        "session_id": "native",
        "anchor": {"turn_id": "first"},
    }
    snapshot(path, {invocation: item})
    await asyncio.sleep(0.2)
    item.update(
        {"state": "finished", "anchor": {"turn_id": "other"}, "terminal": {"status": "completed"}}
    )
    snapshot(path, {invocation: item})
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert (
        result is not None and result.error is not None and result.error.code == "receipt_changed"
    )
    await manager.shutdown()


def test_make_launch_account_and_shared_contract_fixture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import re

    from agentnave.inbox_snapshot import InboxSnapshot
    from agentnave.isolated.prompts import TaskIsolationInfo
    from agentnave.terminal_execution import TerminalRun, make_launch

    fixture_path = Path(__file__).parent / "fixtures" / "derived-terminal-contract.json"
    fixture = cast(dict[str, Any], json.loads(fixture_path.read_text(encoding="utf-8")))
    tray = TrayIdentity(5678, 1700000000.0)
    boss = ("s-0123456789abcdef0123456789abcdef", "raw-boss")
    workspace = "s-0123456789abcdef0123456789abcdef"

    def fake_pid() -> int:
        return 1234

    def fake_started(_pid: int) -> float:
        return 1700000100.0

    monkeypatch.setattr(terminal_execution.os, "getpid", fake_pid)
    monkeypatch.setattr(terminal_execution, "read_process_started", fake_started)

    # Codex, Antigravity, and default Claude must pass account=None and account_dir=None.
    for provider in ("codex", "antigravity", "claude"):
        launch = make_launch(
            tmp_path, tray, boss, workspace, InvocationRequest(provider, "t", tmp_path), None
        )
        assert launch.account_dir is None

    wt_dir = (tmp_path / "repo/.agentnave-worktrees/wt-abcdef012345").resolve()
    wt_dir.mkdir(parents=True)
    iso = TaskIsolationInfo(
        id="wt-abcdef012345",
        path=str(wt_dir),
        cwd=str(wt_dir),
        branch="agentnave/wt-abcdef012345",
        repo_common_dir=tmp_path / "repo/.git",
    )
    claude_req = InvocationRequest(
        "claude",
        "Run task",
        wt_dir,
        provider_options={
            "model": "claude-sonnet-4-6",
            "effort": "high",
            "permission_mode": "auto",
            "permission_fallback": "bypassPermissions",
        },
        account="work",
    )
    claude_launch = make_launch(tmp_path, tray, boss, workspace, claude_req, iso)
    assert claude_launch.account_dir is None

    run = TerminalRun(
        claude_launch,
        claude_req,
        "11111111-1111-4111-8111-111111111111",
        "Run task",
        lambda _item: None,
        lambda _item: None,
        lambda _req, _tray: None,
    )
    generated_run = run._run_request()  # pyright: ignore[reportPrivateUsage]
    expected_run = cast(dict[str, Any], fixture["run_request"])
    assert set(generated_run) == set(expected_run)
    generated_cfg = cast(dict[str, Any], generated_run["config"])
    expected_cfg = cast(dict[str, Any], expected_run["config"])
    assert set(generated_cfg) == set(expected_cfg)
    assert set(cast(dict[str, Any], generated_cfg["isolation"])) == set(expected_cfg["isolation"])
    assert generated_cfg["provider"] == expected_cfg["provider"]
    assert generated_cfg["account"] == expected_cfg["account"]
    assert generated_cfg["account_dir"] is expected_cfg["account_dir"] is None
    assert generated_cfg["model"] == expected_cfg["model"]
    assert generated_cfg["effort"] == expected_cfg["effort"]
    assert generated_cfg["permissions"] == expected_cfg["permissions"]
    assert generated_run["parent"] == expected_run["parent"]
    assert generated_run["task"] == expected_run["task"]

    run.session_id = "33333333-3333-4333-8333-333333333333"
    run.anchor = {"turn_id": "turn-1", "client_user_message_id": run.invocation_id}
    run._send_cancel()  # pyright: ignore[reportPrivateUsage]
    cancel_files = list((tmp_path / "spawn-requests").glob("*.json"))
    assert len(cancel_files) == 1
    assert re.fullmatch(
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", cancel_files[0].stem
    )
    generated_cancel = cast(dict[str, Any], json.loads(cancel_files[0].read_text()))
    expected_cancel = cast(dict[str, Any], fixture["cancel_request"])
    assert set(generated_cancel) == set(expected_cancel)
    assert generated_cancel["expected_anchor"] == expected_cancel["expected_anchor"]

    # Verify Python consumes the tray's push_receipt fixture (run + cancel) and emits inbox_settlement.
    push_doc = cast(dict[str, object], fixture["push_receipt"])
    assert push_doc["derived_pids"] == [2468]
    run_receipt = terminal_requests.receipt(
        push_doc, expected_run["request_id"], expected_run["invocation_id"]
    )
    assert run_receipt is not None

    def fake_read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "done"

    monkeypatch.setattr(terminal_execution, "read_turn", fake_read_turn)
    run.anchor = cast(dict[str, object], run_receipt["anchor"])
    finished = run._finish(run_receipt)  # pyright: ignore[reportPrivateUsage]
    assert finished.status == InvocationStatus.SUCCEEDED
    assert finished.output == "done"

    acked: list[tuple[str, TrayIdentity]] = []
    run.ack = lambda req_id, t: acked.append((req_id, t))
    run._cancel_id = expected_cancel["request_id"]  # pyright: ignore[reportPrivateUsage]
    run._cancel_receipt(push_doc)  # pyright: ignore[reportPrivateUsage]
    assert acked == [(expected_cancel["request_id"], tray)]

    inbox = InboxSnapshot(tmp_path, 1234)
    settled = cast(list[dict[str, object]], fixture["inbox_settlement"]["settled_requests"])
    inbox.publish({}, settled)
    assert json.loads(inbox.path.read_text()) == fixture["inbox_settlement"]


@pytest.mark.asyncio
async def test_cancel_during_on_settled_returns_already_finished_without_hanging(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import threading

    launch, path = setup_tray(tmp_path, monkeypatch)
    manager = InvocationManager()

    def read_turn(
        _provider: str, _account: str | None, _session: str, _anchor: dict[str, object]
    ) -> str:
        return "done"

    monkeypatch.setattr(terminal_execution, "read_turn", read_turn)
    entered_settle = threading.Event()
    release_settle = threading.Event()

    def slow_settle(_invocation: str, _status: InvocationStatus) -> None:
        entered_settle.set()
        release_settle.wait(timeout=5)

    invocation = manager.start(
        InvocationRequest("codex", "task", tmp_path),
        prepared=PreparedCommand((), tmp_path),
        terminal=launch,
        on_settled=slow_settle,
    )
    await wait_for_file(tmp_path / "spawn-requests" / f"{invocation}.json")
    snapshot(
        path,
        {
            invocation: {
                "invocation_id": invocation,
                "service_pid": os.getpid(),
                "state": "finished",
                "session_id": "native",
                "anchor": {"turn_id": "turn", "client_user_message_id": "client"},
                "terminal": {"status": "completed"},
            }
        },
    )
    for _ in range(100):
        if entered_settle.is_set():
            break
        await asyncio.sleep(0.02)
    assert entered_settle.is_set()
    run = manager.terminal_runs[invocation]
    outcome = await asyncio.wait_for(run.cancel(), 1)
    assert outcome == "already_finished"
    release_settle.set()
    result = await asyncio.wait_for(manager.wait(invocation), 2)
    assert result is not None and result.status is InvocationStatus.SUCCEEDED
    await manager.shutdown()


@pytest.mark.asyncio
async def test_mcp_cancel_converts_not_safe_to_interrupt_to_tool_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from mcp import Client

    from agentnave import mcp_runtime, mcp_tasks
    from agentnave.mcp_server import mcp
    from agentnave.models import Host

    root = tmp_path / "agentnave-data"
    root.mkdir(exist_ok=True)
    (root / "push").mkdir()
    path = root / "push/42.json"
    snapshot(path, {})
    started = path.stat().st_mtime - 1
    tray = TrayIdentity(42, started)

    def find_tray(_root: Path, _provider: str | None) -> TrayIdentity:
        return tray

    def read_capability(_root: Path, _tray: TrayIdentity) -> dict[str, object]:
        return {"terminal_providers": ["codex"]}

    def process_started(_pid: int) -> float:
        return started

    monkeypatch.setattr(mcp_runtime, "HOST", Host(name="codex"))
    monkeypatch.setattr(mcp_runtime, "terminal_available", lambda: True)
    monkeypatch.setattr(mcp_tasks, "find_tray", find_tray)
    monkeypatch.setattr(mcp_tasks, "read_tray", read_capability)
    monkeypatch.setattr(terminal_requests, "read_process_started", process_started)
    monkeypatch.setattr(terminal_execution, "read_process_started", process_started)
    async with Client(mcp) as client:
        response = await client.call_tool(
            "start_agent",
            {"provider": "codex", "cwd": str(tmp_path), "prompt": "hello", "terminal": True},
        )
        assert not response.is_error, response.content
        invocation = str(cast(dict[str, object], response.structured_content)["invocation_id"])
        await wait_for_file(root / "spawn-requests" / f"{invocation}.json")
        item: dict[str, object] = {
            "invocation_id": invocation,
            "service_pid": os.getpid(),
            "state": "started",
            "session_id": "native",
            "anchor": {"turn_id": "turn"},
        }
        snapshot(path, {invocation: item})
        await asyncio.sleep(0.2)
        cancel_task = asyncio.create_task(client.call_tool("cancel", {"id": invocation}))
        cancel_file: Path | None = None
        for _ in range(100):
            files = [p for p in (root / "spawn-requests").glob("*.json") if p.stem != invocation]
            if files:
                cancel_file = files[0]
                break
            await asyncio.sleep(0.02)
        assert cancel_file is not None
        snapshot(
            path,
            {
                invocation: item,
                cancel_file.stem: {
                    "invocation_id": invocation,
                    "service_pid": os.getpid(),
                    "state": "finished",
                    "terminal": {"status": "not_safe_to_interrupt"},
                },
            },
        )
        cancel_resp = await asyncio.wait_for(cancel_task, 2)
        assert cancel_resp.is_error
        assert "could not safely" in str(cancel_resp.content)


def test_terminal_unavailable_when_ancestor_is_derived_terminal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import mcp_runtime, push_capability

    monkeypatch.delenv("AGENTNAVE_DERIVED_SESSION", raising=False)
    monkeypatch.setattr(mcp_runtime.sys, "platform", "darwin")
    monkeypatch.setattr(push_capability.sys, "platform", "darwin")

    # Simulate Codex derived terminal tree without AGENTNAVE_DERIVED_SESSION:
    # mcp_server -> relay (80) -> codex app-server (50) -> derived wrapper (20) -> tray (10)
    parents = {80: 50, 50: 20, 20: 10, 10: 1}
    derived_pids: list[int] = [20]

    def parent(pid: int) -> int:
        return parents[pid]

    def find_tray(_root: Path, _provider: str | None) -> TrayIdentity:
        return TrayIdentity(10, 1.0)

    def read_capability(_root: Path, _tray: TrayIdentity) -> dict[str, object]:
        return {"terminal_providers": ["codex"], "derived_pids": list(derived_pids)}

    monkeypatch.setattr(push_capability.os, "getppid", lambda: 80)
    monkeypatch.setattr(push_capability, "read_process_parent", parent)
    monkeypatch.setattr(push_capability, "find_tray", find_tray)
    monkeypatch.setattr(terminal_requests, "read_tray", read_capability)
    assert not mcp_runtime.terminal_available()

    # Sibling derived terminal PID (not in our ancestor chain) must not block boss terminal.
    derived_pids[:] = [99]
    assert mcp_runtime.terminal_available()
