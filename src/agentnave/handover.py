"""Workspace handover links: forward source labels and one-time old-boss warning (ADR-0099)."""

from __future__ import annotations

from collections.abc import Iterable
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from agentnave.db import Database, WorkspaceLink
from agentnave.store import Record, TaskRecord

if TYPE_CHECKING:
    from agentnave.conversations import Conversations


def _iso_us(value: str | None) -> int | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(value)
        if parsed.tzinfo is None:
            return None
        delta = parsed - datetime(1970, 1, 1, tzinfo=UTC)
        return (delta.days * 86400 + delta.seconds) * 1_000_000 + delta.microseconds
    except (ValueError, OverflowError, OSError):
        return None


def linked_source_prefix(records: Iterable[Record], db: Database, source_workspace: str) -> str:
    """Prefix added to a forwarded message's source when it comes from a linked workspace."""
    members = [r.state for r in records if r.state.manager_session == source_workspace]
    title: str | None = None
    if members:
        title = min(members, key=lambda s: (s.created, s.id)).title
    elif (row := db.workspace(source_workspace)) is not None:
        title = row.title
    cleaned = (title or "").strip()
    return (
        f"workspace {source_workspace} ({cleaned}) · "
        if cleaned
        else f"workspace {source_workspace} · "
    )


def _latest_claim(claims: list[WorkspaceLink]) -> tuple[WorkspaceLink, int] | None:
    dated = [(claim, ts) for claim in claims if (ts := _iso_us(claim.claimed_at)) is not None]
    return max(dated, key=lambda item: item[1], default=None)


def handover_dispatch_warning(
    conversations: Conversations,
    workspace_id: str | None,
    *,
    current_task: TaskRecord | None = None,
    current_round: tuple[str, int] | None = None,
) -> list[str]:
    """Warn the source boss once on its first dispatch after its workspace was claimed."""
    if workspace_id is None:
        return []
    found = _latest_claim(conversations.db.claims_from(workspace_id))
    if found is None:
        return []
    claim, claimed_us = found
    conversations.scan()
    current_us: int | None = _iso_us(current_task.time) if current_task is not None else None
    previous_us: list[int] = []
    for record in conversations.records.values():
        state = record.state
        if state.manager_session != workspace_id:
            continue
        if state.kind == "task":
            for task in state.tasks:
                if task.error == "start_failed":
                    continue
                if current_task is not None and task.id == current_task.id:
                    continue
                if (ts := _iso_us(task.time)) is not None:
                    previous_us.append(ts)
        else:
            for entry in state.entries:
                if entry.kind != "task":
                    continue
                if current_round is not None and (state.id, entry.round) == current_round:
                    current_us = _iso_us(entry.time)
                    continue
                if (ts := _iso_us(entry.time)) is not None:
                    previous_us.append(ts)
    if current_us is None or current_us < claimed_us or any(ts >= claimed_us for ts in previous_us):
        return []
    who = f"the {claim.target_host} boss" if claim.target_host else "another boss"
    return [
        f"Workspace {workspace_id} was claimed by {who} ({claim.claimed_at}); "
        "check the working tree and git status before making overlapping changes."
    ]
