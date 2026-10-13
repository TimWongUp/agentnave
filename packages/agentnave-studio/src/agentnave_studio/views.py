"""Whitelisted dashboard projections. Public views carry only committed entries."""

from __future__ import annotations

import json

from agentnave_studio.store import Conversation, RoundRecord, Unavailable

_ABSENT = {
    "failed": "失败",
    "blocked": "受阻",
    "cancelled": "已取消",
    "interrupted": "已中断",
}


def _encode(data: dict[str, object]) -> bytes:
    return json.dumps(data, ensure_ascii=False).encode()


def _show(state: Conversation, view: str) -> dict[str, object]:
    return {
        "view": view,
        "id": state.id,
        "title": state.title,
        "mode": state.kind,
        "review": state.review,
        "isolated": state.isolated,
        "seats": [
            {
                "id": seat.id,
                "label": seat.id,
                "model": str(seat.provider_options.get("model", "原生设置")),
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
    common: dict[str, object] = {
        "archived": state.archived,
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
        return _encode(
            {
                "view": "director",
                "id": state.id,
                "title": state.title,
                "mode": "task",
                "seats": [],
                "messages": [],
                "tasks": tasks,
            }
            | common
        )
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
    return _encode(data)


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
                    "archived": state.archived,
                    "director_url": url,
                    "status": (
                        "review"
                        if state.pending_review()
                        else state.rounds[-1].status
                        if state.rounds
                        else "running"
                        if any(task.status == "running" for task in state.tasks)
                        else state.tasks[-1].status
                        if state.tasks
                        else "idle"
                    ),
                }
                for state, url in shows
            ],
            "refresh_error": refresh_error,
            "unavailable": [item.model_dump() for item in unavailable],
            "storage": storage,
        }
    )
