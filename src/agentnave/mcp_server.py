"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import os
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal, NotRequired, TypedDict, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import ToolAnnotations
from pydantic import Field

from agentnave import __version__
from agentnave.core import InvocationManager
from agentnave.discussion import RoomSummary, RoomView, Seat
from agentnave.models import InvocationRequest, InvocationResult, ProviderOption
from agentnave.workbench import DeletionResult, Workbench

type ProviderName = Literal["antigravity", "claude", "codebuddy", "codex", "grok"]


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

_EXCLUDED_PROVIDERS = _read_excluded_providers()
_PROVIDER_SELECTION = (
    "Providers permitted for ordinary start_agent by this host configuration: "
    + (
        ", ".join(
            name for name in get_args(ProviderName.__value__) if name not in _EXCLUDED_PROVIDERS
        )
        or "none"
    )
    + ". Excluded providers: "
    + (", ".join(sorted(_EXCLUDED_PROVIDERS)) or "none")
    + ". Excluded providers are rejected by start_agent. Explicit discussion rooms may include every provider, including the host CLI."
)
_PROVIDER_OPTIONS: dict[ProviderName, str] = {
    "claude": "permission_mode, agent, fallback_model, max_budget_usd, discussion_mode (boolean; restricted tools)",
    "codebuddy": "permission_mode, agent, fallback_model, discussion_mode (boolean; restricted tools)",
    "codex": (
        "skip_git_repo_check (boolean; explicitly true outside Git repositories), "
        "dangerously_bypass_approvals_and_sandbox "
        "(boolean; true skips approvals and disables sandboxing; false or omitted adds no flag), "
        "discussion_mode (boolean; native read/tool restrictions)"
    ),
    "grok": "permission_mode, agent, max_turns, sandbox, discussion_mode (boolean; denies tool execution)",
    "antigravity": (
        "agent, mode, project, print_timeout, sandbox (boolean), disable_slash_commands (boolean), "
        "dangerously_skip_permissions (boolean; explicitly true enables YOLO for this invocation), "
        "discussion_mode (boolean; dedicated tool-free participant profile)"
    ),
}


class ProviderDescriptionPayload(TypedDict):
    provider: ProviderName
    permitted: bool
    discussion_permitted: bool
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
    conversation_id: NotRequired[str]
    workbench_url: NotRequired[str]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


class Runtime:
    def __init__(self) -> None:
        self.manager = InvocationManager()
        self.rooms = Workbench(
            self.manager,
            Path(os.environ.get("AGENTNAVE_DATA_DIR", str(Path.home() / ".agentnave"))),
        )


@asynccontextmanager
async def _lifespan(_server: MCPServer[Runtime]) -> AsyncGenerator[Runtime]:
    runtime = Runtime()
    try:
        yield runtime
    finally:
        try:
            await runtime.manager.shutdown()
        finally:
            await runtime.rooms.shutdown()


mcp = MCPServer(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave launches local CLI subagents. Use the agentnave-manager Skill for CLI usage "
        "and model selection. describe_provider reports permitted status and supported options. "
        "Call start_agent, then wait_agent with its invocation_id; "
        "cancel_agent stops work. Handles last only for this server process. "
        "Optional discussion rooms expose public dialogue on a local read-only Dashboard; "
        "open_discussion opts in. Provider-native permissions remain the security boundary. "
        + _PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


def _manager(ctx: Context[Runtime]) -> InvocationManager:
    return ctx.request_context.lifespan_context.manager


def _finished_payload(invocation_id: str, result: InvocationResult) -> InvocationPayload:
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


def _unknown_invocation() -> ToolError:
    return ToolError(
        "Unknown invocation_id. Use the invocation_id returned by start_agent during this "
        "MCP server session."
    )


@mcp.tool(
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
        Field(description="CLI requested by the user, not a model ID. " + _PROVIDER_SELECTION),
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
    conversation_id: str | None = None,
    title: str | None = None,
    *,
    ctx: Context[Runtime],
) -> InvocationPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

    Output is collected directly into a private local task conversation. Use conversation_id to
    group related tasks; title names a new conversation. No task output is shared with another CLI.
    AgentNave imposes no runtime deadline; use cancel_agent to stop work explicitly.
    Provider-native limits still apply.
    Active invocations do not accept messages. To continue a finished conversation, pass
    its returned native session_id with a new prompt to a new start_agent call.
    """
    if provider in _EXCLUDED_PROVIDERS:
        raise ToolError(f"Provider '{provider}' is excluded by this host. " + _PROVIDER_SELECTION)
    try:
        request = InvocationRequest(
            provider=provider,
            prompt=prompt,
            cwd=Path(cwd),
            session_id=session_id,
            provider_options=provider_options or {},
        )
        invocation_id, conversation = ctx.request_context.lifespan_context.rooms.start_task(
            request, title, conversation_id
        )
    except ValueError as exc:
        supported = (
            f" Supported {provider} options: model, effort, {_PROVIDER_OPTIONS[provider]}."
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
    return {
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "started",
        "elapsed_ms": 0,
        "conversation_id": conversation.room.id,
        "workbench_url": conversation.workbench_url,
    }


@mcp.tool(
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
    """
    manager = _manager(ctx)
    try:
        result = await manager.wait_for_update(invocation_id, WAIT_SECONDS)
        if isinstance(result, InvocationResult):
            return _finished_payload(invocation_id, result)
        snapshot = manager.snapshot(invocation_id)
        response: InvocationPayload = {
            "invocation_id": invocation_id,
            "status": "running",
            "reason": "execution_blocked" if result is not None else "wait_elapsed",
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
        if result is not None:
            response["error"] = {"code": result.code, "message": result.message}
        return response
    except KeyError as exc:
        raise _unknown_invocation() from exc


@mcp.tool(
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
    """
    try:
        result = await _manager(ctx).cancel(invocation_id)
    except KeyError as exc:
        raise _unknown_invocation() from exc
    return _finished_payload(invocation_id, result)


@mcp.tool(
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

    Does not launch a CLI or check installation/login. permitted applies to ordinary start_agent;
    discussion_permitted applies to the explicit room tools, including the host CLI.
    """
    return {
        "provider": provider,
        "permitted": provider not in _EXCLUDED_PROVIDERS,
        "discussion_permitted": True,
        "supported_options": "model, effort, " + _PROVIDER_OPTIONS[provider],
    }


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def open_discussion(
    title: Annotated[str, Field(min_length=1, max_length=200)],
    seats: Annotated[list[Seat], Field(min_length=2, max_length=6)],
    ctx: Context[Runtime],
    mode: Literal["discussion", "blind"] = "discussion",
    directory: str | None = None,
) -> RoomView:
    """Create/reopen a public discussion board and a separate director desk. Directory must be absolute.

    Persists a private room.json in the chosen directory. Reopening requires the original title/seats/mode.
    Each directory is a separate show with independent dialogue, director state and native seat sessions.
    All five providers can participate, including this host's CLI; ordinary start_agent exclusions
    remain unchanged. Native discussion profiles restrict tools, not OS processes.
    The director alone controls turns. Use blind mode for independent answers: CLI replies are automatically
    sealed privately, then the director explicitly reveals the full round. discussion mode publishes individually.
    Never reuse already-exposed native histories as a fresh blind test.
    """
    try:
        rooms = ctx.request_context.lifespan_context.rooms
        return rooms.open(
            Path(directory) if directory else rooms.new_directory(), title, seats, mode
        )
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=True, destructive_hint=False, open_world_hint=False)
)
async def list_conversations(ctx: Context[Runtime]) -> list[RoomSummary]:
    """List private workbench conversations, including ordinary tasks. Task conversations have no public URL.

    Director-only: never send this list to participants. Reopen a saved directory with
    open_discussion after a server restart. Each room keeps independent native seat sessions.
    """
    return ctx.request_context.lifespan_context.rooms.list()


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=True, open_world_hint=True)
)
async def start_discussion_turn(
    room_id: str,
    seat_id: str,
    instruction: Annotated[str, Field(min_length=1)],
    ctx: Context[Runtime],
) -> RoomView:
    """Ask one seat to speak; the shared public board supplies only this seat's unread dialogue.

    All participants read the same published messages. Each seat resumes its own native session;
    previously delivered dialogue is not repeated. Private drafts and other seats' histories are excluded.
    Do not put director-only secrets in instruction. No scheduling, retries or model fallback.
    Use wait_agent/cancel_agent with the returned turn.invocation_id, then read_conversation.
    Final JSON public_text is collected directly from the CLI, without manager transcription.
    Discussion mode retains a candidate for review; blind mode automatically seals a valid reply
    in pending_answers so the next seat can start. It is NEVER published automatically.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.start(room_id, seat_id, instruction)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=True, destructive_hint=False, open_world_hint=False)
)
async def read_conversation(room_id: str, ctx: Context[Runtime]) -> RoomView:
    """Read director state, including the private candidate. Never forward this response to a seat.

    public_url serves only the shared public board; director_url also shows the private current turn.
    Neither page exposes native session or invocation IDs. Never give director_url to participants.
    """
    try:
        return await ctx.request_context.lifespan_context.rooms.read(room_id)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def decide_discussion_turn(
    room_id: str,
    turn_id: str,
    action: Literal["publish", "discard"],
    ctx: Context[Runtime],
) -> RoomView:
    """Publish or discard a discussion-mode candidate. Repeating the same decision is idempotent.

    Blind replies are automatically sealed by the collector, without this per-seat call.
    Blind mode rejects individual publication; use decide_discussion_round for the full set.

    No editing or impersonation of participant dialogue. For a rewrite, discard and request a new turn.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.decide(room_id, turn_id, action)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def decide_discussion_round(
    room_id: str,
    round_id: str,
    action: Literal["publish", "discard"],
    ctx: Context[Runtime],
) -> RoomView:
    """Reveal all sealed answers atomically after every seat submits, or discard the sealed round.

    round_id is the blind_round_id from director state. Cannot reveal a partial or active turn.
    Discard can end an incomplete round after resolving its active candidate. Never forwards
    sealed answers to participants; only explicit publication adds them to their public context.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.decide_round(room_id, round_id, action)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def post_discussion_message(
    room_id: str,
    text: Annotated[str, Field(min_length=1, max_length=12000)],
    ctx: Context[Runtime],
) -> RoomView:
    """Speak publicly as the host played by the director. Never send backstage notes here.

    Resolve any active turn first. The stable speaker key is director."""
    try:
        return ctx.request_context.lifespan_context.rooms.post(room_id, text)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=True, open_world_hint=False)
)
async def reset_discussion_seat(room_id: str, seat_id: str, ctx: Context[Runtime]) -> RoomView:
    """Explicitly detach one seat's session and known-message list; public dialogue is retained.

    Use for a new identity or uncertain failed/interrupted history. Public board history remains visible.
    Resolve a running or ready turn first. Native CLI history is not deleted.
    The next turn starts a new session; other seats keep their own sessions.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.reset(room_id, seat_id)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=True, destructive_hint=False, open_world_hint=False)
)
async def open_workbench(ctx: Context[Runtime]) -> dict[str, str]:
    """Open the private unified workbench. Local history survives restart, running processes do not.

    Only tasks launched through this MCP runtime are live here. Other hosts' locked histories are
    listed as unavailable. The dashboard is read-only and never grants shared context to CLIs.
    """
    rooms = ctx.request_context.lifespan_context.rooms
    return {"workbench_url": rooms.home(), "storage": str(rooms.directory)}


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def update_conversation(
    room_id: str, ctx: Context[Runtime], title: str | None = None, archived: bool | None = None
) -> RoomView:
    """Rename, end or reopen a conversation without deleting history. Resolve active work first."""
    try:
        return ctx.request_context.lifespan_context.rooms.update(room_id, title, archived)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=True, open_world_hint=False)
)
async def delete_conversation(room_id: str, ctx: Context[Runtime]) -> DeletionResult:
    """Permanently delete an archived, inactive conversation after user authorization.

    Removes AgentNave history, index, views and finished invocation handles. Native CLI files
    and nonempty directories are retained and reported. External root directories are preserved.
    No automatic retention policy. The browser remains read-only.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.delete(room_id)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def mark_discussion_absent(
    room_id: str, seat_id: str, reason: str, ctx: Context[Runtime]
) -> RoomView:
    """Explicitly excuse an unanswered seat from a blind round; revealing then marks results incomplete.

    Cancel and wait for this seat's active invocation first. No automatic timeout, skip or retry.
    Excused seats cannot submit late into the same round after other answers are revealed.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.mark_absent(room_id, seat_id, reason)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


@mcp.tool(
    annotations=ToolAnnotations(read_only_hint=False, destructive_hint=False, open_world_hint=False)
)
async def continue_discussion(room_id: str, ctx: Context[Runtime]) -> RoomView:
    """After revealing independent answers, continue together in the same native seat sessions.

    One-way blind-to-discussion transition only; never claims exposed history is a fresh blind test.
    """
    try:
        return ctx.request_context.lifespan_context.rooms.continue_discussion(room_id)
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
