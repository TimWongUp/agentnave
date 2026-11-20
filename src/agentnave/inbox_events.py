"""Project live undelivered events into the App's metadata-only inbox."""

from __future__ import annotations

from agentnave.conversations import Conversations
from agentnave.group import Round
from agentnave.inbox_snapshot import BossInbox, InboxItem


def inbox_events(
    conversations: Conversations, waiting: set[str | None], cancelling: set[str]
) -> dict[str, BossInbox]:
    bosses: dict[str, BossInbox] = {}
    # Workspaces can be handed over, so their IDs are not necessarily the boss chat key.
    owners = conversations._bosses  # pyright: ignore[reportPrivateUsage]
    listings = {item.invocation_id: item for item in conversations.manager.list_invocations()}
    for workspace, (boss, _) in list(owners.items()):
        items: list[InboxItem] = []
        for handle, (conversation_id, task_id) in list(conversations.task_invocations.items()):
            record = conversations.records.get(conversation_id)
            listing = listings.get(handle)
            if (
                record is None
                or record.state.manager_session != workspace
                or listing is None
                or handle in conversations.delivered_tasks
                or handle in cancelling
            ):
                continue
            task = next((task for task in record.state.tasks if task.id == task_id), None)
            role = task.role if task else None
            if listing.status is not None:
                items.append(_item(f"task_end:{handle}", "task_end", role, listing.status.value))
            else:
                for prompt in conversations.manager.pending_prompts(handle):
                    if not prompt.notified:
                        items.append(
                            _item(
                                f"prompt:{handle}:{prompt.prompt_id}",
                                "prompt",
                                role,
                                "prompt_pending",
                            )
                        )
                for code in conversations.manager.unread_blockers(handle):
                    items.append(
                        _item(f"blocked:{handle}:{code}", "blocked", role, "execution_blocked")
                    )
        rounds = {**conversations._completed_rounds, **conversations._rounds}  # pyright: ignore[reportPrivateUsage]
        for handle, round_ in rounds.items():
            record = conversations.records.get(round_.group.id)
            if (
                record is not None
                and record.state.manager_session == workspace
                and handle not in conversations.delivered_rounds
                and handle not in cancelling
            ):
                items.extend(_round_events(conversations, round_))
        items.sort(key=lambda item: item["key"])
        is_waiting = workspace in waiting
        if items or is_waiting:
            bosses[boss] = {"waiting": is_waiting, "items": items}
    return bosses


def _item(key: str, kind: str, role: str | None, status: str) -> InboxItem:
    return {"key": key, "kind": kind, "role": role, "status": status}


def _round_events(conversations: Conversations, round_: Round) -> list[InboxItem]:
    handle = round_.id
    if round_.finished:
        if round_.cancelling and not round_.error:
            return []
        status = "failed" if round_.error else "finished"
        return [_item(f"task_end:{handle}", "task_end", None, status)]
    if round_.cancelling and not round_.error:
        return []
    items: list[InboxItem] = []
    reviews = conversations.reviews.get(handle, {})
    lost = {run.seat_id for run in round_.unreported_lost()}
    for run in round_.runs:
        role = round_.group.seats[run.seat_id].role
        attempt = f"{run.seat_id}:{run.attempt}"
        if reviews.get(run.seat_id) == "pending":
            items.append(
                _item(f"review:{handle}:{run.seat_id}:{attempt}", "review", role, "review_needed")
            )
        if run.status == "running":
            for prompt in run.prompts:
                if prompt.status == "pending" and prompt.prompt_id not in run.reported_prompts:
                    items.append(
                        _item(
                            f"prompt:{handle}:{attempt}:{prompt.prompt_id}",
                            "prompt",
                            role,
                            "prompt_pending",
                        )
                    )
            for index in range(run.reported_blockers, len(run.blockers)):
                items.append(
                    _item(
                        f"blocked:{handle}:{attempt}:{index}", "blocked", role, "execution_blocked"
                    )
                )
        if run.seat_id in lost:
            items.append(
                _item(f"seat_lost:{handle}:{run.seat_id}:{attempt}", "seat_lost", role, run.status)
            )
    return items
