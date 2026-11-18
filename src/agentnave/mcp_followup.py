"""wait 收口时的待跟进清单（ADR-0064 修订）：只列早先已交付、当前仍需 boss 处理的项，每项一次。"""

from __future__ import annotations

import asyncio

from agentnave.conversations import Conversations
from agentnave.isolated import IsolatedListItem
from agentnave.mcp_payloads import FollowUpPayload, WaitPayload

_MAX_ITEMS = 20
_LOST_SEATS = ("failed", "blocked")


async def settle_follow_up(
    conversations: Conversations,
    payload: WaitPayload,
    *,
    inbox: bool,
    manager_session_id: str | None,
) -> None:
    """先按账本给收口的 inbox wait 补清单，再把本次回执新交付的记进账本。

    顺序不能反：本次回执的 updates 里已经带了这些结果，不必在同一回执里再报一遍。
    显式 ids 的 wait 只记账，不出清单。"""
    if inbox and payload["all_finished"]:
        await _attach(conversations, payload, manager_session_id)
    _record(conversations, payload)


def _record(conversations: Conversations, payload: WaitPayload) -> None:
    ledger = conversations.follow_ups
    for update in payload["updates"]:
        handle = update["invocation_id"]
        status = update["status"]
        if status == "running":
            continue
        workspace = _task_workspace(conversations, handle)
        if status in ("failed", "blocked"):
            ledger.add("task", handle, workspace)
        isolation = update.get("isolation")
        if isolation is not None and not isolation.get("auto_released"):
            ledger.add("isolation", isolation["id"], workspace)
    for round_ in payload["rounds"]:
        if round_["status"] in ("finished", "failed"):
            record_id = conversations.round_conversation(round_["round_id"])
            record = conversations.records.get(record_id) if record_id is not None else None
            workspace = record.state.manager_session if record is not None else None
            ledger.add("round", round_["round_id"], workspace)


def _task_workspace(conversations: Conversations, invocation_id: str) -> str | None:
    found = conversations.task_invocations.get(invocation_id)
    record = conversations.records.get(found[0]) if found is not None else None
    return record.state.manager_session if record is not None else None


async def _attach(
    conversations: Conversations, payload: WaitPayload, manager_session_id: str | None
) -> None:
    try:
        workspace = conversations.workspace_id(manager_session_id)
    except ValueError:
        return  # a boss that handed its workspace over gets no list here
    if workspace is None:
        return
    ledger = conversations.follow_ups
    mine = [key for key, owner in ledger.entries.items() if owner == workspace]
    live: dict[str, IsolatedListItem] | None = {}
    if any(kind == "isolation" for kind, _ in mine):
        try:
            listed = await asyncio.to_thread(conversations.isolated.list_for_workspace, workspace)
            live = {item.id: item for item in listed}
        except (OSError, ValueError):
            live = None  # keep the directories queued; a later closing wait lists them
    items: list[FollowUpPayload] = []
    for kind, handle in mine:
        if kind == "isolation" and live is None:
            continue
        del ledger.entries[(kind, handle)]
        if kind == "task":
            item = _task_item(conversations, handle)
        elif kind == "round":
            item = _round_item(conversations, handle)
        else:
            item = _isolation_item(live or {}, handle)
        if item is not None:
            items.append(item)
    if items:
        payload["follow_up"] = items[:_MAX_ITEMS]
        if len(items) > _MAX_ITEMS:
            payload["follow_up_omitted"] = len(items) - _MAX_ITEMS


def _task_item(conversations: Conversations, invocation_id: str) -> FollowUpPayload | None:
    found = conversations.task_invocations.get(invocation_id)
    record = conversations.records.get(found[0]) if found is not None else None
    if found is None or record is None:
        return None
    task = next((item for item in record.state.tasks if item.id == found[1]), None)
    if task is None or task.status not in ("failed", "blocked"):
        return None
    workspace = record.state.manager_session
    if task.session_id and any(
        later.session_id == task.session_id and later.id != task.id and later.time >= task.time
        for other in conversations.records.values()
        if other.state.manager_session == workspace
        for later in other.state.tasks
    ):
        return None  # resumed: the thread continues in a newer task
    item: FollowUpPayload = {"id": invocation_id, "kind": "task", "status": task.status}
    if message_id := conversations.task_message_id(invocation_id):
        item["message_id"] = message_id
    if task.session_id:
        item["session_id"] = task.session_id
    return item


def _round_item(conversations: Conversations, round_id: str) -> FollowUpPayload | None:
    try:
        round_ = conversations.round(round_id)
    except KeyError:
        return None
    if round_.cancelling:
        return None
    if round_.error:
        return {"id": round_id, "kind": "round", "status": "failed", "group_id": round_.group.id}
    lost = [run.seat_id for run in round_.runs if run.status in _LOST_SEATS]
    if not lost:
        return None
    return {
        "id": round_id,
        "kind": "round",
        "status": "seat_lost",
        "group_id": round_.group.id,
        "seats": lost,
    }


def _isolation_item(live: dict[str, IsolatedListItem], isolation_id: str) -> FollowUpPayload | None:
    item = live.get(isolation_id)
    if item is None or item.in_use:
        return None
    return {
        "id": isolation_id,
        "kind": "isolation",
        "status": item.state,
        "path": item.path,
        "branch": item.branch,
    }
