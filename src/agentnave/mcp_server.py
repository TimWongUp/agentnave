"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
import os
import time
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal, NotRequired, TypedDict, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import ToolAnnotations
from pydantic import BaseModel, ConfigDict, Field

from agentnave import __version__
from agentnave.conversations import (
    CommittedEntry,
    Conversations,
    Decision,
    ForwardError,
    entry_message_id,
)
from agentnave.core import InvocationManager
from agentnave.group import GroupSeat, Round
from agentnave.host import HostName, HostSource, detect_host
from agentnave.members import (
    AdapterOptionsError,
    MemberFileError,
    MemberList,
    ProviderExcludedError,
    ProviderName,
    apply_member,
    read_members,
    validate_provider_options,
)
from agentnave.members import list_members as read_member_list
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
)
from agentnave.nesting import detect_nesting
from agentnave.store import Conversation, Review, Unavailable

type ManagerModel = Annotated[
    str | None,
    Field(
        min_length=1,
        max_length=200,
        description=(
            "Your current manager model ID, when known from trusted host context. "
            "Metadata for this message only; never the subagent model or a CLI-derived guess. "
            "Omit when unknown. Does not change provider settings."
        ),
    ),
]

type ManagerSessionId = Annotated[
    str | None,
    Field(
        min_length=1,
        max_length=200,
        description=(
            "Current manager chat/session ID, shared by all tasks and groups in that chat. "
            "Use the host's real session ID when available; otherwise generate one UUID once "
            "for this manager chat and keep reusing it. A different chat needs a different ID. "
            "Never use the CLI name, MCP connection ID, or subagent session_id. "
            "Omission uses AGENTNAVE_HOST_SESSION_ID if configured, else keeps records separate."
        ),
    ),
]


def _read_excluded_providers() -> frozenset[str]:
    excluded = frozenset(
        name.strip().lower()
        for name in os.environ.get("AGENTNAVE_EXCLUDED_PROVIDERS", "").split(",")
        if name.strip()
    )
    unknown = excluded.difference(get_args(ProviderName.__value__))
    if unknown:
        raise ValueError("Unknown AGENTNAVE_EXCLUDED_PROVIDERS: " + ", ".join(sorted(unknown)))
    host_provider = HOST.provider
    return excluded if host_provider is None else excluded | {host_provider}


WAIT_SECONDS = 300

HOST = detect_host()
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
    + ". start_agent rejects excluded providers; groups follow discussion member limits without host exclusions."
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
    show_permitted: bool
    supported_options: str
    host: HostName | None
    host_source: HostSource


class ViewerLinks(TypedDict):
    dashboard_url: NotRequired[str]
    dashboard_error: NotRequired[str]


class DashboardLinks(ViewerLinks):
    public_url: NotRequired[str]
    director_url: NotRequired[str]


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
    defaults_applied: NotRequired[list[str]]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]
    message_id: NotRequired[str]  # terminal saved task, referencable by forward


class TaskStartPayload(InvocationPayload, DashboardLinks):
    conversation_id: str
    resumed: bool  # true when start_agent continued this desk's most recent private thread
    access: str | None  # declared desk tier after inheritance; never enforced


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
    restart_safe: bool


class GroupSeatSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: Annotated[str, Field(pattern=r"^[a-z][a-z0-9_-]{0,31}$")]
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(
        default_factory=lambda: {},
        description="Options for this seat's CLI; omitted keys use discussion member defaults.",
    )
    cwd: str | None = Field(
        default=None,
        description="Absolute directory for this seat; defaults to the group cwd.",
    )
    role: str | None = Field(
        default=None,
        description=(
            "Optional desk role for this seat; 1–40 characters after trimming, no newlines "
            "or control characters. Blank or omitted means no role."
        ),
    )
    access: Literal["read", "write"] | None = Field(
        default=None,
        description="Declared desk access: read for exploring, planning or reviewing, write for changing files. Recorded and shown only; it does not change the CLI's native permissions. A desk keeps its first declared tier in this manager workspace; omitting it inherits that tier, and a different tier is rejected, so use another role and pass findings with forward or a meeting instead.",
    )


class GroupSeatPayload(TypedDict):
    id: str
    provider: str
    cwd: str
    model: str | None
    effort: str | None
    role: str | None
    access: str | None
    defaults_applied: list[str]


class ShowPayload(DashboardLinks):
    group_id: str
    conversation_id: str
    seats: list[GroupSeatPayload]


class SeatRunPayload(TypedDict):
    seat_id: str
    status: Literal[
        "pending",
        "running",
        "succeeded",
        "failed",
        "blocked",
        "cancelled",
        "not_started",
        "pending_review",
        "discarded",
    ]
    invocation_id: NotRequired[str]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


class CommittedEntryPayload(TypedDict):
    message_id: str
    kind: Literal["task", "answer", "absent", "forward", "host"]
    round: int
    seat: NotRequired[str]


class RoundPayload(DashboardLinks):
    round_id: str
    group_id: str
    round: int
    order: Literal["parallel", "serial"]
    status: Literal["running", "finished", "failed", "cancelled"]
    reason: str
    elapsed_ms: int
    seats: list[SeatRunPayload]
    entries: list[CommittedEntryPayload]
    error: NotRequired[str]


class Runtime:
    def __init__(self) -> None:
        self.manager = InvocationManager()
        root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
        self.conversations = Conversations(self.manager, root, host=HOST)


@asynccontextmanager
async def _lifespan(_server: MCPServer[Runtime]) -> AsyncGenerator[Runtime]:
    runtime = Runtime()
    try:
        yield runtime
    finally:
        try:
            await runtime.conversations.shutdown()
        finally:
            try:
                await runtime.manager.shutdown()
            finally:
                await runtime.conversations.close()


mcp = MCPServer(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave launches local CLI subagents and saves their history. Use the "
        "agentnave-manager Skill for CLI usage and model selection. "
        "start_agent/wait_agent/cancel_agent run tasks saved as private task conversations; "
        "wait_any waits on several and list_invocations lists handles. "
        "describe_provider reports permitted status and supported options. "
        "list_members reads App members by mode; omitted model/effort/options use App defaults "
        "reported in defaults_applied. Explicit arguments take priority within allowed ranges. "
        "open_group creates a saved show; run_round, wait_round and cancel_round drive it. "
        "With review=true, decide_answers publishes or discards answers before other seats see "
        "them. post_host_message adds public host remarks. Saved operations and "
        "list_conversations provide local read-only dashboard links; nothing listens before "
        "these are called. History survives restart; running processes do not. "
        "Invocation handles expire seven days after completion. "
        "Provider-native permissions remain the security boundary; isolation is not a sandbox. "
        + PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


def _manager(ctx: Context[Runtime]) -> InvocationManager:
    return ctx.request_context.lifespan_context.manager


def _conversations(ctx: Context[Runtime]) -> Conversations:
    return ctx.request_context.lifespan_context.conversations


def _rejected(exc: Exception) -> ToolError:
    return ToolError(f"{exc}. Correct the request and retry.")


def _dashboard_links(conversations: Conversations, conversation_id: str) -> DashboardLinks:
    """A saved operation stays successful even if the optional viewer cannot start."""
    try:
        home = conversations.open_dashboard()
    except OSError as exc:
        return {"dashboard_error": str(exc)}
    links: DashboardLinks = {"dashboard_url": home}
    urls = conversations.urls(conversation_id)
    if urls is not None:
        links["director_url"] = urls[1]
        if conversations.records[conversation_id].state.kind == "show":
            links["public_url"] = urls[0]
    return links


def started_payload(
    invocation_id: str, request: InvocationRequest, defaults_applied: list[str] | None = None
) -> InvocationPayload:
    payload: InvocationPayload = {
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "started",
        "elapsed_ms": 0,
        "provider": request.provider,
        "defaults_applied": defaults_applied or [],
        "effort": str(request.provider_options.get("effort", "继承原生设置")),
    }
    if "model" in request.provider_options:
        payload["model"] = str(request.provider_options["model"])
    return payload


def invocation_request_error(provider: ProviderName, exc: ValueError) -> ToolError:
    if isinstance(exc, MemberFileError):
        return ToolError(str(exc))
    if isinstance(exc, ProviderExcludedError):
        return ToolError(str(exc) + ". " + PROVIDER_SELECTION)
    if isinstance(exc, ForwardError):
        return ToolError(f"{exc}. Correct the arguments and retry.")
    supported = (
        f" Supported {provider} options: model, effort, {PROVIDER_OPTIONS[provider]}."
        if isinstance(exc, AdapterOptionsError)
        else ""
    )
    return ToolError(
        f"Invalid invocation request: {exc}.{supported} Correct the arguments and retry."
    )


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
    if result.reported_model:
        payload["model"] = result.reported_model
    if result.error:
        payload["error"] = {"code": result.error.code, "message": result.error.message}
    return payload


def update_payload(
    manager: InvocationManager,
    conversations: Conversations,
    invocation_id: str,
    update: InvocationResult | InvocationError | None,
) -> InvocationPayload:
    """Final result, or a running snapshot that reports the given blocker."""
    if isinstance(update, InvocationResult):
        response = finished_payload(invocation_id, update)
        message_id = conversations.task_message_id(invocation_id)
        if message_id is not None:
            response["message_id"] = message_id
        return response
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
                "To continue a specific native conversation, use the session_id returned by a "
                "finished invocation of the same provider; never use an invocation_id here. "
                "When omitted with a known manager_session_id, the task continues this desk's "
                "most recent private thread unless new_thread=true."
            ),
        ),
    ] = None,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                "Explicit options for the selected CLI. Call describe_provider(provider) for "
                "supported keys. Omitted options use configured App defaults, then native CLI settings."
            )
        ),
    ] = None,
    conversation_id: Annotated[
        str | None, Field(description="Task conversation to add this task to.")
    ] = None,
    title: Annotated[
        str | None, Field(max_length=200, description="Title for a new task conversation.")
    ] = None,
    role: Annotated[
        str | None,
        Field(
            description=(
                "Optional desk role for this CLI; 1–40 characters after trimming, no newlines "
                "or control characters. Blank or omitted means no role. The desk identity is "
                "(provider, requested model, role)."
            )
        ),
    ] = None,
    new_thread: Annotated[
        bool,
        Field(
            description=(
                "true always opens a fresh private thread instead of continuing this desk's "
                "most recent one; it cannot be combined with session_id or conversation_id."
            )
        ),
    ] = False,
    forward: Annotated[
        list[str] | None,
        Field(
            description=(
                "Message ids from wait_agent, wait_any, wait_round, decide_answers or "
                "post_host_message to inject before the prompt in order. Only succeeded tasks "
                "and committed entries of this manager workspace qualify; at most 20, no "
                "repeats. The injected text is labelled as data, not instructions, and the "
                "saved task prompt stays your original text."
            )
        ),
    ] = None,
    access: Annotated[
        Literal["read", "write"] | None,
        Field(
            description="Declared desk access: read for exploring, planning or reviewing, write for changing files. Recorded and shown only; it does not change the CLI's native permissions. A desk keeps its first declared tier in this manager workspace; omitting it inherits that tier, and a different tier is rejected, so use another role and pass findings with forward or a meeting instead."
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

    The reply includes the CLI and requested model/effort. defaults_applied reports App defaults;
    otherwise omitted settings inherit the native CLI configuration.
    These are requested settings, not confirmation of the CLI's effective settings.
    With a known manager_session_id and neither session_id nor conversation_id given, the task
    continues this desk's (provider, requested model, role) most recent private thread; the
    reply's resumed field says which happened. Pass new_thread=true for a fresh thread. A request
    while that thread still runs is rejected with the same advice. session_id keeps its exact
    meaning: continue that native session and join its open conversation.
    forward injects already-committed workspace messages ahead of the prompt as data, not
    instructions; wait_agent and wait_any return the task's message_id once it finishes.
    Nothing else is shared with the CLI.
    The task and its final output are saved in a private task conversation; a resumed native
    session joins its open conversation. No output is shared with another CLI.
    AgentNave imposes no runtime deadline; use cancel_agent to stop work explicitly.
    Provider-native limits still apply. Active invocations do not accept messages.
    """
    try:
        options, defaults_applied = apply_member(
            read_members(), "task", provider, provider_options or {}, EXCLUDED_PROVIDERS
        )
        request = InvocationRequest(
            provider=provider,
            prompt=prompt,
            cwd=Path(cwd),
            session_id=session_id,
            provider_options=options,
        )
        validate_provider_options(request)
        started = _conversations(ctx).start_task(
            request,
            title,
            conversation_id,
            manager_model=manager_model,
            manager_session_id=manager_session_id,
            role=role,
            new_thread=new_thread,
            forward=forward,
            access=access,
        )
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except ValueError as exc:
        raise invocation_request_error(provider, exc) from exc
    except OSError as exc:
        raise ToolError(f"Unable to prepare or save the task: {exc}.") from exc
    return {
        **started_payload(started.invocation_id, request, defaults_applied),
        "conversation_id": started.conversation_id,
        "resumed": started.resumed,
        "access": started.access,
        **_dashboard_links(_conversations(ctx), started.conversation_id),
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
    Finished invocation handles expire seven days after completion, even if not yet read.
    """
    manager = _manager(ctx)
    conversations = _conversations(ctx)
    try:
        update = await manager.wait_for_update(invocation_id, WAIT_SECONDS)
        return update_payload(manager, conversations, invocation_id, update)
    except KeyError as exc:
        raise _unknown_invocation() from exc


@mcp.tool(
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
    conversations = _conversations(ctx)
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
                update_payload(manager, conversations, invocation_id, update)
                for invocation_id, update in updates.items()
            ],
        }
    return {
        "reason": "wait_elapsed",
        "updates": [
            update_payload(manager, conversations, invocation_id, None)
            for invocation_id in invocation_ids
        ],
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


@mcp.tool(
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
    restart_safe is false while this server holds conversation work, review, or unsaved history.
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
    return {"invocations": items, "restart_safe": _conversations(ctx).restart_safe()}


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

    Does not launch a CLI or check installation/login. permitted applies to start_agent.
    show_permitted reads the current discussion CLI limits; host exclusions do not apply
    to shows.
    """
    try:
        document = read_members()
    except MemberFileError as exc:
        raise ToolError(str(exc)) from exc
    discussion = document.modes.get("discussion") if document is not None else None
    return {
        "provider": provider,
        "permitted": provider not in EXCLUDED_PROVIDERS,
        "show_permitted": discussion is None or provider in discussion,
        "supported_options": "model, effort, " + PROVIDER_OPTIONS[provider],
        "host": HOST.name,
        "host_source": HOST.source,
    }


@mcp.tool(
    title="Read configured members by mode",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def list_members() -> MemberList:
    """Read members.json afresh, with effective defaults, mode limits and host permission.

    valid checks Adapter options, not installation, authentication or model availability.
    Missing files leave both modes unrestricted. Repair invalid files in the AgentNave App.
    """
    try:
        return read_member_list(EXCLUDED_PROVIDERS, HOST)
    except (OSError, ValueError) as exc:
        raise ToolError(str(exc)) from exc


def _committed_payload(entries: list[CommittedEntry]) -> list[CommittedEntryPayload]:
    committed: list[CommittedEntryPayload] = []
    for entry in entries:
        item: CommittedEntryPayload = {
            "message_id": entry.message_id,
            "kind": entry.kind,
            "round": entry.round,
        }
        if entry.seat is not None:
            item["seat"] = entry.seat
        committed.append(item)
    return committed


def round_payload(
    round_: Round,
    manager: InvocationManager,
    reason: str,
    *,
    include_output: bool = True,
    reviews: dict[str, Review | None] | None = None,
    entries: list[CommittedEntry] | None = None,
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
        "entries": _committed_payload(entries or []),
    }
    if round_.error is not None:
        payload["error"] = round_.error
    if reviews is not None:
        for seat in seats:
            review = reviews.get(seat["seat_id"])
            if review == "pending":
                seat["status"] = "pending_review"
            elif review == "discard":
                seat["status"] = "discarded"
    return payload


def _unknown_round() -> ToolError:
    return ToolError("Unknown round_id; read_conversation lists saved rounds.")


@mcp.tool(
    title="Open a saved show of CLI seats",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)
async def open_group(
    title: Annotated[str, Field(min_length=1, max_length=200)],
    seats: Annotated[list[GroupSeatSpec], Field(min_length=2, max_length=7)],
    cwd: Annotated[
        str | None,
        Field(description="Absolute existing directory seats run in; ignored when isolated."),
    ] = None,
    review: Annotated[
        bool,
        Field(description="Hold each successful answer until decide_answers publishes it."),
    ] = False,
    isolated: Annotated[
        bool,
        Field(
            description=(
                "Give each seat its own empty directory and restricted_tools; seat options may "
                "only set model and effort (and Pi's provider)."
            )
        ),
    ] = False,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> ShowPayload:
    """Create a saved show of 2–7 CLI seats; drive it with run_round.

    group_id is the conversation id. Seats are subagents with native tools and permissions
    unless isolated. Each keeps its native session across rounds, including after restart.
    Host exclusions apply only to start_agent. Shows follow discussion member limits.
    Isolation restricts tools; it is not an OS sandbox.
    """
    try:
        document = read_members()
        prepared_seats: list[GroupSeat] = []
        defaults: dict[str, list[str]] = {}
        for seat in seats:
            options, defaults[seat.id] = apply_member(
                document,
                "discussion",
                seat.provider,
                seat.provider_options,
                EXCLUDED_PROVIDERS,
                restricted=isolated,
            )
            prepared_seats.append(
                GroupSeat(
                    seat.id,
                    seat.provider,
                    options,
                    Path(seat.cwd if seat.cwd is not None else cwd or "."),
                    seat.role,
                    seat.access,
                )
            )
        record = _conversations(ctx).open_show(
            title,
            Path(cwd) if cwd is not None else None,
            prepared_seats,
            review=review,
            isolated=isolated,
            manager_session_id=manager_session_id,
        )
    except MemberFileError as exc:
        raise ToolError(str(exc)) from exc
    except (OSError, ValueError) as exc:
        raise ToolError(f"Invalid show: {exc}. Correct the arguments and retry.") from exc
    state = record.state
    return {
        **_dashboard_links(_conversations(ctx), state.id),
        "group_id": state.id,
        "conversation_id": state.id,
        "seats": [
            {
                "id": seat.id,
                "provider": seat.provider,
                "cwd": seat.cwd,
                "model": str(seat.provider_options["model"])
                if "model" in seat.provider_options
                else None,
                "effort": str(seat.provider_options["effort"])
                if "effort" in seat.provider_options
                else None,
                "role": seat.role,
                "access": seat.access,
                "defaults_applied": defaults[seat.id],
            }
            for seat in state.seats
        ],
    }


def _review_map(conversations: Conversations, round_: Round) -> dict[str, Review | None]:
    reviews = conversations.reviews.get(round_.id)
    if reviews is not None:
        return reviews
    return {
        seat.seat_id: seat.review
        for saved in conversations.records[round_.group.id].state.rounds
        if saved.id == round_.id
        for seat in saved.seats
    }


@mcp.tool(
    title="Run one round of a show",
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
    forward: Annotated[
        list[str] | None,
        Field(
            description=(
                "Message ids from wait_agent, wait_any, wait_round, decide_answers or "
                "post_host_message to add as committed forward entries before this round's "
                "task, in order. Only succeeded tasks and committed entries of this manager "
                "workspace qualify; at most 20, no repeats. Seats receive them as data, not "
                "instructions."
            )
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> RoundPayload:
    """Start a saved show round, then wait_round; only committed answers reach other seats.

    parallel starts together and seals answers until all end; serial follows seat order.
    Private instructions go only to their seat. Includes viewer links for the manager only.
    forward adds already-committed messages of this workspace as forward entries before the
    task; entries lists every committed entry of this round with its message_id.
    """
    conversations = _conversations(ctx)
    try:
        round_ = conversations.run(
            group_id,
            seats,
            order,
            prompt,
            instructions or {},
            manager_model=manager_model,
            manager_session_id=manager_session_id,
            forward=forward,
        )
    except KeyError as exc:
        raise ToolError("Unknown group_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    payload = round_payload(
        round_,
        conversations.manager,
        "started",
        reviews=_review_map(conversations, round_),
        entries=conversations.committed_entries(group_id, round_.number),
    )
    return {**payload, **_dashboard_links(conversations, group_id)}


@mcp.tool(
    title="Wait for a show round",
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
    """Wait up to five minutes; returns early when the round finishes, a seat reports a blocker,
    or answers wait for review (reason=review_needed, statuses pending_review).

    Review answers by calling decide_answers, then wait again. Finished rounds can be read again.
    include_output=false omits bodies, including pending review answers; true can read them later.
    """
    conversations = _conversations(ctx)
    try:
        round_, reason = await conversations.wait(round_id, WAIT_SECONDS)
    except KeyError as exc:
        raise _unknown_round() from exc
    return round_payload(
        round_,
        conversations.manager,
        reason or "wait_elapsed",
        include_output=include_output,
        reviews=_review_map(conversations, round_),
        entries=conversations.committed_entries(round_.group.id, round_.number),
    )


@mcp.tool(
    title="Cancel a show round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=True, open_world_hint=False
    ),
)
async def cancel_round(
    round_id: Annotated[str, Field(min_length=1)], *, ctx: Context[Runtime]
) -> RoundPayload:
    """Stop a round: running seats are cancelled and recorded absent, unstarted serial seats skip.

    Answers already published or decided publish are kept; undecided answers are discarded.
    Does not undo CLI side effects.
    """
    conversations = _conversations(ctx)
    try:
        round_ = await conversations.cancel(round_id)
    except KeyError as exc:
        raise _unknown_round() from exc
    return round_payload(
        round_,
        conversations.manager,
        "finished",
        reviews=_review_map(conversations, round_),
        entries=conversations.committed_entries(round_.group.id, round_.number),
    )


class ReviewReceipt(TypedDict):
    round_id: str
    decisions: dict[str, Decision]
    pending_review: list[str]
    committed: list[str]
    entries: list[CommittedEntryPayload]
    status: Literal["running", "review", "finished", "failed", "cancelled", "interrupted"]
    error: NotRequired[str]


@mcp.tool(
    title="Publish or discard answers under review",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def decide_answers(
    round_id: Annotated[str, Field(min_length=1)],
    decisions: Annotated[
        dict[str, Decision],
        Field(min_length=1, description="seat_id -> publish or discard"),
    ],
    *,
    ctx: Context[Runtime],
) -> ReviewReceipt:
    """Decide answers held for review; the whole batch is checked and saved before it applies.

    Published answers become visible to other seats; discarded ones never do. Repeating a
    decision is idempotent; changing one is rejected. Parallel rounds commit once every
    answer is decided; a serial round continues with its next seat. Once no execution or
    review remains, this call waits for local commit and returns the terminal status.
    status=finished needs no confirmation call; running/review still requires wait_round.
    """
    try:
        conversations = _conversations(ctx)
        saved = conversations.decide(round_id, decisions)
        try:
            live = conversations.round(round_id)
        except KeyError:
            live = None  # Restored history has no running driver.
        if (
            live is not None
            and not saved.pending_review()
            and all(run.status not in ("pending", "running") for run in live.runs)
        ):
            # Only local commits remain; never wait for a future serial seat here.
            assert live.task is not None
            await asyncio.shield(live.task)
            saved = next(
                item
                for item in conversations.record(live.group.id).state.rounds
                if item.id == round_id
            )
        group_id = live.group.id if live is not None else conversations.round_conversation(round_id)
        receipt: ReviewReceipt = {
            "round_id": round_id,
            "decisions": dict(decisions),
            "pending_review": [seat.seat_id for seat in saved.seats if seat.review == "pending"],
            "committed": [
                seat.seat_id for seat in saved.seats if seat.committed and seat.review != "discard"
            ],
            "entries": _committed_payload(
                conversations.committed_entries(group_id, saved.number) if group_id else []
            ),
            "status": "review" if saved.pending_review() else saved.status,
        }
        if live is not None and live.finished:
            receipt["status"] = (
                "failed" if live.error else "cancelled" if live.cancelling else "finished"
            )
        error = live.error if live is not None else saved.error
        if error:
            receipt["error"] = error
        return receipt
    except KeyError as exc:
        raise ToolError("Unknown round_id; read_conversation lists saved rounds.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc


class HostReceipt(TypedDict):
    conversation_id: str
    entry_index: int
    message_id: str


class UpdateReceipt(TypedDict):
    conversation_id: str
    title: str


class ConversationView(TypedDict):
    conversation: Conversation
    writable: bool
    public_url: NotRequired[str]
    director_url: NotRequired[str]


@mcp.tool(
    title="Post a host message to a show",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)
async def post_host_message(
    conversation_id: str,
    text: Annotated[str, Field(min_length=1, max_length=12000)],
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> HostReceipt:
    """Add a public host remark that every seat receives as data with its next round.

    Only between rounds, with no answers pending review.
    """
    conversations = _conversations(ctx)
    try:
        conversations.post(
            conversation_id,
            text,
            manager_model=manager_model,
            manager_session_id=manager_session_id,
        )
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    index = len(conversations.records[conversation_id].state.entries) - 1
    return {
        "conversation_id": conversation_id,
        "entry_index": index,
        "message_id": entry_message_id(conversation_id, index),
    }


def _view(conversations: Conversations, conversation_id: str, writable: bool) -> ConversationView:
    view: ConversationView = {
        "conversation": conversations.records[conversation_id].state,
        "writable": writable,
    }
    urls = conversations.urls(conversation_id)
    if urls is not None:
        if conversations.records[conversation_id].state.kind == "show":
            view["public_url"] = urls[0]
        view["director_url"] = urls[1]
    return view


class ConversationSummary(TypedDict):
    conversation_id: str
    title: str
    kind: Literal["show", "task"]
    writable: bool
    created: str


class ConversationList(ViewerLinks):
    conversations: list[ConversationSummary]
    unavailable: list[Unavailable]


@mcp.tool(
    title="List saved conversations",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def list_conversations(*, ctx: Context[Runtime]) -> ConversationList:
    """List saved shows and task conversations. Private: never send this to seats.

    writable=false means another AgentNave server holds it: read it here, continue it there
    or once it becomes idle. unavailable lists unreadable or legacy records.
    """
    conversations = _conversations(ctx)
    try:
        writable = conversations.scan()
    except OSError as exc:
        raise _rejected(exc) from exc
    result: ConversationList = {
        "conversations": [
            {
                "conversation_id": record.state.id,
                "title": record.state.title,
                "kind": record.state.kind,
                "writable": writable.get(record.state.id, True),
                "created": record.state.created,
            }
            for record in sorted(
                conversations.records.values(), key=lambda item: item.state.created
            )
        ],
        "unavailable": conversations.unavailable,
    }
    try:
        result["dashboard_url"] = conversations.open_dashboard(history_loaded=True)
    except OSError as exc:
        result["dashboard_error"] = str(exc)
    return result


@mcp.tool(
    title="Read a saved conversation",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def read_conversation(conversation_id: str, *, ctx: Context[Runtime]) -> ConversationView:
    """Read a conversation, including private instructions, answers under review and native
    session ids. Never forward this to seats. A show's id is its group_id for run_round.
    """
    conversations = _conversations(ctx)
    try:
        record = conversations.record(conversation_id)
        writable = record.refresh()
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return _view(conversations, conversation_id, writable)


@mcp.tool(
    title="Rename a conversation",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def update_conversation(
    conversation_id: str,
    title: Annotated[str, Field(min_length=1, max_length=200)],
    *,
    ctx: Context[Runtime],
) -> UpdateReceipt:
    """Rename a conversation."""
    conversations = _conversations(ctx)
    try:
        record = conversations.update(conversation_id, title)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return {"conversation_id": conversation_id, "title": record.state.title}


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
