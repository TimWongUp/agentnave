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


def test_inbox_tracker_preserves_mid_wait_events_and_delivered_reviews() -> None:
    from agentnave.inbox_events import BossInboxState, InboxTracker, review_event_key
    from agentnave.inbox_snapshot import InboxItem

    tracker = InboxTracker()
    boss = "s-boss"
    t1: InboxItem = {"key": "task_end:t1", "kind": "task_end", "role": None, "status": "succeeded"}
    t2: InboxItem = {"key": "task_end:t2", "kind": "task_end", "role": None, "status": "succeeded"}

    # Task 1 finishes first.
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[t1])})
    assert snap[boss]["event_seq"] == 1 and snap[boss]["delivered_seq"] == 0

    # While wait delivers t1, t2 finishes before wait marks delivery.
    snap = tracker.mark_delivered(
        {boss: BossInboxState(waiting=False, in_flight=0, items=[t2])},
        {boss},
    )
    assert snap[boss]["event_seq"] == 2
    assert snap[boss]["delivered_seq"] == 1
    assert snap[boss]["event_seq"] > snap[boss]["delivered_seq"]

    # Delivering t2 catches delivered_seq up and drops the idle entry.
    snap = tracker.mark_delivered(
        {boss: BossInboxState(waiting=False, in_flight=0, items=[])}, {boss}
    )
    assert boss not in snap

    # Review reported by wait advances delivered_seq even while pending; deciding clears items cleanly.
    rev_key = review_event_key("r1", "a", 1)
    rev: InboxItem = {"key": rev_key, "kind": "review", "role": None, "status": "review_needed"}
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[rev])})
    assert snap[boss]["event_seq"] == 3 and snap[boss]["delivered_seq"] == 2
    snap = tracker.mark_delivered(
        {boss: BossInboxState(waiting=False, in_flight=1, items=[rev])},
        {boss},
        {rev_key},
    )
    assert snap[boss]["event_seq"] == 3 and snap[boss]["delivered_seq"] == 3
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[])})
    assert snap[boss]["event_seq"] == 3 and snap[boss]["delivered_seq"] == 3

    # A prompt that closes natively before any wait must not leave a phantom wake.
    prompt: InboxItem = {"key": "prompt:t3:p1", "kind": "prompt", "role": None, "status": "x"}
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[prompt])})
    assert snap[boss]["event_seq"] == 4 and snap[boss]["delivered_seq"] == 3
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[])})
    assert snap[boss]["event_seq"] == 4 and snap[boss]["delivered_seq"] == 4

    # A delivered review still awaiting decision is flagged so the tray does not count it.
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[rev])})
    snap = tracker.mark_delivered(
        {boss: BossInboxState(waiting=False, in_flight=1, items=[rev])}, {boss}, {rev_key}
    )
    t6: InboxItem = {"key": "task_end:t6", "kind": "task_end", "role": None, "status": "succeeded"}
    snap = tracker.project({boss: BossInboxState(waiting=False, in_flight=1, items=[rev, t6])})
    assert [item.get("delivered", False) for item in snap[boss]["items"]] == [True, False]


@pytest.mark.asyncio
async def test_dispatch_and_wait_flush_inbox_snapshot_synchronously(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.native_sessions import boss_key

    data_dir = tmp_path / "data"
    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(data_dir))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed)
    gate_one = tmp_path / "gate1"
    gate_two = tmp_path / "gate2"
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, pathlib, sys, time\n"
        "prompt = sys.stdin.read()\n"
        f"gate = pathlib.Path({str(gate_one)!r}) if 'ONE' in prompt else pathlib.Path({str(gate_two)!r})\n"
        "while not gate.exists():\n"
        "    time.sleep(0.01)\n"
        "print(json.dumps({'type': 'result', 'result': 'ok', 'session_id': 's1', 'num_turns': 1}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    chat = "chat-sync-flush"
    key = boss_key(None, chat)
    async with Client(mcp) as client:
        first = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "ONE",
                "cwd": str(tmp_path),
                "manager_session_id": chat,
            },
        )
        assert not first.is_error, first.content
        # Snapshot is flushed synchronously before start_agent returns (no sleep needed).
        doc = _read_inbox(data_dir)
        assert isinstance(doc.get("started_at"), float) and cast(float, doc["started_at"]) > 0
        entry = _bosses(data_dir)[key]
        assert entry["in_flight"] == 1
        assert entry["event_seq"] == 0 and entry["delivered_seq"] == 0

        second = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "TWO",
                "cwd": str(tmp_path),
                "manager_session_id": chat,
            },
        )
        assert not second.is_error, second.content
        assert _bosses(data_dir)[key]["in_flight"] == 2

        # Release ONE and wait for it; TWO remains in_flight.
        gate_one.write_text("go", encoding="utf-8")
        waited = await client.call_tool("wait", {"manager_session_id": chat})
        assert not waited.is_error, waited.content
        after_one = _bosses(data_dir)[key]
        assert after_one["in_flight"] == 1
        assert after_one["event_seq"] == after_one["delivered_seq"]

        # Release TWO and wait; snapshot is synchronously removed.
        gate_two.write_text("go", encoding="utf-8")
        waited_two = await client.call_tool("wait", {"manager_session_id": chat})
        assert not waited_two.is_error, waited_two.content
        assert _read_inbox(data_dir) == {}
