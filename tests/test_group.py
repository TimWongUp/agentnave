from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import cast

import pytest

from agentnave.adapters.antigravity import write_restricted_profile
from agentnave.core import InvocationManager
from agentnave.group import Groups, GroupSeat
from agentnave.mcp_server import round_payload
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    InvocationStatus,
)


class FakeManager:
    """Scripted stand-in: each invocation ends only when a test finishes or cancels it."""

    def __init__(self) -> None:
        self.requests: dict[str, InvocationRequest] = {}
        self.results: dict[str, asyncio.Future[InvocationResult]] = {}
        self.attention: dict[str, asyncio.Queue[InvocationError]] = {}

    def start(self, request: InvocationRequest) -> str:
        invocation_id = str(len(self.requests) + 1)
        self.requests[invocation_id] = request
        self.results[invocation_id] = asyncio.get_running_loop().create_future()
        self.attention[invocation_id] = asyncio.Queue()
        return invocation_id

    async def wait_for_update(
        self, invocation_id: str, timeout_seconds: float
    ) -> InvocationResult | InvocationError | None:
        # Wake on completion or attention like InvocationManager, not by polling.
        result = asyncio.shield(self.results[invocation_id])
        attention = asyncio.create_task(self.attention[invocation_id].get())
        try:
            await asyncio.wait(
                (result, attention), timeout=timeout_seconds, return_when=asyncio.FIRST_COMPLETED
            )
        finally:
            attention.cancel()
        if result.done():
            return result.result()
        if attention.done() and not attention.cancelled():
            return attention.result()
        return None

    async def cancel(self, invocation_id: str) -> InvocationResult:
        self.finish(invocation_id, InvocationStatus.CANCELLED)
        return await self.results[invocation_id]

    def finish(
        self,
        invocation_id: str,
        status: InvocationStatus = InvocationStatus.SUCCEEDED,
        output: str | None = None,
        error: InvocationError | None = None,
    ) -> None:
        request = self.requests[invocation_id]
        # InvocationManager echoes the requested session on failed or cancelled runs.
        session_id = (
            f"session-{invocation_id}"
            if status is InvocationStatus.SUCCEEDED
            else request.session_id
        )
        future = self.results[invocation_id]
        if not future.done():
            future.set_result(
                InvocationResult(
                    status,
                    request.provider,
                    output or f"answer-{invocation_id}",
                    session_id,
                    {},
                    1,
                    error,
                )
            )

    def prompt(self, invocation_id: str) -> str:
        return self.requests[invocation_id].prompt


def entries(prompt: str) -> list[dict[str, object]]:
    start = prompt.index("New entries (JSON data):\n") + len("New entries (JSON data):\n")
    end = prompt.find("\n\nPrivate instruction")
    return cast(list[dict[str, object]], json.loads(prompt[start : end if end >= 0 else None]))


def open_group(tmp_path: Path, *seat_ids: str) -> tuple[Groups, FakeManager, str]:
    manager = FakeManager()
    groups = Groups(cast(InvocationManager, manager))
    group = groups.open(
        tmp_path, [GroupSeat(seat_id, "claude", {"model": "m"}, tmp_path) for seat_id in seat_ids]
    )
    return groups, manager, group.id


async def settle(groups: Groups, round_id: str) -> None:
    round_, _ = await groups.wait(round_id, 2)
    assert round_.finished


@pytest.mark.asyncio
async def test_parallel_round_isolates_seats_until_all_end_then_serial_round_chains(
    tmp_path: Path,
) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    first = groups.run(group_id, ["a", "b"], "parallel", "Q1", {"b": "be skeptical"})
    assert set(manager.requests) == {"1", "2"}  # both launched before any answer
    assert "be skeptical" not in manager.prompt("1")
    assert manager.prompt("2").endswith("Private instruction for you only:\nbe skeptical")
    assert all(manager.prompt(i).startswith("You are one seat") for i in ("1", "2"))
    assert entries(manager.prompt("1")) == [{"round": 1, "kind": "task", "text": "Q1"}]

    manager.finish("1", output="A1")
    await asyncio.sleep(0.05)
    assert [entry.kind for entry in first.group.entries] == ["task"]  # nothing committed yet
    manager.finish("2", output="B1")
    await settle(groups, first.id)
    ended = round_payload(first, cast(InvocationManager, manager), "finished")
    await asyncio.sleep(0.01)
    assert (
        round_payload(first, cast(InvocationManager, manager), "finished")["elapsed_ms"]
        == ended["elapsed_ms"]
    )
    assert [(e.kind, e.seat_id, e.text) for e in first.group.entries] == [
        ("task", None, "Q1"),
        ("answer", "a", "A1"),
        ("answer", "b", "B1"),
    ]

    second = groups.run(group_id, ["b", "a"], "serial", "Q2", {})
    assert set(manager.requests) == {"1", "2", "3"}  # only the first serial seat runs
    b_request = manager.requests["3"]
    assert b_request.session_id == "session-2"
    assert not b_request.prompt.startswith("You are one seat")
    assert entries(b_request.prompt) == [
        {"round": 1, "kind": "answer", "seat": "a", "text": "A1"},
        {"round": 2, "kind": "task", "text": "Q2"},
    ]
    manager.finish("3", output="B2")
    for _ in range(100):
        if "4" in manager.requests:
            break
        await asyncio.sleep(0.01)
    assert entries(manager.prompt("4")) == [
        {"round": 1, "kind": "answer", "seat": "b", "text": "B1"},
        {"round": 2, "kind": "task", "text": "Q2"},
        {"round": 2, "kind": "answer", "seat": "b", "text": "B2"},
    ]
    manager.finish("4")
    await settle(groups, second.id)


@pytest.mark.asyncio
async def test_unexpected_state_hook_failure_does_not_leave_a_running_round(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")

    def broken_hook(_round: object) -> None:
        raise RuntimeError("PRIVATE INTERNAL DETAIL")

    monkeypatch.setattr(groups, "_on_change", broken_hook)
    round_ = groups.run(group_id, ["a", "b"], "serial", "Q", {})
    await settle(groups, round_.id)
    payload = round_payload(round_, cast(InvocationManager, manager), "finished")
    assert payload["status"] == "failed"
    assert [seat["status"] for seat in payload["seats"]] == ["failed", "not_started"]
    assert not manager.requests and round_.group.active is None
    assert "PRIVATE INTERNAL DETAIL" not in str(payload)


@pytest.mark.asyncio
async def test_driver_failure_settles_other_seats_before_releasing_the_group(
    tmp_path: Path,
) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    round_ = groups.run(group_id, ["a", "b"], "parallel", "Q", {})
    manager.results["1"].set_exception(RuntimeError("broken execution"))
    await settle(groups, round_.id)
    payload = round_payload(round_, cast(InvocationManager, manager), "finished")
    assert payload["status"] == "failed" and "round_execution_failed" in payload.get("error", "")
    assert [seat["status"] for seat in payload["seats"]] == ["failed", "cancelled"]
    assert all(result.done() for result in manager.results.values())
    assert round_.group.active is None
    next_round = groups.run(group_id, ["a"], "serial", "recover", {})
    assert manager.requests["3"].session_id is None
    manager.finish("3")
    await settle(groups, next_round.id)


@pytest.mark.asyncio
async def test_failures_are_absent_and_only_proven_delivery_advances_sessions(
    tmp_path: Path,
) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    first = groups.run(group_id, ["a", "b"], "parallel", "Q1", {})
    manager.finish("1")
    manager.finish("2")
    await settle(groups, first.id)

    second = groups.run(group_id, ["a", "b"], "serial", "Q2", {})
    manager.attention["3"].put_nowait(InvocationError("provider_blocked", "waiting"))
    round_, reason = await groups.wait(second.id, 2)
    assert (
        reason == "execution_blocked" and round_.runs[0].status == "running"
    )  # a blocker is not an absence
    assert [entry.kind for entry in second.group.entries][-1] == "task"
    # The failure echoes the old session id, which must not count as delivery.
    manager.finish("3", InvocationStatus.FAILED, error=InvocationError("provider_error", "x"))
    for _ in range(100):
        if "4" in manager.requests:
            break
        await asyncio.sleep(0.01)
    assert {"round": 2, "kind": "absent", "seat": "a", "reason": "failed"} in entries(
        manager.prompt("4")
    )
    manager.finish("4", InvocationStatus.FAILED, error=InvocationError("launch_error", "no binary"))
    await settle(groups, second.id)

    third = groups.run(group_id, ["a", "b"], "parallel", "Q3", {})
    a_request, b_request = manager.requests["5"], manager.requests["6"]
    assert a_request.session_id is None and a_request.prompt.startswith("You are one seat")
    assert {"round": 1, "kind": "answer", "seat": "a", "text": "answer-1"} in entries(
        a_request.prompt
    )
    assert b_request.session_id == "session-2"  # launch_error keeps the native session
    manager.finish("5")
    manager.finish("6")
    await settle(groups, third.id)


@pytest.mark.asyncio
async def test_cancel_round_keeps_finished_answers_and_skips_unstarted_seats(
    tmp_path: Path,
) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b", "c")
    parallel = groups.run(group_id, ["a", "b"], "parallel", "Q1", {})
    manager.finish("1", output="A1")
    await asyncio.sleep(0.05)
    await groups.cancel(parallel.id)
    assert [(e.kind, e.seat_id, e.text) for e in parallel.group.entries[1:]] == [
        ("answer", "a", "A1"),
        ("absent", "b", "cancelled"),
    ]

    serial = groups.run(group_id, ["a", "b", "c"], "serial", "Q2", {})
    with pytest.raises(ValueError, match="already has a running round"):
        groups.run(group_id, ["c"], "serial", "Q3", {})
    cancelled = await groups.cancel(serial.id)
    assert [run.status for run in cancelled.runs] == ["cancelled", "not_started", "not_started"]
    assert [(e.kind, e.seat_id) for e in serial.group.entries[-2:]] == [
        ("task", None),
        ("absent", "a"),
    ]
    assert await groups.cancel(serial.id) is cancelled
    assert len(manager.requests) == 3


@pytest.mark.asyncio
async def test_single_seat_cancel_lets_serial_round_continue(tmp_path: Path) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    round_ = groups.run(group_id, ["a", "b"], "serial", "Q1", {})
    await manager.cancel("1")
    for _ in range(100):
        if "2" in manager.requests:
            break
        await asyncio.sleep(0.01)
    manager.finish("2")
    await settle(groups, round_.id)
    assert [run.status for run in round_.runs] == ["cancelled", "succeeded"]


@pytest.mark.asyncio
async def test_invalid_rounds_and_groups_fail_before_launch(tmp_path: Path) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    for seats, instructions, message in (
        ([], {}, "non-empty"),
        (["a", "a"], {}, "non-empty"),
        (["a", "z"], {}, "unknown seat"),
        (["a"], {"b": "secret"}, "outside this round"),
    ):
        with pytest.raises(ValueError, match=message):
            groups.run(group_id, seats, "parallel", "Q", instructions)
    assert manager.requests == {}

    with pytest.raises(ValueError, match="unique"):
        groups.open(tmp_path, [GroupSeat("a", "claude", {}, tmp_path)] * 2)
    restricted = GroupSeat("g", "antigravity", {"restricted_tools": True}, tmp_path)
    with pytest.raises(ValueError, match="dedicated cwd"):
        groups.open(tmp_path, [restricted, GroupSeat("a", "claude", {}, tmp_path)])
    assert not (tmp_path / ".agents").exists()  # validation never writes into the project
    seat_dir = tmp_path / "seat"
    seat_dir.mkdir()
    write_restricted_profile(seat_dir)
    groups.open(tmp_path, [GroupSeat("g", "antigravity", {"restricted_tools": True}, seat_dir)])


@pytest.mark.asyncio
async def test_shutdown_cancels_running_rounds(tmp_path: Path) -> None:
    groups, _manager, group_id = open_group(tmp_path, "a", "b")
    round_ = groups.run(group_id, ["a", "b"], "serial", "Q1", {})
    await groups.shutdown()
    assert round_.finished and [run.status for run in round_.runs] == ["cancelled", "not_started"]


@pytest.mark.asyncio
async def test_shutdown_never_launches_the_next_serial_seat(tmp_path: Path) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")

    async def wake_first(invocation_id: str, _timeout: float) -> InvocationResult:
        return await manager.results[invocation_id]  # one hop, so the driver resumes first

    manager.wait_for_update = wake_first  # type: ignore[method-assign]
    round_ = groups.run(group_id, ["a", "b"], "serial", "Q1", {})
    await asyncio.sleep(0)  # the driver now waits on seat a
    manager.finish("1")  # the driver has been woken but has not run yet
    await groups.shutdown()
    assert [run.status for run in round_.runs] == ["succeeded", "not_started"]
    assert set(manager.requests) == {"1"}


@pytest.mark.asyncio
async def test_blockers_of_finished_seats_do_not_wake_wait(tmp_path: Path) -> None:
    groups, manager, group_id = open_group(tmp_path, "a", "b")
    round_ = groups.run(group_id, ["a", "b"], "parallel", "Q1", {})
    manager.attention["1"].put_nowait(InvocationError("provider_blocked", "waiting"))
    await asyncio.sleep(0.05)
    manager.finish("1")
    await asyncio.sleep(0.05)
    _, reason = await groups.wait(round_.id, 0.1)
    assert reason is None and [run.status for run in round_.runs] == ["succeeded", "running"]
    manager.finish("2")
    await settle(groups, round_.id)


@pytest.mark.parametrize("cwd", ["relative", "/does/not/exist"])
def test_group_cwd_is_checked_even_when_every_seat_overrides_it(tmp_path: Path, cwd: str) -> None:
    groups = Groups(cast(InvocationManager, FakeManager()))
    with pytest.raises(ValueError, match="group cwd"):
        groups.open(Path(cwd), [GroupSeat(s, "claude", {}, tmp_path) for s in "ab"])
