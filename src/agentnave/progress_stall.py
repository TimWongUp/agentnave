"""One-shot reminder for a running task whose CLI shows no substantive progress (ADR-0116)."""

from __future__ import annotations

import asyncio
import time
from collections.abc import Callable

from agentnave.models import InvocationError

PROGRESS_STALLED = "progress_stalled"
PROGRESS_STALL_SECONDS = 6.0 * 60.0
_MAX_POLL_SECONDS = 5.0


class ProgressTracker:
    """When the CLI last made substantive progress, and whether this silence was reported.

    The Adapter decides which events count; the tracker only keeps the clock. A new substantive
    event restarts the clock and re-arms the reminder, so one silence is reported once.
    """

    def __init__(self) -> None:
        self.at: float | None = None
        self.reminded = False

    def note(self, now: float | None = None) -> None:
        self.at = time.monotonic() if now is None else now
        self.reminded = False

    def hold(self, now: float | None = None) -> None:
        """Restart the clock without re-arming a reminder already sent for this silence."""
        self.at = time.monotonic() if now is None else now

    def remaining(self, threshold: float, now: float | None = None) -> float | None:
        """Seconds until the reminder is due, or None once it was sent for this silence."""
        if self.reminded:
            return None
        current = time.monotonic() if now is None else now
        if self.at is None:
            self.at = current
        return max(0.0, self.at + threshold - current)

    def fire(self, now: float | None = None) -> float:
        """Mark this silence as reported and return how long it has lasted."""
        current = time.monotonic() if now is None else now
        self.reminded = True
        return max(0.0, current - (self.at if self.at is not None else current))


def progress_stalled_error(
    quiet_seconds: float, doing: str | None, last_tool: str | None
) -> InvocationError:
    """Metadata-only execution_blocked error; never includes the CLI's output."""
    activity = f"currently {doing}" if doing else "no activity reported yet"
    if last_tool:
        activity += f"; last tool: {last_tool}"
    return InvocationError(
        PROGRESS_STALLED,
        f"No substantive progress for {round(quiet_seconds / 60)} minutes ({activity}). The task "
        "is still running: it may be thinking, waiting on a retry, or stuck. Read the output "
        "tail with wait(wait_seconds=0), then keep waiting or cancel and resume the session.",
    )


async def progress_stall_guard(
    tracker: ProgressTracker,
    threshold: float,
    counting: Callable[[], bool],
    notify: Callable[[float], bool],
) -> None:
    """Call notify(quiet_seconds) once the CLI has been silent for threshold seconds.

    While counting() is false (a pending native prompt, a background-task wait with its own
    reminder) the clock keeps restarting, so the silence is measured from when it ends.
    """
    interval = min(_MAX_POLL_SECONDS, threshold / 4)
    while True:
        if not counting():
            tracker.hold()
            await asyncio.sleep(interval)
            continue
        remaining = tracker.remaining(threshold)
        if remaining is None:
            await asyncio.sleep(interval)
        elif remaining <= 0:
            notify(tracker.fire())
        else:
            await asyncio.sleep(min(remaining, interval))
