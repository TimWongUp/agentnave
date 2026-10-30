"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
import json
import os
import time
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Any, Literal, NotRequired, TypedDict, cast, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import CallToolResult, InputRequiredResult, TextContent, ToolAnnotations
from pydantic import AfterValidator, BaseModel, ConfigDict, Field

from agentnave import __version__, views
from agentnave.conversations import (
    CommittedEntry,
    Conversations,
    Decision,
    ForwardError,
    entry_message_id,
)
from agentnave.core import InvocationManager
from agentnave.group import GroupSeat, Round
from agentnave.host import detect_host, host_session_id
from agentnave.members import (
    AdapterOptionsError,
    CapacityError,
    CLIOptionsFileError,
    MemberFileError,
    MemberList,
    PermissionFileError,
    ProviderExcludedError,
    ProviderName,
    SettingsFileError,
    apply_member,
    apply_seat,
    read_cli_options,
    read_members,
    read_permissions,
    role_seat,
    validate_provider_options,
)
from agentnave.members import list_members as read_member_list
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
)
from agentnave.store import Record, Review, Unavailable

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

TITLE_WIDTH = 48
_TITLE_WIDE_RANGES = cast(
    list[list[int]], json.loads(Path(__file__).with_name("title-width.json").read_text())
)


def _short_title(title: str | None) -> str | None:
    """Keep titles to one short line: wide (CJK) characters take two columns, others one."""
    if title is None:
        return None
    width = sum(
        2 if any(low <= ord(c) <= high for low, high in _TITLE_WIDE_RANGES) else 1 for c in title
    )
    if width > TITLE_WIDTH:
        raise ValueError(
            f"title is {width} columns wide; keep it within {TITLE_WIDTH} "
            "(about 24 CJK characters or 48 Latin letters)"
        )
    return title


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
            "Required by start_agent and run_round, and by handover to hand over or take over, "
            "unless AGENTNAVE_HOST_SESSION_ID is injected for this chat or the host gives this "
            "server its native session ID (Claude Code: CLAUDE_CODE_SESSION_ID)."
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


WAIT_SECONDS = 240

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
    + ". start_agent rejects excluded providers; meetings follow discussion member limits without host exclusions."
)
ACCESS = (
    "Declared desk access: read for exploring, planning or reviewing, write for changing files. "
    "Recorded and shown only; it never changes the CLI's native permissions."
)
PROVIDER_OPTIONS: dict[ProviderName, str] = {
    "zcode": "permission_mode (auto, build, edit, plan, yolo, guarded); model accepts providerId/modelId; effort is native thoughtLevel; restricted_tools is not supported",
    "pi": "provider (model backend; requires model), tools, restricted_tools (boolean; disables tools, extensions and MCP)",
    "claude": "permission_mode, agent, fallback_model, max_budget_usd, restricted_tools (boolean; restricted tools)",
    "codebuddy": "permission_mode, agent, fallback_model, restricted_tools (boolean; restricted tools)",
    "codex": (
        "skip_git_repo_check (boolean; true outside Git repositories unless cli_options has it), "
        "approve_for_me (boolean; Codex's automatic reviewer approves tool calls within the "
        "workspace-write sandbox; not with sandbox or "
        "dangerously_bypass_approvals_and_sandbox), "
        "sandbox (read-only, workspace-write or danger-full-access), "
        "dangerously_bypass_approvals_and_sandbox "
        "(boolean; true skips approvals and disables sandboxing; false or omitted adds no flag), "
        "restricted_tools (boolean; native read/tool restrictions)"
    ),
    "deepseek": (
        "provider (model route; requires model; omitted, dsh's default model and models its "
        "configured providers declare keep their route, any other uses deepseek-official), "
        "effort requires model, "
        "permission_mode (read-only, workspace-write or danger-full-access), "
        "stored_credentials (boolean; reuses the sign-in and keys saved by the DeepSeek "
        "Harness app; defaults to true on deepseek-official, else headless reads "
        "DEEPSEEK_API_KEY), "
        "restricted_tools (boolean; disables tool plugins, read-only sandbox)"
    ),
    "grok": "permission_mode, agent, max_turns, sandbox, restricted_tools (boolean; denies tool execution)",
    "antigravity": (
        "agent, mode, project, print_timeout, sandbox (boolean), disable_slash_commands (boolean), "
        "dangerously_skip_permissions (boolean; explicitly true enables YOLO for this invocation), "
        "restricted_tools (boolean; tool-free profile, only in a dedicated seat cwd that already "
        "contains it)"
    ),
}


class ProviderDetails(BaseModel):
    provider: ProviderName
    permitted: bool  # start_agent; host exclusions apply
    show_permitted: bool  # run_round seats; discussion member limits apply
    supported_options: str


class MemberListPayload(MemberList):
    provider_details: ProviderDetails | None = None


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
    warnings: NotRequired[list[str]]


class UnreceivedPayload(TypedDict):
    id: str
    kind: Literal["task", "round"]
    # a task's end status or execution_blocked; a round's finished, failed, cancelled,
    # review_needed or execution_blocked
    status: str
    error: NotRequired[
        InvocationErrorPayload
    ]  # the oldest unread blocker of a running task or seat


class TaskStartPayload(InvocationPayload):
    conversation_id: str
    access: str | None  # the tier this call declared; recorded only, never enforced
    unreceived: NotRequired[list[UnreceivedPayload]]


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
    error: NotRequired[InvocationErrorPayload]


class InvocationListPayload(TypedDict):
    invocations: list[InvocationListItem]
    restart_safe: bool


class GroupSeatSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: Annotated[str, Field(pattern=r"^[a-z][a-z0-9_-]{0,31}$")]
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(
        default_factory=lambda: {},
        description=(
            'Options for this seat\'s CLI, e.g. {"model": "gpt-6.1-sol", "effort": "medium"}; '
            "omitted keys use discussion member defaults."
        ),
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
    access: Literal["read", "write"] | None = Field(default=None, description=ACCESS)


class GroupSeatPayload(TypedDict):
    id: str
    provider: str
    cwd: str
    model: str | None
    effort: str | None
    role: str | None
    access: str | None
    defaults_applied: list[str]


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
        "private",
    ]
    invocation_id: NotRequired[str]
    warnings: NotRequired[list[str]]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    output_returned: NotRequired[bool]
    session_id: NotRequired[str]


class CommittedEntryPayload(TypedDict):
    message_id: str
    kind: Literal["task", "answer", "absent", "forward", "host"]
    round: int
    seat: NotRequired[str]


class RoundPayload(TypedDict):
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


class WaitPayload(TypedDict):
    reason: Literal["ready", "wait_elapsed", "nothing_pending"]
    updates: list[InvocationPayload]
    rounds: list[RoundPayload]


class CancelPayload(TypedDict):
    invocation: NotRequired[InvocationPayload]
    round: NotRequired[RoundPayload]


class ShowRunPayload(TypedDict):
    group_id: str
    members: NotRequired[list[GroupSeatPayload]]  # only when this call opened the meeting
    entries: NotRequired[list[CommittedEntryPayload]]  # a host remark posted without seats
    round: NotRequired[RoundPayload]
    unreceived: NotRequired[list[UnreceivedPayload]]


class ConversationSummary(TypedDict):
    conversation_id: str
    title: str
    kind: Literal["show", "task"]
    writable: bool
    created: str
    updated: str


class ListPayload(TypedDict):
    invocations: list[InvocationListItem]
    conversations: list[ConversationSummary]
    total: int
    unavailable: list[Unavailable]
    dashboard_url: NotRequired[str]
    dashboard_error: NotRequired[str]


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


# The relay's idle probe: callable by name, never listed to agents.
_HIDDEN_TOOLS = frozenset({"list_invocations"})
# Tools that save history under the boss chat; without its identity they would scatter it.
_CHAT_TOOLS = frozenset({"start_agent", "run_round"})


def _check_arguments(name: str, arguments: dict[str, Any], schema: dict[str, Any]) -> None:
    """Reject what the schema does not name instead of silently dropping it.

    Every problem is reported at once, so one retry is enough.
    """
    accepted = schema.get("properties", {})
    problems: list[str] = []
    unknown = sorted(set(arguments) - set(accepted))
    if unknown:
        hint = (
            " CLI settings such as model and effort go inside provider_options."
            if "provider_options" in accepted
            else ""
        )
        problems.append(
            f"Unknown argument(s) for {name}: {', '.join(unknown)}. "
            f"Accepted: {', '.join(accepted)}.{hint}"
        )
    if (
        name in _CHAT_TOOLS
        and arguments.get("manager_session_id") is None
        and not (host_session_id(HOST) or "").strip()
    ):
        problems.append(
            f"{name} needs manager_session_id: pass this manager chat's session ID (or one "
            "UUID generated once for this chat) on every call so its history stays together."
        )
    if problems:
        raise ToolError(" ".join(problems) + " Nothing was started. Correct and retry.")


def _without_nulls(value: object) -> object:
    if isinstance(value, dict):
        items = cast(dict[str, object], value).items()
        return {key: _without_nulls(item) for key, item in items if item is not None}
    if isinstance(value, list):
        return [_without_nulls(item) for item in cast(list[object], value)]
    return value


class _Server(MCPServer[Runtime]):
    async def list_tools(self) -> list[Any]:
        return [
            # Published schemas say what call_tool enforces: nothing beyond the listed arguments.
            tool.model_copy(
                update={"input_schema": {**tool.input_schema, "additionalProperties": False}}
            )
            for tool in await super().list_tools()
            if tool.name not in _HIDDEN_TOOLS
        ]

    async def call_tool(
        self, name: str, arguments: dict[str, Any], context: Context[Runtime, Any] | None = None
    ) -> CallToolResult | InputRequiredResult:
        tool = self._tool_manager.get_tool(name)
        if tool is not None:
            _check_arguments(name, arguments, tool.parameters)
        result = await super().call_tool(name, arguments, context)
        if (
            isinstance(result, CallToolResult)
            and not result.is_error
            and result.structured_content is not None
        ):
            # The same data as structuredContent, compact and without empty fields.
            text = json.dumps(
                _without_nulls(result.structured_content), ensure_ascii=False, separators=(",", ":")
            )
            result = result.model_copy(update={"content": [TextContent(type="text", text=text)]})
        return result


mcp = _Server(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave launches local CLI subagents and saves their history. Use the "
        "agentnave-manager Skill for CLI usage and model selection. "
        "start_agent runs a private task whose result returns only to you; run_round opens a "
        "saved meeting and runs rounds whose answers seats share automatically. wait and cancel "
        "take task invocation_ids and round_ids; list shows handles, saved conversations and "
        "the dashboard link; read_conversation reads one back. list_members reads App members "
        "and, for one provider, its options; omitted model/effort/options use App defaults "
        "reported in defaults_applied. With review=true, decide_answers publishes each answer "
        "or keeps it private before other seats see it. History survives restart; running "
        "processes do not. Invocation handles expire seven days after completion. "
        "Provider-native permissions remain the security boundary; isolation is not a sandbox. "
        "Every dispatched prompt includes an instruction to complete the assigned task, "
        "report back to the manager, and not call AgentNave tools or delegate through AgentNave. "
        "handover passes a whole workspace to another boss chat; use it only when the user asks "
        "to hand over or take over. " + PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


def _manager(ctx: Context[Runtime]) -> InvocationManager:
    return ctx.request_context.lifespan_context.manager


def _conversations(ctx: Context[Runtime]) -> Conversations:
    return ctx.request_context.lifespan_context.conversations


def _rejected(exc: Exception) -> ToolError:
    return ToolError(f"{exc}. Correct the request and retry.")


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
    if isinstance(
        exc,
        (
            MemberFileError,
            PermissionFileError,
            CLIOptionsFileError,
            SettingsFileError,
            CapacityError,
        ),
    ):
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
    if result.warnings:
        payload["warnings"] = list(result.warnings)
    return payload


def update_payload(
    manager: InvocationManager,
    conversations: Conversations,
    invocation_id: str,
    update: InvocationResult | InvocationError | None,
    *,
    include_running_output: bool = True,
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
    output, age = manager.recent_output(invocation_id) if include_running_output else ("", None)
    if output and age is not None:
        response["output"] = output
        response["output_age_ms"] = age
    if update is not None:
        response["error"] = {"code": update.code, "message": update.message}
    return response


def _unknown_handle(handle: str) -> ToolError:
    return ToolError(
        f"Unknown or expired id {handle}. Use an invocation_id from start_agent or a round_id "
        "from run_round of this MCP server session; invocation handles expire seven days after "
        "completion, and saved rounds of earlier servers are read with read_conversation."
    )


def _unreceived(
    conversations: Conversations, manager_session_id: str | None
) -> list[UnreceivedPayload]:
    """Ended tasks and rounds whose end no wait or cancel has returned yet, and running tasks
    with a blocker no wait has returned. A dispatch reply lists them so a boss busy with other
    work still learns of them; wait receives them."""
    tasks, rounds = conversations.inbox(manager_session_id)
    ended = {
        listing.invocation_id: listing.status.value
        for listing in conversations.manager.list_invocations()
        if listing.status is not None
    }
    found: list[UnreceivedPayload] = []
    for handle in tasks:
        if handle in ended:
            found.append({"id": handle, "kind": "task", "status": ended[handle]})
        elif blockers := conversations.manager.unread_blockers(handle):
            found.append(
                {
                    "id": handle,
                    "kind": "task",
                    "status": "execution_blocked",
                    "error": {
                        "code": blockers[0],
                        "message": "CLI reported an execution blocker; wait returns it.",
                    },
                }
            )
    for handle in rounds:
        round_ = conversations.round(handle)
        if "pending" in conversations.reviews.get(handle, {}).values():
            found.append({"id": handle, "kind": "round", "status": "review_needed"})
        elif round_.finished:
            status = "failed" if round_.error else "cancelled" if round_.cancelling else "finished"
            found.append({"id": handle, "kind": "round", "status": status})
        elif blocked := [
            run.blockers[-1]
            for run in round_.runs
            if run.status == "running" and run.reported_blockers < len(run.blockers)
        ]:
            found.append(
                {
                    "id": handle,
                    "kind": "round",
                    "status": "execution_blocked",
                    "error": {"code": blocked[0].code, "message": blocked[0].message},
                }
            )
    return found


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
                "Omitted, the task starts a new native thread: a new desk in the App."
            ),
        ),
    ] = None,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                'Explicit options for the selected CLI, e.g. {"model": "gpt-6.1-sol", '
                '"effort": "medium"}; model is this CLI\'s own model ID as list_members shows '
                "it. Call list_members(provider=...) for supported keys. Omitted options use "
                "configured App defaults, then native CLI settings."
            )
        ),
    ] = None,
    conversation_id: Annotated[
        str | None, Field(description="Task conversation to add this task to.")
    ] = None,
    title: Annotated[
        str | None,
        Field(
            description=(
                "Short title for a new task conversation, at most 48 columns (CJK characters "
                "count 2). The first conversation of a manager chat also names its workspace "
                "for good, so give that one the chat's purpose; later private tasks may omit it."
            )
        ),
        AfterValidator(_short_title),
    ] = None,
    role: Annotated[
        str | None,
        Field(
            description=(
                "Optional label shown on this task's desk; 1–40 characters after trimming, no "
                "newlines or control characters. Blank or omitted means no label. Each native "
                "thread is its own desk, whatever its role. An agentnave-review role (e.g. "
                "correctness-reviewer) or explorer-easy, explorer-hard, worker-easy, "
                "worker-hard also brings the model and effort the user picked for that seat in "
                "the App when provider is the seat's CLI; explicit options win, and "
                "defaults_applied names the seat."
            )
        ),
    ] = None,
    forward: Annotated[
        list[str] | None,
        Field(
            description=(
                "Message ids from wait, run_round, decide_answers or read_conversation "
                "to inject before the prompt in order. Only succeeded tasks "
                "and committed entries of this manager workspace qualify; at most 20, no "
                "repeats. The injected text is labelled as data, not instructions, and the "
                "saved task prompt stays your original text."
            )
        ),
    ] = None,
    access: Annotated[Literal["read", "write"] | None, Field(description=ACCESS)] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start one private CLI task and return its invocation_id; use wait for the result.

    Ends are never pushed: call wait right after starting and keep one pending until the
    result arrives, even while doing other work. unreceived lists your tasks and rounds that
    ended, hit a blocker or await review but no wait or cancel has returned that yet.

    The reply reports the requested model/effort, not the CLI's effective settings;
    defaults_applied lists App defaults, including each CLI's permission tier (auto unless
    the user picked full in the App); other omitted settings inherit the native CLI.
    Each task starts a new native thread unless session_id continues one; a resumed native
    session joins its open conversation. CLI runs active at once (tasks plus seats) are
    limited by the App's setting, 8 by default; beyond it the call is rejected until some finish. The task and its final
    reply are saved in a private task conversation; nothing is shared with another CLI except
    what forward injects. No runtime deadline: cancel stops work explicitly. Active
    invocations do not accept messages.
    """
    manager = _manager(ctx)
    if session_id is not None and (
        _known_invocation(manager, session_id) or _known_round(ctx, session_id)
    ):
        raise ToolError(
            "session_id is an AgentNave handle, not a native session. Use the session_id from "
            "the finished wait result or read_conversation."
        )
    seat_applied: list[str] = []
    try:
        options, seat_applied = apply_seat(role_seat(role), provider, provider_options or {})
        options, defaults_applied = apply_member(
            read_members(),
            "task",
            provider,
            options,
            EXCLUDED_PROVIDERS,
            permissions=read_permissions(),
            cli_options=read_cli_options(),
        )
        if seat_applied:
            defaults_applied = [f"{(role or '').strip()} seat", *seat_applied, *defaults_applied]
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
            forward=forward,
            access=access,
        )
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list.") from exc
    except ValueError as exc:
        error = invocation_request_error(provider, exc)
        if seat_applied:
            raise ToolError(
                f"{error} App defaults applied: {(role or '').strip()!r} seat "
                f"({', '.join(seat_applied)})."
            ) from exc
        raise error from exc
    except OSError as exc:
        raise ToolError(f"Unable to prepare or save the task: {exc}.") from exc
    payload: TaskStartPayload = {
        **started_payload(started.invocation_id, request, defaults_applied),
        "conversation_id": started.conversation_id,
        "access": started.access,
    }
    if access != "read":
        sharing = [
            listing.invocation_id
            for listing in manager.list_invocations()
            if listing.status is None
            and listing.cwd == request.cwd
            and listing.invocation_id != started.invocation_id
        ]
        if sharing:
            payload["warnings"] = [
                f"{len(sharing)} other CLI run(s) active in this cwd ({', '.join(sharing)}); "
                "parallel writers need separate worktrees. Declare access=read for read-only work."
            ]
    unreceived = _unreceived(_conversations(ctx), manager_session_id)
    if unreceived:
        payload["unreceived"] = unreceived
    return payload


def _known_round(ctx: Context[Runtime], handle: str) -> bool:
    try:
        _conversations(ctx).round(handle)
    except KeyError:
        return False
    return True


def _known_invocation(manager: InvocationManager, handle: str) -> bool:
    try:
        manager.snapshot(handle)
    except KeyError:
        return False
    return True


async def _ready_handles(
    manager: InvocationManager,
    conversations: Conversations,
    invocation_ids: list[str],
    round_ids: list[str],
    timeout_seconds: float,
) -> tuple[dict[str, InvocationResult | InvocationError], dict[str, tuple[Round, str | None]]]:
    """Wait until any handle is ready, then collect every handle ready at that moment."""

    def ready(found: tuple[Round, str | None]) -> bool:
        return found[0].finished or found[1] is not None

    invocations: asyncio.Task[dict[str, InvocationResult | InvocationError]] | None = None
    if invocation_ids:
        invocations = asyncio.create_task(manager.wait_for_any(invocation_ids, timeout_seconds))
    rounds = {
        item: asyncio.create_task(conversations.wait(item, timeout_seconds)) for item in round_ids
    }
    waiters: list[asyncio.Task[Any]] = [
        task for task in (invocations, *rounds.values()) if task is not None
    ]
    try:
        await asyncio.wait(waiters, return_when=asyncio.FIRST_COMPLETED)
    finally:
        for task in waiters:
            task.cancel()
        await asyncio.gather(*waiters, return_exceptions=True)
    # A finished waiter already consumed what it reports; poll only the ones cut short.
    ready_invocations: dict[str, InvocationResult | InvocationError] = {}
    if invocations is not None:
        ready_invocations = (
            await manager.wait_for_any(invocation_ids, 0)
            if invocations.cancelled()
            else invocations.result()
        )
    ready_rounds: dict[str, tuple[Round, str | None]] = {}
    for item, task in rounds.items():
        found = await conversations.wait(item, 0) if task.cancelled() else task.result()
        if ready(found):
            ready_rounds[item] = found
    return ready_invocations, ready_rounds


@mcp.tool(
    title="Wait for tasks or rounds",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def wait(
    ids: Annotated[
        list[str] | None,
        Field(
            description=(
                "Distinct invocation_ids from start_agent and round_ids from run_round. Omit to "
                "wait on the inbox: every task and round of this server whose end you have not "
                "received yet."
            ),
        ),
    ] = None,
    include_output: Annotated[
        bool,
        Field(description="false omits reply bodies and tails, including answers under review."),
    ] = True,
    wait_seconds: Annotated[
        Literal[0, 240] | None,
        Field(
            description=(
                "Omit for normal waiting (up to 240 seconds). Only 0 and 240 are accepted. "
                "Use 0 for a specific progress or intervention check: it returns at once and "
                "may include running reply tails. Ready results are still received normally. "
                "Never stops the work."
            ),
        ),
    ] = None,
    manager_session_id: Annotated[
        str | None,
        Field(
            description=(
                "With ids omitted, only this boss chat's tasks and rounds; same value as "
                "start_agent and run_round. Ignored with ids."
            ),
        ),
    ] = None,
    *,
    ctx: Context[Runtime],
) -> WaitPayload:
    """Wait up to 240 seconds for the first listed task or round to be ready.

    A task is ready when it finished or reports a CLI execution blocker (it keeps running;
    each blocker category wakes once). A round is ready when it finished, a seat reports a
    blocker, or answers wait for review (reason=review_needed, seat status pending_review).
    reason=ready returns every handle ready at that moment, in updates (tasks) and rounds;
    finished ones stay ready, so drop them from the next call. reason=wait_elapsed returns a
    running snapshot of each, with status and recent activity. Only explicit wait_seconds=0
    includes running reply tails (at most 1000 characters each) and their age, subject to
    include_output. After wait_elapsed, normally wait again without a separate 0-second check.
    A round returns each seat's final answer once; later payloads mark it output_returned, so
    reread it with read_conversation. Unknown, expired or repeated ids reject the whole call before waiting. This is
    request/response waiting, not a background subscription.

    Omitting ids waits on the inbox instead: the handles above whose final state no wait or
    cancel has returned. Each end is delivered once; a round under review stays ready until
    decided. reason=nothing_pending returns at once when nothing is outstanding.
    """
    manager = _manager(ctx)
    conversations = _conversations(ctx)
    invocation_ids: list[str] = []
    round_ids: list[str] = []
    if ids is None:
        try:
            invocation_ids, round_ids = conversations.inbox(manager_session_id)
        except ValueError as exc:
            raise ToolError(f"{exc}. Correct and retry.") from exc
        if not invocation_ids and not round_ids:
            return {"reason": "nothing_pending", "updates": [], "rounds": []}
    else:
        if not ids:
            raise ToolError("ids must list at least one handle, or be omitted for the inbox.")
        if len(set(ids)) != len(ids):
            raise ToolError("ids must not repeat. Correct the list and retry.")
        for handle in ids:
            if _known_invocation(manager, handle):
                invocation_ids.append(handle)
                continue
            try:
                conversations.round(handle)
            except KeyError as exc:
                raise _unknown_handle(handle) from exc
            round_ids.append(handle)
    deadline = time.monotonic() + (WAIT_SECONDS if wait_seconds is None else wait_seconds)
    while True:
        ready_invocations, ready_rounds = await _ready_handles(
            manager,
            conversations,
            invocation_ids,
            round_ids,
            max(0.0, deadline - time.monotonic()),
        )
        if ids is not None:
            break
        # Another inbox wait may have delivered these ends while this one slept. From here to
        # the return nothing awaits, so this wait's claim cannot be taken twice.
        ready_invocations = {
            handle: update
            for handle, update in ready_invocations.items()
            if handle not in conversations.delivered_tasks
        }
        ready_rounds = {
            handle: found
            for handle, found in ready_rounds.items()
            if handle not in conversations.delivered_rounds
        }
        if ready_invocations or ready_rounds or time.monotonic() >= deadline:
            break
        invocation_ids, round_ids = conversations.inbox(manager_session_id)
        if not invocation_ids and not round_ids:
            return {"reason": "nothing_pending", "updates": [], "rounds": []}
    elapsed = not ready_invocations and not ready_rounds
    include_running_output = include_output and wait_seconds == 0
    updates: list[InvocationPayload] = []
    for handle in invocation_ids:
        if not elapsed and handle not in ready_invocations:
            continue
        update = ready_invocations.get(handle)
        if isinstance(update, InvocationResult):
            conversations.delivered_tasks.add(handle)
        updates.append(
            update_payload(
                manager,
                conversations,
                handle,
                update,
                include_running_output=include_running_output,
            )
        )
    rounds: list[RoundPayload] = []
    for handle in round_ids:
        if not elapsed and handle not in ready_rounds:
            continue
        round_, reason = ready_rounds.get(handle) or (conversations.round(handle), None)
        if round_.finished and not elapsed:
            # One ending just after the window closed is delivered by the next wait as ready.
            conversations.delivered_rounds.add(handle)
        rounds.append(
            round_payload(
                round_,
                manager,
                reason or "wait_elapsed",
                include_output=include_output,
                include_running_output=include_running_output,
                deliver=not elapsed,
                reviews=_review_map(conversations, round_),
                entries=conversations.committed_entries(round_.group.id, round_.number),
            )
        )
    if not include_output:
        for update in updates:
            update.pop("output", None)
            update.pop("output_age_ms", None)
    return {"reason": "wait_elapsed" if elapsed else "ready", "updates": updates, "rounds": rounds}


@mcp.tool(
    title="Cancel a task or round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=True, open_world_hint=False
    ),
)
async def cancel(
    id: Annotated[  # noqa: A002 - the tool's public argument name
        str,
        Field(min_length=1, description="An invocation_id from start_agent or a round_id."),
    ],
    *,
    ctx: Context[Runtime],
) -> CancelPayload:
    """Stop a task, or a round: its running seats are cancelled and recorded absent, unstarted
    serial seats skip, published answers stay and undecided answers stay private.

    Returns the cancelled or already-finished result. Does not undo CLI side effects.
    """
    manager = _manager(ctx)
    if _known_invocation(manager, id):
        result = await manager.cancel(id)
        _conversations(ctx).delivered_tasks.add(id)
        return {"invocation": finished_payload(id, result)}
    conversations = _conversations(ctx)
    try:
        round_ = await conversations.cancel(id)
    except KeyError as exc:
        raise _unknown_handle(id) from exc
    conversations.delivered_rounds.add(id)
    return {
        "round": round_payload(
            round_,
            conversations.manager,
            "finished",
            reviews=_review_map(conversations, round_),
            entries=conversations.committed_entries(round_.group.id, round_.number),
        )
    }


def _invocation_items(manager: InvocationManager) -> list[InvocationListItem]:
    items: list[InvocationListItem] = []
    for listing in manager.list_invocations():
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
        if listing.error is not None:
            item["error"] = InvocationErrorPayload(
                code=listing.error.code, message=listing.error.message
            )
        items.append(item)
    return items


@mcp.tool(
    title="List invocations for the relay",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def list_invocations(*, ctx: Context[Runtime]) -> InvocationListPayload:
    """Unlisted: the relay asks this before swapping servers; agents use list."""
    return {
        "invocations": _invocation_items(_manager(ctx)),
        "restart_safe": _conversations(ctx).restart_safe(),
    }


@mcp.tool(
    name="list",
    title="List handles and saved conversations",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def list_all(
    kind: Annotated[
        Literal["show", "task"] | None,
        Field(description="Only meetings or only task conversations."),
    ] = None,
    limit: Annotated[int, Field(ge=1, le=200, description="Most recent conversations.")] = 20,
    *,
    ctx: Context[Runtime],
) -> ListPayload:
    """List this server's invocation handles and saved conversations, most recent first.

    Invocations carry metadata only: no prompt, reply or tool data; finished ones tell with
    result_read whether a wait or cancel returned the final result, and failures their error. total counts every
    matching conversation. writable=false means another AgentNave server holds it: read it
    here, continue it there or once it becomes idle. unavailable lists unreadable or legacy
    records. Also starts the local read-only dashboard and returns its link. Private: never
    send this to seats.
    """
    conversations = _conversations(ctx)
    try:
        writable = conversations.scan()
    except OSError as exc:
        raise _rejected(exc) from exc
    matching = sorted(
        (
            record.state
            for record in conversations.records.values()
            if kind is None or record.state.kind == kind
        ),
        key=lambda state: (views.updated(state), state.created),
        reverse=True,
    )
    result: ListPayload = {
        "invocations": _invocation_items(_manager(ctx)),
        "conversations": [
            {
                "conversation_id": state.id,
                "title": state.title,
                "kind": state.kind,
                "writable": writable.get(state.id, True),
                "created": state.created,
                "updated": views.updated(state),
            }
            for state in matching[:limit]
        ],
        "total": len(matching),
        "unavailable": conversations.unavailable,
    }
    try:
        result["dashboard_url"] = conversations.open_dashboard(history_loaded=True)
    except OSError as exc:
        result["dashboard_error"] = str(exc)
    return result


@mcp.tool(
    title="Read configured members",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def list_members(
    provider: Annotated[
        ProviderName | None,
        Field(description="Also return this CLI's permitted status and supported options."),
    ] = None,
) -> MemberListPayload:
    """Read members.json afresh, with effective defaults, mode limits and host permission.

    valid checks Adapter options, not installation, authentication or model availability.
    Missing files leave both modes unrestricted. Repair invalid files in the AgentNave App.
    review holds the CLI and model the user picked in the App for each agentnave-review role,
    from review.json; review_error reports a broken review.json without affecting members.
    orchestration holds the CLI and model picked for each Explorer (read-only: explore, plan,
    review) and Worker (writes) difficulty, easy or hard, from orchestration.json. native=true
    marks the host's own CLI: give that work to the host's built-in subagent instead.
    cli_options holds settings the user saved for a whole CLI, applied to every model of it.
    """
    try:
        members = read_member_list(EXCLUDED_PROVIDERS, HOST)
    except (OSError, ValueError) as exc:
        raise ToolError(str(exc)) from exc
    payload = MemberListPayload.model_validate(members.model_dump())
    if provider is not None:
        try:
            document = read_members()
        except MemberFileError as exc:
            raise ToolError(str(exc)) from exc
        discussion = document.modes.get("discussion") if document is not None else None
        payload.provider_details = ProviderDetails(
            provider=provider,
            permitted=provider not in EXCLUDED_PROVIDERS,
            show_permitted=discussion is None or provider in discussion,
            supported_options="model, effort, " + PROVIDER_OPTIONS[provider],
        )
    return payload


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
    include_running_output: bool = True,
    deliver: bool = True,
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
            if run.output_returned:
                seat["output_returned"] = True
            elif deliver and include_output and result.output:
                # Each answer reaches the boss once per round; later payloads only mark it.
                # A wait_elapsed snapshot holds it back until the round is delivered.
                seat["output"] = result.output
                run.output_returned = True
            if result.session_id:
                seat["session_id"] = result.session_id
            if result.error:
                seat["error"] = {"code": result.error.code, "message": result.error.message}
            if result.warnings:
                seat["warnings"] = list(result.warnings)
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
            output, age = (
                manager.recent_output(run.invocation_id)
                if include_output and include_running_output
                else ("", None)
            )
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
            elif review == "private":
                seat["status"] = "private"
    return payload


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


def _open_show(
    conversations: Conversations,
    title: str,
    members: list[GroupSeatSpec],
    cwd: str | None,
    review: bool,
    isolated: bool,
    manager_session_id: str | None,
) -> tuple[str, list[GroupSeatPayload]]:
    document = read_members()
    permissions = read_permissions()
    cli_options = read_cli_options()
    prepared: list[GroupSeat] = []
    defaults: dict[str, list[str]] = {}
    for seat in members:
        options, defaults[seat.id] = apply_member(
            document,
            "discussion",
            seat.provider,
            seat.provider_options,
            EXCLUDED_PROVIDERS,
            restricted=isolated,
            permissions=permissions,
            cli_options=cli_options,
        )
        prepared.append(
            GroupSeat(
                seat.id,
                seat.provider,
                options,
                Path(seat.cwd if seat.cwd is not None else cwd or "."),
                seat.role,
                seat.access,
            )
        )
    state = conversations.open_show(
        title,
        Path(cwd) if cwd is not None else None,
        prepared,
        review=review,
        isolated=isolated,
        manager_session_id=manager_session_id,
    ).state
    return state.id, [
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
    ]


@mcp.tool(
    title="Run a meeting round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=True
    ),
)
async def run_round(
    prompt: Annotated[
        str | None,
        Field(
            min_length=1,
            max_length=12000,
            description=(
                "Shared task for this round, visible to all seats; without seats, a public "
                "host remark every seat receives with its next round. To get answers, include "
                "seats even when opening a new meeting. Omit to only create the meeting."
            ),
        ),
    ] = None,
    group_id: Annotated[
        str | None,
        Field(
            description="Meeting to continue; omit to open a new meeting from title and members."
        ),
    ] = None,
    seats: Annotated[
        list[str],
        Field(
            description=(
                "Ordered seat ids that each answer once. Include with title, members and prompt "
                "to open a meeting and run round 1 in one call. Empty only posts a host remark."
            )
        ),
    ] = [],  # noqa: B006 - pydantic copies defaults
    order: Annotated[
        Literal["parallel", "serial"],
        Field(
            description=(
                "parallel: all start together and cannot see each other's answer this round. "
                "serial: one by one in list order; later seats see earlier answers."
            )
        ),
    ] = "parallel",
    instructions: Annotated[
        dict[str, str] | None,
        Field(description="Optional private instruction per seat id; never sent to other seats."),
    ] = None,
    forward: Annotated[
        list[str] | None,
        Field(
            description=(
                "Message ids from wait, run_round, decide_answers or read_conversation to add "
                "as committed forward entries before this round's task, in order. Only "
                "succeeded tasks and committed entries of this manager workspace qualify; at "
                "most 20, no repeats. Seats receive them as data, not instructions."
            )
        ),
    ] = None,
    title: Annotated[
        str | None,
        Field(
            description=(
                "New meeting only: its short title, at most 48 columns (CJK characters count 2). "
                "If it is the manager chat's first conversation, it also names the workspace."
            )
        ),
        AfterValidator(_short_title),
    ] = None,
    members: Annotated[
        list[GroupSeatSpec] | None,
        Field(min_length=2, max_length=7, description="New meeting only: its 2–7 seats."),
    ] = None,
    cwd: Annotated[
        str | None,
        Field(
            description="New meeting only: absolute directory seats run in; ignored when isolated."
        ),
    ] = None,
    review: Annotated[
        bool,
        Field(description="New meeting only: hold each answer until decide_answers decides it."),
    ] = False,
    isolated: Annotated[
        bool,
        Field(
            description=(
                "New meeting only: each seat gets its own empty directory and restricted_tools; "
                "seat options may only set model and effort (and Pi's provider)."
            )
        ),
    ] = False,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> ShowRunPayload:
    """Run a round of a saved meeting; answers reach the other seats without passing through you.

    Without group_id, opens a meeting of 2–7 seats (group_id is its conversation id).
    To start immediately, pass title, members, prompt and seats together: this runs round 1
    in the same call. With no prompt it only creates the meeting. With prompt but no seats it
    only posts a host remark, starts no subagents and does not advance the round number.
    Seats are subagents with native tools and permissions unless isolated; each keeps its
    native session across rounds and restarts. Meetings follow discussion member limits, not
    host exclusions. A started round returns at once: use wait with its round_id. Private
    instructions go only to their seat; with review=true each answer waits for decide_answers.
    wait and cancel return each final answer once per round, then mark it output_returned.
    entries lists the round's committed entries with message ids. unreceived lists your tasks
    and rounds that ended, hit a blocker or await review but no wait or cancel has returned
    that yet. Isolation
    restricts tools; it is not an OS sandbox.
    """
    conversations = _conversations(ctx)
    new = group_id is None
    if new:
        if not title or not title.strip() or not members:
            raise ToolError(
                "A new meeting needs title and members. Correct the arguments and retry."
            )
        unknown = sorted(set(seats) - {seat.id for seat in members})
        if unknown:
            raise ToolError("Unknown seat ids: " + ", ".join(unknown) + ". Correct and retry.")
    elif title is not None or members is not None or cwd is not None or review or isolated:
        raise ToolError(
            "title, members, cwd, review and isolated only open a new meeting; omit them with "
            "group_id."
        )
    if not seats and forward:
        raise ToolError("forward needs seats to answer this round. Correct and retry.")
    if prompt is None and (seats or not new):
        raise ToolError("prompt is required unless only opening a new meeting. Correct and retry.")
    opened: list[GroupSeatPayload] | None = None
    if group_id is None:
        assert title is not None and members is not None
        try:
            group_id, opened = _open_show(
                conversations, title, members, cwd, review, isolated, manager_session_id
            )
        except (MemberFileError, PermissionFileError, CLIOptionsFileError) as exc:
            raise ToolError(str(exc)) from exc
        except (OSError, ValueError) as exc:
            raise ToolError(f"Invalid meeting: {exc}. Correct the arguments and retry.") from exc
    payload: ShowRunPayload = {"group_id": group_id}
    if opened is not None:
        payload["members"] = opened
    try:
        if prompt is None:
            return payload
        if not seats:
            conversations.post(
                group_id,
                prompt,
                manager_model=manager_model,
                manager_session_id=manager_session_id,
            )
            index = len(conversations.records[group_id].state.entries) - 1
            entry = conversations.records[group_id].state.entries[index]
            payload["entries"] = [
                {
                    "message_id": entry_message_id(group_id, index),
                    "kind": "host",
                    "round": entry.round,
                }
            ]
            return payload
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
        raise ToolError("Unknown group_id; use list.") from exc
    except (OSError, ValueError) as exc:
        if new:
            raise ToolError(
                f"Meeting {group_id} was opened but nothing was posted: {exc}. Retry with group_id."
            ) from exc
        if isinstance(exc, (SettingsFileError, CapacityError)):
            raise ToolError(str(exc)) from exc
        raise _rejected(exc) from exc
    payload["round"] = round_payload(
        round_,
        conversations.manager,
        "started",
        reviews=_review_map(conversations, round_),
        entries=conversations.committed_entries(group_id, round_.number),
    )
    unreceived = _unreceived(conversations, manager_session_id)
    if unreceived:
        payload["unreceived"] = unreceived
    return payload


class ReviewReceipt(TypedDict):
    round_id: str
    decisions: dict[str, Decision]
    pending_review: list[str]
    committed: list[str]
    entries: list[CommittedEntryPayload]
    status: Literal["running", "review", "finished", "failed", "cancelled", "interrupted"]
    error: NotRequired[str]


@mcp.tool(
    title="Publish answers under review or keep them private",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def decide_answers(
    round_id: Annotated[str, Field(min_length=1)],
    decisions: Annotated[
        dict[str, Decision],
        Field(min_length=1, description="seat_id -> publish or private"),
    ],
    *,
    ctx: Context[Runtime],
) -> ReviewReceipt:
    """Decide answers held for review; the whole batch is checked and saved before it applies.

    publish shares the answer with the other seats. private keeps it between you and that
    seat: the others never see it, and the seat is told so with its next round. Repeating a
    decision is idempotent; changing one is rejected. Parallel rounds commit once every
    answer is decided; a serial round continues with its next seat. Once no execution or
    review remains, this call waits for local commit and returns the terminal status.
    status=finished needs no further call; running/review still requires wait.
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
                seat.seat_id for seat in saved.seats if seat.committed and seat.review != "private"
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


_WORKSPACE_ID = r"^s-[0-9a-f]{32}$"


def _handover_conversations(
    conversations: Conversations, records: list[Record]
) -> list[dict[str, Any]]:
    writable = conversations.scan()
    return [
        {
            "conversation_id": record.state.id,
            "title": record.state.title,
            "kind": record.state.kind,
            "updated": views.updated(record.state),
            "running": any(task.status == "running" for task in record.state.tasks)
            or any(round_.status == "running" for round_ in record.state.rounds),
            "writable": writable.get(record.state.id, True),
        }
        for record in sorted(records, key=lambda record: views.updated(record.state), reverse=True)
    ]


@mcp.tool(
    title="Hand a workspace over to another boss",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)
async def handover(
    release: Annotated[
        bool,
        Field(
            description="Mark this boss chat's whole workspace as waiting for another boss "
            "chat to take over."
        ),
    ] = False,
    note: Annotated[
        str | None,
        Field(
            min_length=1,
            max_length=20000,
            description="With release: notes for the next boss, written for what it must do "
            "next. Say why you hand over (out of quota, a model better suited to the work), the "
            "goal, what is done, decisions and why, what is open, desks in use, and the Skills "
            "to load (agentnave-manager, any review gate): it may come from another host. Name "
            "the conversations to read first and any worktree or uncommitted work needed to "
            "continue. Cite "
            "what already exists by reference only (conversation ids, PR or commit links, ADR "
            "or spec paths); the next boss reads seat answers with read_conversation, so keep "
            "this shorter than the records it points to. No keys, passwords or sensitive "
            "personal data: it is saved and shown in the App.",
        ),
    ] = None,
    workspace_id: Annotated[
        str | None,
        Field(
            pattern=_WORKSPACE_ID,
            description="Take this waiting workspace over (from a call without arguments).",
        ),
    ] = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> dict[str, Any]:
    """Pass a boss workspace (every task and meeting of one boss chat) to another boss chat.

    Only when the user asks to hand over or take over; never as a routine check. release=true
    marks this chat's workspace as waiting. Without arguments, lists waiting workspaces. With
    workspace_id, this chat takes it over: its notes, conversation list and, when known, the
    previous boss's transcript file. Read the note and available transcript tail, then use
    read_conversation for relevant conversations. If the note is missing, incomplete or leaves
    open work unclear, expand reading from recent history until the goal, constraints and next
    step are clear. From then on this chat continues those meetings and tasks and the previous boss
    is refused. Work still running finishes in the previous boss's server and lands in history;
    continue such a conversation once writable is true. Taking over your own waiting workspace
    withdraws the handover. Private: never send this to seats.
    """
    conversations = _conversations(ctx)
    if note is not None and not release:
        raise ToolError("note goes with release=true. Correct the request and retry.")
    if release and workspace_id is not None:
        raise ToolError(
            "Use release=true to hand this chat's workspace over, or workspace_id to take one "
            "over, not both. Correct the request and retry."
        )
    try:
        if release:
            released, count = conversations.release(manager_session_id, note)
            return {
                "workspace_id": released,
                "status": "waiting",
                "conversations": count,
                "next": "Tell the user to open a new chat in any host and ask it to take over "
                "this workspace. Work still running here finishes and lands in history.",
            }
        if workspace_id is None:
            return {
                "waiting": [
                    {
                        "workspace_id": item.id,
                        "title": records[0].state.title,
                        "from_host": item.owner_host or records[-1].state.host,
                        "released": item.released,
                        "conversations": len(records),
                        "note": item.note,
                    }
                    for item, records in conversations.waiting()
                ]
            }
        before, after, records = conversations.claim(manager_session_id, workspace_id)
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    if before.owner == after.owner:
        status = "withdrawn" if before.pending else "yours"
        # Withdrawing hands nothing over: no earlier boss's session to point at.
        previous_host, previous_file = (
            (None, None) if before.pending else (after.previous_host, after.previous_file)
        )
    else:
        status = "claimed"
        previous_host, previous_file = before.owner_host, before.owner_file
    result: dict[str, Any] = {
        "workspace_id": workspace_id,
        "status": status,
        "title": records[0].state.title,
        "from_host": previous_host or records[-1].state.host,
        "note": after.note,
        "conversations": _handover_conversations(conversations, records),
        "next": "Read the note and available previous_session_file tail, then use "
        "read_conversation for relevant conversations. If the note is missing, incomplete or "
        "leaves open work unclear, expand reading from recent history until the goal, constraints "
        "and next step are clear. Continue meetings "
        "with run_round(group_id) and tasks with start_agent(conversation_id or session_id) "
        "once writable is true; running work finishes in the previous boss's server.",
    }
    if previous_file is not None and Path(previous_file).is_file():
        result["previous_session_file"] = previous_file
    return result


@mcp.tool(
    title="Read a saved conversation",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def read_conversation(
    conversation_id: str,
    since_round: Annotated[
        int,
        Field(
            ge=0,
            description="Meetings: only messages from this round on; 0 includes host remarks "
            "posted before the first round.",
        ),
    ] = 0,
    limit: Annotated[
        int | None,
        Field(
            ge=1,
            le=500,
            description="At most this many of the most recent messages or tasks; omitted counts the rest.",
        ),
    ] = None,
    include_private: Annotated[
        bool,
        Field(
            description=(
                "Meetings: also each seat's private instructions and the answers kept private."
            )
        ),
    ] = False,
    *,
    ctx: Context[Runtime],
) -> dict[str, Any]:
    """Read a conversation back as its final messages, the way the App shows them.

    Meetings return seats, the latest round and messages in order: tasks, published answers,
    absences, forwards, host remarks and answers waiting for review (pending_answer). Tasks
    return each prompt, final reply and native session_id. Message ids can be forwarded.
    Private: never forward this whole result to seats. A meeting's id is its group_id.
    """
    conversations = _conversations(ctx)
    try:
        record = conversations.record(conversation_id)
        writable = record.refresh()
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    data = views.timeline(
        record.state, since_round=since_round, include_private=include_private, limit=limit
    ) | {"writable": writable}
    urls = conversations.urls(conversation_id)
    if urls is not None:
        if record.state.kind == "show":
            data["public_url"] = urls[0]
        data["director_url"] = urls[1]
    return data


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
