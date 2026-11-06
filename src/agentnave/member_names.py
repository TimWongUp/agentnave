"""Identity projections from saved facts; no App defaults or mutable desk metadata."""

from __future__ import annotations

from collections.abc import Iterable

from agentnave.naming import MemberIdentity, MemberName, thread
from agentnave.store import Conversation, TaskRecord


def private_key(state: Conversation, task: TaskRecord) -> str:
    native = thread(task.provider, task.session_id)
    return f"thread:{native}" if native else f"task:{state.id}:{task.id}"


def private_members(states: Iterable[Conversation]) -> list[MemberName]:
    threads: dict[str, list[tuple[Conversation, TaskRecord]]] = {}
    for state in states:
        for task in state.tasks:
            threads.setdefault(private_key(state, task), []).append((state, task))
    ordered = sorted(threads.items(), key=lambda pair: min((t.time, t.id) for _, t in pair[1]))
    members: list[MemberName] = []
    for ordinal, (key, records) in enumerate(ordered, 1):
        # Historical names remain peers after a thread changes model or role. The stable
        # key excludes all names of this same thread from self-collision checks.
        members.extend(
            dict.fromkeys(
                MemberName(key, task.provider, task.model, task.reported_model, task.role, ordinal)
                for _, task in records
            )
        )
    return members


def task_identity(
    state: Conversation, task: TaskRecord, peers: list[MemberName] | None = None
) -> MemberIdentity:
    peers = peers if peers is not None else private_members([state])
    key = private_key(state, task)
    ordinal = next((p.ordinal for p in peers if p.key == key), 1)
    return MemberName(
        key, task.provider, task.model, task.reported_model, task.role, ordinal
    ).identity(peers)


def meeting_members(state: Conversation, number: int | None = None) -> list[MemberName]:
    reported: dict[str, str | None] = {}
    rounds = sorted(
        (r for r in state.rounds if number is None or r.number <= number),
        key=lambda r: r.number,
    )
    for round_ in rounds:
        for run in round_.seats:
            # An unreported new run uses this run's request, never an older native model.
            reported[run.seat_id] = run.reported_model
    return [
        MemberName(
            f"seat:{state.id}:{seat.id}",
            seat.provider,
            str(seat.provider_options.get("model", "")),
            reported.get(seat.id),
            seat.role,
            ordinal,
        )
        for ordinal, seat in enumerate(state.seats, 1)
    ]


def seat_identity(
    state: Conversation,
    seat_id: str,
    *,
    reported: str | None = None,
    number: int | None = None,
    current: bool = False,
) -> MemberIdentity:
    peers = meeting_members(state, number)
    key = f"seat:{state.id}:{seat_id}"
    member = next((p for p in peers if p.key == key), MemberName(key, ""))
    if not current:
        member = MemberName(
            member.key, member.provider, member.requested, reported, member.role, member.ordinal
        )
    return member.identity(peers)
