"""T3 Code seat recommendation and candidate ordering (ADR-0121)."""

from __future__ import annotations

import asyncio
import random
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Annotated, Any, Literal, NotRequired, TypedDict, cast

from mcp.server.mcpserver.exceptions import ToolError
from pydantic import BaseModel, ConfigDict, Field

from agentnave import seat_recommendation
from agentnave.core import READ_ONLY_INSTRUCTIONS
from agentnave.decision_model import DecisionError, DecisionQuestion
from agentnave.decision_seats import DecisionSeatsFileError, read_decision_seats
from agentnave.members import ReviewRole
from agentnave.orchestration_roles import CustomSeatDifficulty
from agentnave.t3_cache import InstanceAvailability, check_instance, format_utc
from agentnave.t3_seats import (
    REVIEW_ROLES,
    T3Candidate,
    T3OrchestrationMode,
    T3Seat,
    T3SeatsFile,
    t3_seat_name,
)

MAX_T3_TASKS = 8
_TASK_SUMMARY_MAX = 80
_DEFAULT_DECISION_MODEL = "gemini-decision-1"

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
            description=(
                "T3 seat name: fixed role name (e.g. worker-easy), custom seat id, or review role "
                "name. Required in split/custom modes; optional in decision mode (skips decision "
                "model)."
            ),
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
    probe: Callable[[str, str], InstanceAvailability],
) -> _CandidateFilter:
    usable: list[T3Candidate] = []
    excluded: list[tuple[T3Candidate, InstanceAvailability]] = []
    for candidate in candidates:
        status = probe(candidate.instance, candidate.model)
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
    """A seat's public name with its role and difficulty, so a boss can match what the user said."""
    name = t3_seat_name(seat)
    parts = [part for part in (seat.role, seat.difficulty) if part]
    return f"{name} ({', '.join(parts)})"


def _seat_choice_label(seat: T3Seat) -> str:
    """A seat's public name with role, difficulty, and description for seat_required errors."""
    name = t3_seat_name(seat)
    meta = ", ".join(part for part in (seat.role, seat.difficulty) if part)
    return f"{name} ({meta}: {seat.description})" if meta else f"{name} ({seat.description})"


def _seat_required_error(
    missing: Sequence[T3RecommendTask],
    doc: T3SeatsFile,
    mode: T3OrchestrationMode,
) -> ToolError:
    needed_accesses = list(dict.fromkeys(item.access for item in missing))
    buckets: list[str] = []
    for access in needed_accesses:
        entries = [
            _seat_choice_label(seat)
            for seat in doc.seats
            if seat.access == access and seat.candidates
        ]
        if access == "read":
            reviews = sorted(role for role, items in doc.review.items() if items)
            if reviews:
                entries.append(f"review roles: {', '.join(reviews)}")
        buckets.append(f"access={access}: {'; '.join(entries) if entries else 'none'}")
    return ToolError(
        f"seat_required: In {mode} mode, pass explicit seat on every item (the decision model is "
        f"not used). Available T3 seats — {' | '.join(buckets)}."
    )


def _resolve_explicit_seat(
    item: T3RecommendTask,
    doc: T3SeatsFile,
    seats_by_name: Mapping[str, T3Seat],
    seats_by_id: Mapping[str, T3Seat],
    probe: Callable[[str, str], InstanceAvailability],
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
    seat_def = seats_by_name.get(seat_name) or seats_by_id.get(seat_name)
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
    public_name = t3_seat_name(seat_def)
    filtered = _filter_candidates(seat_def.candidates, probe)
    if not filtered.usable:
        return _Unavailable(
            reason=f"All candidates for seat {seat_name!r} are unavailable: "
            + _format_exclusions(filtered.excluded),
            earliest_reset=filtered.earliest_reset,
            seat_id=public_name,
            role=seat_def.role,
        )
    return _Selection(
        seat_id=public_name, role=seat_def.role, difficulty=seat_def.difficulty, filtered=filtered
    )


def _auto_candidates_for_access(
    access: Literal["read", "write"],
    doc: T3SeatsFile,
    probe: Callable[[str, str], InstanceAvailability],
) -> tuple[dict[str, tuple[T3Seat, _CandidateFilter]], _Unavailable | None]:
    matching = [seat for seat in doc.seats if seat.access == access and seat.candidates]
    if not matching:
        return {}, _Unavailable(reason=f"No configured T3 task seat has access={access}.")
    usable: dict[str, tuple[T3Seat, _CandidateFilter]] = {}
    excluded_notes: list[str] = []
    resets: list[datetime] = []
    for seat in matching:
        public_name = t3_seat_name(seat)
        filtered = _filter_candidates(seat.candidates, probe)
        if filtered.usable:
            usable[public_name] = (seat, filtered)
        else:
            excluded_notes.append(
                f"{public_name} ({seat.role}): {_format_exclusions(filtered.excluded)}"
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
        for seat_name in usable:
            if seat_name not in bucket:
                bucket.append(seat_name)
    available_text = "; ".join(
        f"{access}=[{', '.join(names)}]" for access, names in by_access.items()
    )
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
            {
                seat_name: seat_recommendation.describe(seat)
                for seat_name, (seat, _) in usable.items()
            },
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
                    seat_id=t3_seat_name(seat_def),
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
    mode: T3OrchestrationMode = "decision",
    probe_instance: Callable[[str, str], InstanceAvailability] | None = None,
    rng: random.Random | None = None,
) -> T3RecommendSeatPayload:
    """Core T3 seat recommendation across 1-8 tasks."""
    if mode != "decision":
        missing = [item for item in tasks if item.seat is None]
        if missing:
            raise _seat_required_error(missing, doc, mode)
    cache: dict[tuple[str, str], InstanceAvailability] = {}
    base_probe = check_instance if probe_instance is None else probe_instance

    def probe(instance: str, model: str) -> InstanceAvailability:
        if (instance, model) not in cache:
            cache[instance, model] = base_probe(instance, model)
        return cache[instance, model]

    seats_by_id = {seat.id: seat for seat in doc.seats}
    seats_by_name = {t3_seat_name(seat): seat for seat in doc.seats}
    resolved: list[_Selection | _Unavailable | None] = [None] * len(tasks)
    pending: list[tuple[int, T3RecommendTask, dict[str, tuple[T3Seat, _CandidateFilter]]]] = []
    for index, item in enumerate(tasks):
        if item.seat is not None:
            resolved[index] = _resolve_explicit_seat(item, doc, seats_by_name, seats_by_id, probe)
            continue
        usable, unavailable = _auto_candidates_for_access(item.access, doc, probe)
        if unavailable is not None:
            resolved[index] = unavailable
        elif len(usable) == 1:
            public_name, (seat_def, filtered) = next(iter(usable.items()))
            resolved[index] = _Selection(
                seat_id=public_name,
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
    """Accept legacy `{task, access[, seat][, manager_session_id]}` as a single-item tasks list in T3."""
    if "tasks" in arguments or not ("task" in arguments or "access" in arguments):
        return arguments
    if set(arguments) - {"task", "access", "seat", "manager_session_id"}:
        return arguments
    one: dict[str, Any] = {}
    for key in ("task", "access", "seat"):
        if key in arguments:
            one[key] = arguments[key]
    return {"tasks": [one]}
