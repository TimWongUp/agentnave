"""A seat choice whose account is full when the new thread takes its slot (ADR-0127).

Choosing skips full accounts from a snapshot, and another server may take the last slot before
this one does; a thread the seat chose then chooses again. A CLI or account the caller named,
and a resumed session, are never switched.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable

from mcp.server.mcpserver.exceptions import ToolError

from agentnave.account_limits import AccountCapacityError
from agentnave.host import Host
from agentnave.members import role_seat


def selectable_choices(seat_name: str, host: Host, *, explicit: bool) -> int:
    """How many configured choices a start may go through; 1 when nothing may be switched."""
    configured = None if explicit else role_seat(seat_name, host)
    return 1 + len(configured.fallbacks) if configured else 1


async def with_reselection[T](start: Callable[[], Awaitable[T]], choices: int) -> T:
    """Run start, which selects from a fresh snapshot each time, until a slot is taken, at
    most once per configured choice."""
    attempt = 1
    while True:
        try:
            return await start()
        except AccountCapacityError as exc:
            if attempt >= choices:
                exhausted = ". No other choice of this seat is available." if choices > 1 else ""
                raise ToolError(f"{exc}{exhausted}") from exc
            attempt += 1
