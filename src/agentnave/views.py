"""Whitelisted dashboard projections. Public views carry only committed entries."""

from __future__ import annotations

import json
from datetime import datetime, timedelta

from agentnave.store import Conversation, RoundRecord, Unavailable

_ABSENT = {
    "failed": "失败",
    "blocked": "受阻",
    "cancelled": "已取消",
    "interrupted": "已中断",
}


def _encode(data: dict[str, object]) -> bytes:
    return json.dumps(data, ensure_ascii=False).encode()


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
                "provider": seat.provider,
            }
            for seat in state.seats
        ],
        "messages": [
            {
                "speaker": "@host" if entry.kind in ("task", "host") else entry.seat,
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
                    {"recipients": recipients(entry.round, entry.kind)}
                    if entry.kind in ("task", "host")
                    else {}
                ),
            }
            for entry in state.entries
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
    }
    if state.kind == "task":
        tasks = [
            task.model_dump(exclude={"session_id"})
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


def _updated(state: Conversation) -> str:
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


def snapshot(
    state: Conversation,
    storage: str,
    *,
    tails: dict[str, str] | None = None,
    elapsed: dict[str, int] | None = None,
) -> dict[str, object]:
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
            "updated": _updated(state),
        },
        "director": _director(
            state,
            public_url=f"/p/{state.id}/" if show else "",
            home_url="/",
            storage=storage,
            tails=tails or {},
            elapsed=elapsed,
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
