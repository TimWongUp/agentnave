"""Whitelisted dashboard projections. Public views carry only committed entries."""

from __future__ import annotations

import hashlib
import json
from datetime import datetime, timedelta

from agentnave.store import Conversation, EntryRecord, RoundRecord, Unavailable

_ABSENT = {
    "failed": "失败",
    "blocked": "受阻",
    "cancelled": "已取消",
    "interrupted": "已中断",
}


def _encode(data: dict[str, object]) -> bytes:
    return json.dumps(data, ensure_ascii=False).encode()


def _speaker(entry: EntryRecord) -> str | None:
    if entry.kind == "forward":
        return "@forward"  # forwarded content is the manager's action, never a host remark
    if entry.kind in ("task", "host"):
        return "@host"
    return entry.seat


def _thread(provider: str, session_id: str | None) -> str | None:
    """Opaque private-thread key; views never expose native session IDs."""
    if session_id is None:
        return None
    return hashlib.sha256(f"{provider}\0{session_id}".encode()).hexdigest()[:16]


def _show(state: Conversation, view: str) -> dict[str, object]:
    seats = {seat.id: seat for seat in state.seats}
    rounds = {round_.number: round_ for round_ in state.rounds}

    def recipients(number: int, kind: str) -> list[dict[str, object]]:
        round_ = rounds.get(number)
        targets = (
            [
                (run.seat_id, run.reported_model)
                for run in round_.seats
                if run.status not in {"pending", "not_started"}
                and not (run.error or "").startswith(
                    ("launch_error:", "state_write_failed:", "state_update_failed:")
                )
            ]
            if kind == "task" and round_ is not None
            else [(seat.id, None) for seat in state.seats]
            if kind == "host"
            else []
        )
        return [
            {
                "seat": seat_id,
                "provider": seats[seat_id].provider,
                "model": reported or seats[seat_id].provider_options.get("model", ""),
            }
            for seat_id, reported in targets
            if seat_id in seats
        ]

    return {
        "view": view,
        "host": state.host,
        "id": state.id,
        "title": state.title,
        "mode": state.kind,
        "review": state.review,
        "isolated": state.isolated,
        "seats": [
            {
                "id": seat.id,
                "label": seat.id,
                "model": str(seat.provider_options.get("model", "")),
                "effort": seat.provider_options.get("effort"),
                "provider": seat.provider,
                "role": seat.role,
                "access": seat.access,
            }
            for seat in state.seats
        ],
        "messages": [
            {
                "message_id": f"{state.id}:e:{index}",
                "speaker": _speaker(entry),
                "kind": entry.kind,
                "round": entry.round,
                "text": (
                    f"（本轮未作答：{_ABSENT.get(entry.text or '', entry.text)}）"
                    if entry.kind == "absent"
                    else entry.text
                ),
                "time": entry.time,
                "manager_model": entry.manager_model,
                "manager_reported_model": entry.manager_reported_model,
                "reported_model": entry.reported_model,
                **(
                    {"forward_id": entry.forward_id, "forward_source": entry.forward_source}
                    if entry.kind == "forward"
                    else {}
                ),
                **(
                    {"recipients": recipients(entry.round, entry.kind)}
                    if entry.kind in ("task", "host")
                    else {}
                ),
            }
            for index, entry in enumerate(state.entries)
        ],
    }


def public(state: Conversation, *, stale: bool = False) -> bytes:
    return _encode(_show(state, "public") | {"stale": stale})


def _round(round_: RoundRecord, tails: dict[str, str]) -> dict[str, object]:
    return {
        "number": round_.number,
        "order": round_.order,
        "status": "review" if round_.pending_review() else round_.status,
        "error": round_.error,
        "seats": [
            {
                "seat_id": seat.seat_id,
                "status": seat.status,
                "instruction": seat.instruction,
                "output": tails.get(seat.seat_id) if seat.status == "running" else seat.output,
                "review": seat.review,
                "error": seat.error,
                "reported_model": seat.reported_model,
                "finished": seat.finished,
            }
            for seat in round_.seats
        ],
    }


def director(
    state: Conversation,
    *,
    public_url: str,
    home_url: str,
    storage: str,
    tails: dict[str, str],
    elapsed: dict[str, int] | None = None,
    refresh_error: str | None = None,
) -> bytes:
    """Private desk view. tails maps running seat or task ids to in-memory reply tails."""
    return _encode(
        _director(
            state,
            public_url=public_url,
            home_url=home_url,
            storage=storage,
            tails=tails,
            elapsed=elapsed,
            refresh_error=refresh_error,
        )
    )


def _director(
    state: Conversation,
    *,
    public_url: str,
    home_url: str,
    storage: str,
    tails: dict[str, str],
    elapsed: dict[str, int] | None,
    refresh_error: str | None,
) -> dict[str, object]:
    common: dict[str, object] = {
        "workbench_url": home_url,
        "storage": storage,
        "refresh_error": refresh_error,
        "created": state.created,
        "cwd": state.tasks[-1].cwd if state.kind == "task" and state.tasks else state.cwd,
    }
    if state.kind == "task":
        tasks = [
            task.model_dump(exclude={"session_id"})
            | {
                "message_id": f"{state.id}:t:{task.id}",
                "thread": _thread(task.provider, task.session_id),
            }
            | ({"output": tails[task.id]} if task.id in tails else {})
            | ({"elapsed_ms": elapsed[task.id]} if elapsed and task.id in elapsed else {})
            for task in state.tasks
        ]
        return {
            "view": "director",
            "id": state.id,
            "title": state.title,
            "mode": "task",
            "host": state.host,
            "seats": [],
            "messages": [],
            "tasks": tasks,
        } | common
    latest = state.rounds[-1] if state.rounds else None
    data = _show(state, "director") | common
    round_times = {entry.round: entry.time for entry in state.entries if entry.kind == "task"}
    data["private_messages"] = [
        {
            "round": round_.number,
            "seat_id": seat.seat_id,
            "text": seat.output,
            "reported_model": seat.reported_model,
            "time": seat.finished or round_times.get(round_.number, state.created),
            "message_id": None,  # answers kept private are never shared, so never forwarded
        }
        for round_ in state.rounds
        for seat in round_.seats
        if seat.review == "private" and seat.output is not None
    ]
    # Private instructions go to one seat only; the director view shows them to the boss.
    data["rounds"] = [
        {"number": round_.number, "seats": [{"seat_id": run.seat_id} for run in round_.seats]}
        for round_ in state.rounds
    ]
    data["instructions"] = [
        {
            "round": round_.number,
            "seat_id": seat.seat_id,
            "text": seat.instruction,
            "time": round_times.get(round_.number, state.created),
        }
        for round_ in state.rounds
        for seat in round_.seats
        if seat.instruction
    ]
    data.update(
        {
            "public_url": public_url,
            "status": "idle" if latest is None else _round(latest, {})["status"],
            "speaking": [seat.seat_id for seat in latest.seats if seat.status == "running"]
            if latest
            else [],
            "round": _round(latest, tails) if latest else None,
        }
    )
    return data


def timeline(
    state: Conversation,
    *,
    since_round: int = 0,
    include_private: bool = False,
    limit: int | None = None,
) -> dict[str, object]:
    """A conversation as the manager reads it back: final messages in order, like the App.

    Shows list committed entries plus answers awaiting review; include_private adds each
    seat's private instructions and the answers kept private. Tasks list prompt and reply.
    limit keeps only the most recent messages or tasks and counts the rest in omitted.
    """
    data: dict[str, object] = {
        "conversation_id": state.id,
        "title": state.title,
        "kind": state.kind,
        "status": _status(state),
        "created": state.created,
    }
    if state.kind == "task":
        tasks = [
            _compact(
                {
                    "message_id": f"{state.id}:t:{task.id}",
                    "provider": task.provider,
                    "model": task.reported_model or task.model or None,
                    "role": task.role,
                    "access": task.access,
                    "cwd": task.cwd,
                    "prompt": task.prompt,
                    "status": task.status,
                    "output": task.output or None,
                    "error": task.error,
                    "session_id": task.session_id,
                    "time": task.time,
                }
            )
            for task in state.tasks
        ]
        data["tasks"] = _latest(data, tasks, limit)
        return data
    data["cwd"] = state.cwd
    data["review"] = state.review
    data["seats"] = [
        _compact(
            {
                "id": seat.id,
                "provider": seat.provider,
                "model": seat.provider_options.get("model"),
                "role": seat.role,
                "access": seat.access,
            }
        )
        for seat in state.seats
    ]
    data["rounds"] = len(state.rounds)
    if state.rounds:
        latest = state.rounds[-1]
        data["latest_round"] = {
            "round_id": latest.id,
            "round": latest.number,
            "order": latest.order,
            "status": "review" if latest.pending_review() else latest.status,
        }
    # Per round: private instructions, answers outside the entries, and the seat order.
    extra: dict[int, tuple[list[dict[str, object]], list[dict[str, object]], list[str]]] = {}
    for round_ in state.rounds:
        if round_.number < since_round:
            continue
        before, answers, order = extra.setdefault(round_.number, ([], [], []))
        for seat in round_.seats:
            order.append(seat.seat_id)
            if include_private and seat.instruction:
                before.append(
                    {
                        "round": round_.number,
                        "kind": "instruction",
                        "to": seat.seat_id,
                        "text": seat.instruction,
                    }
                )
            if seat.review == "pending" or (include_private and seat.review == "private"):
                answers.append(
                    {
                        "round": round_.number,
                        "kind": f"{seat.review}_answer",
                        "seat": seat.seat_id,
                        "text": seat.output,
                    }
                )
    messages: list[dict[str, object]] = []
    numbers = sorted({entry.round for entry in state.entries} | set(extra))
    for number in (n for n in numbers if n >= since_round):
        before, answers, order = extra.get(number, ([], [], []))
        # A round number holds forwards and the task, then the answers, then host remarks
        # posted after the round (round 0 holds those posted before the first round).
        head: list[dict[str, object]] = []
        tail: list[dict[str, object]] = []
        for index, entry in enumerate(state.entries):
            if entry.round != number:
                continue
            message = _message(state.id, index, entry)
            if entry.kind in ("answer", "absent"):
                answers.append(message)
            else:
                (tail if entry.kind == "host" else head).append(message)
        position = {seat_id: index for index, seat_id in enumerate(order)}
        answers.sort(key=lambda message: position.get(str(message.get("seat")), len(order)))
        messages.extend([*head, *before, *answers, *tail])
    data["messages"] = _latest(data, messages, limit)
    return data


def _latest(
    data: dict[str, object], items: list[dict[str, object]], limit: int | None
) -> list[dict[str, object]]:
    if limit is None or len(items) <= limit:
        return items
    data["omitted"] = len(items) - limit
    return items[-limit:]


def _message(conversation_id: str, index: int, entry: EntryRecord) -> dict[str, object]:
    return _compact(
        {
            "message_id": f"{conversation_id}:e:{index}",
            "round": entry.round,
            "kind": entry.kind,
            "seat": entry.seat,
            "text": entry.text,
            "forward_source": entry.forward_source,
        }
    )


def _compact(data: dict[str, object]) -> dict[str, object]:
    return {key: value for key, value in data.items() if value is not None}


def _status(state: Conversation) -> str:
    return (
        "review"
        if state.pending_review()
        else state.rounds[-1].status
        if state.rounds
        else "running"
        if any(task.status == "running" for task in state.tasks)
        else state.tasks[-1].status
        if state.tasks
        else "idle"
    )


def updated(state: Conversation) -> str:
    """When the conversation last saw activity: a message, a task start or a task end."""
    times = [_time(state.created)] + [_time(entry.time) for entry in state.entries]
    times += [
        start + timedelta(milliseconds=task.elapsed_ms)
        for task in state.tasks
        if (start := _time(task.time)) is not None
    ]
    known = [time for time in times if time is not None]
    return max(known).isoformat() if known else state.created


def _time(text: str) -> datetime | None:
    try:
        return datetime.fromisoformat(text)
    except ValueError:
        return None


def snapshot(state: Conversation, storage: str) -> dict[str, object]:
    """One conversation as the tray app reads it, linking to its routes: / , /c/<id>/, /p/<id>/."""
    show = state.kind == "show"
    return {
        "version": 1,
        "summary": {
            "id": state.id,
            "title": state.title,
            "mode": state.kind,
            "host": state.host,
            "host_source": state.host_source,
            "manager_session": state.manager_session,
            "status": _status(state),
            "updated": updated(state),
        },
        "director": _director(
            state,
            public_url=f"/p/{state.id}/" if show else "",
            home_url="/",
            storage=storage,
            tails={},
            elapsed=None,
            refresh_error=None,
        ),
        "public": _show(state, "public") | {"stale": False} if show else None,
    }


def home(
    shows: list[tuple[Conversation, str]],
    unavailable: list[Unavailable],
    storage: str,
    *,
    refresh_error: str | None = None,
) -> bytes:
    return _encode(
        {
            "view": "workbench",
            "id": "workbench",
            "title": "工作台",
            "seats": [],
            "messages": [],
            "shows": [
                {
                    "title": state.title,
                    "mode": state.kind,
                    "director_url": url,
                    "status": _status(state),
                }
                for state, url in shows
            ],
            "refresh_error": refresh_error,
            "unavailable": [item.model_dump() for item in unavailable],
            "storage": storage,
        }
    )
