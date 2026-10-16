"""AgentNave Studio STDIO MCP server: the core tools plus saved, reviewable shows."""

from __future__ import annotations

import os
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal, NotRequired, TypedDict, cast

from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp_types import ToolAnnotations
from pydantic import Field

from agentnave import __version__
from agentnave import mcp_server as core
from agentnave.group import GroupSeat, Round
from agentnave.mcp_server import (
    EXCLUDED_PROVIDERS,
    PROVIDER_OPTIONS,
    PROVIDER_SELECTION,
    TOOL_OPTIONS,
    WAIT_SECONDS,
    ActivityPayload,
    GroupSeatSpec,
    InvocationErrorPayload,
    InvocationPayload,
    ProviderDescriptionPayload,
    ProviderName,
)
from agentnave.models import InvocationRequest, ProviderOption
from agentnave.nesting import detect_nesting
from agentnave_studio.store import Conversation, Unavailable
from agentnave_studio.studio import Decision, DeletionResult, Studio


class Runtime(core.Runtime):
    def __init__(self) -> None:
        super().__init__()
        root = Path(os.environ.get("AGENTNAVE_DATA_DIR", str(Path.home() / ".agentnave")))
        self.studio = Studio(self.manager, root)
        # Core round tools reach the studio through the groups attribute.
        self.groups = self.studio


@asynccontextmanager
async def _lifespan(_server: MCPServer[Runtime]) -> AsyncGenerator[Runtime]:
    runtime = Runtime()
    try:
        yield runtime
    finally:
        try:
            await runtime.studio.shutdown()
        finally:
            try:
                await runtime.manager.shutdown()
            finally:
                await runtime.studio.close()


mcp = MCPServer(
    "AgentNave Studio",
    version=__version__,
    instructions=(
        "AgentNave Studio launches local CLI subagents and saves their history. Use the "
        "agentnave-manager Skill. start_agent/wait_agent/cancel_agent run tasks, saved as private "
        "task conversations; wait_any waits on several and list_invocations lists handles. "
        "list_profiles reads user orchestration rules; apply current user requirements first "
        "and pass selected provider/options explicitly, permissions only when requested this turn. "
        "open_group opens a saved show; run_round, wait_round and cancel_round drive it. "
        "With review=true, wait_round returns review_needed and decide_answers publishes or "
        "discards each answer before other seats see it. "
        "Saved operations and list_conversations provide local read-only dashboard links; "
        "nothing listens before these are called. History survives restart; processes do not. "
        "Provider-native permissions remain the security boundary; isolation is not a sandbox. "
        + PROVIDER_SELECTION
    ),
    lifespan=_lifespan,
)


def _studio(ctx: Context[Runtime]) -> Studio:
    return ctx.request_context.lifespan_context.studio


def _rejected(exc: Exception) -> ToolError:
    return ToolError(f"{exc}. Correct the request and retry.")


class ViewerLinks(TypedDict):
    dashboard_url: NotRequired[str]
    dashboard_error: NotRequired[str]


class DashboardLinks(ViewerLinks):
    public_url: NotRequired[str]
    director_url: NotRequired[str]


def _dashboard_links(studio: Studio, conversation_id: str) -> DashboardLinks:
    """A saved operation stays successful even if the optional viewer cannot start."""
    try:
        home = studio.open_dashboard()
    except OSError as exc:
        return {"dashboard_error": str(exc)}
    links: DashboardLinks = {"dashboard_url": home}
    urls = studio.urls(conversation_id)
    if urls is not None:
        links["director_url"] = urls[1]
        if studio.records[conversation_id].state.kind == "show":
            links["public_url"] = urls[0]
    return links


class TaskStartPayload(InvocationPayload, DashboardLinks):
    conversation_id: str


@mcp.tool(
    title="Run a task with a local AI CLI",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=True
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
    conversation_id: Annotated[
        str | None, Field(description="Task conversation to add this task to.")
    ] = None,
    title: Annotated[
        str | None, Field(max_length=200, description="Title for a new task conversation.")
    ] = None,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start one CLI invocation and return its invocation_id; use wait_agent for the result.

    The reply includes the CLI and requested model/effort; omitted options show native inheritance.
    These are requested settings, not confirmation of the CLI's effective settings.
    The task and its final output are saved in a private task conversation; a resumed native
    session joins its open conversation. No output is shared with another CLI. No runtime
    deadline; use cancel_agent to stop work.
    """
    if provider in EXCLUDED_PROVIDERS:
        raise ToolError(f"Provider '{provider}' is excluded by this host. " + PROVIDER_SELECTION)
    try:
        request = InvocationRequest(provider, prompt, Path(cwd), session_id, provider_options or {})
        invocation_id, saved_id = _studio(ctx).start_task(request, title, conversation_id)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except ValueError as exc:
        supported = (
            f" Supported {provider} options: model, effort, {PROVIDER_OPTIONS[provider]}."
            if "option" in str(exc)
            else ""
        )
        raise ToolError(f"Invalid invocation request: {exc}.{supported}") from exc
    except OSError as exc:
        raise ToolError(f"Unable to prepare or save the task: {exc}.") from exc
    return {
        **core.started_payload(invocation_id, request),
        "conversation_id": saved_id,
        **_dashboard_links(_studio(ctx), saved_id),
    }


for _tool in (
    core.wait_agent,
    core.wait_any,
    core.cancel_agent,
    core.list_invocations,
    core.list_profiles,
):
    _title, _annotations = TOOL_OPTIONS[_tool.__name__]
    mcp.add_tool(_tool, title=_title, annotations=_annotations)


class StudioProviderPayload(ProviderDescriptionPayload):
    show_permitted: bool


@mcp.tool(
    title="Read one CLI's permitted status and options",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def describe_provider(
    provider: Annotated[
        ProviderName, Field(description="Provider whose permitted status and options to return.")
    ],
) -> StudioProviderPayload:
    """Read one provider's permitted status and supported options without launching it.

    permitted applies to start_agent; show_permitted to shows, which may seat any provider,
    including the host CLI.
    """
    return {**await core.describe_provider(provider), "show_permitted": True}


class ShowPayload(DashboardLinks):
    group_id: str
    conversation_id: str
    seats: list[core.GroupSeatPayload]


@mcp.tool(
    title="Open a saved show of CLI seats",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)
async def open_group(
    title: Annotated[str, Field(min_length=1, max_length=200)],
    seats: Annotated[list[GroupSeatSpec], Field(min_length=2, max_length=6)],
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
    *,
    ctx: Context[Runtime],
) -> ShowPayload:
    """Create a saved show of 2–6 CLI seats; drive it with run_round. group_id is the conversation id.

    Seats are subagents with native tools and permissions unless isolated. Each keeps its own
    native session across rounds, also after a server restart. Host exclusions apply only to
    start_agent, so any show may seat them. Isolation restricts tools; it is not an OS sandbox.
    """
    try:
        record = _studio(ctx).open_show(
            title,
            Path(cwd) if cwd is not None else None,
            [
                GroupSeat(
                    seat.id,
                    seat.provider,
                    dict(seat.provider_options),
                    Path(seat.cwd if seat.cwd is not None else cwd or "."),
                )
                for seat in seats
            ],
            review=review,
            isolated=isolated,
        )
    except (OSError, ValueError) as exc:
        raise ToolError(f"Invalid show: {exc}. Correct the arguments and retry.") from exc
    state = record.state
    return {
        **_dashboard_links(_studio(ctx), state.id),
        "group_id": state.id,
        "conversation_id": state.id,
        "seats": [
            {"id": seat.id, "provider": seat.provider, "cwd": seat.cwd} for seat in state.seats
        ],
    }


type StudioSeatStatus = Literal[
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


class StudioSeatRunPayload(TypedDict):
    seat_id: str
    status: StudioSeatStatus
    invocation_id: NotRequired[str]
    activity: NotRequired[ActivityPayload]
    error: NotRequired[InvocationErrorPayload]
    output: NotRequired[str]
    output_age_ms: NotRequired[int]
    session_id: NotRequired[str]


class StudioRoundPayload(DashboardLinks):
    round_id: str
    group_id: str
    round: int
    order: Literal["parallel", "serial"]
    status: Literal["running", "finished", "failed", "cancelled"]
    reason: str
    elapsed_ms: int
    seats: list[StudioSeatRunPayload]
    error: NotRequired[str]


def _round_payload(
    studio: Studio, round_: Round, reason: str, *, include_output: bool = True
) -> StudioRoundPayload:
    payload = cast(
        StudioRoundPayload,
        core.round_payload(round_, studio.manager, reason, include_output=include_output),
    )
    reviews = studio.reviews.get(round_.id)
    if reviews is None:
        # Completed rounds are read-only replies; their review decisions live on disk.
        reviews = {
            seat.seat_id: seat.review
            for saved in studio.records[round_.group.id].state.rounds
            if saved.id == round_.id
            for seat in saved.seats
        }
    for seat in payload["seats"]:
        if reviews.get(seat["seat_id"]) == "pending":
            seat["status"] = "pending_review"
        elif reviews.get(seat["seat_id"]) == "discard":
            seat["status"] = "discarded"
    return payload


_title, _annotations = TOOL_OPTIONS["run_round"]


@mcp.tool(title=_title, annotations=_annotations)
async def run_round(
    group_id: str,
    seats: Annotated[list[str], Field(min_length=1)],
    order: Literal["parallel", "serial"],
    prompt: Annotated[str, Field(min_length=1)],
    instructions: dict[str, str] | None = None,
    *,
    ctx: Context[Runtime],
) -> StudioRoundPayload:
    """Start a saved show round, then wait_round; only committed answers reach other seats.

    parallel starts together and seals answers until all end; serial follows seat order.
    Private instructions go only to their seat. Includes viewer links for the manager only.
    """
    studio = _studio(ctx)
    try:
        round_ = studio.run(group_id, seats, order, prompt, instructions or {})
    except KeyError as exc:
        raise ToolError("Unknown group_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    payload = _round_payload(studio, round_, "started")
    return {**payload, **_dashboard_links(studio, group_id)}


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
) -> StudioRoundPayload:
    """Wait up to five minutes; returns early when the round finishes, a seat reports a blocker,
    or answers wait for review (reason=review_needed, statuses pending_review).

    Review answers by calling decide_answers, then wait again. Finished rounds can be read again.
    include_output=false omits bodies, including pending review answers; true can read them later.
    """
    studio = _studio(ctx)
    try:
        round_, reason = await studio.wait(round_id, WAIT_SECONDS)
    except KeyError as exc:
        raise ToolError(
            "Unknown round_id in this server session; read_conversation shows saved rounds."
        ) from exc
    return _round_payload(studio, round_, reason or "wait_elapsed", include_output=include_output)


@mcp.tool(
    title="Cancel a show round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=True, open_world_hint=False
    ),
)
async def cancel_round(
    round_id: Annotated[str, Field(min_length=1)], *, ctx: Context[Runtime]
) -> StudioRoundPayload:
    """Stop a round: running seats are cancelled and recorded absent, unstarted serial seats skip.

    Answers already published or decided publish are kept; undecided answers are discarded.
    Does not undo CLI side effects.
    """
    studio = _studio(ctx)
    try:
        round_ = await studio.cancel(round_id)
    except KeyError as exc:
        raise ToolError("Unknown round_id in this server session.") from exc
    return _round_payload(studio, round_, "finished")


class ReviewReceipt(TypedDict):
    round_id: str
    decisions: dict[str, Decision]
    pending_review: list[str]
    committed: list[str]


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
    answer is decided; a serial round continues with its next seat.
    The receipt confirms saved decisions, not round completion; wait_round to confirm.
    """
    try:
        saved = _studio(ctx).decide(round_id, decisions)
        return {
            "round_id": round_id,
            "decisions": dict(decisions),
            "pending_review": [seat.seat_id for seat in saved.seats if seat.review == "pending"],
            "committed": [
                seat.seat_id for seat in saved.seats if seat.committed and seat.review != "discard"
            ],
        }
    except KeyError as exc:
        raise ToolError("Unknown round_id; read_conversation lists saved rounds.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc


class HostReceipt(TypedDict):
    conversation_id: str
    entry_index: int


class UpdateReceipt(TypedDict):
    conversation_id: str
    title: str
    archived: bool


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
    *,
    ctx: Context[Runtime],
) -> HostReceipt:
    """Add a public host remark that every seat receives as data with its next round.

    Only between rounds, with no answers pending review.
    """
    studio = _studio(ctx)
    try:
        studio.post(conversation_id, text)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return {
        "conversation_id": conversation_id,
        "entry_index": len(studio.records[conversation_id].state.entries) - 1,
    }


def _view(studio: Studio, conversation_id: str, writable: bool) -> ConversationView:
    view: ConversationView = {
        "conversation": studio.records[conversation_id].state,
        "writable": writable,
    }
    urls = studio.urls(conversation_id)
    if urls is not None:
        if studio.records[conversation_id].state.kind == "show":
            view["public_url"] = urls[0]
        view["director_url"] = urls[1]
    return view


class ConversationSummary(TypedDict):
    conversation_id: str
    title: str
    kind: Literal["show", "task"]
    archived: bool
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

    writable=false means another AgentNave Studio server holds it: read it here, continue it
    there or once it becomes idle. unavailable lists unreadable or legacy records.
    """
    studio = _studio(ctx)
    try:
        writable = studio.scan()
    except OSError as exc:
        raise _rejected(exc) from exc
    result: ConversationList = {
        "conversations": [
            {
                "conversation_id": record.state.id,
                "title": record.state.title,
                "kind": record.state.kind,
                "archived": record.state.archived,
                "writable": writable.get(record.state.id, True),
                "created": record.state.created,
            }
            for record in sorted(studio.records.values(), key=lambda item: item.state.created)
        ],
        "unavailable": studio.unavailable,
    }
    try:
        result["dashboard_url"] = studio.open_dashboard(history_loaded=True)
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
    studio = _studio(ctx)
    try:
        record = studio.record(conversation_id)
        writable = record.refresh()
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return _view(studio, conversation_id, writable)


@mcp.tool(
    title="Rename, end or reopen a conversation",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def update_conversation(
    conversation_id: str,
    title: Annotated[str | None, Field(max_length=200)] = None,
    archived: bool | None = None,
    *,
    ctx: Context[Runtime],
) -> UpdateReceipt:
    """Rename, end (archived=true) or reopen a conversation. Ending needs no running work or
    pending review."""
    studio = _studio(ctx)
    try:
        record = studio.update(conversation_id, title, archived)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return {
        "conversation_id": conversation_id,
        "title": record.state.title,
        "archived": record.state.archived,
    }


@mcp.tool(
    title="Delete an ended conversation",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=False
    ),
)
async def delete_conversation(conversation_id: str, *, ctx: Context[Runtime]) -> DeletionResult:
    """Permanently delete an ended conversation's saved history after user authorization.

    Native CLI files and nonempty seat folders are retained and reported.
    """
    try:
        return _studio(ctx).delete(conversation_id)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc


def main() -> None:
    nesting = detect_nesting()
    if nesting.status != "not_nested":
        MCPServer("AgentNave Studio", version=__version__, instructions=nesting.instructions).run(
            transport="stdio"
        )
    else:
        mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
