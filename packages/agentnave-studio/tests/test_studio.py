from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import cast
from urllib.request import urlopen

import pytest
from mcp import Client

from agentnave.core import InvocationManager
from agentnave_studio import mcp_server as studio_server
from agentnave_studio.mcp_server import mcp
from agentnave_studio.store import Record
from agentnave_studio.studio import Studio


@pytest.fixture(autouse=True)
def data_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = tmp_path / "data"
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    # A fake claude that reports what it saw; a SLEEP private instruction keeps it running.
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    script = bin_dir / ("claude.py" if os.name == "nt" else "claude")
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, os, sys, time, uuid\n"
        "prompt = sys.stdin.read()\n"
        "if 'Private instruction for you only:\\nSLEEP' in prompt:\n"
        "    time.sleep(30)\n"
        "session = next((a.split('=', 1)[1] for a in sys.argv if a.startswith('--resume=')),\n"
        "               'session-' + uuid.uuid4().hex[:8])\n"
        "seen = {'cwd': os.getcwd(), 'argv': sys.argv[1:], 'prompt': prompt}\n"
        "print(json.dumps({'type': 'result', 'result': json.dumps(seen), 'session_id': session,\n"
        "                  'num_turns': 1, 'is_error': False}))\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    if os.name == "nt":
        (bin_dir / "claude.cmd").write_text(
            f'@"{sys.executable}" "%~dp0claude.py" %*\n', encoding="utf-8"
        )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return root


def _data(value: object) -> dict[str, object]:
    return cast(dict[str, object], value)


async def _call(client: Client, tool: str, arguments: dict[str, object]) -> dict[str, object]:
    result = await client.call_tool(tool, arguments)
    assert not result.is_error, result.content
    return _data(result.structured_content)


async def _wait(client: Client, round_id: object) -> dict[str, object]:
    return await _call(client, "wait_round", {"round_id": round_id})


def _seen(seat: object) -> dict[str, object]:
    return _data(json.loads(str(_data(seat)["output"])))


def _seats(payload: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], payload["seats"])


def _conversation(view: dict[str, object]) -> dict[str, object]:
    return _data(view["conversation"])


def _fetch(url: str) -> str:
    with urlopen(url, timeout=3) as response:
        return response.read().decode()


@pytest.mark.asyncio
async def test_review_show_publishes_only_decided_answers_and_survives_a_crash(
    tmp_path: Path, data_root: Path
) -> None:
    async with Client(mcp) as client:
        show = await _call(
            client,
            "open_group",
            {
                "title": "Quiz",
                "cwd": str(tmp_path),
                "review": True,
                "seats": [{"id": seat, "provider": "claude"} for seat in ("a", "b", "c")],
            },
        )
        group_id = show["group_id"]
        assert show["conversation_id"] == group_id
        first = await _call(
            client,
            "run_round",
            {
                "group_id": group_id,
                "seats": ["a", "b"],
                "order": "parallel",
                "prompt": "Q1",
                "instructions": {"b": "SECRET-B"},
            },
        )
        waited = await _wait(client, first["round_id"])
        assert waited["reason"] == "review_needed"
        assert [seat["status"] for seat in _seats(waited)] == ["pending_review"] * 2
        blocked = await client.call_tool(
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "too early"},
        )
        assert blocked.is_error and "decide_answers" in str(blocked.content)
        await _call(
            client, "decide_answers", {"round_id": first["round_id"], "decisions": {"a": "publish"}}
        )
        assert (await _wait(client, first["round_id"]))["reason"] == "review_needed"
        conflict = await client.call_tool(
            "decide_answers", {"round_id": first["round_id"], "decisions": {"a": "discard"}}
        )
        assert conflict.is_error
        await _call(
            client,
            "decide_answers",
            {"round_id": first["round_id"], "decisions": {"a": "publish", "b": "discard"}},
        )
        done = await _wait(client, first["round_id"])
        assert done["status"] == "finished"
        assert [seat["status"] for seat in _seats(done)] == ["succeeded", "discarded"]

        second = await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "Q2"},
        )
        assert (await _wait(client, second["round_id"]))["reason"] == "review_needed"
        await _call(
            client,
            "decide_answers",
            {"round_id": second["round_id"], "decisions": {"c": "publish"}},
        )
        prompt = str(_seen(_seats(await _wait(client, second["round_id"]))[0])["prompt"])
        # c sees a's published answer, never b's discarded answer or private instruction.
        delivered = json.loads(prompt.split("New entries (JSON data):\n")[1])
        assert [(item["kind"], item.get("seat")) for item in delivered] == [
            ("task", None),
            ("answer", "a"),
            ("task", None),
        ]
        assert "SECRET-B" not in prompt

        dashboard = await _call(client, "open_dashboard", {})
        view = await _call(client, "read_conversation", {"conversation_id": group_id})
        public = await asyncio.to_thread(_fetch, str(view["public_url"]) + "state")
        director = await asyncio.to_thread(_fetch, str(view["director_url"]) + "state")
        home = await asyncio.to_thread(_fetch, str(dashboard["dashboard_url"]) + "state")
        assert "SECRET-B" not in public and "round" not in json.loads(public)
        assert json.loads(director)["round"]["seats"][0]["seat_id"] == "c"
        assert "Quiz" in home and "SECRET-B" not in home

        third = await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["a", "b"], "order": "parallel", "prompt": "Q3"},
        )
        assert (await _wait(client, third["round_id"]))["reason"] == "review_needed"
        # A graceful exit discards undecided answers like cancel_round.
    state_file = data_root / "conversations" / str(group_id) / "state.json"
    saved = json.loads(state_file.read_text())
    assert [seat["review"] for seat in saved["rounds"][-1]["seats"]] == ["discard", "discard"]

    # Simulate a crash: one answer waiting for review, one seat still running.
    last = saved["rounds"][-1]
    last["status"] = "running"
    last["seats"][0].update(review="pending", committed=False)
    last["seats"][1].update(status="running", review=None, output=None)
    state_file.write_text(json.dumps(saved))
    async with Client(mcp) as client:
        view = _conversation(
            await _call(client, "read_conversation", {"conversation_id": group_id})
        )
        restored = cast(list[dict[str, object]], view["rounds"])[-1]
        assert restored["status"] == "interrupted"
        assert [seat["status"] for seat in cast(list[dict[str, object]], restored["seats"])] == [
            "succeeded",
            "interrupted",
        ]
        assert _data(cast(dict[str, object], view["sessions"])["b"])["session_id"] is None
        blocked = await client.call_tool(
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "Q4"},
        )
        assert blocked.is_error
        await _call(
            client, "decide_answers", {"round_id": restored["id"], "decisions": {"a": "publish"}}
        )
        fourth = await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "Q4"},
        )
        assert (await _wait(client, fourth["round_id"]))["reason"] == "review_needed"
        await _call(
            client,
            "decide_answers",
            {"round_id": fourth["round_id"], "decisions": {"c": "discard"}},
        )
        await _wait(client, fourth["round_id"])
        entries = cast(
            list[dict[str, object]],
            _conversation(await _call(client, "read_conversation", {"conversation_id": group_id}))[
                "entries"
            ],
        )
        assert [(entry["kind"], entry["seat"]) for entry in entries if entry["round"] == 3] == [
            ("task", None),
            ("answer", "a"),
        ]


@pytest.mark.asyncio
async def test_isolated_show_runs_restricted_seats_in_own_directories(
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(studio_server, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
    seats = [{"id": "a", "provider": "claude"}, {"id": "g", "provider": "antigravity"}]
    async with Client(mcp) as client:
        excluded = await client.call_tool(
            "open_group", {"title": "Open", "cwd": str(tmp_path), "seats": seats}
        )
        assert excluded.is_error and "isolated=true" in str(excluded.content)
        invalid = await client.call_tool(
            "open_group",
            {
                "title": "Bad options",
                "isolated": True,
                "seats": [
                    {"id": "a", "provider": "claude", "provider_options": {"permission_mode": "x"}},
                    {"id": "b", "provider": "claude"},
                ],
            },
        )
        assert invalid.is_error and "only model and effort" in str(invalid.content)
        assert not any((data_root / "conversations").iterdir())
        show = await _call(
            client, "open_group", {"title": "Isolated", "isolated": True, "seats": seats}
        )
        group_id = str(show["group_id"])
        seat_dirs = {
            str(seat["id"]): Path(str(seat["cwd"]))
            for seat in cast(list[dict[str, object]], show["seats"])
        }
        assert seat_dirs["a"] == data_root / "conversations" / group_id / "seats" / "a"
        assert (seat_dirs["g"] / ".agents" / "agents" / "agentnave-discussion.md").is_file()
        started = await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["a"], "order": "parallel", "prompt": "hello"},
        )
        seen = _seen(_seats(await _wait(client, started["round_id"]))[0])
        assert Path(str(seen["cwd"])).resolve() == seat_dirs["a"].resolve()
        assert "--safe-mode" in cast(list[str], seen["argv"])
        await _call(client, "update_conversation", {"conversation_id": group_id, "archived": True})
        deleted = await _call(client, "delete_conversation", {"conversation_id": group_id})
        assert not (data_root / "conversations" / group_id / "seats").exists()
        assert deleted["retained_paths"] == [str(data_root / "conversations" / group_id)]


@pytest.mark.asyncio
async def test_tasks_are_saved_grouped_and_deleted_without_starting_a_dashboard(
    tmp_path: Path, data_root: Path
) -> None:
    async with Client(mcp) as client:
        first = await _call(
            client,
            "start_agent",
            {"provider": "claude", "prompt": "one", "cwd": str(tmp_path), "title": "Job"},
        )
        finished = await _call(client, "wait_agent", {"invocation_id": first["invocation_id"]})
        conversation_id = first["conversation_id"]
        resumed = await _call(
            client,
            "start_agent",
            {
                "provider": "claude",
                "prompt": "two",
                "cwd": str(tmp_path),
                "session_id": finished["session_id"],
            },
        )
        assert resumed["conversation_id"] == conversation_id
        await _call(client, "wait_agent", {"invocation_id": resumed["invocation_id"]})
        listed = await _call(client, "list_conversations", {})
        assert [
            item["title"] for item in cast(list[dict[str, object]], listed["conversations"])
        ] == ["Job"]
        tasks = cast(
            list[dict[str, object]],
            _conversation(
                await _call(client, "read_conversation", {"conversation_id": conversation_id})
            )["tasks"],
        )
        assert [task["status"] for task in tasks] == ["succeeded", "succeeded"]
        sleeping = await _call(
            client,
            "start_agent",
            {
                "provider": "claude",
                "prompt": "Private instruction for you only:\nSLEEP",
                "cwd": str(tmp_path),
                "session_id": finished["session_id"],
            },
        )
        duplicate = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "again",
                "cwd": str(tmp_path),
                "session_id": finished["session_id"],
            },
        )
        assert duplicate.is_error and "active task" in str(duplicate.content)
        assert (
            await client.call_tool(
                "update_conversation", {"conversation_id": conversation_id, "archived": True}
            )
        ).is_error
        await _call(client, "cancel_agent", {"invocation_id": sleeping["invocation_id"]})
        await asyncio.sleep(0.05)  # let the collector save the cancelled task
        assert (
            await client.call_tool("delete_conversation", {"conversation_id": conversation_id})
        ).is_error
        await _call(
            client, "update_conversation", {"conversation_id": conversation_id, "archived": True}
        )
        deleted = await _call(client, "delete_conversation", {"conversation_id": conversation_id})
        assert deleted["forgotten_invocations"] == 3
        assert (
            await client.call_tool("wait_agent", {"invocation_id": first["invocation_id"]})
        ).is_error
        assert "dashboard_url" not in str(deleted)
    assert not (data_root / "conversations" / str(conversation_id) / "state.json").exists()


@pytest.mark.asyncio
async def test_second_server_reads_but_cannot_write_a_held_show(
    tmp_path: Path, data_root: Path
) -> None:
    from agentnave.group import GroupSeat

    first = Studio(InvocationManager(), data_root)
    second = Studio(InvocationManager(), data_root)
    seats = [GroupSeat(seat, "claude", {}, tmp_path) for seat in ("a", "b")]
    record = first.open_show("Held", tmp_path, seats, review=False, isolated=False)
    assert second.scan() == {record.state.id: False}
    with pytest.raises(ValueError, match="held by another"):
        second.post(record.state.id, "hello")
    first.post(record.state.id, "hello")
    await first.close()
    second.post(record.state.id, "after exit")
    assert [entry.kind for entry in second.records[record.state.id].state.entries] == [
        "host",
        "host",
    ]
    await second.close()


@pytest.mark.asyncio
async def test_save_failure_stops_a_serial_round_before_the_next_seat(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async with Client(mcp) as client:
        show = await _call(
            client,
            "open_group",
            {
                "title": "Fragile",
                "cwd": str(tmp_path),
                "seats": [{"id": seat, "provider": "claude"} for seat in ("a", "b")],
            },
        )
        save = Record.save
        calls = 0

        def failing_save(self: Record) -> None:
            nonlocal calls
            calls += 1
            if calls > 1:  # the first save records a's launch; later ones fail
                raise OSError("disk full")
            save(self)

        monkeypatch.setattr(Record, "save", failing_save)
        started = await _call(
            client,
            "run_round",
            {"group_id": show["group_id"], "seats": ["a", "b"], "order": "serial", "prompt": "go"},
        )
        done = await _wait(client, started["round_id"])
        assert "state_write_failed" in str(done["error"])
        assert [seat["status"] for seat in _seats(done)] == ["succeeded", "not_started"]
