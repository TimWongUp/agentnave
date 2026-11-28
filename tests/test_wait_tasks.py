"""MCP Tasks: `wait` as a lazily collected background task on 2026-07-28 (ADR-0131)."""

from __future__ import annotations

import asyncio
import os
import time
from collections.abc import AsyncGenerator, Awaitable, Callable, Iterator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Literal, cast

import pytest
from fake_cli import write_fake_cli
from jsonschema import Draft202012Validator
from jsonschema.protocols import Validator
from mcp import Client
from mcp.client.extension import ClaimContext, ClientExtension, ResultClaim
from mcp.shared.exceptions import MCPError
from mcp_types import CallToolResult, Request, RequestParams, Result, TextContent
from pydantic import ConfigDict, TypeAdapter

from agentnave import mcp_runtime, mcp_wait_tasks, t3, wait_task_status
from agentnave.adapters.base import AdapterHooks
from agentnave.core import TASK_SEPARATOR
from agentnave.host import Host
from agentnave.mcp_server import mcp
from agentnave.native_prompts import NativePrompt
from agentnave.wait_guidance import TASK_ALREADY_WAITING, TASK_WAIT_AGAIN, TASK_WAIT_NEXT
from agentnave.wait_task_status import WaitScan, detect_language, status_message

EXTENSION = "io.modelcontextprotocol/tasks"


def _installed(_adapter: AdapterHooks) -> bool:
    return True


@pytest.fixture(autouse=True)
def fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """A Claude CLI that answers at once, or once the file named after WAIT: exists."""
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed)
    monkeypatch.setenv("LANG", "en_US.UTF-8")
    monkeypatch.delenv("LC_ALL", raising=False)
    monkeypatch.delenv("LC_MESSAGES", raising=False)
    wait_task_status.ui_language.cache_clear()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, os, sys, time\n"
        f"prompt = sys.stdin.read().split({TASK_SEPARATOR!r}, 1)[1].strip()\n"
        "if prompt.startswith('WAIT:'):\n"
        "    while not os.path.exists(prompt[5:]):\n"
        "        time.sleep(0.05)\n"
        "print(json.dumps({'type': 'result', 'result': 'ok', 'session_id': 'sess-1', "
        "'num_turns': 1, 'is_error': False}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    yield bin_dir
    wait_task_status.ui_language.cache_clear()


@pytest.fixture
def runtimes(monkeypatch: pytest.MonkeyPatch) -> list[mcp_runtime.Runtime]:
    """The Runtime each in-process client's lifespan creates."""
    created: list[mcp_runtime.Runtime] = []
    original = mcp_runtime.Runtime.__init__

    def init(self: mcp_runtime.Runtime) -> None:
        original(self)
        created.append(self)

    monkeypatch.setattr(mcp_runtime.Runtime, "__init__", init)
    return created


class _TaskResult(Result):
    model_config = ConfigDict(extra="allow")
    result_type: Literal["task"] = "task"
    task_id: str
    status: str


class _Tasks(ClientExtension):
    """A fake 2026-07-28 Tasks client that leaves polling to the test."""

    identifier = EXTENSION

    def __init__(self) -> None:
        self.created: list[dict[str, Any]] = []

    def claims(self) -> list[ResultClaim[Any]]:
        return [ResultClaim(result_type="task", model=_TaskResult, resolve=self._resolve)]

    async def _resolve(self, result: _TaskResult, _ctx: ClaimContext) -> CallToolResult:
        self.created.append(result.model_dump(by_alias=True, mode="json"))
        # Stands in for the result a real host would poll for; only wait returns tasks.
        placeholder: dict[str, Any] = {
            "reason": "wait_elapsed",
            "pending_ids": [],
            "all_finished": False,
            "updates": [],
            "rounds": [],
        }
        return CallToolResult(
            content=[TextContent(type="text", text="task")], structured_content=placeholder
        )


class _Params(RequestParams):
    model_config = ConfigDict(extra="allow")


async def _raw(client: Client, method: str, params: dict[str, Any]) -> dict[str, Any]:
    request = Request[_Params, str](method=method, params=_Params.model_validate(params))
    adapter: Any = TypeAdapter(dict[str, Any])
    return cast(dict[str, Any], await client.session.send_request(request, adapter))


@asynccontextmanager
async def _client(
    tasks: _Tasks | None = None, *, mode: str = "2026-07-28"
) -> AsyncGenerator[Client]:
    async with Client(mcp, mode=mode, extensions=[tasks] if tasks else None) as client:
        yield client


def _data(result: CallToolResult) -> dict[str, Any]:
    assert not result.is_error, result.content
    return cast(dict[str, Any], result.structured_content)


async def _start(client: Client, tmp_path: Path, flag: Path) -> str:
    started = _data(
        await client.call_tool(
            "start_agent",
            {"provider": "claude", "cwd": str(tmp_path), "prompt": f"WAIT:{flag}"},
        )
    )
    return cast(str, started["invocation_id"])


async def _until(check: Callable[[], Awaitable[dict[str, Any] | None]]) -> dict[str, Any]:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if (found := await check()) is not None:
            return found
        await asyncio.sleep(0.05)
    raise AssertionError("condition not reached")


async def _completed(client: Client, task_id: str) -> dict[str, Any]:
    async def check() -> dict[str, Any] | None:
        got = await _raw(client, "tasks/get", {"taskId": task_id})
        return got if got["status"] != "working" else None

    return await _until(check)


async def _wait_schema(client: Client) -> Validator:
    tools = {tool.name: tool for tool in (await client.list_tools()).tools}
    schema = tools["wait"].output_schema
    assert schema is not None
    return Draft202012Validator(schema)


@pytest.mark.asyncio
async def test_wait_becomes_a_task_and_collects_like_a_blocking_wait(
    tmp_path: Path, runtimes: list[mcp_runtime.Runtime]
) -> None:
    tasks = _Tasks()
    flag = tmp_path / "done"
    async with _client(tasks) as client:
        discovered = await _raw(client, "server/discover", {})
        assert discovered["capabilities"]["extensions"] == {EXTENSION: {}}
        started = _data(
            await client.call_tool(
                "start_agent",
                {"provider": "claude", "cwd": str(tmp_path), "prompt": f"WAIT:{flag}"},
            )
        )
        assert started["next"] == TASK_WAIT_NEXT
        await client.call_tool("wait", {"seq": 1})
        (created,) = tasks.created
        assert created["status"] == "working"
        assert created["pollIntervalMs"] == 2000 and created["ttlMs"] is None
        assert created["statusMessage"] == "⌨️ Claude Code is typing away…"
        task_id = created["taskId"]
        runtime = runtimes[0]
        assert runtime.wait_tasks.restart_safe() is False
        assert runtime.active_inbox_waits

        polled = await _raw(client, "tasks/get", {"taskId": task_id})
        assert polled["status"] == "working" and "result" not in polled
        # Another blocking wait is refused while the task holds the inbox.
        refused = _data(await client.call_tool("wait", {}))
        assert refused["reason"] == "already_waiting"
        assert refused["next"] == TASK_ALREADY_WAITING

        flag.touch()
        done = await _completed(client, task_id)
        assert done["status"] == "completed"
        assert done["statusMessage"] == "✨ Claude Code turned in the work — handing it over…"
        payload = done["result"]["structuredContent"]
        (await _wait_schema(client)).validate(payload)
        assert payload["reason"] == "ready" and payload["all_finished"] is True
        assert payload["seq"] == 1
        assert payload["updates"][0]["invocation_id"] == started["invocation_id"]
        assert payload["updates"][0]["status"] == "succeeded"
        assert done["result"]["isError"] is False
        # Repeated polls answer the same result; the end was delivered once.
        assert await _raw(client, "tasks/get", {"taskId": task_id}) == done
        assert _data(await client.call_tool("wait", {}))["reason"] == "nothing_pending"
        assert runtime.wait_tasks.restart_safe() is True
        # Cancelling an ended task keeps its end.
        assert _strip(await _raw(client, "tasks/cancel", {"taskId": task_id})) == {}
        assert (await _raw(client, "tasks/get", {"taskId": task_id}))["status"] == "completed"


def _strip(reply: dict[str, Any]) -> dict[str, Any]:
    """The reply without the fields the SDK stamps on every 2026-07-28 result."""
    return {k: v for k, v in reply.items() if k not in ("_meta", "resultType")}


@pytest.mark.asyncio
async def test_cancel_only_stops_waiting(tmp_path: Path) -> None:
    tasks = _Tasks()
    flag = tmp_path / "done"
    async with _client(tasks) as client:
        handle = await _start(client, tmp_path, flag)
        await client.call_tool("wait", {"ids": [handle]})
        task_id = tasks.created[0]["taskId"]
        assert _strip(await _raw(client, "tasks/cancel", {"taskId": task_id})) == {}
        assert (await _raw(client, "tasks/get", {"taskId": task_id}))["status"] == "cancelled"
        running = _data(await client.call_tool("wait", {"ids": [handle], "wait_seconds": 0}))
        assert running["updates"][0]["status"] == "running"
        flag.touch()

        async def ended() -> dict[str, Any] | None:
            got = _data(await client.call_tool("wait", {"wait_seconds": 0}))
            return got if got["reason"] == "ready" else None

        # Nothing was consumed: the end still arrives through the inbox.
        received = await _until(ended)
        assert received["updates"][0]["status"] == "succeeded"
        assert _strip(await _raw(client, "tasks/cancel", {"taskId": task_id})) == {}
        assert (await _raw(client, "tasks/get", {"taskId": task_id}))["status"] == "cancelled"


@pytest.mark.asyncio
async def test_unknown_task_ids_and_legacy_connections(tmp_path: Path) -> None:
    async with _client(_Tasks()) as client:
        with pytest.raises(MCPError) as unknown:
            await _raw(client, "tasks/get", {"taskId": "missing"})
        assert unknown.value.code == -32602
        assert "Call wait again" in unknown.value.error.message
        with pytest.raises(MCPError) as cancelled:
            await _raw(client, "tasks/cancel", {"taskId": "missing"})
        assert cancelled.value.code == -32602
    async with _client(mode="legacy") as client:
        with pytest.raises(MCPError) as legacy:
            await _raw(client, "tasks/get", {"taskId": "missing"})
        assert legacy.value.code == -32601


@pytest.mark.asyncio
async def test_waits_stay_synchronous_without_the_extension_or_when_ready(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tasks = _Tasks()
    async with _client(tasks) as client:
        handle = await _start(client, tmp_path, tmp_path / "never")
        peek = _data(await client.call_tool("wait", {"ids": [handle], "wait_seconds": 0}))
        assert peek["reason"] == "wait_elapsed"
        # A dispatch that waits for its own result keeps waiting in the call.
        flag = tmp_path / "dispatch"
        asyncio.get_running_loop().call_later(0.3, flag.touch)
        waited = _data(
            await client.call_tool(
                "start_agent",
                {
                    "provider": "claude",
                    "cwd": str(tmp_path),
                    "prompt": f"WAIT:{flag}",
                    "wait": True,
                },
            )
        )
        assert waited["wait"]["updates"][0]["status"] == "succeeded"
        quick = _data(
            await client.call_tool(
                "start_agent", {"provider": "claude", "cwd": str(tmp_path), "prompt": "OK"}
            )
        )
        ids = [quick["invocation_id"]]

        async def finished() -> dict[str, Any] | None:
            got = _data(await client.call_tool("wait", {"ids": ids, "wait_seconds": 0}))
            return got if got["updates"][0]["status"] != "running" else None

        await _until(finished)
        # Ready on the first pass: answered at once, no task.
        ready = _data(await client.call_tool("wait", {"ids": ids}))
        assert ready["reason"] == "ready"
        assert (await client.call_tool("wait", {"ids": ["unknown"]})).is_error
        assert tasks.created == []
        await client.call_tool("cancel", {"id": handle})

    flag = tmp_path / "plain"
    async with _client() as client:
        handle = await _start(client, tmp_path, flag)
        asyncio.get_running_loop().call_later(0.3, flag.touch)
        plain = _data(await client.call_tool("wait", {"ids": [handle]}))
        assert plain["reason"] == "ready"

    monkeypatch.setattr(t3, "current_t3_mode", _t3_main)
    flag = tmp_path / "t3"
    async with _client(tasks) as client:
        handle = await _start(client, tmp_path, flag)
        asyncio.get_running_loop().call_later(0.3, flag.touch)
        in_t3 = _data(await client.call_tool("wait", {"ids": [handle]}))
        assert in_t3["reason"] == "ready"
    assert tasks.created == []


def _t3_main(_host: object) -> str:
    return "main"


@pytest.mark.asyncio
async def test_mid_run_events_complete_the_task_and_name_the_member(
    tmp_path: Path, runtimes: list[mcp_runtime.Runtime]
) -> None:
    tasks = _Tasks()
    async with _client(tasks) as client:
        handle = await _start(client, tmp_path, tmp_path / "done")
        manager = runtimes[0].manager
        record = manager._records[handle]  # pyright: ignore[reportPrivateUsage]

        await client.call_tool("wait", {"ids": [handle]})
        assert manager.notify_progress_stalled(handle, 400.0)
        stalled = await _completed(client, tasks.created[-1]["taskId"])
        update = stalled["result"]["structuredContent"]["updates"][0]
        assert update["reason"] == "execution_blocked"
        assert update["error"]["code"] == "progress_stalled"
        assert stalled["result"]["structuredContent"]["next"] == TASK_WAIT_AGAIN

        await client.call_tool("wait", {"ids": [handle]})
        record.prompts.receive(NativePrompt("p" * 32, "permission", "Bash"))
        record.attention_event.set()
        prompted = await _completed(client, tasks.created[-1]["taskId"])
        assert prompted["result"]["structuredContent"]["updates"][0]["reason"] == "prompt_pending"

        # Delivered but unanswered, the prompt shows in the next task's status line.
        await client.call_tool("wait", {"ids": [handle]})
        assert tasks.created[-1]["statusMessage"] == (
            "🙋 Claude Code raised a hand — waiting for your reply~"
        )
        await client.call_tool("cancel", {"id": handle})
        cancelled = await _completed(client, tasks.created[-1]["taskId"])
        assert cancelled["result"]["structuredContent"]["updates"][0]["status"] == "cancelled"


@pytest.mark.asyncio
async def test_meeting_round_waits_until_review_is_needed(tmp_path: Path) -> None:
    tasks = _Tasks()
    flag = tmp_path / "seat"
    async with _client(tasks) as client:
        group = _data(
            await client.call_tool(
                "run_round",
                {
                    "title": "Review",
                    "cwd": str(tmp_path),
                    "review": True,
                    "members": [{"id": seat, "provider": "claude"} for seat in ("a", "b")],
                },
            )
        )
        round_ = _data(
            await client.call_tool(
                "run_round",
                {
                    "group_id": group["group_id"],
                    "seats": ["a"],
                    "order": "parallel",
                    "prompt": f"WAIT:{flag}",
                },
            )
        )["round"]
        await client.call_tool("wait", {"ids": [round_["round_id"]]})
        created = tasks.created[-1]
        assert created["statusMessage"] == "💬 1 seat chatting in the meeting room…"
        flag.touch()
        done = await _completed(client, created["taskId"])
        assert done["result"]["structuredContent"]["rounds"][0]["reason"] == "review_needed"


@pytest.mark.asyncio
async def test_lease_releases_the_inbox_and_overdue_tasks_end(
    tmp_path: Path, runtimes: list[mcp_runtime.Runtime], monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(mcp_wait_tasks, "LEASE_SECONDS", 0.2)
    monkeypatch.setattr(mcp_wait_tasks, "_TICK_SECONDS", 0.05)
    tasks = _Tasks()
    async with _client(tasks) as client:
        handle = await _start(client, tmp_path, tmp_path / "never")
        await client.call_tool("wait", {})
        task_id = tasks.created[-1]["taskId"]
        runtime = runtimes[0]
        assert runtime.active_inbox_waits
        await asyncio.sleep(0.6)
        # Nobody polled: the inbox is free again and the relay may swap servers.
        assert not runtime.active_inbox_waits
        assert runtime.wait_tasks.restart_safe() is True
        assert (await _raw(client, "tasks/get", {"taskId": task_id}))["status"] == "working"
        assert runtime.active_inbox_waits
        assert runtime.wait_tasks.restart_safe() is False

        monkeypatch.setattr(mcp_wait_tasks, "ORPHAN_SECONDS", 0.0)
        overdue = await _raw(client, "tasks/get", {"taskId": task_id})
        assert overdue["status"] == "completed"
        payload = overdue["result"]["structuredContent"]
        assert payload["reason"] == "wait_elapsed"
        assert payload["next"] == TASK_WAIT_AGAIN
        assert not runtime.active_inbox_waits
        await client.call_tool("cancel", {"id": handle})


def test_status_line_priorities_in_both_languages() -> None:
    working = WaitScan(working=["Codex", "Claude", "GPT"], meeting_seats=2)
    assert status_message(working, "zh") == "⌨️ Codex 等 3 位员工 在忙，另有会议讨论中…"
    assert status_message(WaitScan(working=["Codex"]), "zh") == "⌨️ Codex 正在工位敲键盘…"
    assert status_message(WaitScan(working=["Codex", "Claude"]), "zh") == (
        "⌨️ Codex、Claude 正在工位敲键盘…"
    )
    assert status_message(WaitScan(working=["Codex", "Claude", "GPT"]), "zh") == (
        "⌨️ Codex 等 3 位员工正在赶活…"
    )
    assert status_message(WaitScan(meeting_seats=3), "zh") == "💬 3 位席位正在会议室讨论…"
    stuck = WaitScan(working=["Codex"], stalled=["GPT"])
    assert status_message(stuck, "zh") == "👀 GPT 好像卡壳了，正催你看看…"
    asking = WaitScan(stalled=["GPT"], prompts=["Claude"])
    assert status_message(asking, "zh") == "🙋 Claude 举手等你回话～"
    ready = WaitScan(ready=True, ready_names=["Codex"], prompts=["Claude"])
    assert status_message(ready, "zh") == "✨ Codex 交卷啦，正在递结果…"
    assert status_message(WaitScan(ready=True), "zh") == "✨ 有人交卷啦，正在递结果…"

    assert status_message(WaitScan(working=["Codex", "Claude"]), "en") == (
        "⌨️ Codex & Claude are typing away…"
    )
    assert status_message(WaitScan(working=["Codex", "Claude", "GPT"]), "en") == (
        "⌨️ Codex + 2 more typing away…"
    )
    assert status_message(WaitScan(meeting_seats=3), "en") == (
        "💬 3 seats chatting in the meeting room…"
    )
    assert status_message(WaitScan(stalled=["Codex"]), "en") == (
        "👀 Codex looks stuck — needs a quick look…"
    )
    assert status_message(WaitScan(ready=True, ready_names=["Codex"]), "en") == (
        "✨ Codex turned in the work — handing it over…"
    )


def test_language_follows_the_process_locale(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LC_ALL", "C")
    monkeypatch.setenv("LC_MESSAGES", "zh_CN.UTF-8")
    monkeypatch.setenv("LANG", "en_US.UTF-8")
    assert detect_language() == "zh"
    monkeypatch.delenv("LC_MESSAGES")
    assert detect_language() == "en"
    monkeypatch.setenv("LC_ALL", "zh_TW.UTF-8")
    assert detect_language() == "zh"
