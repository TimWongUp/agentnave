"""Availability and ordered selection of the choices saved for an App seat."""

from __future__ import annotations

import random
from collections.abc import Callable, Collection
from typing import TYPE_CHECKING, Literal, NotRequired, TypedDict

from agentnave import provider_health
from agentnave.host import Host
from agentnave.orchestration_roles import FallbackStatus, ProviderName, ReviewSeat, ReviewSeatStatus

if TYPE_CHECKING:
    from agentnave.members import MemberFile, ModeStatus


IMAGE_HOST_CLI_REASON = "Image CLI is the host's own CLI"


class SkippedChoice(TypedDict):
    provider: str
    model: str
    reason: str


class SeatFallback(TypedDict):
    position: int
    skipped: list[SkippedChoice]
    random: NotRequired[Literal[True]]


def choose_index(count: int) -> int:
    """Uniform random index in range(count); separated so tests can fix draws."""
    return random.randrange(count)


def statuses[Role: str](
    seats: dict[Role, ReviewSeat],
    mode: ModeStatus,
    excluded: frozenset[str],
    host: Host,
    installed: Callable[[str], bool],
    document: MemberFile | None,
    health: dict[tuple[str, str | None], provider_health.Health],
    *,
    image_roles: frozenset[str] = frozenset(),
) -> dict[Role, ReviewSeatStatus]:
    from agentnave.members import single_review_status

    result: dict[Role, ReviewSeatStatus] = {}
    for role, seat in seats.items():
        choices = [
            ReviewSeat(provider=c.provider, model=c.model, effort=c.effort)
            for c in [seat, *seat.fallbacks]
        ]
        items = [
            single_review_status({role: choice}, mode, excluded, host, installed, document, health)[
                role
            ]
            for choice in choices
        ]
        if role == "image-generator" or role in image_roles:
            for choice, item in zip(choices, items, strict=True):
                if choice.provider == host.provider:
                    item.available = False
                    item.reason = IMAGE_HOST_CLI_REASON
                    item.native = None
        position = next((i for i, item in enumerate(items) if item.reason is None), 0)
        selected = items[position].model_copy(deep=True)
        selected.fallback_position = position or None
        selected.fallbacks = [
            FallbackStatus(
                provider=c.provider,
                model=c.model,
                effort=c.effort,
                available=item.reason is None,
                reason=item.reason,
            )
            for c, item in zip(choices[1:], items[1:], strict=True)
        ]
        result[role] = selected
    return result


def select(
    seat: ReviewSeat,
    host: Host,
    excluded: frozenset[str],
    provider: ProviderName | None = None,
    *,
    image: bool = False,
    randomize: bool = False,
    in_use: Collection[tuple[str, str]] = (),
    randrange: Callable[[int], int] | None = None,
) -> tuple[ReviewSeat, SeatFallback | None]:
    """Pick a usable configured choice; leave failures to the existing dispatch path."""
    if provider is not None:
        return seat, None
    from agentnave.adapters import get_adapter
    from agentnave.members import (
        _member_modes,  # pyright: ignore[reportPrivateUsage]
        data_root,
        orchestration_enabled,
        read_members,
        single_review_status,
    )

    document = read_members()

    def installed(name: str) -> bool:
        return get_adapter(name).is_installed()

    mode = _member_modes(document, excluded, installed, cli_only=orchestration_enabled(host))[
        "task"
    ]
    health = provider_health.active(data_root())
    choices = [
        ReviewSeat(provider=c.provider, model=c.model, effort=c.effort)
        for c in [seat, *seat.fallbacks]
    ]
    skipped: list[SkippedChoice] = []
    usable: list[tuple[int, ReviewSeat]] = []
    for index, choice in enumerate(choices):
        status = single_review_status(
            {"seat": choice}, mode, excluded, host, installed, document, health
        )["seat"]
        if image and choice.provider == host.provider:
            status.reason = IMAGE_HOST_CLI_REASON
        if status.reason is None:
            if not randomize:
                receipt = SeatFallback(position=index, skipped=skipped) if index else None
                return choice, receipt
            usable.append((index, choice))
        else:
            skipped.append(
                {"provider": choice.provider, "model": choice.model, "reason": status.reason}
            )
    if not usable:
        return seat, None
    if len(usable) == 1:
        index, choice = usable[0]
        return choice, SeatFallback(position=index, skipped=skipped) if index else None
    busy = set(in_use)
    pool = [item for item in usable if (item[1].provider, item[1].model) not in busy] or usable
    index, choice = pool[0] if len(pool) == 1 else pool[(randrange or choose_index)(len(pool))]
    return choice, SeatFallback(position=index, skipped=skipped, random=True)
