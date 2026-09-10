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
from agentnave.models import InvocationRequest, InvocationResult, ProviderOption

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
    "Providers permitted by this host configuration: "
    + (
        ", ".join(
            name for name in get_args(ProviderName.__value__) if name not in _EXCLUDED_PROVIDERS
        )
        or "none"
    )
    + ". Excluded providers: "
    + (", ".join(sorted(_EXCLUDED_PROVIDERS)) or "none")
    + ". Excluded providers are rejected before launch."
)
_PROVIDER_OPTIONS: dict[ProviderName, str] = {
    "claude": "permission_mode, agent, fallback_model, max_budget_usd",
    "codebuddy": "permission_mode, agent, fallback_model",
    "codex": "skip_git_repo_check (boolean; explicitly true outside Git repositories)",
    "grok": "permission_mode, agent, max_turns, sandbox",
    "antigravity": (
        "agent, mode, project, print_timeout, sandbox (boolean), disable_slash_commands (boolean)"
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
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


@asynccontextmanager
async def _lifespan(_server: MCPServer[InvocationManager]) -> AsyncGenerator[InvocationManager]:
    manager = InvocationManager()
    try:
        yield manager
    finally:
        await manager.shutdown()


mcp = MCPServer(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave launches local CLI subagents. Use the agentnave-manager Skill for CLI usage "
        "and model selection. describe_provider reports permitted status and supported options. "
        "Call start_agent, then wait_agent with its invocation_id; "
        "cancel_agent stops work. Handles last only for this server process. "
        "Provider-native permissions remain the security boundary. " + _PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


def _manager(ctx: Context[InvocationManager]) -> InvocationManager:
    return ctx.request_context.lifespan_context


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
    *,
    ctx: Context[InvocationManager],
) -> InvocationPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

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
        invocation_id = _manager(ctx).start(request)
    except ValueError as exc:
        raise ToolError(
            f"Invalid invocation request: {exc}. Correct the arguments and retry."
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
    ctx: Context[InvocationManager],
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
    ctx: Context[InvocationManager],
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

    Does not launch a CLI or check installation/login. Excluded providers cannot be started.
    """
    return {
        "provider": provider,
        "permitted": provider not in _EXCLUDED_PROVIDERS,
        "supported_options": "model, effort, " + _PROVIDER_OPTIONS[provider],
    }


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
