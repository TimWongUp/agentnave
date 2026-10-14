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
from agentnave.group import GroupSeat
from agentnave.models import InvocationRequest
from agentnave_studio import mcp_server as studio_server
from agentnave_studio import views
from agentnave_studio.dashboard import Dashboard
from agentnave_studio.mcp_server import mcp
from agentnave_studio.store import (
    Conversation,
    EntryRecord,
    Record,
    RoundRecord,
    SeatRecord,
    SeatRunRecord,
)
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
        assert str(show["dashboard_url"]).startswith("http://127.0.0.1:")
        assert show["public_url"] != show["director_url"]
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
        compact = await _call(
            client, "wait_round", {"round_id": first["round_id"], "include_output": False}
        )
        assert compact["reason"] == "review_needed"
        assert all("output" not in seat for seat in _seats(compact))
        reread = await _wait(client, first["round_id"])
        assert [seat["output"] for seat in _seats(reread)] == [
            seat["output"] for seat in _seats(waited)
        ]
        blocked = await client.call_tool(
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "too early"},
        )
        assert blocked.is_error and "decide_answers" in str(blocked.content)
        receipt = await _call(
            client, "decide_answers", {"round_id": first["round_id"], "decisions": {"a": "publish"}}
        )
        assert receipt["decisions"] == {"a": "publish"}
        assert receipt["pending_review"] == ["b"]
        assert receipt["committed"] == []
        assert "output" not in str(receipt) and "SECRET-B" not in str(receipt)
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

        dashboard = await _call(client, "list_conversations", {})
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
            ("absent", "b"),  # still running at the crash
        ]


@pytest.mark.asyncio
async def test_isolated_show_runs_restricted_seats_in_own_directories(
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(studio_server, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
    seats = [{"id": "a", "provider": "claude"}, {"id": "g", "provider": "antigravity"}]
    async with Client(mcp) as client:
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
        # Host exclusions limit start_agent only; ordinary shows may seat the host CLI too.
        ordinary = await _call(
            client, "open_group", {"title": "Open", "cwd": str(tmp_path), "seats": seats}
        )
        started = await _call(
            client,
            "run_round",
            {"group_id": ordinary["group_id"], "seats": ["a"], "order": "parallel", "prompt": "hi"},
        )
        assert _seats(await _wait(client, started["round_id"]))[0]["status"] == "succeeded"


@pytest.mark.asyncio
async def test_tasks_are_saved_grouped_and_deleted_with_automatic_dashboard_links(
    tmp_path: Path, data_root: Path
) -> None:
    async with Client(mcp) as client:
        first = await _call(
            client,
            "start_agent",
            {
                "provider": "claude",
                "prompt": "one",
                "cwd": str(tmp_path),
                "title": "Job",
                "provider_options": {"model": "test-model", "effort": "high"},
            },
        )
        assert first["provider"] == "claude"
        assert "director_url" in first and "public_url" not in first
        assert first["model"] == "test-model"
        assert first["effort"] == "high"
        assert "prompt" not in first
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
        assert duplicate.is_error and "active invocation" in str(duplicate.content)
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
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
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
        replace = os.replace
        calls = 0

        def failing_replace(source: str, target: Path) -> None:
            nonlocal calls
            calls += 1
            if calls > 1:  # the first write records a's launch; later ones fail
                raise OSError("disk full")
            replace(source, target)

        monkeypatch.setattr(os, "replace", failing_replace)
        started = await _call(
            client,
            "run_round",
            {"group_id": show["group_id"], "seats": ["a", "b"], "order": "serial", "prompt": "go"},
        )
        done = await _wait(client, started["round_id"])
        assert "state_write_failed" in str(done["error"])
        assert [seat["status"] for seat in _seats(done)] == ["succeeded", "not_started"]
        monkeypatch.setattr(os, "replace", replace)
        # The next successful write restores the whole round, not only the new entry.
        await _call(
            client, "post_host_message", {"conversation_id": show["group_id"], "text": "ok"}
        )
    saved = json.loads(
        (data_root / "conversations" / str(show["group_id"]) / "state.json").read_text()
    )
    assert [seat["status"] for seat in saved["rounds"][0]["seats"]] == ["succeeded", "not_started"]
    assert [entry["kind"] for entry in saved["entries"]] == ["task", "answer", "host"]
    assert saved["sessions"]["a"]["session_id"] is not None


@pytest.mark.parametrize("review", [False, True])
def test_failed_review_round_does_not_resurrect_unapproved_answers(review: bool) -> None:
    state = Conversation(id="c", title="t", kind="show", review=review)
    state.rounds.append(
        RoundRecord(
            id="r",
            number=1,
            order="parallel",
            status="failed",
            seats=[SeatRunRecord(seat_id="a", status="succeeded", output="unapproved")],
        )
    )
    state.recover()
    assert not state.pending_review()
    assert [(entry.kind, entry.text) for entry in state.entries] == (
        [] if review else [("answer", "unapproved")]
    )


@pytest.mark.parametrize("review", [False, True])
def test_crash_recovery_keeps_answers_that_ended_before_the_crash(review: bool) -> None:
    state = Conversation(id="c", title="t", kind="show", review=review)
    state.rounds.append(
        RoundRecord(
            id="r",
            number=1,
            order="parallel",
            seats=[
                SeatRunRecord(seat_id="a", status="succeeded", output="done"),
                SeatRunRecord(seat_id="b", status="running"),
            ],
        )
    )
    state.recover()
    kinds = [(entry.kind, entry.seat, entry.text) for entry in state.entries]
    if review:
        # The answer had not reached review yet: it waits for a decision instead of vanishing.
        assert state.pending_review() and kinds == []
        state.rounds[0].seats[0].review = "publish"
        state.commit_ended(state.rounds[0])
        kinds = [(entry.kind, entry.seat, entry.text) for entry in state.entries]
    assert kinds == [("answer", "a", "done"), ("absent", "b", "interrupted")]
    state.recover()  # idempotent
    assert len(state.entries) == 2


@pytest.mark.asyncio
async def test_cancel_rejects_a_publish_that_arrives_after_it(
    tmp_path: Path, data_root: Path
) -> None:
    from agentnave.group import GroupSeat

    studio = Studio(InvocationManager(), data_root)
    seats = [GroupSeat(seat, "claude", {}, tmp_path) for seat in ("a", "b")]
    record = studio.open_show("Late", tmp_path, seats, review=True, isolated=False)
    round_ = studio.run(record.state.id, ["a"], "parallel", "go", {})
    assert (await studio.wait(round_.id, 30))[1] == "review_needed"
    cancel = asyncio.create_task(studio.cancel(round_.id))
    await asyncio.sleep(0)  # cancel has flagged the round; the review barrier has not resumed
    with pytest.raises(ValueError, match="being cancelled"):
        studio.decide(round_.id, {"a": "publish"})
    await cancel
    assert [entry.kind for entry in record.state.entries] == ["task"]
    await studio.manager.shutdown()
    await studio.close()


@pytest.mark.skipif(os.name == "nt", reason="symlinks need extra privileges on Windows")
@pytest.mark.asyncio
async def test_delete_never_follows_symlinks_out_of_seat_folders(
    tmp_path: Path, data_root: Path
) -> None:
    from agentnave.group import GroupSeat

    studio = Studio(InvocationManager(), data_root)
    seats = [GroupSeat("a", "claude", {}, tmp_path), GroupSeat("g", "antigravity", {}, tmp_path)]
    record = studio.open_show("Iso", None, seats, review=False, isolated=True)
    seat_dirs = data_root / "conversations" / record.state.id / "seats"
    profile = seat_dirs / "g" / ".agents" / "agents" / "agentnave-discussion.md"
    external = tmp_path / "external" / ".agents"
    (external / "agents").mkdir(parents=True)
    (external / "agents" / profile.name).write_bytes(profile.read_bytes())
    profile.unlink()
    for seat in ("a", "g"):
        target = seat_dirs / seat / ".agents"
        if target.exists():
            (target / "agents").rmdir()
            target.rmdir()
        target.symlink_to(external, target_is_directory=True)
    studio.update(record.state.id, None, True)
    studio.delete(record.state.id)
    assert (external / "agents" / profile.name).is_file()
    await studio.close()


@pytest.mark.asyncio
async def test_failed_review_save_still_discards_after_restart(
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    replace = os.replace
    calls = 0

    def failing_replace(source: str, target: Path) -> None:
        nonlocal calls
        calls += 1
        if calls > 2:  # a's launch and result are saved; entering review fails
            raise OSError("disk full")
        replace(source, target)

    async with Client(mcp) as client:
        show = await _call(
            client,
            "open_group",
            {
                "title": "Fragile review",
                "cwd": str(tmp_path),
                "review": True,
                "seats": [{"id": seat, "provider": "claude"} for seat in ("a", "b")],
            },
        )
        group_id = show["group_id"]
        monkeypatch.setattr(os, "replace", failing_replace)
        started = await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["a"], "order": "parallel", "prompt": "go"},
        )
        done = await _wait(client, started["round_id"])
        assert [seat["status"] for seat in _seats(done)] == ["discarded"]
        monkeypatch.setattr(os, "replace", replace)
        await _call(client, "post_host_message", {"conversation_id": group_id, "text": "ok"})
    async with Client(mcp) as client:
        view = _conversation(
            await _call(client, "read_conversation", {"conversation_id": group_id})
        )
        seat = cast(
            list[dict[str, object]], cast(list[dict[str, object]], view["rounds"])[0]["seats"]
        )[0]
        assert seat["review"] == "discard"
        await _call(
            client,
            "run_round",
            {"group_id": group_id, "seats": ["b"], "order": "parallel", "prompt": "next"},
        )


@pytest.mark.asyncio
async def test_finished_round_keeps_its_memory_reviews_after_a_failed_save(
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.group import GroupSeat

    studio = Studio(InvocationManager(), data_root)
    seats = [GroupSeat(seat, "claude", {}, tmp_path) for seat in ("a", "b")]
    record = studio.open_show("Rollback", tmp_path, seats, review=True, isolated=False)
    round_ = studio.run(record.state.id, ["a"], "parallel", "go", {})
    assert (await studio.wait(round_.id, 30))[1] == "review_needed"

    def failing_replace(source: str, target: Path) -> None:
        raise OSError("disk full")

    replace = os.replace
    monkeypatch.setattr(os, "replace", failing_replace)
    await studio.cancel(round_.id)  # discards in memory; the record still says pending
    monkeypatch.setattr(os, "replace", replace)
    with pytest.raises(ValueError, match="already decided: discard"):
        studio.decide(round_.id, {"a": "publish"})
    await studio.manager.shutdown()
    await studio.close()  # exiting writes the final state the failed save missed
    restarted = Studio(InvocationManager(), data_root)
    with pytest.raises(ValueError, match="already decided: discard"):
        restarted.decide(round_.id, {"a": "publish"})
    await restarted.close()


@pytest.mark.asyncio
async def test_group_and_task_resumes_share_one_session_guard(
    tmp_path: Path, data_root: Path
) -> None:
    studio = Studio(InvocationManager(), data_root)
    try:
        record = studio.open_show(
            "Mixed",
            tmp_path,
            [GroupSeat(s, "claude", {}, tmp_path) for s in ("a", "b")],
            review=False,
            isolated=False,
        )
        first = studio.run(record.state.id, ["a"], "serial", "one", {})
        await studio.wait(first.id, 3)
        session_id = first.group.states["a"].session_id
        assert first.finished and session_id
        task_id, task_conversation = studio.start_task(
            InvocationRequest(
                "claude", "Private instruction for you only:\nSLEEP", tmp_path, session_id
            ),
            None,
            None,
        )
        conflict = studio.run(record.state.id, ["a"], "serial", "conflict", {})
        await studio.wait(conflict.id, 3)
        assert conflict.runs[0].status == "failed"
        assert conflict.runs[0].launch_error is not None
        assert "active invocation" in conflict.runs[0].launch_error.message
        await studio.manager.cancel(task_id)
        await asyncio.gather(*studio.collectors)

        active = studio.run(record.state.id, ["a"], "serial", "resume", {"a": "SLEEP"})
        before = len(studio.records[task_conversation].state.tasks)
        with pytest.raises(ValueError, match="active invocation"):
            studio.start_task(
                InvocationRequest("claude", "conflict", tmp_path, session_id), None, None
            )
        assert len(studio.records[task_conversation].state.tasks) == before
        assert len(studio.records) == 2  # no phantom task conversation
        await studio.cancel(active.id)
    finally:
        await studio.shutdown()
        await studio.manager.shutdown()
        await studio.close()


@pytest.mark.asyncio
async def test_task_keeps_its_native_session_when_saving_the_result_fails(
    tmp_path: Path, data_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    studio = Studio(InvocationManager(), data_root)
    invocation_id, conversation_id = studio.start_task(
        InvocationRequest("claude", "one", tmp_path), "Job", None
    )
    replace = os.replace

    def failing_replace(source: str, target: Path) -> None:
        raise OSError("disk full")

    monkeypatch.setattr(os, "replace", failing_replace)
    await asyncio.gather(*studio.collectors)
    monkeypatch.setattr(os, "replace", replace)
    result = await studio.manager.wait(invocation_id)
    assert result is not None and result.session_id
    resumed, resumed_conversation = studio.start_task(
        InvocationRequest("claude", "two", tmp_path, result.session_id), None, None
    )
    assert resumed_conversation == conversation_id
    await studio.manager.wait(resumed)
    await studio.manager.shutdown()
    await studio.close()


@pytest.mark.asyncio
async def test_dashboard_failure_does_not_repeat_or_lose_started_task(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def fail(_self: Studio, *, history_loaded: bool = False) -> str:
        raise OSError("viewer unavailable")

    monkeypatch.setattr(Studio, "open_dashboard", fail)
    async with Client(mcp) as client:
        started = await _call(
            client, "start_agent", {"provider": "claude", "prompt": "once", "cwd": str(tmp_path)}
        )
        assert started["dashboard_error"] == "viewer unavailable"
        done = await _call(client, "wait_agent", {"invocation_id": started["invocation_id"]})
        assert done["status"] == "succeeded"
        view = await _call(
            client, "read_conversation", {"conversation_id": started["conversation_id"]}
        )
        assert len(cast(list[object], _conversation(view)["tasks"])) == 1


@pytest.mark.asyncio
async def test_dashboard_refresh_sleeps_when_idle_and_recovers_read_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    studio = Studio(manager, tmp_path / "history")
    studio.open_dashboard()
    assert studio.dashboard is not None and studio.refresher is not None
    studio.refresher.cancel()
    await asyncio.gather(studio.refresher, return_exceptions=True)
    dashboard = studio.dashboard
    original_sleep = asyncio.sleep
    sleeps = 0
    scans = 0
    original_scan = studio.scan

    async def step(_delay: float) -> None:
        nonlocal sleeps
        sleeps += 1
        # No reader at tick 1. Tick 2 requests state, tick 5 recovers.
        if sleeps == 2:
            await asyncio.to_thread(_fetch, dashboard.url(studio.home_token) + "state")
        if sleeps == 3:
            assert studio.refresh_error == "read failed"
            state = json.loads(
                await asyncio.to_thread(_fetch, dashboard.url(studio.home_token) + "state")
            )
            assert state["refresh_error"] == "read failed"
        if sleeps == 6:
            assert studio.refresh_error is None
            raise asyncio.CancelledError
        await original_sleep(0)

    def scan() -> dict[str, bool]:
        nonlocal scans
        scans += 1
        if scans == 1:
            raise OSError("read failed")
        return original_scan()

    monkeypatch.setattr(studio, "scan", scan)
    monkeypatch.setattr(asyncio, "sleep", step)
    try:
        with pytest.raises(asyncio.CancelledError):
            await studio._refresh_running()  # pyright: ignore[reportPrivateUsage]
        assert scans == 2
    finally:
        await studio.close()
        await manager.shutdown()


def test_host_projection_does_not_collide_with_director_seat() -> None:
    state = Conversation(
        id="show",
        title="show",
        kind="show",
        created="now",
        seats=[SeatRecord(id="director", provider="claude", provider_options={}, cwd="/tmp")],
        entries=[
            EntryRecord(round=1, kind="host", text="host", time="now"),
            EntryRecord(round=1, kind="answer", seat="director", text="seat", time="now"),
        ],
    )
    projected = json.loads(views.public(state))
    assert [m["speaker"] for m in projected["messages"]] == ["@host", "director"]


def test_idle_http_connection_does_not_block_state_requests() -> None:
    import socket

    dashboard = Dashboard()
    dashboard.update("test", b'{"view":"public"}')
    try:
        with (
            socket.create_connection(("127.0.0.1", dashboard.server.server_port)),
            urlopen(dashboard.url("test") + "state", timeout=0.5) as response,
        ):
            assert response.status == 200
    finally:
        dashboard.close()


def test_writer_can_acquire_after_brief_probe_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    creator = Record.create(
        tmp_path, Conversation(id="test", title="test", kind="show", created="now")
    )
    creator.release()
    reader = Record.read(tmp_path)
    writer = Record.read(tmp_path)
    probe = reader._lock()  # pyright: ignore[reportPrivateUsage]

    def release_probe(_delay: float) -> None:
        reader._unlock(probe)  # pyright: ignore[reportPrivateUsage]

    monkeypatch.setattr("agentnave_studio.store.time.sleep", release_probe)
    try:
        writer.acquire()
        assert writer.lock is not None
    finally:
        writer.release()


@pytest.mark.asyncio
async def test_write_receipts_are_small_and_old_history_has_a_viewer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async with Client(mcp) as client:
        show = await _call(
            client,
            "open_group",
            {
                "title": "History",
                "cwd": str(tmp_path),
                "seats": [{"id": s, "provider": "claude"} for s in ("a", "b")],
            },
        )
        posted = await _call(
            client,
            "post_host_message",
            {"conversation_id": show["group_id"], "text": "PRIVATE-HISTORY-" * 200},
        )
        assert posted == {"conversation_id": show["group_id"], "entry_index": 0}
        renamed = await _call(
            client, "update_conversation", {"conversation_id": show["group_id"], "title": "Renamed"}
        )
        assert renamed == {
            "conversation_id": show["group_id"],
            "title": "Renamed",
            "archived": False,
        }
    scans = 0
    original_scan = Studio.scan

    def count_scan(studio: Studio) -> dict[str, bool]:
        nonlocal scans
        scans += 1
        return original_scan(studio)

    monkeypatch.setattr(Studio, "scan", count_scan)
    async with Client(mcp) as client:
        listed = await _call(client, "list_conversations", {})
        assert scans == 1
        home = json.loads(await asyncio.to_thread(_fetch, str(listed["dashboard_url"]) + "state"))
        assert home["shows"][0]["title"] == "Renamed"
        assert "director_url" not in listed and "public_url" not in listed
        names = [tool.name for tool in (await client.list_tools()).tools]
        assert len(names) == 16 and "open_dashboard" not in names
        assert {"wait_any", "list_invocations"} <= set(names)

    def fail(_self: Studio, *, history_loaded: bool = False) -> str:
        raise OSError("viewer failed")

    monkeypatch.setattr(Studio, "open_dashboard", fail)
    async with Client(mcp) as client:
        listed = await _call(client, "list_conversations", {})
        assert listed["dashboard_error"] == "viewer failed"
        assert cast(list[dict[str, object]], listed["conversations"])[0]["title"] == "Renamed"


@pytest.mark.asyncio
async def test_history_io_failure_keeps_urls_until_recovery(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "history"
    creator = Record.create(
        root / "conversations" / "known",
        Conversation(id="known", title="Known", kind="show", created="now"),
    )
    creator.release()
    manager = InvocationManager()
    studio = Studio(manager, root)
    try:
        studio.open_dashboard()
        urls = studio.urls("known")
        assert urls is not None
        original_load = Record._load  # pyright: ignore[reportPrivateUsage]

        def fail(_directory: Path) -> Conversation:
            raise OSError("temporary history read failure")

        monkeypatch.setattr(Record, "_load", staticmethod(fail))
        assert studio.scan()["known"] is False
        assert studio.urls("known") == urls
        failed = json.loads(await asyncio.to_thread(_fetch, urls[0] + "state"))
        assert failed["stale"] is True and failed["title"] == "Known"
        assert "temporary history read failure" not in str(failed)
        assert studio.refresh_error == "temporary history read failure"
        monkeypatch.setattr(Record, "_load", staticmethod(original_load))
        assert studio.scan()["known"] is True
        assert studio.urls("known") == urls
        recovered = json.loads(await asyncio.to_thread(_fetch, urls[0] + "state"))
        assert recovered["stale"] is False and studio.refresh_error is None
    finally:
        await studio.close()
        await manager.shutdown()


@pytest.mark.asyncio
async def test_restored_review_receipt_counts_only_public_entries(data_root: Path) -> None:
    record = Record.create(
        data_root / "conversations" / "restored",
        Conversation(
            id="restored",
            title="Restored",
            kind="show",
            review=True,
            rounds=[
                RoundRecord(
                    id="restored-round",
                    number=1,
                    order="parallel",
                    seats=[
                        SeatRunRecord(
                            seat_id=seat,
                            status="succeeded",
                            review="pending",
                            output="answer-" + seat,
                        )
                        for seat in ("a", "b")
                    ],
                )
            ],
        ),
    )
    record.release()
    async with Client(mcp) as client:
        receipt = await _call(
            client,
            "decide_answers",
            {
                "round_id": "restored-round",
                "decisions": {"a": "discard", "b": "publish"},
            },
        )
        assert receipt["pending_review"] == [] and receipt["committed"] == ["b"]
        view = _conversation(
            await _call(client, "read_conversation", {"conversation_id": "restored"})
        )
        assert [entry["seat"] for entry in cast(list[dict[str, object]], view["entries"])] == ["b"]
