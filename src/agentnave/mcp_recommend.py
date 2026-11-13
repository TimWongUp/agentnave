"""MCP tool that asks the decision model which of the user's own seats suits a task."""

from __future__ import annotations

from typing import Annotated, Literal

from mcp.server.mcpserver.exceptions import ToolError
from pydantic import Field

from agentnave import mcp_runtime as runtime
from agentnave.mcp_payloads import SeatCandidatePayload, SeatRecommendationPayload
from agentnave.mcp_runtime import ManagerSessionId
from agentnave.seat_recommendation import (
    Candidate,
    Recommendation,
    RecommendationError,
    recommend,
)


def _candidate_payload(candidate: Candidate) -> SeatCandidatePayload:
    payload: SeatCandidatePayload = {"seat": candidate.seat, "role": candidate.role}
    if candidate.probability is not None:
        payload["probability"] = candidate.probability
    if candidate.native:
        payload["native"] = True
    return payload


async def recommend_seat(
    task: Annotated[
        str,
        Field(
            min_length=1,
            description=(
                "The task text to judge by; pass the prompt you mean to dispatch. It is sent to "
                "an external decision model, only its beginning and end when long: leave out "
                "secrets."
            ),
        ),
    ],
    access: Annotated[
        Literal["read", "write"],
        Field(description="Access the task needs; only seats with this access are candidates."),
    ],
    manager_session_id: ManagerSessionId = None,
) -> SeatRecommendationPayload:
    """Recommend one of your own seats for a task; it never dispatches. Candidates are the
    seats of list_members that can be dispatched now and have this access. Dispatch with
    start_agent(seat=...), or choose another seat; native=true means the host's built-in
    subagent. Errors start with a code: not_enabled, config_error, no_key, no_candidates,
    network, rate_limited, auth, rejected or bad_response. Nothing is retried or replaced:
    choose a seat from list_members yourself.
    """
    found = await recommend_or_raise(task, access)
    return recommendation_payload(found)


async def recommend_or_raise(task: str, access: Literal["read", "write"]) -> Recommendation:
    """The recommendation for the running host; failures become coded tool errors."""
    try:
        return await recommend(task, access, runtime.HOST, runtime.EXCLUDED_PROVIDERS)
    except RecommendationError as exc:
        raise ToolError(f"{exc.code}: {exc.message}") from None


def recommendation_payload(found: Recommendation) -> SeatRecommendationPayload:
    payload: SeatRecommendationPayload = {
        "seat": found.seat,
        "role": found.role,
        "decision_model_called": found.decision_model_called,
        "candidates": [_candidate_payload(candidate) for candidate in found.candidates],
    }
    if found.confidence is not None:
        payload["confidence"] = found.confidence
    if found.native:
        payload["native"] = True
        if found.model:
            payload["model"] = found.model
        if found.effort:
            payload["effort"] = found.effort
    if found.note:
        payload["note"] = found.note
    return payload


async def choose_auto_seat(
    prompt: str,
    access: Literal["read", "write"] | None,
    provider: str | None,
    cwd: str | None,
    session_id: str | None,
) -> Recommendation:
    """start_agent(seat="auto"): the seat the decision model picks, or a refusal before any
    task starts. Everything cheap is checked first so a refused call costs no decision."""
    if session_id is not None:
        raise ToolError(
            "seat='auto' cannot be combined with session_id: a resumed thread keeps its seat. "
            "Omit seat, or name the seat to start a new thread."
        )
    if provider is not None:
        raise ToolError(
            "seat='auto' cannot be combined with provider: the chosen seat decides the CLI. "
            "Omit provider, or name the seat."
        )
    if access is None:
        raise ToolError("seat='auto' needs access (read or write).")
    if cwd is None:
        raise ToolError("cwd is required for a new task; pass an absolute existing directory")
    found = await recommend_or_raise(prompt, access)
    if found.native:
        raise ToolError(f"native_seat: {native_seat_message(found)}")
    return found


def native_seat_message(found: Recommendation) -> str:
    """Why a chosen seat on the host's own CLI starts nothing, and how to run it instead."""
    settings = ", ".join(
        f"{name} {value}"
        for name, value in (("model", found.model), ("effort", found.effort))
        if value
    )
    return (
        f"the chosen seat {found.seat} ({found.role}) is the host's own CLI, so nothing was "
        "started. Dispatch it with the host's built-in subagent"
        + (f" using {settings}." if settings else ".")
    )
