"""Shapes of the MCP tool replies; the server fills them, the published output schemas show them."""

from __future__ import annotations

from typing import Literal, NotRequired, TypedDict

from pydantic import BaseModel

from agentnave.members import MemberList, ProviderName
from agentnave.store import Unavailable


class ProviderDetails(BaseModel):
    provider: ProviderName
    permitted: bool  # start_agent; host exclusions apply
    show_permitted: bool  # run_round seats; discussion member limits apply
    supported_options: str


class MemberListPayload(MemberList):
    provider_details: ProviderDetails | None = None


class InvocationErrorPayload(TypedDict):
    code: str
    message: str


class ActivityPayload(TypedDict):
    kind: str
    state: NotRequired[str]
    tool_name: NotRequired[str]
    age_ms: NotRequired[int]


class InvocationPayload(TypedDict):
    invocation_id: str
    status: Literal["running", "succeeded", "failed", "blocked", "cancelled"]
    reason: Literal["started", "wait_elapsed", "execution_blocked", "finished"]
    elapsed_ms: int
    provider: NotRequired[str]
    model: NotRequired[str]
    effort: NotRequired[str]
    defaults_applied: NotRequired[list[str]]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]
    message_id: NotRequired[str]  # terminal saved task, referencable by forward
    warnings: NotRequired[list[str]]


class UnreceivedPayload(TypedDict):
    id: str
    kind: Literal["task", "round"]
    # a task's end status or execution_blocked; a round's finished, failed, cancelled,
    # review_needed, execution_blocked or seat_lost
    status: str
    error: NotRequired[
        InvocationErrorPayload
    ]  # the oldest unread blocker of a running task or seat, or a lost seat's error


class TaskStartPayload(InvocationPayload):
    conversation_id: str
    access: str | None  # the tier this call declared; recorded only, never enforced
    unreceived: NotRequired[list[UnreceivedPayload]]


class InvocationListItem(TypedDict):
    invocation_id: str
    provider: str
    cwd: str
    status: Literal["running", "succeeded", "failed", "blocked", "cancelled"]
    elapsed_ms: int
    events_seen: bool
    phase: NotRequired[Literal["preparing", "running", "stopping"]]
    session_id: NotRequired[str]
    result_read: NotRequired[bool]
    error: NotRequired[InvocationErrorPayload]


class InvocationListPayload(TypedDict):
    invocations: list[InvocationListItem]
    restart_safe: bool


class GroupSeatPayload(TypedDict):
    id: str
    provider: str
    cwd: str
    model: str | None
    effort: str | None
    role: str | None
    access: str | None
    defaults_applied: list[str]


class SeatRunPayload(TypedDict):
    seat_id: str
    status: Literal[
        "pending",
        "running",
        "succeeded",
        "failed",
        "blocked",
        "cancelled",
        "not_started",
        "pending_review",
        "private",
    ]
    invocation_id: NotRequired[str]
    warnings: NotRequired[list[str]]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    output_returned: NotRequired[bool]
    session_id: NotRequired[str]


class CommittedEntryPayload(TypedDict):
    message_id: str
    kind: Literal["task", "answer", "absent", "forward", "host"]
    round: int
    seat: NotRequired[str]


class RoundPayload(TypedDict):
    round_id: str
    group_id: str
    round: int
    order: Literal["parallel", "serial"]
    status: Literal["running", "finished", "failed", "cancelled"]
    reason: str
    elapsed_ms: int
    seats: list[SeatRunPayload]
    entries: list[CommittedEntryPayload]
    error: NotRequired[str]


class WaitPayload(TypedDict):
    reason: Literal["ready", "wait_elapsed", "nothing_pending"]
    pending_ids: list[str]
    all_finished: bool
    updates: list[InvocationPayload]
    rounds: list[RoundPayload]


class CancelPayload(TypedDict):
    invocation: NotRequired[InvocationPayload]
    round: NotRequired[RoundPayload]


class ShowRunPayload(TypedDict):
    group_id: str
    members: NotRequired[list[GroupSeatPayload]]  # only when this call opened the meeting
    entries: NotRequired[list[CommittedEntryPayload]]  # a host remark posted without seats
    round: NotRequired[RoundPayload]
    unreceived: NotRequired[list[UnreceivedPayload]]
    warnings: NotRequired[list[str]]


class ConversationSummary(TypedDict):
    conversation_id: str
    title: str
    kind: Literal["show", "task"]
    writable: bool
    created: str
    updated: str


class ListPayload(TypedDict):
    invocations: list[InvocationListItem]
    conversations: list[ConversationSummary]
    total: int
    unavailable: list[Unavailable]
    dashboard_url: NotRequired[str]
    dashboard_error: NotRequired[str]
