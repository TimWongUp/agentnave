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
    + ". Exclusions are enforced before an invocation is created. Choose an installed, "
    "authenticated permitted CLI; if none is available, report the blocker instead of falling "
    "back to an excluded provider."
)
_MODEL_SELECTION = (
    "Pass model and effort explicitly using these defaults: "
    "claude: model=opus, effort=max; codebuddy: model=hy3, effort=high; "
    "codex: model=gpt-5.6-sol, effort=high; grok: model=grok-4.6, effort=high; "
    "antigravity: model=gemini-3.8-flash, effort=high. "
    "User-specified values override the corresponding defaults; keep defaults for unspecified "
    "fields. If the user requests native provider settings, omit those options. "
    "These are Manager instructions: the server does not inject model or effort defaults."
)


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


class InvocationSnapshotPayload(TypedDict):
    phase: InvocationPhaseName
    elapsed_ms: int
    last_event_age_ms: int | None


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
        "AgentNave is an agent-only MCP server that launches local Antigravity CLI, Claude Code, "
        "CodeBuddy Code, Codex CLI, or Grok CLI subagents. The calling Manager owns planning, "
        "role/model selection, "
        "parallelism, retries, review, synthesis, permissions, and worktrees. Call start_agent, "
        "then wait_agent; use cancel_agent only to stop an invocation. An active invocation does "
        "not accept follow-up messages. To continue or redirect a provider conversation, wait for "
        "it to finish, then pass its returned session_id and a new prompt to a new start_agent "
        "call. Invocation handles exist only for this MCP server lifetime. AgentNave is not a "
        "sandbox; provider-native permissions remain the security boundary."
        " Use AgentNave for an explicitly requested CLI subagent or a bounded task the Manager "
        "has chosen to delegate to a local CLI. "
        + _PROVIDER_SELECTION
        + " "
        + _MODEL_SELECTION
        + " Inherit native permissions and tools unless the user explicitly requests changes. "
        "Verify succeeded output before synthesis; resolve blocked results through user input, "
        "login or native permissions, without bypassing gates. Retry failed work only when the "
        "task remains valid and there is a concrete reason. Cancelled and timed_out invocations "
        "are terminal. Running snapshots describe lifecycle, not semantic task progress."
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
    title="Start a local CLI subagent",
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
        Field(description="Local subagent provider to launch. " + _PROVIDER_SELECTION),
    ],
    prompt: Annotated[
        str,
        Field(min_length=1, description="Complete, self-contained task for the subagent."),
    ],
    cwd: Annotated[
        str,
        Field(min_length=1, description="Absolute existing directory where the subagent runs."),
    ],
    session_id: Annotated[
        str | None,
        Field(
            min_length=1,
            description="Provider session ID returned by an earlier finished invocation.",
        ),
    ] = None,
    timeout_seconds: Annotated[
        float,
        Field(gt=0, le=86_400, description="Maximum provider runtime in seconds."),
    ] = 1800,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                "Explicit provider-native options. Omit to inherit the provider's own settings; "
                "supported keys: all providers accept model and effort; "
                "claude also accepts permission_mode, agent, fallback_model, max_budget_usd; "
                "codebuddy: permission_mode, agent, fallback_model; "
                "codex: skip_git_repo_check (boolean, explicitly true outside Git repositories); "
                "grok: permission_mode, agent, max_turns, sandbox; "
                "antigravity: agent, mode, project, print_timeout, sandbox (boolean), "
                "disable_slash_commands (boolean). " + _MODEL_SELECTION
            )
        ),
    ] = None,
    *,
    ctx: Context[InvocationManager],
) -> StartAgentPayload:
    """Start one subagent and return its in-memory invocation ID without waiting for completion.

    Use wait_agent with the returned ID to observe the invocation. The launched provider may read,
    write, or run commands in cwd subject to its native permission controls.
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
        Field(gt=0, le=300, description="Maximum time to wait during this call, in seconds."),
    ] = 30,
    *,
    ctx: Context[InvocationManager],
) -> WaitAgentPayload:
    """Wait briefly for one invocation and return either a running snapshot or its final result.

    A running response leaves the invocation active; call wait_agent again later. Use cancel_agent
    only when the invocation should be stopped.
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
    """Stop one invocation and return its final cancelled or already-terminal result.

    Use this only when the Manager intends to stop active provider work; use wait_agent to observe
    work without stopping it.
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


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
