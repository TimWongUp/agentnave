"""T3 Code MCP tool projections, instructions, refusals, and member listing (ADR-0121)."""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping, Sequence
from typing import Annotated, Any, Literal

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.server.mcpserver.tools import Tool
from mcp_types import Tool as MCPTool
from mcp_types import ToolAnnotations
from pydantic import BaseModel, ConfigDict, Field

from agentnave.accounts import AccountListing
from agentnave.host import Host, HostName, HostSource
from agentnave.mcp_payloads import ProviderDetails
from agentnave.members import (
    ListedMember,
    MemberFileError,
    Mode,
    PermissionTier,
    ProviderName,
    ReviewRole,
    read_members,
)
from agentnave.members import list_members as read_member_list
from agentnave.orchestration_roles import CustomSeatDifficulty, StrictModel
from agentnave.t3 import current_t3_mode
from agentnave.t3_cache import InstanceAvailability, check_instance, format_utc
from agentnave.t3_recommend import (
    MAX_T3_TASKS,
    T3RecommendSeatPayload,
    T3RecommendTask,
    normalize_t3_recommend_args,
    recommend_t3_seats,
)
from agentnave.t3_seats import (
    T3Candidate,
    T3Option,
    T3OrchestrationMode,
    T3SeatsFile,
    T3SeatsFileError,
    effective_t3_seats,
    t3_host_mode,
    t3_seat_name,
)

T3_HIDDEN_DISPATCH_TOOLS = frozenset({"start_agent", "start_agents"})

_T3_MAIN_TAIL = (
    "Then call delegate_task once per item copying candidate[0].target and clientRequestId, "
    "switching to the next candidate only if T3 creates no task and rejects the instance, model, "
    "or option. Never use start_agent. Use run_round for meetings (no recommend_seat needed). "
    "Waiting, worktrees, and in-T3 thread handoff belong to T3; use handover only to cross in or "
    "out of T3. "
)

T3_MAIN_INSTRUCTIONS_DECISION = (
    "T3 orchestration is active (decision mode): unless the user named a T3 account or model, "
    "call recommend_seat(tasks) before any delegate_task or built-in subagent so the decision "
    "model picks seats. " + _T3_MAIN_TAIL
)

T3_MAIN_INSTRUCTIONS_BOSS = (
    "T3 orchestration is active (boss picks seats): unless the user named a T3 account or model, "
    "call recommend_seat(tasks) with seat from list_members before any delegate_task or built-in "
    "subagent. " + _T3_MAIN_TAIL
)

T3_MAIN_SKILL = (
    "Load agentnave-manager (or agentnave-meeting when only that is installed) only for "
    "meetings, reviews, or handover. "
)

_RECOMMEND_DESC_DECISION = (
    "Choose T3 Code seats and ordered delegate_task targets for 1–8 tasks; never starts anything.\n\n"
    "In decision mode, omit seat so the decision model picks among usable task seats with matching "
    "access (skipped when only one fits), or pass explicit seat (fixed role name such as "
    "worker-easy, custom T3 seat id, or review role name) to skip the decision model. Returns "
    "ordered T3 targets (options always included, [] when none), a uuid4 clientRequestId, and "
    "read-only brief_suffix when access='read'. Errors start with a code: seat_required, "
    "not_enabled, config_error, no_key, network, rate_limited, auth, rejected, or bad_response. "
    "Follow receipt next to call T3's delegate_task once per selected item."
)

_RECOMMEND_DESC_BOSS = (
    "Resolve explicit T3 Code seats into ordered delegate_task targets for 1–8 tasks; never starts "
    "anything.\n\n"
    "In this mode the boss chooses seats: pass seat on every item using a fixed role name (such as "
    "worker-easy or explorer-web-hard), a custom T3 seat id from list_members, or a review role "
    "name; omitting seat fails with seat_required and lists available seats without calling any "
    "decision model. Returns ordered T3 targets (options always included, [] when none), a uuid4 "
    "clientRequestId, and read-only brief_suffix when access='read'. Errors start with a code: "
    "seat_required, not_enabled, config_error. Follow receipt next to call T3's delegate_task once "
    "per selected item."
)

_LIST_MEMBERS_DESC_DECISION = (
    "Read T3 task seats, T3 review roles, and AgentNave meeting members afresh.\n\n"
    "In T3 Code main threads (decision mode), single tasks and reviews use t3_orchestration via "
    "recommend_seat (decision model picks unless seat is passed) + T3 delegate_task; standard "
    "catalog seats are keyed by fixed name (e.g. worker-easy), custom seats by id. members lists "
    "meeting (discussion) CLIs only. Unavailable T3 candidates reflect local T3 cache status and "
    "quota windows."
)

_LIST_MEMBERS_DESC_BOSS = (
    "Read T3 task seats, T3 review roles, and AgentNave meeting members afresh.\n\n"
    "In T3 Code main threads (boss picks seats), single tasks and reviews use t3_orchestration: "
    "pick a seat key from t3_orchestration.seats (fixed role name such as worker-easy for standard "
    "roles, or id for custom seats) or t3_orchestration.review and pass it to recommend_seat + T3 "
    "delegate_task. members lists meeting (discussion) CLIs only. Unavailable T3 candidates reflect "
    "local T3 cache status and quota windows."
)


def t3_main_instructions(mode: T3OrchestrationMode) -> str:
    """Leading MCP instructions for an active T3 main thread according to the host's mode."""
    return T3_MAIN_INSTRUCTIONS_DECISION if mode == "decision" else T3_MAIN_INSTRUCTIONS_BOSS


class T3CandidateStatus(StrictModel):
    instance: str
    model: str
    options: list[T3Option] = Field(default_factory=list[T3Option])
    available: bool
    reason: str | None = None
    resets_at: str | None = None


class T3SeatStatus(StrictModel):
    kind: str
    role: str
    description: str
    access: Literal["read", "write"]
    difficulty: CustomSeatDifficulty | None = None
    candidates: list[T3CandidateStatus] = Field(default_factory=list[T3CandidateStatus])
    available: Literal[False] | None = None
    reason: str | None = None


class T3ReviewRoleStatus(StrictModel):
    candidates: list[T3CandidateStatus] = Field(default_factory=list[T3CandidateStatus])
    available: Literal[False] | None = None
    reason: str | None = None


class T3OrchestrationStatus(StrictModel):
    enabled: Literal[True] = True
    mode: T3OrchestrationMode
    random_candidates: bool = False
    seats: dict[str, T3SeatStatus] = {}
    review: dict[ReviewRole, T3ReviewRoleStatus] = {}


class T3MemberListPayload(BaseModel):
    # Clients may have cached this schema before T3 switches or file health changed.
    model_config = ConfigDict(extra="allow", strict=True, allow_inf_nan=False)

    configured: bool
    error: str | None = None
    host: HostName | None = None
    host_source: HostSource
    restricted: list[Mode] = Field(default_factory=list[Mode])
    members: list[ListedMember]
    t3_orchestration: T3OrchestrationStatus | None = None
    orchestration_error: str | None = None
    permissions: dict[ProviderName, PermissionTier] = {}
    permissions_error: str | None = None
    accounts: list[AccountListing] = Field(default_factory=list[AccountListing])
    accounts_error: str | None = None
    provider_details: ProviderDetails | None = None


async def t3_recommend_seat(
    tasks: Annotated[
        list[T3RecommendTask],
        Field(
            min_length=1,
            max_length=MAX_T3_TASKS,
            description=(
                "1–8 tasks to resolve T3 seats and ordered delegate_task targets for; "
                "each item specifies task, access (read/write), and seat (fixed role name such as "
                "worker-easy, custom T3 seat id, or review role name; required in split/custom, "
                "optional in decision mode)."
            ),
        ),
    ],
) -> T3RecommendSeatPayload:
    """Choose T3 Code seats and ordered delegate_task targets for 1–8 tasks; never starts anything."""
    from agentnave import mcp_runtime as runtime

    try:
        doc = effective_t3_seats(runtime.HOST)
    except T3SeatsFileError as exc:
        raise ToolError(f"config_error: {exc}") from None
    mode = t3_host_mode(runtime.HOST)
    if doc is None or mode is None:
        raise ToolError("not_enabled: T3 orchestration is not active for this connection.")
    return await recommend_t3_seats(tasks, doc, mode=mode)


def _candidate_statuses(
    candidates: Sequence[T3Candidate], probe: Callable[[str, str], InstanceAvailability]
) -> tuple[list[T3CandidateStatus], Literal[False] | None, str | None]:
    if not candidates:
        return [], False, "no candidates configured"
    items: list[T3CandidateStatus] = []
    reasons: list[str] = []
    any_usable = False
    for cand in candidates:
        status = probe(cand.instance, cand.model)
        if status.available:
            any_usable = True
        else:
            reasons.append(f"{cand.instance}/{cand.model}: {status.reason or 'unavailable'}")
        items.append(
            T3CandidateStatus(
                instance=cand.instance,
                model=cand.model,
                options=list(cand.options),
                available=status.available,
                reason=status.reason,
                resets_at=format_utc(status.resets_at) if status.resets_at is not None else None,
            )
        )
    if any_usable:
        return items, None, None
    return items, False, "; ".join(reasons[:4])


def _discussion_only_members(members: Sequence[ListedMember]) -> list[ListedMember]:
    result: list[ListedMember] = []
    for member in members:
        if member.modes is not None and "discussion" not in member.modes:
            continue
        default: list[Mode] | None = (
            ["discussion"]
            if member.default is True
            or (isinstance(member.default, list) and "discussion" in member.default)
            else None
        )
        result.append(member.model_copy(update={"modes": ["discussion"], "default": default}))
    return result


async def t3_list_members(
    provider: Annotated[
        ProviderName | None,
        Field(description="Also return this CLI's meeting status and supported options."),
    ] = None,
) -> T3MemberListPayload:
    """Read T3 task seats, T3 review roles, and AgentNave meeting members afresh."""
    from agentnave import mcp_runtime as runtime

    try:
        doc = effective_t3_seats(runtime.HOST)
        base = read_member_list(runtime.task_excluded_providers(), runtime.HOST)
    except (OSError, ValueError) as exc:
        raise ToolError(str(exc)) from exc
    mode = t3_host_mode(runtime.HOST)
    if doc is None or mode is None:
        raise ToolError("T3 orchestration is not active for this connection.")
    cache: dict[tuple[str, str], InstanceAvailability] = {}

    def probe(instance: str, model: str) -> InstanceAvailability:
        if (instance, model) not in cache:
            cache[instance, model] = check_instance(instance, model)
        return cache[instance, model]

    seat_statuses: dict[str, T3SeatStatus] = {}
    for seat in doc.seats:
        items, avail, reason = _candidate_statuses(seat.candidates, probe)
        seat_statuses[t3_seat_name(seat)] = T3SeatStatus(
            kind=seat.kind,
            role=seat.role,
            description=seat.description,
            access=seat.access,
            difficulty=seat.difficulty,
            candidates=items,
            available=avail,
            reason=reason,
        )
    review_statuses: dict[ReviewRole, T3ReviewRoleStatus] = {}
    for role, candidates in doc.review.items():
        if not candidates:
            continue
        items, avail, reason = _candidate_statuses(candidates, probe)
        review_statuses[role] = T3ReviewRoleStatus(candidates=items, available=avail, reason=reason)
    restricted: list[Mode] = ["task"]
    if "discussion" in base.restricted:
        restricted.append("discussion")
    provider_details: ProviderDetails | None = None
    if provider is not None:
        try:
            member_doc = read_members()
        except MemberFileError as exc:
            raise ToolError(str(exc)) from exc
        discussion = member_doc.modes.get("discussion") if member_doc is not None else None
        provider_details = ProviderDetails(
            provider=provider,
            permitted=False,
            show_permitted=discussion is None or provider in discussion,
            supported_options="model, effort, " + runtime.PROVIDER_OPTIONS[provider],
        )
    return T3MemberListPayload(
        configured=base.configured,
        error=base.error,
        host=base.host,
        host_source=base.host_source,
        restricted=restricted,
        members=_discussion_only_members(base.members),
        t3_orchestration=T3OrchestrationStatus(
            enabled=True,
            mode=mode,
            random_candidates=doc.random_candidates,
            seats=seat_statuses,
            review=review_statuses,
        ),
        permissions=base.permissions,
        permissions_error=base.permissions_error,
        accounts=base.accounts,
        accounts_error=base.accounts_error,
        provider_details=provider_details,
    )


T3_RECOMMEND_SEAT_TOOL = Tool.from_function(
    t3_recommend_seat,
    name="recommend_seat",
    title="Recommend T3 Code seats and targets for tasks",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=False, open_world_hint=True
    ),
)

T3_LIST_MEMBERS_TOOL = Tool.from_function(
    t3_list_members,
    name="list_members",
    title="Read T3 seats, review roles and meeting members",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)


def t3_dispatch_refusal(tool_name: str, arguments: Mapping[str, Any]) -> str:
    """Guidance when start_agent or start_agents is called while T3 orchestration is active."""
    resuming = tool_name == "start_agent" and bool(arguments.get("session_id"))
    resume_note = (
        "To resume an earlier AgentNave private chat thread, use handover to transfer the "
        "workspace or continue that thread outside T3 Code (you can still read its history "
        "with list and read_conversation). "
    )
    lead = (
        f"Refused: {tool_name} is disabled in T3 Code main threads while T3 orchestration is "
        "enabled. "
    )
    if resuming:
        return (
            lead
            + resume_note
            + "For new tasks, call recommend_seat(tasks) and then T3's delegate_task."
        )
    return (
        lead + "Call recommend_seat(tasks) to pick T3 targets, then dispatch each item with T3's "
        "delegate_task. " + resume_note.strip()
    )


def t3_tool_definition(tool: Tool, mode: T3OrchestrationMode) -> MCPTool:
    """Build the published MCPTool representation for a T3-specific tool in the active mode."""
    schema = json.loads(json.dumps(tool.parameters))
    schema["additionalProperties"] = False
    output = json.loads(json.dumps(tool.output_schema)) if tool.output_schema else None
    if tool.name == "recommend_seat":
        description = _RECOMMEND_DESC_DECISION if mode == "decision" else _RECOMMEND_DESC_BOSS
    elif tool.name == "list_members":
        description = _LIST_MEMBERS_DESC_DECISION if mode == "decision" else _LIST_MEMBERS_DESC_BOSS
    else:
        description = tool.description
    return MCPTool(
        name=tool.name,
        title=tool.title,
        description=description,
        input_schema=schema,
        output_schema=output,
        annotations=tool.annotations,
        icons=tool.icons,
        _meta=tool.meta,
    )


def t3_state_for_host(
    host: Host,
) -> tuple[T3SeatsFile | None, T3OrchestrationMode | None, T3SeatsFileError | None]:
    """Return (active_doc, mode, file_error) for this connection without crashing list_tools."""
    if current_t3_mode(host) != "main":
        return None, None, None
    mode = t3_host_mode(host)
    if mode is None:
        return None, None, None
    try:
        doc = effective_t3_seats(host)
    except T3SeatsFileError as exc:
        return None, mode, exc
    return (doc, mode, None) if doc is not None else (None, None, None)


async def run_t3_tool(
    tool: Tool,
    arguments: dict[str, Any],
    context: Context[Any, Any],
    check_arguments: Callable[[str, dict[str, Any], dict[str, Any]], None],
) -> Any:
    """Validate and run a T3-specific Tool with the same argument check as standard tools."""
    effective_args = (
        normalize_t3_recommend_args(arguments) if tool.name == "recommend_seat" else arguments
    )
    check_arguments(tool.name, effective_args, tool.parameters)
    return await tool.run(effective_args, context, convert_result=True)
