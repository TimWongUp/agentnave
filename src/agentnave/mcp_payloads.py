"""Shapes of the MCP tool replies; the server fills them, the published output schemas show them."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Literal, NotRequired, TypedDict

from pydantic import BaseModel

from agentnave.members import MemberList, ProviderName
from agentnave.native_prompts import PromptPayload
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


# Replies carry the projected identity: handle and model_source only (see mcp_projection).
type ReplyIdentity = Mapping[str, object]


class ActivityPayload(TypedDict):
    kind: str
    state: NotRequired[str]
    tool_name: NotRequired[str]
    age_ms: NotRequired[int]


class TaskIsolationPayload(TypedDict):
    id: str
    path: str
    cwd: str
    branch: str
    base_oid: NotRequired[str]


class TaskIsolationSummaryPayload(TypedDict):
    id: str
    path: str
    branch: str
    summary_status: Literal["complete", "no_baseline", "error"]
    nested_repos: NotRequired[list[str]]  # their contents were not checked
    head_oid: NotRequired[str]
    actual_branch: NotRequired[str]
    changed_files_from_base: NotRequired[list[str]]
    truncated: NotRequired[bool]
    uncommitted: NotRequired[bool]
    error: NotRequired[str]


class InvocationPayload(TypedDict):
    identity: NotRequired[ReplyIdentity]
    invocation_id: str
    status: Literal["running", "succeeded", "failed", "blocked", "cancelled"]
    reason: Literal["started", "wait_elapsed", "execution_blocked", "prompt_pending", "finished"]
    elapsed_ms: NotRequired[int]
    provider: NotRequired[str]
    model: NotRequired[str]
    effort: NotRequired[str]
    defaults_applied: NotRequired[list[str]]
    isolation: NotRequired[TaskIsolationPayload | TaskIsolationSummaryPayload]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    prompts: NotRequired[list[PromptPayload]]
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
    access: NotRequired[str | None]  # the declared tier; recorded only, never enforced
    unreleased_isolation_count: NotRequired[int]
    unreceived: NotRequired[list[UnreceivedPayload]]
    next: NotRequired[str]  # what to call before ending the turn while ends wait (ADR-0064)
    wait: NotRequired[WaitPayload]  # the wait this dispatch ran when asked to


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
    identity: NotRequired[ReplyIdentity]
    id: str
    provider: str
    cwd: str
    model: NotRequired[str | None]
    effort: NotRequired[str | None]
    role: NotRequired[str | None]
    access: NotRequired[str | None]
    defaults_applied: NotRequired[list[str]]


class SeatRunPayload(TypedDict):
    identity: NotRequired[ReplyIdentity]
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
    prompts: NotRequired[list[PromptPayload]]
    output_age_ms: NotRequired[int]
    output_returned: NotRequired[bool]
    session_id: NotRequired[str]


class CommittedEntryPayload(TypedDict):
    identity: NotRequired[ReplyIdentity]
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
    next: NotRequired[str]
    seq: NotRequired[int]


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
    next: NotRequired[str]
    wait: NotRequired[WaitPayload]


class ConversationSummary(TypedDict):
    conversation_id: str
    title: str
    kind: Literal["show", "task"]
    writable: bool
    created: str
    updated: str


class WorkspaceSummary(TypedDict):
    workspace_id: NotRequired[str]  # absent for conversations saved without a boss
    title: str
    host: NotRequired[str]
    mine: bool
    updated: str
    conversation_count: int
    latest: ConversationSummary  # its most recent conversation of the asked kind


class ListPayload(TypedDict):
    scope: Literal["mine", "all"]
    invocations: list[InvocationListItem]
    invocations_omitted: NotRequired[int]
    conversations: NotRequired[list[ConversationSummary]]  # scope="mine"
    workspaces: NotRequired[list[WorkspaceSummary]]  # scope="all"
    total: int
    unavailable: list[Unavailable]


class IsolatedDirItemPayload(TypedDict):
    id: str
    path: str
    cwd: str
    branch: str
    base_oid: str
    state: Literal["creating", "ready", "deleting", "needs_attention"]
    in_use: bool
    created_at: str
    own_workspace: bool
    attention: NotRequired[str]


class IsolatedDirsPayload(TypedDict):
    items: NotRequired[list[IsolatedDirItemPayload]]
    id: NotRequired[str]
    status: NotRequired[
        Literal["deleted", "confirmation_required", "blocked", "needs_attention", "not_found"]
    ]
    head_oid: NotRequired[str]
    uncommitted: NotRequired[bool]
    unique_commits: NotRequired[bool]
    confirmation: NotRequired[str]
    nested_repos: NotRequired[list[str]]  # their contents were not checked
    reason: NotRequired[str]
