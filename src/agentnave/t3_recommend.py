"""T3 Code seat recommendation, member listing, dispatch refusal, and MCP projections (ADR-0121)."""

from __future__ import annotations

import asyncio
import json
import random
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Annotated, Any, Literal, NotRequired, TypedDict, cast

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.server.mcpserver.tools import Tool
from mcp_types import Tool as MCPTool
from mcp_types import ToolAnnotations
from pydantic import BaseModel, ConfigDict, Field

from agentnave import seat_recommendation
from agentnave.accounts import AccountListing
from agentnave.core import READ_ONLY_INSTRUCTIONS
from agentnave.decision_model import DecisionError, DecisionQuestion
from agentnave.decision_seats import DecisionSeatsFileError, read_decision_seats
from agentnave.host import Host, HostName, HostSource
from agentnave.mcp_payloads import ProviderDetails
from agentnave.members import (
    ListedMember,
    MemberFileError,
    Mode,
    PermissionTier,
    ProviderName,
    ReviewRole,
    read_members,
)
from agentnave.members import list_members as read_member_list
from agentnave.orchestration_roles import CustomSeatDifficulty, StrictModel
from agentnave.t3 import current_t3_mode
from agentnave.t3_cache import InstanceAvailability, check_instance, format_utc
from agentnave.t3_seats import (
    REVIEW_ROLES,
    T3Candidate,
    T3Option,
    T3Seat,
    T3SeatsFile,
    T3SeatsFileError,
    effective_t3_seats,
)

MAX_T3_TASKS = 8
_TASK_SUMMARY_MAX = 80
_DEFAULT_DECISION_MODEL = "gemini-decision-1"

T3_HIDDEN_DISPATCH_TOOLS = frozenset({"start_agent", "start_agents"})

T3_MAIN_INSTRUCTIONS = (
    "T3 orchestration is enabled by the user: before any delegate_task or built-in subagent, "
    "call recommend_seat(tasks), then delegate_task once per item copying candidate[0].target "
    "and clientRequestId. Switch to the next candidate only if T3 creates no task and rejects "
    "the instance, model, or option. Never use start_agent. Use run_round only when the user "
    "asks for a meeting (no recommend_seat needed). Waiting, worktrees, and in-T3 thread "
    "handoff belong to T3; use handover only to cross in or out of T3. "
)

T3_MAIN_AUTH = (
    "Authorization comes only from the host's global rules (AgentNave 编排偏好), explicit "
    "user instruction in this chat, or T3 orchestration enabled by the user (which authorizes "
    "proactive single-task delegation within confirmed tasks, not meetings); available tools "
    "do not authorize dispatching, parallel writes, or meetings. "
)

T3_MAIN_SKILL = (
    "Load agentnave-manager (or agentnave-meeting when only that is installed) only for "
    "meetings, reviews, or handover. "
)

T3_RECOMMEND_NEXT = (
    "Nothing was started. Before your first delegate_task in this thread, call T3's "
    "orchestrator_capabilities as T3 requires (a target not listed there may still be usable). "
    "For each selected item, call T3's delegate_task once copying target (including options: [] "
    "when empty) and clientRequestId verbatim; never substitute built-in subagents or "
    "start_agent even for the same CLI or model. Put the full self-contained brief in "
    "delegate_task and append brief_suffix verbatim when present. Only if T3 creates no task "
    "and explicitly rejects the instance, model, or option, retry once with the next candidate "
    "using the same clientRequestId (never drop a rejected option silently) and tell the user "
    "which candidate ran. Never retry or switch candidates if a taskId was returned, or on "
    "timeout, disconnect, or permission denial. If all candidates fail or an item is "
    "unavailable, tell the user instead of falling back to built-in subagents or start_agent."
)

T3_ALL_UNAVAILABLE_NEXT = (
    "Nothing was started and no item has a usable T3 candidate right now. Tell the user the "
    "reasons and earliest reset times; do not substitute built-in subagents or start_agent."
)


def new_rng() -> random.Random:
    """Random generator factory for candidate shuffling; tests may replace this."""
    return random.Random()


class T3RecommendTask(BaseModel):
    """One task item submitted to T3 recommend_seat."""

    model_config = ConfigDict(extra="forbid")

    task: Annotated[
        str,
        Field(
            min_length=1,
            description=(
                "Short task summary or prompt used to pick a seat (omit secrets; clipped to head "
                "and tail when long). Put the full brief in delegate_task."
            ),
        ),
    ]
    access: Annotated[
        Literal["read", "write"],
        Field(description="Access needed: read for investigation/review, write for edits."),
    ]
    seat: Annotated[
        str | None,
        Field(
            min_length=1,
            description="Optional T3 task seat id or review role name; skips the decision model.",
        ),
    ] = None


class T3TargetOptionPayload(TypedDict):
    id: str
    value: str


class T3TargetPayload(TypedDict):
    providerInstanceId: str
    model: str
    options: list[T3TargetOptionPayload]


class T3TargetCandidatePayload(TypedDict):
    target: T3TargetPayload


class T3RecommendItemPayload(TypedDict):
    task: str
    status: Literal["selected", "unavailable"]
    seat: NotRequired[str]
    role: NotRequired[str]
    difficulty: NotRequired[CustomSeatDifficulty]
    candidates: NotRequired[list[T3TargetCandidatePayload]]
    clientRequestId: NotRequired[str]
    brief_suffix: NotRequired[str]
    note: NotRequired[str]
    reason: NotRequired[str]
    earliest_reset_at: NotRequired[str]


class T3RecommendSeatPayload(TypedDict):
    results: list[T3RecommendItemPayload]
    next: str


class T3CandidateStatus(StrictModel):
    instance: str
    model: str
    options: list[T3Option] = Field(default_factory=list[T3Option])
    available: bool
    reason: str | None = None
    resets_at: str | None = None


class T3SeatStatus(StrictModel):
    kind: str
    role: str
    description: str
    access: Literal["read", "write"]
    difficulty: CustomSeatDifficulty | None = None
    candidates: list[T3CandidateStatus] = Field(default_factory=list[T3CandidateStatus])
    available: Literal[False] | None = None
    reason: str | None = None


class T3ReviewRoleStatus(StrictModel):
    candidates: list[T3CandidateStatus] = Field(default_factory=list[T3CandidateStatus])
    available: Literal[False] | None = None
    reason: str | None = None


class T3OrchestrationStatus(StrictModel):
    enabled: Literal[True] = True
    random_candidates: bool = False
    seats: dict[str, T3SeatStatus] = {}
    review: dict[ReviewRole, T3ReviewRoleStatus] = {}


class T3MemberListPayload(BaseModel):
    # Clients may have cached this schema before T3 switches or file health changed.
    model_config = ConfigDict(extra="allow", strict=True, allow_inf_nan=False)

    configured: bool
    error: str | None = None
    host: HostName | None = None
    host_source: HostSource
    restricted: list[Mode] = Field(default_factory=list[Mode])
    members: list[ListedMember]
    t3_orchestration: T3OrchestrationStatus | None = None
    orchestration_error: str | None = None
    permissions: dict[ProviderName, PermissionTier] = {}
    permissions_error: str | None = None
    accounts: list[AccountListing] = Field(default_factory=list[AccountListing])
    accounts_error: str | None = None
    provider_details: ProviderDetails | None = None


@dataclass(frozen=True)
class _CandidateFilter:
    usable: list[T3Candidate]
    excluded: list[tuple[T3Candidate, InstanceAvailability]]

    @property
    def earliest_reset(self) -> datetime | None:
        resets = [status.resets_at for _, status in self.excluded if status.resets_at is not None]
        return min(resets) if resets else None


@dataclass(frozen=True)
class _Selection:
    seat_id: str
    role: str
    difficulty: CustomSeatDifficulty | None
    filtered: _CandidateFilter
    note: str | None = None


@dataclass(frozen=True)
class _Unavailable:
    reason: str
    earliest_reset: datetime | None = None
    seat_id: str | None = None
    role: str | None = None


def _summarize_task(text: str) -> str:
    collapsed = " ".join(text.split())
    if len(collapsed) <= _TASK_SUMMARY_MAX:
        return collapsed
    return collapsed[: _TASK_SUMMARY_MAX - 1] + "…"


def _decision_model_name() -> str:
    try:
        doc = read_decision_seats()
    except DecisionSeatsFileError:
        return _DEFAULT_DECISION_MODEL
    return doc.decision_model if doc is not None else _DEFAULT_DECISION_MODEL


def _filter_candidates(
    candidates: Sequence[T3Candidate],
    probe: Callable[[str], InstanceAvailability],
) -> _CandidateFilter:
    usable: list[T3Candidate] = []
    excluded: list[tuple[T3Candidate, InstanceAvailability]] = []
    for candidate in candidates:
        status = probe(candidate.instance)
        if status.available:
            usable.append(candidate)
        else:
            excluded.append((candidate, status))
    return _CandidateFilter(usable=usable, excluded=excluded)


def _format_exclusions(excluded: Sequence[tuple[T3Candidate, InstanceAvailability]]) -> str:
    parts = [
        f"{cand.instance}/{cand.model}: {status.reason or 'unavailable'}"
        for cand, status in excluded[:4]
    ]
    return "; ".join(parts)


def order_candidates(
    candidates: Sequence[T3Candidate],
    *,
    randomize: bool,
    used_first: set[tuple[str, str]],
    rng: random.Random | None = None,
) -> list[T3Candidate]:
    """Order usable candidates: keep saved order when off; shuffle and avoid first-slot duplicates when on."""
    ordered = list(candidates)
    if not randomize or not ordered:
        return ordered
    (rng or new_rng()).shuffle(ordered)
    fresh = next(
        (idx for idx, cand in enumerate(ordered) if (cand.instance, cand.model) not in used_first),
        None,
    )
    if fresh is not None and fresh > 0:
        ordered.insert(0, ordered.pop(fresh))
    used_first.add((ordered[0].instance, ordered[0].model))
    return ordered


def _seat_label(seat: T3Seat) -> str:
    """A seat id with its display name, so a boss can match the name a user said."""
    parts = [part for part in (seat.role, seat.difficulty) if part]
    return f"{seat.id} ({', '.join(parts)})"


def _resolve_explicit_seat(
    item: T3RecommendTask,
    doc: T3SeatsFile,
    seats_by_id: Mapping[str, T3Seat],
    probe: Callable[[str], InstanceAvailability],
) -> _Selection | _Unavailable:
    assert item.seat is not None
    seat_name = item.seat
    if seat_name in REVIEW_ROLES:
        if item.access != "read":
            raise ToolError(
                f"Review role {seat_name!r} is read-only; pass access='read' instead of {item.access!r}."
            )
        candidates = doc.review.get(cast(ReviewRole, seat_name), [])
        if not candidates:
            configured = sorted(role for role, items in doc.review.items() if items)
            listing = ", ".join(configured) if configured else "none"
            raise ToolError(
                f"Review role {seat_name!r} is not configured in t3-seats.json (no automatic "
                f"substitution). Configured review roles: {listing}."
            )
        filtered = _filter_candidates(candidates, probe)
        if not filtered.usable:
            return _Unavailable(
                reason=f"All candidates for review role {seat_name!r} are unavailable: "
                + _format_exclusions(filtered.excluded),
                earliest_reset=filtered.earliest_reset,
                seat_id=seat_name,
                role=seat_name,
            )
        return _Selection(seat_id=seat_name, role=seat_name, difficulty=None, filtered=filtered)
    seat_def = seats_by_id.get(seat_name)
    if seat_def is None:
        task_ids = [_seat_label(seat) for seat in doc.seats if seat.candidates]
        review_ids = sorted(role for role, items in doc.review.items() if items)
        raise ToolError(
            f"Unknown T3 seat {seat_name!r}. Configured task seats: {', '.join(task_ids) or 'none'}; "
            f"review roles: {', '.join(review_ids) or 'none'}."
        )
    if seat_def.access != item.access:
        raise ToolError(
            f"T3 seat {seat_name!r} has access={seat_def.access!r}, not {item.access!r}."
        )
    if not seat_def.candidates:
        same_access = [
            _seat_label(s) for s in doc.seats if s.access == item.access and s.candidates
        ]
        raise ToolError(
            f"T3 seat {seat_name!r} has no configured candidates. Configured access={item.access} "
            f"seats: {', '.join(same_access) or 'none'}."
        )
    filtered = _filter_candidates(seat_def.candidates, probe)
    if not filtered.usable:
        return _Unavailable(
            reason=f"All candidates for seat {seat_name!r} are unavailable: "
            + _format_exclusions(filtered.excluded),
            earliest_reset=filtered.earliest_reset,
            seat_id=seat_def.id,
            role=seat_def.role,
        )
    return _Selection(
        seat_id=seat_def.id, role=seat_def.role, difficulty=seat_def.difficulty, filtered=filtered
    )


def _auto_candidates_for_access(
    access: Literal["read", "write"],
    doc: T3SeatsFile,
    probe: Callable[[str], InstanceAvailability],
) -> tuple[dict[str, tuple[T3Seat, _CandidateFilter]], _Unavailable | None]:
    matching = [seat for seat in doc.seats if seat.access == access and seat.candidates]
    if not matching:
        return {}, _Unavailable(reason=f"No configured T3 task seat has access={access}.")
    usable: dict[str, tuple[T3Seat, _CandidateFilter]] = {}
    excluded_notes: list[str] = []
    resets: list[datetime] = []
    for seat in matching:
        filtered = _filter_candidates(seat.candidates, probe)
        if filtered.usable:
            usable[seat.id] = (seat, filtered)
        else:
            excluded_notes.append(
                f"{seat.id} ({seat.role}): {_format_exclusions(filtered.excluded)}"
            )
            if filtered.earliest_reset is not None:
                resets.append(filtered.earliest_reset)
    if not usable:
        return {}, _Unavailable(
            reason=f"No access={access} T3 seat has a usable candidate right now. Unavailable: "
            + "; ".join(excluded_notes[:5]),
            earliest_reset=min(resets) if resets else None,
        )
    return usable, None


def _decision_failure_error(
    code: str,
    message: str,
    pending: Sequence[tuple[int, T3RecommendTask, dict[str, tuple[T3Seat, _CandidateFilter]]]],
) -> ToolError:
    by_access: dict[str, list[str]] = {}
    for _, item, usable in pending:
        bucket = by_access.setdefault(item.access, [])
        for seat_id in usable:
            if seat_id not in bucket:
                bucket.append(seat_id)
    available_text = "; ".join(f"{access}=[{', '.join(ids)}]" for access, ids in by_access.items())
    return ToolError(
        f"{code}: {message} Available usable seats: {available_text}. "
        "Retry recommend_seat with an explicit seat."
    )


async def _run_decision_batch(
    pending: Sequence[tuple[int, T3RecommendTask, dict[str, tuple[T3Seat, _CandidateFilter]]]],
) -> list[tuple[int, _Selection]]:
    try:
        client = seat_recommendation.client_for_model(_decision_model_name())
    except seat_recommendation.RecommendationError as exc:
        raise _decision_failure_error(exc.code, exc.message, pending) from None
    questions = [
        DecisionQuestion(
            seat_recommendation.task_text(item.task, item.access),
            {seat_id: seat_recommendation.describe(seat) for seat_id, (seat, _) in usable.items()},
        )
        for _, item, usable in pending
    ]
    try:
        answers = await asyncio.to_thread(client.decide_many, questions)
    except DecisionError as exc:
        raise _decision_failure_error(exc.code, exc.message, pending) from None
    chosen_list: list[tuple[int, _Selection]] = []
    for (index, item, usable), answer in zip(pending, answers, strict=True):
        if isinstance(answer, DecisionError):
            raise _decision_failure_error(answer.code, answer.message, [(index, item, usable)])
        if answer.choice not in usable:
            raise _decision_failure_error(
                "bad_response",
                "The decision model chose no listed seat.",
                [(index, item, usable)],
            )
        seat_def, filtered = usable[answer.choice]
        chosen_list.append(
            (
                index,
                _Selection(
                    seat_id=seat_def.id,
                    role=seat_def.role,
                    difficulty=seat_def.difficulty,
                    filtered=filtered,
                ),
            )
        )
    return chosen_list


def _build_item_payload(
    item: T3RecommendTask,
    resolved: _Selection | _Unavailable,
    *,
    randomize: bool,
    used_first: set[tuple[str, str]],
    rng: random.Random,
) -> T3RecommendItemPayload:
    summary = _summarize_task(item.task)
    if isinstance(resolved, _Unavailable):
        payload: T3RecommendItemPayload = {
            "task": summary,
            "status": "unavailable",
            "reason": resolved.reason,
        }
        if resolved.seat_id is not None:
            payload["seat"] = resolved.seat_id
        if resolved.role is not None:
            payload["role"] = resolved.role
        if resolved.earliest_reset is not None:
            payload["earliest_reset_at"] = format_utc(resolved.earliest_reset)
        return payload
    ordered = order_candidates(
        resolved.filtered.usable, randomize=randomize, used_first=used_first, rng=rng
    )
    candidates_payload: list[T3TargetCandidatePayload] = [
        {
            "target": {
                "providerInstanceId": cand.instance,
                "model": cand.model,
                "options": [{"id": opt.id, "value": opt.value} for opt in cand.options],
            }
        }
        for cand in ordered
    ]
    payload = {
        "task": summary,
        "status": "selected",
        "seat": resolved.seat_id,
        "role": resolved.role,
        "candidates": candidates_payload,
        "clientRequestId": str(uuid.uuid4()),
    }
    if resolved.difficulty is not None:
        payload["difficulty"] = resolved.difficulty
    if item.access == "read":
        payload["brief_suffix"] = READ_ONLY_INSTRUCTIONS
    notes: list[str] = []
    if resolved.note:
        notes.append(resolved.note)
    if resolved.filtered.excluded:
        notes.append(
            f"Skipped {len(resolved.filtered.excluded)} unavailable candidate(s): "
            + _format_exclusions(resolved.filtered.excluded)
            + "."
        )
    if notes:
        payload["note"] = " ".join(notes)
    return payload


async def recommend_t3_seats(
    tasks: Sequence[T3RecommendTask],
    doc: T3SeatsFile,
    *,
    probe_instance: Callable[[str], InstanceAvailability] | None = None,
    rng: random.Random | None = None,
) -> T3RecommendSeatPayload:
    """Core T3 seat recommendation across 1-8 tasks."""
    cache: dict[str, InstanceAvailability] = {}
    base_probe = check_instance if probe_instance is None else probe_instance

    def probe(instance: str) -> InstanceAvailability:
        if instance not in cache:
            cache[instance] = base_probe(instance)
        return cache[instance]

    seats_by_id = {seat.id: seat for seat in doc.seats}
    resolved: list[_Selection | _Unavailable | None] = [None] * len(tasks)
    pending: list[tuple[int, T3RecommendTask, dict[str, tuple[T3Seat, _CandidateFilter]]]] = []
    for index, item in enumerate(tasks):
        if item.seat is not None:
            resolved[index] = _resolve_explicit_seat(item, doc, seats_by_id, probe)
            continue
        usable, unavailable = _auto_candidates_for_access(item.access, doc, probe)
        if unavailable is not None:
            resolved[index] = unavailable
        elif len(usable) == 1:
            seat_def, filtered = next(iter(usable.values()))
            resolved[index] = _Selection(
                seat_id=seat_def.id,
                role=seat_def.role,
                difficulty=seat_def.difficulty,
                filtered=filtered,
                note="Only one usable seat has this access; the decision model was not called.",
            )
        else:
            pending.append((index, item, usable))
    if pending:
        for index, chosen in await _run_decision_batch(pending):
            resolved[index] = chosen
    effective_rng = new_rng() if rng is None else rng
    used_first: set[tuple[str, str]] = set()
    results = [
        _build_item_payload(
            item,
            cast(_Selection | _Unavailable, outcome),
            randomize=doc.random_candidates,
            used_first=used_first,
            rng=effective_rng,
        )
        for item, outcome in zip(tasks, resolved, strict=True)
    ]
    any_selected = any(entry["status"] == "selected" for entry in results)
    return {
        "results": results,
        "next": T3_RECOMMEND_NEXT if any_selected else T3_ALL_UNAVAILABLE_NEXT,
    }


def normalize_t3_recommend_args(arguments: dict[str, Any]) -> dict[str, Any]:
    """Accept legacy `{task, access[, manager_session_id]}` as a single-item tasks list in T3."""
    if "tasks" in arguments or not ("task" in arguments or "access" in arguments):
        return arguments
    if set(arguments) - {"task", "access", "manager_session_id"}:
        return arguments
    one: dict[str, Any] = {}
    if "task" in arguments:
        one["task"] = arguments["task"]
    if "access" in arguments:
        one["access"] = arguments["access"]
    return {"tasks": [one]}


async def t3_recommend_seat(
    tasks: Annotated[
        list[T3RecommendTask],
        Field(
            min_length=1,
            max_length=MAX_T3_TASKS,
            description=(
                "1–8 tasks to choose T3 seats and ordered delegate_task targets for; "
                "each item specifies task, access (read/write), and optional explicit seat "
                "(T3 task seat id or review role name)."
            ),
        ),
    ],
) -> T3RecommendSeatPayload:
    """Choose T3 Code seats and ordered delegate_task targets for 1–8 tasks; never starts anything.

    Pass explicit seat (task seat id or review role) to skip the decision model; otherwise the
    decision model picks among usable task seats with matching access (skipped when only one
    fits). Returns ordered T3 targets (options always included, [] when none), a uuid4
    clientRequestId, and read-only brief_suffix when access='read'. Follow receipt next to call
    T3's delegate_task once per selected item.
    """
    from agentnave import mcp_runtime as runtime

    try:
        doc = effective_t3_seats(runtime.HOST)
    except T3SeatsFileError as exc:
        raise ToolError(f"config_error: {exc}") from None
    if doc is None:
        raise ToolError("not_enabled: T3 orchestration is not active for this connection.")
    return await recommend_t3_seats(tasks, doc)


def _candidate_statuses(
    candidates: Sequence[T3Candidate], probe: Callable[[str], InstanceAvailability]
) -> tuple[list[T3CandidateStatus], Literal[False] | None, str | None]:
    if not candidates:
        return [], False, "no candidates configured"
    items: list[T3CandidateStatus] = []
    reasons: list[str] = []
    any_usable = False
    for cand in candidates:
        status = probe(cand.instance)
        if status.available:
            any_usable = True
        else:
            reasons.append(f"{cand.instance}/{cand.model}: {status.reason or 'unavailable'}")
        items.append(
            T3CandidateStatus(
                instance=cand.instance,
                model=cand.model,
                options=list(cand.options),
                available=status.available,
                reason=status.reason,
                resets_at=format_utc(status.resets_at) if status.resets_at is not None else None,
            )
        )
    if any_usable:
        return items, None, None
    return items, False, "; ".join(reasons[:4])


def _discussion_only_members(members: Sequence[ListedMember]) -> list[ListedMember]:
    result: list[ListedMember] = []
    for member in members:
        if member.modes is not None and "discussion" not in member.modes:
            continue
        default: list[Mode] | None = (
            ["discussion"]
            if member.default is True
            or (isinstance(member.default, list) and "discussion" in member.default)
            else None
        )
        result.append(member.model_copy(update={"modes": ["discussion"], "default": default}))
    return result


async def t3_list_members(
    provider: Annotated[
        ProviderName | None,
        Field(description="Also return this CLI's meeting status and supported options."),
    ] = None,
) -> T3MemberListPayload:
    """Read T3 task seats, T3 review roles, and AgentNave meeting members afresh.

    In T3 Code main threads with T3 orchestration enabled, single tasks and reviews use
    t3_orchestration via recommend_seat + T3 delegate_task; members lists meeting (discussion)
    CLIs only. Unavailable T3 candidates reflect local T3 cache status and quota windows.
    """
    from agentnave import mcp_runtime as runtime

    try:
        doc = effective_t3_seats(runtime.HOST)
        base = read_member_list(runtime.task_excluded_providers(), runtime.HOST)
    except (OSError, ValueError) as exc:
        raise ToolError(str(exc)) from exc
    if doc is None:
        raise ToolError("T3 orchestration is not active for this connection.")
    cache: dict[str, InstanceAvailability] = {}

    def probe(instance: str) -> InstanceAvailability:
        if instance not in cache:
            cache[instance] = check_instance(instance)
        return cache[instance]

    seat_statuses: dict[str, T3SeatStatus] = {}
    for seat in doc.seats:
        items, avail, reason = _candidate_statuses(seat.candidates, probe)
        seat_statuses[seat.id] = T3SeatStatus(
            kind=seat.kind,
            role=seat.role,
            description=seat.description,
            access=seat.access,
            difficulty=seat.difficulty,
            candidates=items,
            available=avail,
            reason=reason,
        )
    review_statuses: dict[ReviewRole, T3ReviewRoleStatus] = {}
    for role, candidates in doc.review.items():
        if not candidates:
            continue
        items, avail, reason = _candidate_statuses(candidates, probe)
        review_statuses[role] = T3ReviewRoleStatus(candidates=items, available=avail, reason=reason)
    restricted: list[Mode] = ["task"]
    if "discussion" in base.restricted:
        restricted.append("discussion")
    provider_details: ProviderDetails | None = None
    if provider is not None:
        try:
            member_doc = read_members()
        except MemberFileError as exc:
            raise ToolError(str(exc)) from exc
        discussion = member_doc.modes.get("discussion") if member_doc is not None else None
        provider_details = ProviderDetails(
            provider=provider,
            permitted=False,
            show_permitted=discussion is None or provider in discussion,
            supported_options="model, effort, " + runtime.PROVIDER_OPTIONS[provider],
        )
    return T3MemberListPayload(
        configured=base.configured,
        error=base.error,
        host=base.host,
        host_source=base.host_source,
        restricted=restricted,
        members=_discussion_only_members(base.members),
        t3_orchestration=T3OrchestrationStatus(
            enabled=True,
            random_candidates=doc.random_candidates,
            seats=seat_statuses,
            review=review_statuses,
        ),
        permissions=base.permissions,
        permissions_error=base.permissions_error,
        accounts=base.accounts,
        accounts_error=base.accounts_error,
        provider_details=provider_details,
    )


T3_RECOMMEND_SEAT_TOOL = Tool.from_function(
    t3_recommend_seat,
    name="recommend_seat",
    title="Recommend T3 Code seats and targets for tasks",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=False, open_world_hint=True
    ),
)

T3_LIST_MEMBERS_TOOL = Tool.from_function(
    t3_list_members,
    name="list_members",
    title="Read T3 seats, review roles and meeting members",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)


def t3_dispatch_refusal(tool_name: str, arguments: Mapping[str, Any]) -> str:
    """Guidance when start_agent or start_agents is called while T3 orchestration is active."""
    resuming = tool_name == "start_agent" and bool(arguments.get("session_id"))
    resume_note = (
        "To resume an earlier AgentNave private chat thread, use handover to transfer the "
        "workspace or continue that thread outside T3 Code (you can still read its history "
        "with list and read_conversation). "
    )
    lead = (
        f"Refused: {tool_name} is disabled in T3 Code main threads while T3 orchestration is "
        "enabled. "
    )
    if resuming:
        return (
            lead
            + resume_note
            + "For new tasks, call recommend_seat(tasks) and then T3's delegate_task."
        )
    return (
        lead + "Call recommend_seat(tasks) to pick T3 targets, then dispatch each item with T3's "
        "delegate_task. " + resume_note.strip()
    )


def t3_tool_definition(tool: Tool) -> MCPTool:
    """Build the published MCPTool representation for a T3-specific tool."""
    schema = json.loads(json.dumps(tool.parameters))
    schema["additionalProperties"] = False
    output = json.loads(json.dumps(tool.output_schema)) if tool.output_schema else None
    return MCPTool(
        name=tool.name,
        title=tool.title,
        description=tool.description,
        input_schema=schema,
        output_schema=output,
        annotations=tool.annotations,
        icons=tool.icons,
        _meta=tool.meta,
    )


def t3_state_for_host(host: Host) -> tuple[T3SeatsFile | None, T3SeatsFileError | None]:
    """Return (active_doc, file_error) for this connection without crashing list_tools."""
    if current_t3_mode(host) != "main":
        return None, None
    try:
        return effective_t3_seats(host), None
    except T3SeatsFileError as exc:
        return None, exc


async def run_t3_tool(
    tool: Tool,
    arguments: dict[str, Any],
    context: Context[Any, Any],
    check_arguments: Callable[[str, dict[str, Any], dict[str, Any]], None],
) -> Any:
    """Validate and run a T3-specific Tool with the same argument check as standard tools."""
    effective_args = (
        normalize_t3_recommend_args(arguments) if tool.name == "recommend_seat" else arguments
    )
    check_arguments(tool.name, effective_args, tool.parameters)
    return await tool.run(effective_args, context, convert_result=True)
