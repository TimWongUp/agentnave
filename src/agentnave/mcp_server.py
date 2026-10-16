"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import os
import time
from collections.abc import AsyncGenerator, Callable
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Any, Literal, NotRequired, TypedDict, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import ToolAnnotations
from pydantic import BaseModel, ConfigDict, Field

from agentnave import __version__
from agentnave.core import InvocationManager
from agentnave.group import Groups, GroupSeat, Round
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
)
from agentnave.nesting import detect_nesting

type ProviderName = Literal["antigravity", "claude", "codebuddy", "codex", "grok", "pi"]


def _read_excluded_providers() -> frozenset[str]:
    excluded = frozenset(
        name.strip().lower()
        for name in os.environ.get("AGENTNAVE_EXCLUDED_PROVIDERS", "").split(",")
        if name.strip()
    )
    unknown = excluded.difference(get_args(ProviderName.__value__))
    if unknown:
        raise ValueError("Unknown AGENTNAVE_EXCLUDED_PROVIDERS: " + ", ".join(sorted(unknown)))
    return excluded


WAIT_SECONDS = 300

EXCLUDED_PROVIDERS = _read_excluded_providers()
PROVIDER_SELECTION = (
    "Providers permitted by this host configuration: "
    + (
        ", ".join(
            name for name in get_args(ProviderName.__value__) if name not in EXCLUDED_PROVIDERS
        )
        or "none"
    )
    + ". Excluded providers: "
    + (", ".join(sorted(EXCLUDED_PROVIDERS)) or "none")
    + ". start_agent rejects excluded providers; groups may seat any provider."
)
PROVIDER_OPTIONS: dict[ProviderName, str] = {
    "pi": "provider (model backend; requires model), tools, restricted_tools (boolean; disables tools, extensions and MCP)",
    "claude": "permission_mode, agent, fallback_model, max_budget_usd, restricted_tools (boolean; restricted tools)",
    "codebuddy": "permission_mode, agent, fallback_model, restricted_tools (boolean; restricted tools)",
    "codex": (
        "skip_git_repo_check (boolean; explicitly true outside Git repositories), "
        "dangerously_bypass_approvals_and_sandbox "
        "(boolean; true skips approvals and disables sandboxing; false or omitted adds no flag), "
        "restricted_tools (boolean; native read/tool restrictions)"
    ),
    "grok": "permission_mode, agent, max_turns, sandbox, restricted_tools (boolean; denies tool execution)",
    "antigravity": (
        "agent, mode, project, print_timeout, sandbox (boolean), disable_slash_commands (boolean), "
        "dangerously_skip_permissions (boolean; explicitly true enables YOLO for this invocation), "
        "restricted_tools (boolean; tool-free profile, only in a dedicated seat cwd that already "
        "contains it)"
    ),
}


class ProviderDescriptionPayload(TypedDict):
    provider: ProviderName
    permitted: bool
    supported_options: str


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
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


class WaitAnyPayload(TypedDict):
    reason: Literal["ready", "wait_elapsed"]
    updates: list[InvocationPayload]


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


class InvocationListPayload(TypedDict):
    invocations: list[InvocationListItem]


class GroupSeatSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: Annotated[str, Field(pattern=r"^[a-z][a-z0-9_-]{0,31}$")]
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(
        default_factory=lambda: {},
        description="Explicit options for this seat's CLI, as for start_agent.",
    )
    cwd: str | None = Field(
        default=None,
        description="Absolute directory for this seat; defaults to the group cwd.",
    )


class GroupSeatPayload(TypedDict):
    id: str
    provider: str
    cwd: str


class GroupPayload(TypedDict):
    group_id: str
    seats: list[GroupSeatPayload]


class SeatRunPayload(TypedDict):
    seat_id: str
    status: Literal[
        "pending", "running", "succeeded", "failed", "blocked", "cancelled", "not_started"
    ]
    invocation_id: NotRequired[str]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


class RoundPayload(TypedDict):
    round_id: str
    group_id: str
    round: int
    order: Literal["parallel", "serial"]
    status: Literal["running", "finished", "failed", "cancelled"]
    reason: str
    elapsed_ms: int
    seats: list[SeatRunPayload]
    error: NotRequired[str]


class Runtime:
    def __init__(self) -> None:
        self.manager = InvocationManager()
        self.groups = Groups(self.manager)


@asynccontextmanager
async def _lifespan(_server: MCPServer[Runtime]) -> AsyncGenerator[Runtime]:
    runtime = Runtime()
    try:
        yield runtime
    finally:
        try:
            # Stop rounds first so serial rounds launch nothing new and commit their results.
            await runtime.groups.shutdown()
        finally:
            await runtime.manager.shutdown()


mcp = MCPServer(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave launches local CLI subagents. Use the agentnave-manager Skill for CLI usage "
        "and model selection. describe_provider reports permitted status and supported options. "
        "Call start_agent, then wait_agent with its invocation_id, or wait_any for several; "
        "cancel_agent stops work and list_invocations lists handles. "
        "For several CLIs answering together, open_group then "
        "run_round (parallel or serial), wait_round and cancel_round. "
        "Handles and groups last only for this server process; nothing is saved. "
        "Invocation handles expire seven days after completion. "
        "Provider-native permissions remain the security boundary. " + PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


# Registration options per tool, so the studio server can register the same functions.
TOOL_OPTIONS: dict[str, tuple[str, ToolAnnotations]] = {}


def _tool[F: Callable[..., Any]](title: str, annotations: ToolAnnotations) -> Callable[[F], F]:
    def register(fn: F) -> F:
        TOOL_OPTIONS[fn.__name__] = (title, annotations)
        return mcp.tool(title=title, annotations=annotations)(fn)

    return register


def _manager(ctx: Context[Runtime]) -> InvocationManager:
    return ctx.request_context.lifespan_context.manager


def started_payload(invocation_id: str, request: InvocationRequest) -> InvocationPayload:
    return {
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "started",
        "elapsed_ms": 0,
        "provider": request.provider,
        "model": str(request.provider_options.get("model", "继承原生设置")),
        "effort": str(request.provider_options.get("effort", "继承原生设置")),
    }


def finished_payload(invocation_id: str, result: InvocationResult) -> InvocationPayload:
    payload: InvocationPayload = {
        "invocation_id": invocation_id,
        "status": result.status.value,
        "reason": "finished",
        "elapsed_ms": result.duration_ms,
    }
    if result.output:
        payload["output"] = result.output
    if result.session_id:
        payload["session_id"] = result.session_id
    if result.error:
        payload["error"] = {"code": result.error.code, "message": result.error.message}
    return payload


def update_payload(
    manager: InvocationManager,
    invocation_id: str,
    update: InvocationResult | InvocationError | None,
) -> InvocationPayload:
    """Final result, or a running snapshot that reports the given blocker."""
    if isinstance(update, InvocationResult):
        return finished_payload(invocation_id, update)
    snapshot = manager.snapshot(invocation_id)
    response: InvocationPayload = {
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "execution_blocked" if update is not None else "wait_elapsed",
        "elapsed_ms": snapshot.elapsed_ms,
    }
    activity = snapshot.last_activity
    if activity is not None:
        summary: ActivityPayload = {"kind": activity.kind}
        if activity.state:
            summary["state"] = activity.state
        if activity.tool_name:
            summary["tool_name"] = activity.tool_name
        if snapshot.last_activity_age_ms is not None:
            summary["age_ms"] = snapshot.last_activity_age_ms
        response["activity"] = summary
    output, age = manager.recent_output(invocation_id)
    if output and age is not None:
        response["output"] = output
        response["output_age_ms"] = age
    if update is not None:
        response["error"] = {"code": update.code, "message": update.message}
    return response


def _unknown_invocation() -> ToolError:
    return ToolError(
        "Unknown or expired invocation_id. Use the invocation_id returned by start_agent during "
        "this MCP server session; handles expire seven days after completion. "
        "Keep the final output and native session_id before expiry."
    )


@_tool(
    title="Run a task with a local AI CLI",
    annotations=ToolAnnotations(
        read_only_hint=False,
        destructive_hint=True,
        idempotent_hint=False,
        open_world_hint=True,
    ),
)
async def start_agent(
    provider: Annotated[
        ProviderName,
        Field(description="CLI requested by the user, not a model ID. " + PROVIDER_SELECTION),
    ],
    prompt: Annotated[
        str,
        Field(
            min_length=1,
            description="Complete task, context, constraints, and expected output; the CLI does not inherit this conversation.",
        ),
    ],
    cwd: Annotated[
        str,
        Field(min_length=1, description="Absolute existing directory where the subagent runs."),
    ],
    session_id: Annotated[
        str | None,
        Field(
            min_length=1,
            description=(
                "To continue a conversation, use the session_id returned by a finished invocation "
                "of the same provider; omit for a new conversation. Never use an invocation_id here."
            ),
        ),
    ] = None,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                "Explicit options for the selected CLI. Call describe_provider(provider) for "
                "supported keys. Omitted options inherit native CLI settings."
            )
        ),
    ] = None,
    *,
    ctx: Context[Runtime],
) -> InvocationPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

    The reply includes the CLI and requested model/effort; omitted options show native inheritance.
    These are requested settings, not confirmation of the CLI's effective settings.
    AgentNave imposes no runtime deadline; use cancel_agent to stop work explicitly.
    Provider-native limits still apply.
    Active invocations do not accept messages. To continue a finished conversation, pass
    its returned native session_id with a new prompt to a new start_agent call.
    """
    if provider in EXCLUDED_PROVIDERS:
        raise ToolError(f"Provider '{provider}' is excluded by this host. " + PROVIDER_SELECTION)
    try:
        request = InvocationRequest(
            provider=provider,
            prompt=prompt,
            cwd=Path(cwd),
            session_id=session_id,
            provider_options=provider_options or {},
        )
        invocation_id = _manager(ctx).start(request)
    except ValueError as exc:
        supported = (
            f" Supported {provider} options: model, effort, {PROVIDER_OPTIONS[provider]}."
            if "option" in str(exc)
            else ""
        )
        raise ToolError(
            f"Invalid invocation request: {exc}.{supported} Correct the arguments and retry."
        ) from exc
    except OSError as exc:
        raise ToolError(
            f"Unable to prepare invocation: {exc}. Check local filesystem access."
        ) from exc
    return started_payload(invocation_id, request)


@_tool(
    title="Wait for a subagent invocation",
    annotations=ToolAnnotations(
        read_only_hint=True,
        destructive_hint=False,
        idempotent_hint=True,
        open_world_hint=False,
    ),
)
async def wait_agent(
    invocation_id: Annotated[
        str,
        Field(min_length=1, description="Invocation ID returned by start_agent."),
    ],
    *,
    ctx: Context[Runtime],
) -> InvocationPayload:
    """Wait for a fixed window of five minutes; completion or an explicit CLI execution blocker returns early.

    Running responses include the latest public reply tail (at most 1000 characters) and its age.
    execution_blocked leaves the CLI running: continue waiting or cancel with the same ID.
    Each blocker category wakes once per invocation. Ordinary tool failures, transient retries,
    and silence are not proof the task cannot proceed. Finished responses contain the final reply.
    Wait expiry leaves the invocation running. Call again with the same ID to continue.
    This is request/response waiting, not a background notification subscription.
    Finished invocation handles expire seven days after completion, even if not yet read.
    """
    manager = _manager(ctx)
    try:
        update = await manager.wait_for_update(invocation_id, WAIT_SECONDS)
        return update_payload(manager, invocation_id, update)
    except KeyError as exc:
        raise _unknown_invocation() from exc


@_tool(
    title="Wait for any of several subagent invocations",
    annotations=ToolAnnotations(
        read_only_hint=True,
        destructive_hint=False,
        idempotent_hint=True,
        open_world_hint=False,
    ),
)
async def wait_any(
    invocation_ids: Annotated[
        list[str],
        Field(
            min_length=1,
            description="Distinct invocation IDs returned by start_agent; all must be known.",
        ),
    ],
    *,
    ctx: Context[Runtime],
) -> WaitAnyPayload:
    """Wait up to five minutes for the first of several invocations to finish or report a blocker.

    reason=ready returns every listed invocation that is finished or has an unread blocker, in
    wait_agent's shape; the others keep running and are not in updates. Only the blockers
    returned are consumed; a later wait still returns the rest. Finished invocations stay ready
    on every call, so drop them from the next list. reason=wait_elapsed returns a running
    snapshot of each and consumes nothing. Empty, repeated, unknown or expired IDs reject the
    whole call before waiting.
    """
    manager = _manager(ctx)
    try:
        updates = await manager.wait_for_any(invocation_ids, WAIT_SECONDS)
    except KeyError as exc:
        raise _unknown_invocation() from exc
    except ValueError as exc:
        raise ToolError(f"Invalid invocation_ids: {exc}. Correct the list and retry.") from exc
    if updates:
        return {
            "reason": "ready",
            "updates": [
                update_payload(manager, invocation_id, update)
                for invocation_id, update in updates.items()
            ],
        }
    return {
        "reason": "wait_elapsed",
        "updates": [
            update_payload(manager, invocation_id, None) for invocation_id in invocation_ids
        ],
    }


@_tool(
    title="Cancel a subagent invocation",
    annotations=ToolAnnotations(
        read_only_hint=False,
        destructive_hint=True,
        idempotent_hint=True,
        open_world_hint=False,
    ),
)
async def cancel_agent(
    invocation_id: Annotated[
        str,
        Field(min_length=1, description="Invocation ID returned by start_agent."),
    ],
    *,
    ctx: Context[Runtime],
) -> InvocationPayload:
    """Stop a task by invocation_id; use wait_agent to observe without stopping.

    Returns the cancelled or already-finished result. Does not undo prior CLI side effects.
    Finished invocation handles expire seven days after completion.
    """
    try:
        result = await _manager(ctx).cancel(invocation_id)
    except KeyError as exc:
        raise _unknown_invocation() from exc
    return finished_payload(invocation_id, result)


@_tool(
    title="List subagent invocations",
    annotations=ToolAnnotations(
        read_only_hint=True,
        destructive_hint=False,
        idempotent_hint=True,
        open_world_hint=False,
    ),
)
async def list_invocations(*, ctx: Context[Runtime]) -> InvocationListPayload:
    """List this server's active invocations and finished ones not yet expired, oldest first.

    Metadata only: no prompt, reply or tool data. events_seen tells whether the CLI has emitted
    any structured event. Active entries include phase; finished ones keep a fixed elapsed_ms,
    tell with result_read whether a wait or cancel has returned the final result, and include
    a non-empty native session_id. Use wait_agent or wait_any to read results.
    """
    items: list[InvocationListItem] = []
    for listing in _manager(ctx).list_invocations():
        item: InvocationListItem = {
            "invocation_id": listing.invocation_id,
            "provider": listing.provider,
            "cwd": str(listing.cwd),
            "status": "running" if listing.status is None else listing.status.value,
            "elapsed_ms": listing.elapsed_ms,
            "events_seen": listing.events_seen,
        }
        if listing.phase is not None:
            item["phase"] = listing.phase.value
        else:
            item["result_read"] = listing.result_read
        if listing.session_id:
            item["session_id"] = listing.session_id
        items.append(item)
    return {"invocations": items}


@_tool(
    title="Read one CLI's permitted status and options",
    annotations=ToolAnnotations(
        read_only_hint=True,
        destructive_hint=False,
        idempotent_hint=True,
        open_world_hint=False,
    ),
)
async def describe_provider(
    provider: Annotated[
        ProviderName, Field(description="Provider whose permitted status and options to return.")
    ],
) -> ProviderDescriptionPayload:
    """Read one provider's permitted status and supported options.

    Does not launch a CLI or check installation/login. permitted applies to start_agent;
    open_group accepts every provider.
    """
    return {
        "provider": provider,
        "permitted": provider not in EXCLUDED_PROVIDERS,
        "supported_options": "model, effort, " + PROVIDER_OPTIONS[provider],
    }


def _groups(ctx: Context[Runtime]) -> Groups:
    return ctx.request_context.lifespan_context.groups


def round_payload(
    round_: Round, manager: InvocationManager, reason: str, *, include_output: bool = True
) -> RoundPayload:
    seats: list[SeatRunPayload] = []
    for run in round_.runs:
        seat: SeatRunPayload = {"seat_id": run.seat_id, "status": run.status}
        if run.invocation_id is not None:
            seat["invocation_id"] = run.invocation_id
        result = run.result
        if result is not None:
            if include_output and result.output:
                seat["output"] = result.output
            if result.session_id:
                seat["session_id"] = result.session_id
            if result.error:
                seat["error"] = {"code": result.error.code, "message": result.error.message}
        elif run.launch_error is not None:
            seat["error"] = {"code": run.launch_error.code, "message": run.launch_error.message}
        elif run.status == "running" and run.invocation_id is not None:
            snapshot = manager.snapshot(run.invocation_id)
            activity = snapshot.last_activity
            if activity is not None:
                summary: ActivityPayload = {"kind": activity.kind}
                if activity.state:
                    summary["state"] = activity.state
                if activity.tool_name:
                    summary["tool_name"] = activity.tool_name
                if snapshot.last_activity_age_ms is not None:
                    summary["age_ms"] = snapshot.last_activity_age_ms
                seat["activity"] = summary
            output, age = manager.recent_output(run.invocation_id) if include_output else ("", None)
            if output and age is not None:
                seat["output"] = output
                seat["output_age_ms"] = age
            if run.blockers:
                latest = run.blockers[-1]
                seat["error"] = {"code": latest.code, "message": latest.message}
        seats.append(seat)
    finished = round_.finished
    payload: RoundPayload = {
        "round_id": round_.id,
        "group_id": round_.group.id,
        "round": round_.number,
        "order": round_.order,
        "status": ("failed" if round_.error else "cancelled" if round_.cancelling else "finished")
        if finished
        else "running",
        "reason": "finished" if finished else reason,
        "elapsed_ms": round(
            (
                (round_.finished_at if round_.finished_at is not None else time.monotonic())
                - round_.started_at
            )
            * 1000
        ),
        "seats": seats,
    }
    if round_.error is not None:
        payload["error"] = round_.error
    return payload


def _unknown_round() -> ToolError:
    return ToolError(
        "Unknown round_id. Use a round_id returned by run_round in this server session."
    )


@_tool(
    title="Open a group of CLI seats",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)
async def open_group(
    cwd: Annotated[
        str, Field(min_length=1, description="Absolute existing directory seats run in by default.")
    ],
    seats: Annotated[list[GroupSeatSpec], Field(min_length=2, max_length=6)],
    *,
    ctx: Context[Runtime],
) -> GroupPayload:
    """Create an in-memory group of 2–6 CLI seats for collaborative rounds. Nothing is saved.

    Seats are ordinary subagents: native tools and permissions, the given cwd, explicit options.
    Each seat keeps its own native session across rounds. Host exclusions do not apply to seats.
    """
    try:
        group = _groups(ctx).open(
            Path(cwd),
            [
                GroupSeat(
                    seat.id,
                    seat.provider,
                    dict(seat.provider_options),
                    Path(seat.cwd if seat.cwd is not None else cwd),
                )
                for seat in seats
            ],
        )
    except (OSError, ValueError) as exc:
        raise ToolError(f"Invalid group: {exc}. Correct the arguments and retry.") from exc
    return {
        "group_id": group.id,
        "seats": [
            {"id": seat.id, "provider": seat.provider, "cwd": str(seat.cwd)}
            for seat in group.seats.values()
        ],
    }


@_tool(
    title="Run one round of a group",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=True
    ),
)
async def run_round(
    group_id: str,
    seats: Annotated[
        list[str],
        Field(min_length=1, description="Ordered seat ids that each answer once this round."),
    ],
    order: Annotated[
        Literal["parallel", "serial"],
        Field(
            description=(
                "parallel: all start together and cannot see each other's answer this round. "
                "serial: one by one in list order; later seats see earlier answers."
            )
        ),
    ],
    prompt: Annotated[
        str, Field(min_length=1, description="Shared task for this round, visible to all seats.")
    ],
    instructions: Annotated[
        dict[str, str] | None,
        Field(description="Optional private instruction per seat id; never sent to other seats."),
    ] = None,
    *,
    ctx: Context[Runtime],
) -> RoundPayload:
    """Start one round without waiting; use wait_round with the returned round_id.

    Seats share only final answers. Each seat receives the entries it has not yet seen: earlier
    tasks, other seats' answers and absences. Parallel answers are committed together once every
    seat ends; serial answers are committed one by one. Failed, blocked or cancelled seats are
    recorded as absent and a serial round continues. No retries or per-seat deadlines.
    """
    groups = _groups(ctx)
    try:
        round_ = groups.run(group_id, seats, order, prompt, instructions or {})
    except KeyError as exc:
        raise ToolError("Unknown group_id. Use a group_id returned by open_group.") from exc
    except ValueError as exc:
        raise ToolError(f"Invalid round: {exc}.") from exc
    return round_payload(round_, groups.manager, "started")


@_tool(
    title="Wait for a group round",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def wait_round(
    round_id: Annotated[str, Field(min_length=1)],
    include_output: Annotated[
        bool,
        Field(
            description="Include answer bodies and running reply tails; false returns status only."
        ),
    ] = True,
    *,
    ctx: Context[Runtime],
) -> RoundPayload:
    """Wait up to five minutes; returns early when the round finishes or a seat reports a blocker.

    Running seats include activity and their latest reply tail. A blocker leaves that seat
    running: keep waiting, or cancel_agent its invocation_id to record it absent.
    Finished rounds can be read again. Set include_output=false to omit answer bodies and
    reply tails; later calls with include_output=true still return the full captured answers.
    """
    groups = _groups(ctx)
    try:
        round_, reason = await groups.wait(round_id, WAIT_SECONDS)
    except KeyError as exc:
        raise _unknown_round() from exc
    return round_payload(
        round_, groups.manager, reason or "wait_elapsed", include_output=include_output
    )


@_tool(
    title="Cancel a group round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=True, open_world_hint=False
    ),
)
async def cancel_round(
    round_id: Annotated[str, Field(min_length=1)], *, ctx: Context[Runtime]
) -> RoundPayload:
    """Stop a round: running seats are cancelled and recorded absent, unstarted serial seats skip.

    Answers that already succeeded are kept and committed. Does not undo CLI side effects.
    """
    groups = _groups(ctx)
    try:
        round_ = await groups.cancel(round_id)
    except KeyError as exc:
        raise _unknown_round() from exc
    return round_payload(round_, groups.manager, "finished")


def main() -> None:
    nesting = detect_nesting()
    if nesting.status != "not_nested":
        MCPServer("AgentNave", version=__version__, instructions=nesting.instructions).run(
            transport="stdio"
        )
    else:
        mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
