"""MCP tool that starts several new tasks at once, the decision model choosing every seat."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated, Literal

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import AfterValidator, BaseModel, ConfigDict, Field

from agentnave import mcp_runtime as runtime
from agentnave.conversations_store import normalize_role
from agentnave.decision_seats import custom_seats
from agentnave.mcp_isolated import IsolationInput
from agentnave.mcp_payloads import NotStartedPayload, StartAgentsPayload, TaskStartPayload
from agentnave.mcp_recommend import native_seat_message, recommendation_payload
from agentnave.mcp_runtime import (
    ManagerModel,
    ManagerSessionId,
    Runtime,
    _conversations,
    _short_title,
    _unreceived,
)
from agentnave.mcp_tasks import TaskLaunch, launch_task
from agentnave.members import SettingsFileError
from agentnave.models import InvocationRequest
from agentnave.seat_recommendation import (
    Recommendation,
    RecommendationError,
    recommend_many,
)

MAX_TASKS = 8


class StartTask(BaseModel):
    """One task of start_agents."""

    model_config = ConfigDict(extra="forbid")

    access: Annotated[
        Literal["read", "write"],
        Field(description="Access this task needs; only seats with this access are candidates."),
    ]
    prompt: Annotated[
        str,
        Field(
            min_length=1,
            description=(
                "Task, context, constraints and expected output. It is sent to an external "
                "decision model to choose the seat, only its beginning and end when long: "
                "leave out secrets."
            ),
        ),
    ]
    role: Annotated[str | None, Field(description="Desk label, as in start_agent.")] = None
    cwd: Annotated[
        str | None,
        Field(min_length=1, description="Existing absolute directory; omit for the top-level cwd."),
    ] = None
    isolation: Annotated[
        IsolationInput | None,
        Field(
            description=(
                "New isolated Git worktree (optional ref), as in start_agent. Several write "
                "tasks get one automatically; this replaces that one."
            )
        ),
    ] = None
    forward: Annotated[
        list[str] | None,
        Field(description="Message_ids to inject before this task, as in start_agent."),
    ] = None


def _unusable(index: int, problem: str) -> ToolError:
    return ToolError(f"tasks[{index}]: {problem}. Nothing was started. Correct and retry.")


def _directories(tasks: list[StartTask], cwd: str | None) -> list[str]:
    """Every task's directory, checked as a started task's is; nothing starts on a bad one."""
    found: list[str] = []
    for index, one in enumerate(tasks):
        where = one.cwd or cwd
        if where is None:
            raise _unusable(index, "cwd is required: pass it here or at the top level")
        try:
            InvocationRequest(provider="codex", prompt="-", cwd=Path(where))
            normalize_role(one.role)
        except ValueError as exc:
            raise _unusable(index, str(exc)) from exc
        found.append(where)
    return found


def _check_capacity(ctx: Context[Runtime], wanted: int) -> None:
    try:
        running, limit = _conversations(ctx).capacity()
    except SettingsFileError as exc:
        raise ToolError(f"{exc}. Nothing was started. Correct and retry.") from exc
    if wanted > min(limit, MAX_TASKS):
        raise ToolError(
            f"too_many_tasks: {wanted} tasks given; at most {min(limit, MAX_TASKS)} at once "
            f"(the App limit is {limit}, and {MAX_TASKS} is the most one call takes). "
            "Nothing was started."
        )
    if running + wanted > limit:
        raise ToolError(
            f"capacity: {running} CLI runs are active or held by serial rounds for their next "
            f"seat, and this would start {wanted}; the AgentNave App limits this server to "
            f"{limit} at once. Nothing was started. Wait for some to finish, then retry."
        )


def _not_started(
    code: str, message: str, recommendation: Recommendation | None = None
) -> NotStartedPayload:
    payload: NotStartedPayload = {
        "status": "not_started",
        "error": {"code": code, "message": message},
    }
    if recommendation is not None:
        payload["recommendation"] = recommendation_payload(recommendation)
    return payload


async def start_agents(
    tasks: Annotated[
        list[StartTask],
        Field(
            min_length=2,
            max_length=MAX_TASKS,
            description=(
                "Two or more new tasks, at most the App's run limit and "
                f"{MAX_TASKS}; each seat is chosen by the decision model in one request. "
                "Too many for the free capacity rejects the whole call."
            ),
        ),
    ],
    cwd: Annotated[
        str | None,
        Field(
            min_length=1,
            description="Existing absolute directory for every task that gives none of its own.",
        ),
    ] = None,
    title: Annotated[
        str | None,
        Field(description="New workspace title, as in start_agent."),
        AfterValidator(_short_title),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> StartAgentsPayload:
    """Start several new private CLI tasks at once; the decision model picks every seat in one
    request, which is faster than start_agent(seat='auto') for each. Each prompt is sent to
    that external model, only its beginning and end when long. Everything that costs
    nothing is checked first and a bad task rejects the whole call. results follow the order
    of tasks: a started one is start_agent's receipt (invocation_id goes to wait); one that did
    not start says why in error.code (no_candidates, bad_response, native_seat, start_failed)
    and, when a seat was chosen, in recommendation; the others are unaffected. A decision that
    fails as a whole (not_enabled, config_error, no_key, network, rate_limited, auth, rejected,
    bad_response) starts nothing. Nothing is retried or replaced. With two or more
    access=write tasks the service gives each its own new isolated directory, as if it had
    passed isolation (a task's own isolation wins; read tasks and a lone writer get none);
    receipts show isolation and "isolation (automatic: ...)" in defaults_applied, and a task
    whose directory cannot be made (for example cwd is not a Git repository) is not started
    (start_failed), never started unisolated. Whether to run writes in parallel is your
    decision, not the tool's.
    """
    if custom_seats(runtime.HOST) is None:
        raise ToolError(
            "not_enabled: Seat recommendation is not enabled for this boss; start tasks one "
            "at a time with start_agent."
        )
    _check_capacity(ctx, len(tasks))
    directories = _directories(tasks, cwd)
    try:
        found = await recommend_many(
            [(one.prompt, one.access) for one in tasks], runtime.HOST, runtime.EXCLUDED_PROVIDERS
        )
    except RecommendationError as exc:
        raise ToolError(f"{exc.code}: {exc.message}") from None
    # Writers started together get a directory each, unless the task asked for its own.
    isolate = sum(one.access == "write" for one in tasks) >= 2
    results: list[TaskStartPayload | NotStartedPayload] = []
    for one, where, chosen in zip(tasks, directories, found, strict=True):
        if isinstance(chosen, RecommendationError):
            results.append(_not_started(chosen.code, chosen.message))
        elif chosen.native:
            results.append(_not_started("native_seat", native_seat_message(chosen), chosen))
        else:
            results.append(
                await _started(
                    ctx, one, where, chosen, isolate, title, manager_model, manager_session_id
                )
            )
    conversations = _conversations(ctx)
    payload: StartAgentsPayload = {"results": results}
    if any(item["status"] != "not_started" for item in results):
        if unreceived := _unreceived(conversations, manager_session_id):
            payload["unreceived"] = unreceived
        payload["next"] = runtime.WAIT_NEXT
    return payload


async def _started(
    ctx: Context[Runtime],
    one: StartTask,
    where: str,
    chosen: Recommendation,
    isolate: bool,
    title: str | None,
    manager_model: str | None,
    manager_session_id: str | None,
) -> TaskStartPayload | NotStartedPayload:
    """Start one chosen seat through start_agent's path; a failure belongs to this task alone."""
    automatic = isolate and one.access == "write" and one.isolation is None
    try:
        payload = await launch_task(
            ctx,
            TaskLaunch(
                prompt=one.prompt,
                provider=None,
                cwd=where,
                session_id=None,
                provider_options=None,
                conversation_id=None,
                title=title,
                role=one.role,
                seat=chosen.seat,
                forward=one.forward,
                access=one.access,
                isolation=IsolationInput() if automatic else one.isolation,
                manager_model=manager_model,
                manager_session_id=manager_session_id,
                isolation_auto=automatic,
            ),
        )
    except (ToolError, ValueError, OSError) as exc:
        return _not_started("start_failed", str(exc), chosen)
    payload["recommendation"] = recommendation_payload(chosen)
    return payload
