from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from typing import cast

from agentnave.conversations import Conversations
from agentnave.group import Group, GroupSeat, Round, SeatRun
from agentnave.inbox_events import _round_events  # pyright: ignore[reportPrivateUsage]
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
