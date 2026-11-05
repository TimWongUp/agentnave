# pyright: reportPrivateUsage=false
from __future__ import annotations

import json
from pathlib import Path
from typing import cast

import pytest
from mcp import Client
from test_conversations import _call, _state, _views, _wait, _wait_task
from test_conversations import data_root as data_root

from agentnave import mcp_runtime
from agentnave.host import Host
from agentnave.mcp_server import mcp


@pytest.mark.asyncio
async def test_codex_missing_chat_id_error_explains_how_to_read_it(
    data_root: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex"))
    monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
    monkeypatch.setenv("CODEX_THREAD_ID", "not-forwarded-to-mcp-in-production")
    async with Client(mcp) as client:
        for tool in ("start_agent", "run_round", "wait"):
            result = await client.call_tool(
                tool,
                {"provider": "claude", "cwd": str(tmp_path), "prompt": "check"}
                if tool == "start_agent"
                else {"title": "check"}
                if tool == "run_round"
                else {},
            )
            assert result.is_error
            error = str(result.content)
            assert "$env:CODEX_THREAD_ID" in error and "printenv CODEX_THREAD_ID" in error
            assert "failed shell command" in error
            assert ("Nothing was received" if tool == "wait" else "Nothing was started") in error
    assert not (data_root / "agentnave.db").exists()


@pytest.mark.asyncio
async def test_anonymous_inbox_rejects_without_receiving_other_chats_results(
    data_root: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex"))
    monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
    async with Client(mcp) as client:
        tasks = [
            await _call(
                client,
                "start_agent",
                {
                    "provider": "claude",
                    "cwd": str(tmp_path),
                    "prompt": "check",
                    "manager_session_id": chat,
                },
            )
            for chat in ("chat-a", "chat-b")
        ]
        for arguments in ({}, {"wait_seconds": 0, "include_output": False}):
            rejected = await client.call_tool("wait", arguments)
            assert rejected.is_error
            assert "needs manager_session_id" in str(rejected.content)
        explicit = await _call(client, "wait", {"manager_session_id": "chat-a"})
        updates = cast(list[dict[str, object]], explicit["updates"])
        assert [item["invocation_id"] for item in updates] == [tasks[0]["invocation_id"]]
        assert explicit["all_finished"] is True
        monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "chat-b")
        injected = await _call(client, "wait", {})
        updates = cast(list[dict[str, object]], injected["updates"])
        assert [item["invocation_id"] for item in updates] == [tasks[1]["invocation_id"]]
        assert injected["all_finished"] is True
        monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
        reread = await _wait_task(client, tasks[0]["invocation_id"])
        assert reread["status"] == "succeeded" and reread["output"]


@pytest.mark.asyncio
@pytest.mark.parametrize("identity", ["native", "invented", "declared"])
async def test_codex_boss_model_reaches_saved_app_views_or_returns_a_diagnostic(
    data_root: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, identity: str
) -> None:
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex"))
    home = tmp_path / "codex"
    monkeypatch.setenv("CODEX_HOME", str(home))
    native = "01a19096-1d71-7771-8056-da1fb28f4e27"
    folder = home / "sessions" / "2026" / "10" / "31"
    folder.mkdir(parents=True)
    (folder / f"rollout-test-{native}.jsonl").write_text(
        json.dumps(
            {
                "type": "turn_context",
                "timestamp": "2026-01-01T00:00:00Z",
                "payload": {"model": "actual-boss-model"},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    arguments: dict[str, object] = {
        "manager_session_id": native
        if identity == "native"
        else "4b526e97-5039-4cff-9910-ef353392c758",
    }
    if identity == "declared":
        arguments["manager_model"] = "trusted-boss-model"
    async with Client(mcp) as client:
        task = await _call(
            client,
            "start_agent",
            {
                "provider": "claude",
                "cwd": str(tmp_path),
                "prompt": "check",
                **arguments,
            },
        )
        await _wait_task(client, task["invocation_id"])
        meeting = await _call(
            client,
            "run_round",
            {
                "title": "Model metadata",
                "cwd": str(tmp_path),
                "prompt": "check",
                "members": [{"id": seat, "provider": "claude"} for seat in ("a", "b")],
                "seats": ["a", "b"],
                **arguments,
            },
        )
        await _wait(client, cast(dict[str, object], meeting["round"])["round_id"])
        remark = await _call(
            client,
            "run_round",
            {
                "group_id": meeting["group_id"],
                "prompt": "host remark",
                **arguments,
            },
        )
    for receipt in (task, meeting, remark):
        if identity == "invented":
            warnings = cast(list[str], receipt["warnings"])
            assert len(warnings) == 1 and "CODEX_THREAD_ID" in warnings[0]
            assert "pass it as manager_session_id" in warnings[0]
            assert len(warnings[0]) < 250
        else:
            assert "warnings" not in receipt
    saved = _state(data_root, task["conversation_id"])["tasks"][0]
    messages = _views(data_root, meeting["group_id"])["director"]["messages"]
    for message in (saved, *(m for m in messages if m["kind"] in {"task", "host"})):
        if identity == "native":
            assert message["manager_reported_model"] == "actual-boss-model"
        elif identity == "declared":
            assert message["manager_model"] == "trusted-boss-model"
            assert message.get("manager_reported_model") is None
        else:
            assert message.get("manager_reported_model") is None
