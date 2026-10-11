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
from agentnave_studio import __version__
from agentnave_studio.store import Conversation, RoundRecord, Unavailable
from agentnave_studio.studio import Decision, DeletionResult, Studio


class Runtime(core.Runtime):
    def __init__(self) -> None:
        super().__init__()
        root = Path(os.environ.get("AGENTNAVE_DATA_DIR", str(Path.home() / ".agentnave")))
        self.studio = Studio(self.manager, root, EXCLUDED_PROVIDERS)
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
        "task conversations. open_group opens a saved show; run_round, wait_round and "
        "cancel_round drive it. With review=true, wait_round returns review_needed and "
        "decide_answers publishes or discards each answer before other seats see it. "
        "open_dashboard starts a local read-only page for the user to watch; nothing listens "
        "before it is called. History survives restart; running processes do not. "
        "Provider-native permissions remain the security boundary; isolation is not a sandbox. "
        + PROVIDER_SELECTION
        + " Isolated shows may include excluded providers, including the host CLI."
    ),
    lifespan=_lifespan,
)


def _studio(ctx: Context[Runtime]) -> Studio:
    return ctx.request_context.lifespan_context.studio


def _rejected(exc: Exception) -> ToolError:
    return ToolError(f"{exc}. Correct the request and retry.")


class TaskStartPayload(InvocationPayload):
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
        "invocation_id": invocation_id,
        "status": "running",
        "reason": "started",
        "elapsed_ms": 0,
        "conversation_id": saved_id,
    }


for _tool in (core.wait_agent, core.cancel_agent):
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

    permitted applies to start_agent and ordinary shows; show_permitted to isolated shows,
    which may include the host CLI.
    """
    return {**await core.describe_provider(provider), "show_permitted": True}


class ShowPayload(TypedDict):
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
    native session across rounds, also after a server restart. Excluded providers are rejected
    unless isolated. Isolation restricts tools; it is not an OS sandbox.
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
        "group_id": state.id,
        "conversation_id": state.id,
        "seats": [
            {"id": seat.id, "provider": seat.provider, "cwd": seat.cwd} for seat in state.seats
        ],
    }


_title, _annotations = TOOL_OPTIONS["run_round"]
mcp.add_tool(core.run_round, title=_title, annotations=_annotations)

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


class StudioRoundPayload(TypedDict):
    round_id: str
    group_id: str
    round: int
    order: Literal["parallel", "serial"]
    status: Literal["running", "finished", "cancelled"]
    reason: str
    elapsed_ms: int
    seats: list[StudioSeatRunPayload]
    error: NotRequired[str]


def _round_payload(studio: Studio, round_: Round, reason: str) -> StudioRoundPayload:
    payload = cast(StudioRoundPayload, core.round_payload(round_, studio.manager, reason))
    saved = next(
        (item for item in studio.records[round_.group.id].state.rounds if item.id == round_.id),
        None,  # never saved when its first write failed
    )
    for seat, record in zip(payload["seats"], saved.seats if saved else [], strict=False):
        if record.review == "pending":
            seat["status"] = "pending_review"
        elif record.review == "discard":
            seat["status"] = "discarded"
    return payload


@mcp.tool(
    title="Wait for a show round",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def wait_round(
    round_id: Annotated[str, Field(min_length=1)], *, ctx: Context[Runtime]
) -> StudioRoundPayload:
    """Wait up to five minutes; returns early when the round finishes, a seat reports a blocker,
    or answers wait for review (reason=review_needed, statuses pending_review).

    Review answers by calling decide_answers, then wait again. Finished rounds can be read again.
    """
    studio = _studio(ctx)
    try:
        round_, reason = await studio.wait(round_id, WAIT_SECONDS)
    except KeyError as exc:
        raise ToolError(
            "Unknown round_id in this server session; read_conversation shows saved rounds."
        ) from exc
    return _round_payload(studio, round_, reason or "wait_elapsed")


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
) -> RoundRecord:
    """Decide answers held for review; the whole batch is checked and saved before it applies.

    Published answers become visible to other seats; discarded ones never do. Repeating a
    decision is idempotent; changing one is rejected. Parallel rounds commit once every
    answer is decided; a serial round continues with its next seat.
    """
    try:
        return _studio(ctx).decide(round_id, decisions)
    except KeyError as exc:
        raise ToolError("Unknown round_id; read_conversation lists saved rounds.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc


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
) -> ConversationView:
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
    return _view(studio, conversation_id, True)


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


class ConversationList(TypedDict):
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
    there or after that server exits. unavailable lists unreadable or legacy records.
    """
    studio = _studio(ctx)
    writable = studio.scan()
    return {
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
) -> ConversationView:
    """Rename, end (archived=true) or reopen a conversation. Ending needs no running work or
    pending review."""
    studio = _studio(ctx)
    try:
        studio.update(conversation_id, title, archived)
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list_conversations.") from exc
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return _view(studio, conversation_id, True)


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


class DashboardPayload(TypedDict):
    dashboard_url: str
    storage: str


@mcp.tool(
    title="Open the local dashboard",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)
async def open_dashboard(*, ctx: Context[Runtime]) -> DashboardPayload:
    """Start the read-only local dashboard on first call and return its home page for the user.

    Each show has a public page (committed entries only) and a private director page.
    """
    studio = _studio(ctx)
    return {"dashboard_url": studio.open_dashboard(), "storage": str(studio.root)}


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
