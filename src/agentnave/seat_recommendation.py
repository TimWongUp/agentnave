"""Recommend one of the user's own seats for a task with a decision model (ADR-0087).

The service only recommends; the boss dispatches with start_agent and may choose otherwise.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Literal

from agentnave.decision_gemini import KEY_NAME, GeminiDecisionClient
from agentnave.decision_model import (
    Decision,
    DecisionClient,
    DecisionError,
    DecisionKeysFileError,
    DecisionQuestion,
    read_decision_key,
)
from agentnave.decision_seats import effective_decision_seats
from agentnave.host import Host
from agentnave.members import MemberFileError, SettingsFileError, list_members
from agentnave.orchestration_roles import IMAGE_KIND, CustomSeatDifficulty, CustomSeatStatus

# What the decision model reads of a task: all of it up to this many characters, otherwise
# the beginning and the end with the middle left out.
MAX_DECISION_TEXT = 4000
_KEPT_HEAD = 3000
_KEPT_TAIL = 1000
MAX_CANDIDATES = 3

type RecommendationErrorCode = Literal[
    "not_enabled",
    "config_error",
    "no_key",
    "no_candidates",
    "network",
    "rate_limited",
    "auth",
    "rejected",
    "bad_response",
]

# decision_model name -> (key name in decision-keys.json, client for that key)
_MODELS: dict[str, tuple[str, Callable[[str], DecisionClient]]] = {
    "gemini-decision-1": (KEY_NAME, GeminiDecisionClient),
}


class RecommendationError(Exception):
    """Why no recommendation came back; code tells the failure apart for the boss."""

    def __init__(self, code: RecommendationErrorCode, message: str) -> None:
        super().__init__(message)
        self.code: RecommendationErrorCode = code
        self.message = message


@dataclass(frozen=True)
class Candidate:
    seat: str
    role: str
    probability: float | None = None
    native: bool = False
    difficulty: CustomSeatDifficulty | None = None


@dataclass(frozen=True)
class Recommendation:
    seat: str
    role: str
    confidence: float | None
    native: bool
    decision_model_called: bool
    candidates: list[Candidate]
    difficulty: CustomSeatDifficulty | None = None
    note: str | None = None
    # A native seat's saved model and effort: what the host's own subagent is asked to use.
    model: str | None = None
    effort: str | None = None


def _native_settings(seat: CustomSeatStatus) -> dict[str, str | None]:
    if not seat.native:
        return {}
    return {"model": seat.model or None, "effort": seat.effort or None}


def new_client(model: str, key: str) -> DecisionClient:
    """The client of the selected decision model; tests replace this."""
    return _MODELS[model][1](key)


# What a difficulty asks of the work. Each seat's line carries its own meaning, so a specialty
# that offers only some of the levels still tells them apart.
_DIFFICULTY_MEANING: dict[CustomSeatDifficulty, str] = {
    "easy": "settled scope and explicit steps: locating, collecting evidence, a specified change",
    "medium": "a defined goal that still needs some investigation or decisions on the way",
    "hard": "design trade-offs, root-cause judgment or unclear boundaries",
}


def describe(seat: CustomSeatStatus) -> str:
    """All the decision model learns about a seat: its specialty, difficulty and access."""
    named = seat.role if seat.description == seat.role else f"{seat.role}: {seat.description}"
    if seat.kind == IMAGE_KIND:
        return f"{named} (image generation) | access: {seat.access}"
    meaning = f" ({_DIFFICULTY_MEANING[seat.difficulty]})" if seat.difficulty else ""
    return f"{named} | difficulty: {seat.difficulty}{meaning} | access: {seat.access}"


def clip_for_decision(text: str) -> str:
    """The text as the decision model gets it; counted in characters."""
    if len(text) <= MAX_DECISION_TEXT:
        return text
    return f"{text[:_KEPT_HEAD]}\n…\n{text[-_KEPT_TAIL:]}"


def _task_text(task: str, access: Literal["read", "write"]) -> str:
    return f"Task:\n{clip_for_decision(task)}\nRequired access: {access}"


@dataclass(frozen=True)
class _Catalog:
    """What a recommendation chooses from: the decision model and the seats in the user's file."""

    decision_model: str
    seats: Mapping[str, CustomSeatStatus]


def _catalog(host: Host, excluded: frozenset[str]) -> _Catalog:
    try:
        # A broken decision-seats.json raises here, which is not "off".
        document = effective_decision_seats(host)
        if document is None:
            raise RecommendationError(
                "not_enabled",
                "Seat recommendation is not enabled for this boss; choose a seat from "
                "list_members yourself.",
            )
        seats = list_members(excluded, host).orchestration.custom_seats
    except (MemberFileError, SettingsFileError, OSError, ValueError) as exc:
        raise RecommendationError("config_error", str(exc)) from exc
    return _Catalog(document.decision_model, seats)


def _usable(
    catalog: _Catalog, access: Literal["read", "write"]
) -> dict[str, CustomSeatStatus] | RecommendationError:
    """The seats that can be dispatched now with this access, or why there are none."""
    matching = {seat_id: seat for seat_id, seat in catalog.seats.items() if seat.access == access}
    usable = {seat_id: seat for seat_id, seat in matching.items() if seat.available is None}
    if not usable:
        return RecommendationError("no_candidates", _no_candidates(access, matching))
    return usable


def _only(seat_id: str, seat: CustomSeatStatus) -> Recommendation:
    return Recommendation(
        seat_id,
        seat.role,
        None,
        bool(seat.native),
        False,
        [Candidate(seat_id, seat.role, None, bool(seat.native), seat.difficulty)],
        seat.difficulty,
        "Only one dispatchable seat has this access; the decision model was not called.",
        **_native_settings(seat),
    )


async def recommend(
    task: str, access: Literal["read", "write"], host: Host, excluded: frozenset[str]
) -> Recommendation:
    catalog = _catalog(host, excluded)
    usable = _usable(catalog, access)
    if isinstance(usable, RecommendationError):
        raise usable
    if len(usable) == 1:
        return _only(*next(iter(usable.items())))
    client = _client(catalog.decision_model)
    try:
        decision = await asyncio.to_thread(
            client.decide, _task_text(task, access), _described(usable)
        )
    except DecisionError as exc:
        raise RecommendationError(exc.code, exc.message) from None
    return _chosen(decision, usable)


async def recommend_many(
    tasks: Sequence[tuple[str, Literal["read", "write"]]], host: Host, excluded: frozenset[str]
) -> list[Recommendation | RecommendationError]:
    """One recommendation for each task, in order, from at most one decision request.

    A task with no dispatchable seat, or a decision the model answered badly, is a
    RecommendationError in its place. A failure of the request itself raises, and then no
    task has a recommendation.
    """
    catalog = _catalog(host, excluded)
    results: list[Recommendation | RecommendationError | None] = [None] * len(tasks)
    pending: list[tuple[int, dict[str, CustomSeatStatus]]] = []
    for index, (_, access) in enumerate(tasks):
        usable = _usable(catalog, access)
        if isinstance(usable, RecommendationError):
            results[index] = usable
        elif len(usable) == 1:
            results[index] = _only(*next(iter(usable.items())))
        else:
            pending.append((index, usable))
    if pending:
        client = _client(catalog.decision_model)
        questions = [
            DecisionQuestion(_task_text(tasks[index][0], tasks[index][1]), _described(usable))
            for index, usable in pending
        ]
        try:
            answers = await asyncio.to_thread(client.decide_many, questions)
        except DecisionError as exc:
            raise RecommendationError(exc.code, exc.message) from None
        for (index, usable), answer in zip(pending, answers, strict=True):
            results[index] = (
                RecommendationError(answer.code, answer.message)
                if isinstance(answer, DecisionError)
                else _safe_choice(answer, usable)
            )
    return [result for result in results if result is not None]


def _client(model: str) -> DecisionClient:
    key_name = _MODELS[model][0]
    try:
        key = read_decision_key(key_name)
    except DecisionKeysFileError as exc:
        raise RecommendationError("config_error", str(exc)) from exc
    if key is None:
        raise RecommendationError(
            "no_key",
            f"No API key is set for {model}; choose a seat from list_members yourself.",
        )
    return new_client(model, key)


def _described(usable: Mapping[str, CustomSeatStatus]) -> dict[str, str]:
    return {seat_id: describe(seat) for seat_id, seat in usable.items()}


def _safe_choice(
    decision: Decision, usable: Mapping[str, CustomSeatStatus]
) -> Recommendation | RecommendationError:
    try:
        return _chosen(decision, usable)
    except RecommendationError as exc:
        return exc


def _chosen(decision: Decision, usable: Mapping[str, CustomSeatStatus]) -> Recommendation:
    if decision.choice not in usable:
        raise RecommendationError("bad_response", "The decision model chose no listed seat.")
    chosen = usable[decision.choice]
    others = sorted(
        (seat_id for seat_id in usable if seat_id != decision.choice),
        key=lambda seat_id: decision.probabilities.get(seat_id, 0.0),
        reverse=True,
    )
    return Recommendation(
        decision.choice,
        chosen.role,
        decision.confidence,
        bool(chosen.native),
        True,
        [
            Candidate(
                seat_id,
                usable[seat_id].role,
                decision.probabilities.get(seat_id),
                bool(usable[seat_id].native),
                usable[seat_id].difficulty,
            )
            for seat_id in [decision.choice, *others][:MAX_CANDIDATES]
        ],
        chosen.difficulty,
        **_native_settings(chosen),
    )


def _no_candidates(
    access: Literal["read", "write"], matching: Mapping[str, CustomSeatStatus]
) -> str:
    if not matching:
        return (
            f"No seat of your own catalog has access={access}; the decision model was not called."
        )
    reasons = "; ".join(
        f"{seat_id} ({seat.role}): {seat.reason or 'unavailable'}"
        for seat_id, seat in list(matching.items())[:5]
    )
    return (
        f"No access={access} seat can be dispatched now, so the decision model was not "
        f"called. Unavailable: {reasons}"
    )


__all__ = [
    "MAX_DECISION_TEXT",
    "Candidate",
    "Recommendation",
    "RecommendationError",
    "clip_for_decision",
    "new_client",
    "recommend",
    "recommend_many",
]
