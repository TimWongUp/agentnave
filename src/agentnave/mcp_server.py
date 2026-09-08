"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import os
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal, NotRequired, TypedDict, cast, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import ToolAnnotations
from pydantic import Field

from agentnave import __version__
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationResult, ProviderOption

type ProviderName = Literal["antigravity", "claude", "codebuddy", "codex", "grok"]
type InvocationStatusName = Literal["succeeded", "failed", "blocked", "cancelled", "timed_out"]
type InvocationPhaseName = Literal["preparing", "running", "stopping"]


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
    details: str | None


class ProviderUsagePayload(TypedDict, total=False):
    num_turns: int
    total_cost_usd: float


class InvocationResultPayload(TypedDict):
    status: InvocationStatusName
    provider: ProviderName
    output: str
    session_id: str | None
    provider_usage: ProviderUsagePayload
    duration_ms: int
    error: InvocationErrorPayload | None


class ProviderActivityPayload(TypedDict):
    kind: str
    event_type: str
    state: str | None
    tool_name: str | None
    message: str | None
    tool_call_id: str | None


class InvocationSnapshotPayload(TypedDict):
    phase: InvocationPhaseName
    elapsed_ms: int
    last_event_age_ms: int | None
    last_activity: ProviderActivityPayload | None
    last_activity_age_ms: int | None
    remaining_ms: int | None


class StartAgentPayload(TypedDict):
    invocation_id: str
    state: Literal["running"]


class WaitAgentPayload(TypedDict):
    invocation_id: str
    state: Literal["running", "finished"]
    snapshot: NotRequired[InvocationSnapshotPayload]
    result: NotRequired[InvocationResultPayload]


class CancelAgentPayload(TypedDict):
    invocation_id: str
    state: Literal["finished"]
    result: InvocationResultPayload


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


def _result_payload(result: InvocationResult) -> InvocationResultPayload:
    return cast(InvocationResultPayload, result.to_dict())


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
    timeout_seconds: Annotated[
        float | None,
        Field(
            gt=0,
            le=86_400,
            description="Optional total runtime limit in seconds; expiry stops the invocation. Omit or null for no AgentNave deadline; native CLI limits still apply.",
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
) -> StartAgentPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

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
            timeout_seconds=timeout_seconds,
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
    return {"invocation_id": invocation_id, "state": "running"}


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
    wait_timeout_seconds: Annotated[
        float,
        Field(
            gt=0,
            le=300,
            description="Seconds to wait for this response; expiry leaves the invocation running.",
        ),
    ] = 120,
    *,
    ctx: Context[InvocationManager],
) -> WaitAgentPayload:
    """Wait for a task using the invocation_id returned by start_agent.

    state=running: call again with the same ID; do not launch a duplicate.
    state=finished: inspect result.status, output, and error; completion does not imply success.
    This wait returns early on completion; expiry does not stop the task or mean timed_out.
    Snapshots include the latest observed activity and its age, not all active work or proof
    of a stall. Missing activity is unknown; silence alone does not justify cancellation.
    """
    manager = _manager(ctx)
    try:
        result = await manager.wait(invocation_id, wait_timeout_seconds)
        if result is None:
            snapshot = manager.snapshot(invocation_id)
            return {
                "invocation_id": invocation_id,
                "state": "running",
                "snapshot": cast(InvocationSnapshotPayload, snapshot.to_dict()),
            }
    except KeyError as exc:
        raise _unknown_invocation() from exc
    return {
        "invocation_id": invocation_id,
        "state": "finished",
        "result": _result_payload(result),
    }


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
) -> CancelAgentPayload:
    """Stop a task by invocation_id; use wait_agent to observe without stopping.

    Returns the cancelled or already-finished result. Does not undo prior CLI side effects.
    """
    try:
        result = await _manager(ctx).cancel(invocation_id)
    except KeyError as exc:
        raise _unknown_invocation() from exc
    return {
        "invocation_id": invocation_id,
        "state": "finished",
        "result": _result_payload(result),
    }


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
