"""Shared background-task waiting reminder and cancel-settlement helpers (ADR-0114)."""

from __future__ import annotations

import time
from collections.abc import Sequence
from dataclasses import dataclass
from typing import cast

from agentnave.models import InvocationError

BACKGROUND_TASKS_PENDING = "background_tasks_pending"
BACKGROUND_WAIT_REMINDER_SECONDS = 30.0 * 60.0
_MAX_TASK_IDS_IN_MESSAGE = 5
_MAX_TASK_ID_CHARS = 64


@dataclass(frozen=True, slots=True)
class BackgroundTasksState:
    """Normalized background-task waiting snapshot reported by a helper or runtime.

    ``waiting`` drives the 30-minute reminder timer (reply already written while this
    turn's background work is still running). ``only_background`` marks that the main
    turn is idle and only background tasks remain, so ``cancel`` can settle directly
    with ``reply`` without sending keys to the terminal.
    """

    waiting: bool
    only_background: bool = False
    task_ids: tuple[str, ...] = ()
    reply: str | None = None
    session_id: str | None = None
    model: str | None = None


def _clean_task_ids(task_ids: Sequence[str]) -> tuple[str, ...]:
    cleaned: list[str] = []
    seen: set[str] = set()
    for raw in task_ids:
        item = " ".join(raw.split())[:_MAX_TASK_ID_CHARS]
        if item and item not in seen:
            seen.add(item)
            cleaned.append(item)
    return tuple(cleaned)


def _describe_tasks(task_ids: Sequence[str], count: int | None = None) -> str:
    cleaned = _clean_task_ids(task_ids)
    effective_count = count if count is not None and count > 0 else len(cleaned)
    if cleaned:
        shown = ", ".join(cleaned[:_MAX_TASK_IDS_IN_MESSAGE])
        if len(cleaned) > _MAX_TASK_IDS_IN_MESSAGE:
            shown += f", +{len(cleaned) - _MAX_TASK_IDS_IN_MESSAGE} more"
        return f"{effective_count} background task(s) ({shown})"
    if effective_count > 0:
        return f"{effective_count} background task(s)"
    return "background task(s)"


def background_tasks_pending_error(
    task_ids: Sequence[str] = (), *, count: int | None = None
) -> InvocationError:
    """Build the metadata-only execution_blocked error (never includes reply body)."""
    summary = _describe_tasks(task_ids, count=count)
    return InvocationError(
        BACKGROUND_TASKS_PENDING,
        f"Reply already written, still waiting for {summary}. "
        "Wait for them to finish, or cancel to settle with the reply already written.",
    )


def background_cancel_warning(task_ids: Sequence[str] = ()) -> str:
    """Explain settlement and background-task disposition when cancelling a helper."""
    summary = _describe_tasks(task_ids)
    return (
        f"Cancelled while only {summary} remained; settled with the reply already written "
        "and terminated the helper process group."
    )


def parse_background_tasks_event(event: dict[str, object]) -> BackgroundTasksState:
    """Decode the helper-emitted agentnave_background_tasks event into a state snapshot."""
    raw_ids = event.get("task_ids")
    ids: list[str] = []
    if isinstance(raw_ids, list):
        for item in cast(list[object], raw_ids):
            if isinstance(item, str) and item:
                ids.append(item)
    raw_reply = event.get("reply")
    reply = raw_reply if isinstance(raw_reply, str) and raw_reply.strip() else None
    raw_session = event.get("session_id") or event.get("conversation_id")
    session_id = raw_session if isinstance(raw_session, str) and raw_session else None
    raw_model = event.get("model")
    model = raw_model if isinstance(raw_model, str) and raw_model else None
    waiting = bool(event.get("waiting"))
    only_background = waiting and bool(event.get("only_background")) and reply is not None
    return BackgroundTasksState(
        waiting=waiting,
        only_background=only_background,
        task_ids=_clean_task_ids(ids),
        reply=reply if only_background else None,
        session_id=session_id,
        model=model,
    )


class BackgroundWaitTracker:
    """Track when a run has been waiting on background tasks after writing a reply.

    Starts timing once ``waiting`` becomes True, resets only when the active background
    task set changes (a task starts or finishes; intermediate Monitor events leave the
    set unchanged and do not reset the timer), and fires at most once per invocation.
    """

    def __init__(self) -> None:
        self.waiting: bool = False
        self.waiting_since: float | None = None
        self.task_ids: tuple[str, ...] = ()
        self._task_set: frozenset[str] = frozenset()
        self.reminded: bool = False

    def update(self, state: BackgroundTasksState, now: float | None = None) -> bool:
        """Apply a new state snapshot; return True when the reminder deadline changed."""
        current = time.monotonic() if now is None else now
        if not state.waiting:
            changed = self.waiting or self.waiting_since is not None
            self.waiting = False
            self.waiting_since = None
            self.task_ids = ()
            self._task_set = frozenset()
            return changed
        new_set = frozenset(state.task_ids)
        if not self.waiting:
            self.waiting = True
            self.waiting_since = current
            self.task_ids = state.task_ids
            self._task_set = new_set
            return True
        self.task_ids = state.task_ids
        if new_set != self._task_set:
            self._task_set = new_set
            self.waiting_since = current
            return True
        return False

    def remaining_seconds(self, threshold: float, now: float | None = None) -> float | None:
        """Seconds until reminder fires, or None when not waiting or already reminded."""
        if self.reminded or not self.waiting or self.waiting_since is None:
            return None
        current = time.monotonic() if now is None else now
        return max(0.0, self.waiting_since + threshold - current)

    def fire(self) -> BackgroundTasksState | None:
        """Mark the one-shot reminder as fired and return the current state, if still waiting."""
        if self.reminded or not self.waiting:
            return None
        self.reminded = True
        return BackgroundTasksState(waiting=True, task_ids=self.task_ids)
