"""wait 收口的 follow_up：早先已交付的坏结局与仍保留的隔离目录，每项只报一次。"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from fake_cli import write_fake_cli
from mcp import Client
from mcp_types import CallToolResult

from agentnave import mcp_runtime
from agentnave.adapters.base import AdapterHooks
from agentnave.host import Host
from agentnave.mcp_server import mcp

pytestmark = pytest.mark.asyncio


def _installed(_adapter: AdapterHooks) -> bool:
    return True


@pytest.fixture(autouse=True)
def fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, sys\n"
        "prompt = sys.stdin.read().split('\\n\\nAssigned task:\\n', 1)[1]\n"
        "print(json.dumps({'type': 'result', 'result': 'boom' if prompt == 'FAIL' else 'ok', "
        "'session_id': 'sess-f' if prompt in ('FAIL', 'RESUME') else 'sess-' + prompt, "
        "'num_turns': 1, "
        "'is_error': prompt == 'FAIL'}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return bin_dir


def _data(result: CallToolResult) -> dict[str, object]:
    return cast(dict[str, object], result.structured_content)


async def _run(client: Client, prompt: str, cwd: Path, chat: str, **extra: object) -> str:
    reply = await client.call_tool(
        "start_agent",
        {"provider": "claude", "prompt": prompt, "cwd": str(cwd), "manager_session_id": chat}
        | extra,
    )
    assert not reply.is_error, reply.content
    return str(_data(reply)["invocation_id"])


async def _inbox(client: Client, chat: str) -> dict[str, object]:
    """Wait on the inbox until it closes; a failing fake CLI reports a blocker first."""
    for _ in range(5):
        reply = await client.call_tool("wait", {"manager_session_id": chat})
        assert not reply.is_error, reply.content
        if _data(reply)["all_finished"]:
            return _data(reply)
    raise AssertionError("the inbox never closed")


async def _ids(client: Client, *handles: str) -> dict[str, object]:
    for _ in range(5):
        reply = _data(await client.call_tool("wait", {"ids": list(handles)}))
        if reply["all_finished"]:
            return reply
    raise AssertionError("the ids never finished")


def _items(reply: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], reply.get("follow_up", []))


async def test_a_failure_delivered_earlier_is_listed_once_when_the_inbox_closes(
    tmp_path: Path,
) -> None:
    async with Client(mcp) as client:
        failed = await _run(client, "FAIL", tmp_path, "chat-f")
        early = await _ids(client, failed)
        assert early["all_finished"] is True and "follow_up" not in early  # ids only record

        await _run(client, "fine", tmp_path, "chat-f")
        closing = await _inbox(client, "chat-f")
        assert closing["all_finished"] is True
        [item] = _items(closing)
        assert item["id"] == failed and item["kind"] == "task" and item["status"] == "failed"
        assert item["session_id"] == "sess-f" and "message_id" in item

        assert "follow_up" not in await _inbox(client, "chat-f")  # nothing_pending, once only


async def test_a_failure_in_the_closing_reply_is_not_repeated_and_a_resumed_one_is_dropped(
    tmp_path: Path,
) -> None:
    async with Client(mcp) as client:
        failed = await _run(client, "FAIL", tmp_path, "chat-r")
        same = await _inbox(client, "chat-r")
        assert same["all_finished"] is True and "follow_up" not in same

        await _run(client, "RESUME", tmp_path, "chat-r", session_id="sess-f")
        resumed = await _inbox(client, "chat-r")
        assert resumed["all_finished"] is True and "follow_up" not in resumed
        assert failed not in str(resumed.get("follow_up", ""))


async def test_a_retained_isolated_directory_is_listed_until_released(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    git = [
        "git",
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@example.com",
        "-c",
        "commit.gpgsign=false",
    ]
    subprocess.run([*git, "init", "-b", "main"], cwd=repo, check=True, capture_output=True)
    (repo / "a.txt").write_text("a\n", encoding="utf-8")
    subprocess.run([*git, "add", "."], cwd=repo, check=True, capture_output=True)
    subprocess.run([*git, "commit", "-m", "init"], cwd=repo, check=True, capture_output=True)
    async with Client(mcp) as client:
        kept = await _run(client, "work-a", repo, "chat-i", isolation={})
        released = await _run(client, "work-b", repo, "chat-i", isolation={})
        await _ids(client, kept)
        await _ids(client, released)
        dirs = _data(
            await client.call_tool(
                "isolated_dirs", {"action": "list", "manager_session_id": "chat-i"}
            )
        )
        ids = [str(item["id"]) for item in cast(list[dict[str, object]], dirs["items"])]
        assert len(ids) == 2
        release = await client.call_tool(
            "isolated_dirs", {"action": "release", "id": ids[0], "manager_session_id": "chat-i"}
        )
        assert not release.is_error, release.content

        await _run(client, "fine", tmp_path, "chat-i")
        [item] = _items(await _inbox(client, "chat-i"))
        assert item["kind"] == "isolation" and item["id"] == ids[1]
        assert "path" in item and "branch" in item
        assert "follow_up" not in await _inbox(client, "chat-i")


async def test_the_ledger_stays_with_the_workspace_after_a_handover(tmp_path: Path) -> None:
    async with Client(mcp) as client:
        failed = await _run(client, "FAIL", tmp_path, "chat-1")
        await _ids(client, failed)
        released = _data(
            await client.call_tool(
                "handover", {"release": True, "note": "continue", "manager_session_id": "chat-1"}
            )
        )
        claimed = await client.call_tool(
            "handover",
            {"workspace_id": released["workspace_id"], "manager_session_id": "chat-2"},
        )
        assert not claimed.is_error, claimed.content
        # The taking boss does not run the old workspace (ADR-0099), so it is not told about it.
        assert "follow_up" not in await _inbox(client, "chat-2")
        await _run(client, "fine", tmp_path, "chat-1")
        [item] = _items(await _inbox(client, "chat-1"))
        assert item["id"] == failed
