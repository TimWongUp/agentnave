"""Project live undelivered events into the metadata-only inbox snapshot."""

from __future__ import annotations

from dataclasses import dataclass, field

from agentnave.conversations import Conversations
from agentnave.group import Round
from agentnave.inbox_snapshot import BossInbox, InboxItem
from agentnave.models import InvocationStatus


def _empty_items() -> list[InboxItem]:
    return []


@dataclass
class BossInboxState:
    """Raw per-boss inbox state before sequence tracking."""

    waiting: bool = False
    in_flight: int = 0
    items: list[InboxItem] = field(default_factory=_empty_items)


class InboxTracker:
    """Track per-boss monotonic event and delivery sequences without losing mid-wait wakes."""

    def __init__(self) -> None:
        self._event_seq: dict[str, int] = {}
        self._delivered_seq: dict[str, int] = {}
        self._item_seq: dict[str, dict[str, int]] = {}
        self._delivered_keys: dict[str, set[str]] = {}

    def _sync_boss(self, boss: str, items: list[InboxItem]) -> tuple[int, int]:
        seq = self._event_seq.get(boss, 0)
        item_seq = self._item_seq.setdefault(boss, {})
        current_keys: set[str] = set()
        for item in items:
            key = item["key"]
            current_keys.add(key)
            if key not in item_seq:
                seq += 1
                item_seq[key] = seq
        self._event_seq[boss] = seq
        for gone in list(item_seq):
            if gone not in current_keys:
                del item_seq[gone]
        delivered_keys = self._delivered_keys.get(boss, set())
        if delivered_keys:
            delivered_keys &= current_keys
            if not delivered_keys:
                self._delivered_keys.pop(boss, None)
        # Items that vanished undelivered (a prompt closed natively, a review decided
        # directly) no longer need a wake, so delivery catches up to the oldest pending one.
        pending = [item_seq[key] for key in current_keys if key not in delivered_keys]
        floor = min(pending) - 1 if pending else seq
        delivered = max(self._delivered_seq.get(boss, 0), floor)
        self._delivered_seq[boss] = delivered
        return seq, delivered

    def _mark_delivered_items(self, boss: str, items: list[InboxItem]) -> list[InboxItem]:
        delivered_keys = self._delivered_keys.get(boss, set())
        return [
            {**item, "delivered": True} if item["key"] in delivered_keys else item for item in items
        ]

    def project(self, states: dict[str, BossInboxState]) -> dict[str, BossInbox]:
        for boss in list(self._item_seq):
            if boss not in states:
                self._item_seq[boss].clear()
                self._delivered_keys.pop(boss, None)
        bosses: dict[str, BossInbox] = {}
        for boss, state in states.items():
            event_seq, delivered_seq = self._sync_boss(boss, state.items)
            if state.waiting or state.items or state.in_flight > 0:
                bosses[boss] = {
                    "waiting": state.waiting,
                    "in_flight": state.in_flight,
                    "event_seq": event_seq,
                    "delivered_seq": delivered_seq,
                    "items": self._mark_delivered_items(boss, state.items),
                }
        return bosses

    def mark_delivered(
        self,
        states: dict[str, BossInboxState],
        bosses: set[str],
        delivered_review_keys: set[str] | None = None,
    ) -> dict[str, BossInbox]:
        """Advance delivered_seq for bosses that just received ready items.

        Any still-undelivered item (for instance an event that arrived during wait, or an
        unrequested inbox item) keeps event_seq > delivered_seq so watchers never miss a wake.
        """
        for boss, state in states.items():
            self._sync_boss(boss, state.items)
        review_keys = delivered_review_keys or set()
        for boss in bosses:
            state = states.get(boss)
            current_items = state.items if state is not None else []
            current_keys = {item["key"] for item in current_items}
            if matched_reviews := review_keys & current_keys:
                self._delivered_keys.setdefault(boss, set()).update(matched_reviews)
            delivered_keys = self._delivered_keys.get(boss, set())
            item_seq = self._item_seq.get(boss, {})
            undelivered = [
                item_seq[key]
                for key in current_keys
                if key not in delivered_keys and key in item_seq
            ]
            current_delivered = self._delivered_seq.get(boss, 0)
            if undelivered:
                self._delivered_seq[boss] = max(current_delivered, min(undelivered) - 1)
            else:
                self._delivered_seq[boss] = max(current_delivered, self._event_seq.get(boss, 0))
        return self.project(states)


def collect_inbox_state(
    conversations: Conversations, waiting: set[str | None], cancelling: set[str]
) -> dict[str, BossInboxState]:
    states: dict[str, BossInboxState] = {}
    # Workspaces can be handed over, so their IDs are not necessarily the boss chat key.
    owners = conversations._bosses  # pyright: ignore[reportPrivateUsage]
    listings = {item.invocation_id: item for item in conversations.manager.list_invocations()}
    for workspace, (boss, _) in list(owners.items()):
        entry = states.setdefault(boss, BossInboxState())
        if workspace in waiting:
            entry.waiting = True
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
            if listing.status is None:
                entry.in_flight += 1
            role = task.role if task else None
            entry.items.extend(task_events(conversations, handle, role, listing.status))
        rounds = {**conversations._completed_rounds, **conversations._rounds}  # pyright: ignore[reportPrivateUsage]
        for handle, round_ in rounds.items():
            record = conversations.records.get(round_.group.id)
            if (
                record is not None
                and record.state.manager_session == workspace
                and handle not in conversations.delivered_rounds
                and handle not in cancelling
            ):
                if not round_.finished and not (round_.cancelling and not round_.error):
                    entry.in_flight += 1
                entry.items.extend(round_events(conversations, round_))
    for entry in states.values():
        entry.items.sort(key=lambda item: item["key"])
    return states


def inbox_events(
    conversations: Conversations, waiting: set[str | None], cancelling: set[str]
) -> dict[str, BossInbox]:
    return InboxTracker().project(collect_inbox_state(conversations, waiting, cancelling))


def _item(key: str, kind: str, role: str | None, status: str) -> InboxItem:
    return {"key": key, "kind": kind, "role": role, "status": status}


def review_event_key(round_id: str, seat_id: str, attempt: int) -> str:
    return f"review:{round_id}:{seat_id}:{seat_id}:{attempt}"


def task_events(
    conversations: Conversations, handle: str, role: str | None, status: InvocationStatus | None
) -> list[InboxItem]:
    """Undelivered events of one task invocation: its end, or fresh prompts and blockers."""
    manager = conversations.manager
    if status is not None:
        return [_item(f"task_end:{handle}", "task_end", role, status.value)]
    items = [
        _item(f"prompt:{handle}:{prompt.prompt_id}", "prompt", role, "prompt_pending")
        for prompt in manager.pending_prompts(handle)
        if not prompt.notified
    ]
    items.extend(
        _item(f"blocked:{handle}:{code}", "blocked", role, "execution_blocked")
        for code in manager.unread_blockers(handle)
    )
    return items


def round_events(conversations: Conversations, round_: Round) -> list[InboxItem]:
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
                _item(
                    review_event_key(handle, run.seat_id, run.attempt),
                    "review",
                    role,
                    "review_needed",
                )
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
