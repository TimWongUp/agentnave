"""The user's own seat catalog while it is effective for a boss (ADR-0087).

It is effective only while the boss's mode in orchestration.json is decision; in any other mode
everything stays as without the file. In that mode a missing file is an empty catalog.
Imports members at load; members reaches this module through a late import.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import get_args

from agentnave import provider_health
from agentnave.host import Host
from agentnave.members import (
    MemberFile,
    MemberFileError,
    ModeStatus,
    ReviewRole,
    _read_app_file,  # pyright: ignore[reportPrivateUsage]
    _repair,  # pyright: ignore[reportPrivateUsage]
    _review_status,  # pyright: ignore[reportPrivateUsage]
    orchestration_decision,
)
from agentnave.orchestration_roles import (
    IMAGE_KIND,
    CustomSeat,
    CustomSeatStatus,
    DecisionSeatsFile,
    OrchestrationStatus,
    ReviewSeatStatus,
)


class DecisionSeatsFileError(MemberFileError):
    """decision-seats.json cannot be read or validated."""


def read_decision_seats() -> DecisionSeatsFile | None:
    """decision-seats.json; None when missing."""
    document = _read_app_file("decision-seats.json", DecisionSeatsFile, DecisionSeatsFileError)
    for seat in document.seats if document is not None else ():
        if seat.id in get_args(ReviewRole.__value__):
            raise DecisionSeatsFileError(
                f"Invalid decision-seats.json: seat id {seat.id!r} is reserved for a review role. "
                + _repair("decision-seats.json")
            )
    return document


def effective_decision_seats(host: Host) -> DecisionSeatsFile | None:
    """The file while the catalog is effective for this boss, else None.

    A broken file raises DecisionSeatsFileError.
    """
    if not orchestration_decision(host):
        return None
    return read_decision_seats() or DecisionSeatsFile(version=1)


def decision_seats_for(host: Host) -> dict[str, CustomSeat] | None:
    """The user's own seats by id while the catalog is effective for this boss, else None."""
    document = effective_decision_seats(host)
    return (
        None
        if document is None
        else {
            seat.id: seat
            for seat in document.seats
            if seat.kind != IMAGE_KIND
            or any(choice.provider != host.provider for choice in [seat, *seat.fallbacks])
        }
    )


def custom_seats(host: Host) -> dict[str, CustomSeat] | None:
    """decision_seats_for for paths that must keep working while list_members reports the file."""
    try:
        return decision_seats_for(host)
    except DecisionSeatsFileError:
        return None


def custom_seat(name: str, host: Host) -> CustomSeat | None:
    return (custom_seats(host) or {}).get(name)


def custom_orchestration_status(
    custom: dict[str, CustomSeat],
    task_mode: ModeStatus,
    excluded: frozenset[str],
    host: Host,
    installed: Callable[[str], bool],
    document: MemberFile | None,
    active_health: dict[tuple[str, str | None], provider_health.Health],
) -> OrchestrationStatus:
    """The user's own seats instead of the fixed groups and the fixed image seat."""
    statuses = _review_status(
        {seat.id: seat.review_seat() for seat in custom.values()},
        task_mode,
        excluded,
        host,
        installed,
        document,
        active_health,
        image_roles=frozenset(seat.id for seat in custom.values() if seat.kind == IMAGE_KIND),
    )
    return OrchestrationStatus(
        enabled=True,
        custom_seats={
            seat.id: CustomSeatStatus(
                **{
                    name: getattr(statuses[seat.id], name) for name in ReviewSeatStatus.model_fields
                },
                kind=seat.kind,
                role=seat.role,
                description=seat.description,
                access=seat.access,
                difficulty=seat.difficulty,
            )
            for seat in custom.values()
        },
    )
