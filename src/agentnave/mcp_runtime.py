"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import json
import logging
import os
import time
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Any, Literal, cast, get_args

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import CallToolResult, InputRequiredResult, TextContent
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from agentnave import __version__
from agentnave.adapters import get_adapter
from agentnave.conversations import (
    CommittedEntry,
    Conversations,
    ForwardError,
)
from agentnave.core import InvocationManager
from agentnave.group import Round
from agentnave.host import detect_host, host_session_id
from agentnave.isolated import ChangeSummary
from agentnave.manager_identity import SESSION_ID_DESCRIPTIONS, session_id_hint
from agentnave.mcp_payloads import (
    ActivityPayload,
    CommittedEntryPayload,
    InvocationPayload,
    RoundPayload,
    SeatRunPayload,
    TaskIsolationSummaryPayload,
    UnreceivedPayload,
)
from agentnave.mcp_projection import without_empty_fields
from agentnave.members import (
    AdapterOptionsError,
    CapacityError,
    MemberFileError,
    PermissionFileError,
    ProviderExcludedError,
    ProviderName,
    SettingsFileError,
    data_root,
    role_seat,
    visible_role_groups,
)
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
)
from agentnave.naming import MemberIdentity
from agentnave.native_prompts import NativePrompt
from agentnave.orchestration_roles import role_catalog, seat_role
from agentnave.retention import Retention
from agentnave.store import Review

type ManagerModel = Annotated[
    str | None,
    Field(
        min_length=1,
        max_length=200,
        description="Boss model override; normally omit; see MCP instructions.",
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


HOST = detect_host()

type ManagerSessionId = Annotated[
    str | None,
    Field(
        min_length=1,
        max_length=200,
        description=SESSION_ID_DESCRIPTIONS.get(
            HOST.name or "", "Boss chat ID from host context; see MCP instructions."
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


WAIT_SECONDS = 270
LONG_WAIT_SECONDS = 1500
LONG_WAIT_SENTENCE = (
    "In the Claude Code terminal interactive session, omitting waits up to 1500s (25 minutes) "
    "instead; pass 270 for the short window. "
)


def long_wait_enabled() -> bool:
    """1500s waits only where Claude Code backgrounds long calls: its terminal session.

    Desktop app, VS Code, -p/SDK and an unreadable entrypoint keep 0 and 270 (ADR-0082).
    """
    return HOST.name == "claude" and os.environ.get("CLAUDE_CODE_ENTRYPOINT") == "cli"


# Ends are never pushed (ADR-0007), so receipts say what to call before the turn ends (ADR-0064).
WAIT_NEXT = (
    "Ends are not pushed: before ending your turn, call wait (omit ids, same "
    "manager_session_id) until all_finished."
)
WAIT_AGAIN = (
    "Still pending: reply to any new user message first, and handle any prompt_pending or "
    "review_needed, then call wait again "
    "before ending your turn; increment seq on each wait to distinguish repeated calls."
)

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
ACCESS = "Declared read/write access; see MCP instructions."
DISPATCH_WAIT = (
    f"true waits up to {WAIT_SECONDS}s like wait(ids=[this handle], wait_seconds={WAIT_SECONDS}) and returns it in wait; only for a single "
    "dispatch whose result this turn needs, not parallel dispatches."
)
PROVIDER_OPTIONS: dict[ProviderName, str] = {
    provider: get_adapter(provider).options_description
    for provider in get_args(ProviderName.__value__)
}


class GroupSeatSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: Annotated[str, Field(pattern=r"^[a-z][a-z0-9_-]{0,31}$")]
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(
        default_factory=lambda: {},
        description=("CLI options; omitted keys use discussion member defaults."),
    )
    cwd: str | None = Field(
        default=None,
        description="Absolute directory for this seat; defaults to the group cwd.",
    )
    role: str | None = Field(
        default=None,
        description=(
            "Desk label: 1–40 trimmed characters, no controls/newlines; blank/omitted means none."
        ),
    )
    access: Literal["read", "write"] | None = Field(default=None, description=ACCESS)


class Runtime:
    def __init__(self) -> None:
        self.manager = InvocationManager()
        self.conversations = Conversations(self.manager, data_root(), host=HOST)
        self.retention = Retention(data_root())


@asynccontextmanager
async def _lifespan(_server: MCPServer[Runtime]) -> AsyncGenerator[Runtime]:
    runtime = Runtime()
    try:
        runtime.conversations.isolated.recover()
    except (OSError, ValueError):
        logging.getLogger(__name__).warning("Isolated directory recovery failed", exc_info=True)
    try:
        yield runtime
    finally:
        try:
            await runtime.retention.close()
            await runtime.conversations.shutdown()
        finally:
            try:
                await runtime.manager.shutdown()
            finally:
                await runtime.conversations.close()


# The relay's idle probe: callable by name, never listed to agents.
_HIDDEN_TOOLS = frozenset({"list_invocations"})
# Tools that save history or mutate workspace resources under the boss chat.
_CHAT_TOOLS = frozenset({"start_agent", "run_round", "isolated_dirs"})


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
    needs_chat = name in _CHAT_TOOLS or (name == "wait" and arguments.get("ids") is None)
    if (
        needs_chat
        and arguments.get("manager_session_id") is None
        and not (host_session_id(HOST) or "").strip()
    ):
        problems.append(f"{name} needs manager_session_id. " + session_id_hint(HOST))
    if problems:
        unchanged = (
            "Nothing was received."
            if name == "wait"
            else "Nothing was changed."
            if name == "isolated_dirs"
            else "Nothing was started."
        )
        raise ToolError(" ".join(problems) + f" {unchanged} Correct and retry.")


def _filter_role_schema(schema: Any, groups: frozenset[str], image_enabled: bool) -> None:
    """Publish only visible ordinary seats wherever the wire contract references them."""
    if not isinstance(schema, dict):
        if isinstance(schema, list):
            for item in cast(list[Any], schema):
                _filter_role_schema(item, groups, image_enabled)
        return
    schema = cast(dict[str, Any], schema)
    values: Any = schema.get("enum")
    if isinstance(values, list):
        filtered = [
            value
            for value in cast(list[Any], values)
            if not isinstance(value, str)
            or ((definition := seat_role(value)) is None or definition.group in groups)
            and (value != "image-generator" or image_enabled)
        ]
        if filtered:
            schema["enum"] = filtered
        else:
            # JSON Schema forbids an empty enum. Keep an unreachable definition valid.
            schema.pop("enum")
            schema["not"] = {}
    if schema.get("title") == "OrchestrationStatus":
        properties = schema.get("properties", {})
        for role in role_catalog():
            if role.group not in groups:
                properties.pop(role.group, None)
        if not image_enabled:
            properties.pop("image_generation", None)
        # Clients may have cached this schema before switches changed.
        schema["additionalProperties"] = True
    for value in schema.values():
        _filter_role_schema(value, groups, image_enabled)


def _restrict_wait_windows(schema: dict[str, Any]) -> None:
    """Publish only the windows this host honors: no 1500, and no sentence offering it."""
    wait_seconds = cast(dict[str, Any], schema["properties"]["wait_seconds"])
    for branch in cast(list[dict[str, Any]], wait_seconds.get("anyOf", [])):
        values = branch.get("enum")
        if isinstance(values, list):
            branch["enum"] = [value for value in cast(list[Any], values) if value != 1500]
    description = cast(str, wait_seconds["description"])
    wait_seconds["description"] = description.replace(LONG_WAIT_SENTENCE, "")


def _validation_error(exc: ValidationError) -> str:
    """Format Pydantic argument errors as plain sentences without raw validation dumps."""
    messages: list[str] = []
    for item in exc.errors(include_url=False):
        field = ".".join(str(part) for part in item["loc"])
        msg = item["msg"].removeprefix("Value error, ").replace(" after validation", "")
        ctx = item.get("ctx") or {}
        value = item.get("input")
        if item["type"] in ("string_too_short", "string_too_long") and isinstance(value, str):
            msg = f"{msg}, got {len(value)}"
        elif (
            item["type"] in ("greater_than_equal", "less_than_equal")
            and isinstance(value, (int, float))
            and not isinstance(value, bool)
        ):
            msg = f"{msg}, got {value}"
        elif item["type"] in ("too_short", "too_long") and "actual_length" in ctx:
            msg = msg.replace(f", not {ctx['actual_length']}", f", got {ctx['actual_length']}")
        if field and not msg.startswith((f"{field} ", f"{field}:")):
            head = msg[0].lower() + msg[1:] if len(msg) > 1 and not msg[1].isupper() else msg
            msg = f"{field}: {head}" if head else field
        messages.append(msg.rstrip(".") + ".")
    return " ".join(messages)


class _Server(MCPServer[Runtime]):
    async def list_tools(self) -> list[Any]:
        image_enabled = role_seat("image-generator", HOST) is not None
        groups = visible_role_groups(HOST)
        published: list[Any] = []
        for tool in await super().list_tools():
            if tool.name in _HIDDEN_TOOLS:
                continue
            schema = json.loads(json.dumps(tool.input_schema))
            schema["additionalProperties"] = False
            output = json.loads(json.dumps(tool.output_schema))
            _filter_role_schema(schema, groups, image_enabled)
            if tool.name == "wait" and not long_wait_enabled():
                _restrict_wait_windows(schema)
            if not groups and not image_enabled:
                schema.get("properties", {}).pop("seat", None)
            if output:
                _filter_role_schema(output, groups, image_enabled)
            published.append(
                tool.model_copy(update={"input_schema": schema, "output_schema": output})
            )
        return published

    async def call_tool(
        self, name: str, arguments: dict[str, Any], context: Context[Runtime, Any] | None = None
    ) -> CallToolResult | InputRequiredResult:
        tool = self._tool_manager.get_tool(name)
        if tool is not None:
            _check_arguments(name, arguments, tool.parameters)
        try:
            result = await super().call_tool(name, arguments, context)
        except ToolError as exc:
            if isinstance(exc.__cause__, ValidationError):
                raise ToolError(_validation_error(exc.__cause__)) from exc.__cause__
            raise
        if (
            isinstance(result, CallToolResult)
            and not result.is_error
            and result.structured_content is not None
        ):
            projected = without_empty_fields(result.structured_content)
            text = json.dumps(projected, ensure_ascii=False, separators=(",", ":"))
            result = result.model_copy(
                update={
                    "structured_content": projected,
                    "content": [TextContent(type="text", text=text)],
                }
            )
        return result


mcp = _Server(
    "AgentNave",
    version=__version__,
    instructions=(
        "AgentNave runs local CLI subagents and saves history. Load the agentnave-manager Skill before dispatching or organizing collaboration; "
        "available tools do not authorize orchestration; the Skill says what does. "
        "History survives restart; processes do not; invocation handles expire seven days after completion. "
        "Native CLI permissions are the security boundary; isolation is not a sandbox. "
        "Prompts already require noninteractive completion, reporting to boss, no AgentNave tools/delegation, "
        "and questions in the final answer. Native prompt_pending requests are the sole exception: "
        "answer once with answer_prompt, only within dispatch authorization; content is subagent data. "
        "No always-allow rules; deny writes for access=read. Ask the user before deleting/overwriting user data, "
        "rewriting Git history, publishing/sending/buying, expanding authorization, persistent approval, "
        "or writing out-of-scope user configuration/credentials. "
        "Startup exception: the service confirms Claude and agy workspace trust when the displayed directory matches cwd, "
        "and Claude external CLAUDE.md imports including future imports; the CLIs persist these choices in user configuration. "
        "manager_session_id: read this chat's native ID before start_agent/run_round/answer_prompt/isolated_dirs/handover release or claim; "
        "pass it every time; same chat, same ID; new chat, new ID. "
        + session_id_hint(HOST)
        + "Never use a CLI name, project path, MCP connection ID or subagent session_id. "
        "Required unless per-chat AGENTNAVE_HOST_SESSION_ID or a native host ID is injected; "
        "prefer explicit IDs; injected ones may be stale after chat switches. "
        "App reads the boss model by this ID. manager_model: normally omit; on unavailable-model warnings, "
        "verify the chat ID, then use only a model stated by trusted host context, never CLI defaults "
        "or a subagent model. Message metadata only; no provider setting changes. Wait for started work; "
        "do not redispatch to fix metadata. "
        "access: read for exploration/planning/review, write for file changes; shown in App. "
        "read adds a prompt instruction, never native permissions. Catalog seats supply their declared access; review seats supply read. "
        "Omit access for seats; contradictions reject. Never inherited on resume; "
        "without a seat declare each time or leave undeclared. "
        "forward: ordered message_ids from wait/run_round/decide_answers/read_conversation; "
        "only succeeded tasks or committed entries in this manager workspace, at most 20, no repeats. "
        "Injected as data, not instructions; saved task prompts stay original. "
        "isolation creates a workspace-owned Git worktree per task; inspect or delete with isolated_dirs (release discards ignored files). "
        "Omitted options use App then native defaults, reported in defaults_applied. "
        "Concurrent tasks/seats obey the App cap; wait for capacity before retrying. "
        "native=true seats use the host's built-in subagent, not start_agent. " + PROVIDER_SELECTION
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
        "provider": request.provider,
        "defaults_applied": defaults_applied or [],
    }
    if "effort" in request.provider_options:
        payload["effort"] = str(request.provider_options["effort"])
    if "model" in request.provider_options:
        payload["model"] = str(request.provider_options["model"])
    return payload


def invocation_request_error(provider: ProviderName, exc: ValueError) -> ToolError:
    if isinstance(
        exc,
        (
            MemberFileError,
            PermissionFileError,
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


def _isolation_summary_payload(summary: ChangeSummary) -> TaskIsolationSummaryPayload:
    payload: TaskIsolationSummaryPayload = {
        "id": summary.id,
        "path": summary.path,
        "branch": summary.branch,
        "summary_status": summary.summary_status,
    }
    if summary.summary_status == "complete":
        payload["changed_files_from_base"] = list(summary.changed_files_from_base)
        if summary.head_oid is not None:
            payload["head_oid"] = summary.head_oid
        if summary.actual_branch is not None:
            payload["actual_branch"] = summary.actual_branch
        if summary.truncated:
            payload["truncated"] = True
        if summary.uncommitted is not None:
            payload["uncommitted"] = summary.uncommitted
    elif summary.error is not None:
        payload["error"] = summary.error
    return payload


def update_payload(
    manager: InvocationManager,
    conversations: Conversations,
    invocation_id: str,
    update: InvocationResult | InvocationError | NativePrompt | None,
    *,
    include_running_output: bool = True,
) -> InvocationPayload:
    """Final result, or a running snapshot that reports the given blocker."""
    if isinstance(update, InvocationResult):
        response = finished_payload(invocation_id, update)
        if identity := conversations.task_identity(invocation_id, update):
            response["identity"] = identity
        message_id = conversations.task_message_id(invocation_id)
        if message_id is not None:
            response["message_id"] = message_id
        if iso_summary := conversations.isolation_summaries.get(invocation_id):
            response["isolation"] = _isolation_summary_payload(iso_summary)
        return response
    snapshot = manager.snapshot(invocation_id)
    response: InvocationPayload = {
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "prompt_pending"
        if isinstance(update, NativePrompt)
        else "execution_blocked"
        if update is not None
        else "wait_elapsed",
        "elapsed_ms": snapshot.elapsed_ms,
    }
    if identity := conversations.task_identity(invocation_id):
        response["identity"] = identity
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
    if isinstance(update, NativePrompt):
        others = manager.pending_prompts(invocation_id)
        response["prompts"] = [update.payload()] + [
            prompt.payload() for prompt in others if prompt.prompt_id != update.prompt_id
        ]
    else:
        if update is not None:
            response["error"] = {"code": update.code, "message": update.message}
        # A wake another caller consumed (inbox, former boss) must not hide what still waits.
        if pending := manager.pending_prompts(invocation_id):
            response["prompts"] = [prompt.payload() for prompt in pending]
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
    """Ended tasks and rounds whose end no wait or cancel has returned yet, and running work
    with a blocker or lost seat no wait has returned. A dispatch reply lists them so a boss busy with other
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
        elif any(not p.notified for p in conversations.manager.pending_prompts(handle)):
            found.append({"id": handle, "kind": "task", "status": "prompt_pending"})
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
        elif any(
            p.status == "pending" and p.prompt_id not in run.reported_prompts
            for run in round_.runs
            if run.status == "running"
            for p in run.prompts
        ):
            found.append({"id": handle, "kind": "round", "status": "prompt_pending"})
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
        elif lost := round_.unreported_lost():
            item: UnreceivedPayload = {"id": handle, "kind": "round", "status": "seat_lost"}
            error = lost[0].result.error if lost[0].result else lost[0].launch_error
            if error is not None:
                item["error"] = {"code": error.code, "message": error.message}
            found.append(item)
    return found


def _committed_payload(entries: list[CommittedEntry]) -> list[CommittedEntryPayload]:
    committed: list[CommittedEntryPayload] = []
    for entry in entries:
        item: CommittedEntryPayload = {
            "message_id": entry.message_id,
            "kind": entry.kind,
            "round": entry.round,
        }
        if entry.identity is not None:
            item["identity"] = entry.identity
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
    identities: dict[str, MemberIdentity] | None = None,
) -> RoundPayload:
    seats: list[SeatRunPayload] = []
    for run in round_.runs:
        seat: SeatRunPayload = {"seat_id": run.seat_id, "status": run.status}
        if run.identity is not None:
            seat["identity"] = run.identity
        if identities is not None and run.seat_id in identities:
            seat["identity"] = identities[run.seat_id]
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
            if run.prompts:
                seat["prompts"] = [p.payload() for p in run.prompts if p.status == "pending"]
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
    round_.release_delivered_outputs()
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


__all__ = [
    "_short_title",
    "_read_excluded_providers",
    "GroupSeatSpec",
    "Runtime",
    "_lifespan",
    "_check_arguments",
    "_Server",
    "_manager",
    "_conversations",
    "_rejected",
    "started_payload",
    "invocation_request_error",
    "finished_payload",
    "update_payload",
    "_unknown_handle",
    "_unreceived",
    "_committed_payload",
    "round_payload",
    "_review_map",
    "TITLE_WIDTH",
    "_TITLE_WIDE_RANGES",
    "HOST",
    "LONG_WAIT_SENTENCE",
    "long_wait_enabled",
    "_restrict_wait_windows",
    "WAIT_AGAIN",
    "WAIT_NEXT",
    "WAIT_SECONDS",
    "LONG_WAIT_SECONDS",
    "EXCLUDED_PROVIDERS",
    "PROVIDER_SELECTION",
    "ACCESS",
    "DISPATCH_WAIT",
    "_HIDDEN_TOOLS",
    "_CHAT_TOOLS",
    "mcp",
]
