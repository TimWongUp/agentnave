from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest
from fake_cli import write_fake_cli
from mcp import Client

from agentnave import mcp_runtime
from agentnave.adapters.base import AdapterHooks
from agentnave.conversations import Conversations
from agentnave.group import Group, GroupSeat, Round, SeatRun
from agentnave.host import Host
from agentnave.inbox_events import _round_events  # pyright: ignore[reportPrivateUsage]
from agentnave.mcp_server import mcp
from agentnave.models import InvocationError
from agentnave.native_prompts import NativePrompt


def test_round_attention_keys_and_consumption(tmp_path: Path) -> None:
    seats = {name: GroupSeat(name, "claude", {}, tmp_path, name) for name in ("a", "b")}
    group = Group("group", tmp_path, seats)
    lost = SeatRun("a", None, status="failed", attempt=1)
    running = SeatRun("b", None, status="running", attempt=1)
    running.prompts.append(NativePrompt("prompt-one", "permission", "Bash"))
    running.blockers.append(InvocationError("provider_failed", "message"))
    round_ = Round("round", group, 1, "parallel", [lost, running])
    conversations = cast(Conversations, SimpleNamespace(reviews={"round": {"a": "pending"}}))

    items = _round_events(conversations, round_)  # pyright: ignore[reportPrivateUsage]
    assert {item["kind"] for item in items} == {"review", "prompt", "blocked", "seat_lost"}
    assert items == _round_events(conversations, round_)  # pyright: ignore[reportPrivateUsage]
    previous_lost = next(item["key"] for item in items if item["kind"] == "seat_lost")
    running.reported_prompts.add("prompt-one")
    running.reported_blockers = 1
    lost.reported_lost = True
    assert [item["kind"] for item in _round_events(conversations, round_)] == ["review"]  # pyright: ignore[reportPrivateUsage]
    lost.reported_lost = False
    lost.attempt = 2
    assert (
        next(
            item["key"]
            for item in _round_events(conversations, round_)
            if item["kind"] == "seat_lost"
        )
        != previous_lost
    )  # pyright: ignore[reportPrivateUsage]


def test_prompt_keys_distinguish_seats_and_retries(tmp_path: Path) -> None:
    seats = {name: GroupSeat(name, "claude", {}, tmp_path) for name in ("a", "b")}
    runs = [SeatRun(name, None, status="running", attempt=1) for name in seats]
    for run in runs:
        run.prompts.append(NativePrompt("same", "permission", "Bash"))
    round_ = Round("round", Group("group", tmp_path, seats), 1, "parallel", runs)
    conversations = cast(Conversations, SimpleNamespace(reviews={}))
    keys = [item["key"] for item in _round_events(conversations, round_)]  # pyright: ignore[reportPrivateUsage]
    assert len(set(keys)) == 2
    runs[0].attempt += 1
    updated = [item["key"] for item in _round_events(conversations, round_)]  # pyright: ignore[reportPrivateUsage]
    assert updated[0] != keys[0] and updated[1] == keys[1]


def _installed(_adapter: AdapterHooks) -> bool:
    return True


def _read_inbox(data_dir: Path) -> dict[str, object]:
    path = data_dir / "inbox" / f"{os.getpid()}.json"
    if not path.exists():
        return {}
    return cast(dict[str, object], json.loads(path.read_text(encoding="utf-8")))


def _bosses(data_dir: Path) -> dict[str, dict[str, object]]:
    return cast(dict[str, dict[str, object]], _read_inbox(data_dir).get("bosses", {}))


@pytest.mark.asyncio
async def test_decide_answers_flushes_review_from_inbox_synchronously(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    data_dir = tmp_path / "data"
    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(data_dir))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, sys\n"
        "sys.stdin.read()\n"
        "print(json.dumps({'type': 'result', 'result': 'ok', 'session_id': 's1', 'num_turns': 1}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    chat = "chat-decide-inbox"
    async with Client(mcp) as client:
        started = await client.call_tool(
            "run_round",
            {
                "title": "review round",
                "cwd": str(tmp_path),
                "review": True,
                "members": [
                    {"id": "a", "provider": "claude"},
                    {"id": "b", "provider": "claude"},
                ],
                "seats": ["a"],
                "prompt": "hello",
                "manager_session_id": chat,
            },
        )
        assert not started.is_error, started.content
        round_info = cast(
            dict[str, object], cast(dict[str, object], started.structured_content)["round"]
        )
        round_id = str(round_info["round_id"])

        waited = await client.call_tool("wait", {"manager_session_id": chat})
        assert not waited.is_error, waited.content
        assert cast(dict[str, object], waited.structured_content)["reason"] == "ready"

        decided = await client.call_tool(
            "decide_answers", {"round_id": round_id, "decisions": {"a": "publish"}}
        )
        assert not decided.is_error, decided.content
        # Synchronous refresh_inbox replaced the pending review item before returning.
        items = [
            item
            for entry in _bosses(data_dir).values()
            for item in cast(list[dict[str, object]], entry.get("items", []))
        ]
        assert all(item["kind"] != "review" for item in items)
        assert any(item["kind"] == "task_end" for item in items)


@pytest.mark.asyncio
async def test_explicit_ids_blocking_wait_marks_workspace_waiting(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    data_dir = tmp_path / "data"
    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(data_dir))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed)
    gate_file = tmp_path / "release"
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, pathlib, sys, time\n"
        "sys.stdin.read()\n"
        f"gate = pathlib.Path({str(gate_file)!r})\n"
        "while not gate.exists():\n"
        "    time.sleep(0.02)\n"
        "print(json.dumps({'type': 'result', 'result': 'done', 'session_id': 's1', 'num_turns': 1}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    chat = "chat-explicit-wait"
    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "slow",
                "cwd": str(tmp_path),
                "manager_session_id": chat,
            },
        )
        assert not started.is_error, started.content
        handle = str(cast(dict[str, object], started.structured_content)["invocation_id"])

        wait_task = asyncio.create_task(client.call_tool("wait", {"ids": [handle]}))
        saw_waiting = False
        for _ in range(50):
            bosses = _bosses(data_dir)
            if any(entry.get("waiting") is True for entry in bosses.values()):
                saw_waiting = True
                break
            await asyncio.sleep(0.02)
        gate_file.write_text("go", encoding="utf-8")
        reply = await wait_task
        assert not reply.is_error, reply.content
        assert saw_waiting
        assert _bosses(data_dir) == {}
