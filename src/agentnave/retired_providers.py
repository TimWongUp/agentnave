"""App files that still name a CLI the service no longer runs read as if they did not (ADR-0119).

Members, permissions and seat choices of a retired CLI are dropped before validation; a seat
whose choices were all retired is dropped too, which leaves it unbound and so unavailable.
Saved history is not touched here: its provider is a plain string and stays readable.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from typing import Any, cast, get_args

from agentnave.models import RETIRED_PROVIDERS
from agentnave.orchestration_roles import OrchestrationGroup

_SEAT_FIELDS = ("provider", "model", "effort", "account")


def _drop_keys(value: object) -> None:
    if isinstance(value, dict):
        for provider in RETIRED_PROVIDERS:
            cast(dict[str, object], value).pop(provider, None)


def _seat(seat: object) -> object | None:
    """The seat without retired choices, the next remaining choice taking the primary's place;
    None when nothing remains."""
    if not isinstance(seat, dict):
        return seat
    primary = cast(dict[str, object], seat)
    fallbacks = primary.get("fallbacks")
    choices = [primary]
    if isinstance(fallbacks, list):
        choices += [
            cast(dict[str, object], c) for c in cast(list[object], fallbacks) if isinstance(c, dict)
        ]
    kept = [c for c in choices if c.get("provider") not in RETIRED_PROVIDERS]
    if len(kept) == len(choices):
        return primary
    if not kept:
        return None
    head = kept[0]
    result = {key: value for key, value in primary.items() if key not in _SEAT_FIELDS}
    result.update({key: head[key] for key in _SEAT_FIELDS if key in head})
    result["fallbacks"] = kept[1:]
    return result


def _seat_map(value: object) -> dict[str, object] | object:
    """A mapping of names to seats, such as a group's difficulties."""
    if not isinstance(value, dict):
        return value
    kept = {name: _seat(seat) for name, seat in cast(dict[str, object], value).items()}
    return {name: seat for name, seat in kept.items() if seat is not None}


def _members(document: dict[str, Any]) -> None:
    _drop_keys(document.get("clis"))
    for section in ("modes", "defaults"):
        providers = document.get(section)
        if isinstance(providers, dict):
            for value in cast(dict[str, object], providers).values():
                _drop_keys(value)


def _permissions(document: dict[str, Any]) -> None:
    _drop_keys(document.get("clis"))


def _review(document: dict[str, Any]) -> None:
    if "seats" in document:
        document["seats"] = _seat_map(document["seats"])


def _orchestration(document: dict[str, Any]) -> None:
    for group in get_args(OrchestrationGroup.__value__):
        if group in document:
            document[group] = _seat_map(document[group])
    if "image_generation" in document:
        seat = _seat(document["image_generation"])
        if seat is None:
            del document["image_generation"]
        else:
            document["image_generation"] = seat


def _decision_seats(document: dict[str, Any]) -> None:
    seats = document.get("seats")
    if isinstance(seats, list):
        kept = [_seat(seat) for seat in cast(list[object], seats)]
        document["seats"] = [seat for seat in kept if seat is not None]


_FILES: dict[str, Callable[[dict[str, Any]], None]] = {
    "members.json": _members,
    "permissions.json": _permissions,
    "review.json": _review,
    "orchestration.json": _orchestration,
    "decision-seats.json": _decision_seats,
}


def without_retired(name: str, content: str) -> str:
    """The App file's text with retired CLIs removed; unchanged when it names none."""
    clean = _FILES.get(name)
    if clean is None or not any(provider in content for provider in RETIRED_PROVIDERS):
        return content
    try:
        document = cast(object, json.loads(content))
    except ValueError:
        return content
    if not isinstance(document, dict):
        return content
    clean(cast(dict[str, Any], document))
    return json.dumps(document)
