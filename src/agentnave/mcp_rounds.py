"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Annotated, Literal, NotRequired, TypedDict

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import AfterValidator, Field

from agentnave import mcp_runtime as runtime
from agentnave.conversations import (
    Conversations,
    Decision,
    entry_message_id,
)
from agentnave.group import GroupSeat, Round
from agentnave.manager_identity import model_warnings
from agentnave.mcp_payloads import (
    CommittedEntryPayload,
    GroupSeatPayload,
    ShowRunPayload,
)
from agentnave.mcp_runtime import (
    DISPATCH_WAIT,
    GroupSeatSpec,
    ManagerModel,
    ManagerSessionId,
    Runtime,
    _committed_payload,
    _conversations,
    _rejected,
    _review_map,
    _short_title,
    _unreceived,
    round_payload,
)
from agentnave.mcp_tasks import waited_dispatch
from agentnave.member_names import seat_identity
from agentnave.members import (
    CapacityError,
    MemberFileError,
    PermissionFileError,
    SettingsFileError,
    apply_member,
    read_members,
    read_permissions,
)
from agentnave.task_start import (
    READ_ONLY_APPLIED,
)


def _add_started(
    payload: ShowRunPayload,
    conversations: Conversations,
    round_: Round,
    manager_session_id: str | None,
) -> None:
    payload["round"] = round_payload(
        round_,
        conversations.manager,
        "started",
        deliver=False,
        include_running_output=False,
        reviews=_review_map(conversations, round_),
        identities=conversations.round_identities(round_),
        entries=conversations.committed_entries(round_.group.id, round_.number),
    )
    message = next(
        entry
        for entry in reversed(round_.group.entries)
        if entry.kind == "task" and entry.round == round_.number
    )
    warnings = model_warnings(conversations.host, message)
    if warnings:
        payload["warnings"] = warnings
    unreceived = _unreceived(conversations, manager_session_id)
    if unreceived:
        payload["unreceived"] = unreceived


def _retry_seats(
    conversations: Conversations,
    group_id: str,
    seat_ids: list[str],
    manager_session_id: str | None,
) -> ShowRunPayload:
    try:
        round_ = conversations.retry(group_id, seat_ids, manager_session_id=manager_session_id)
    except KeyError as exc:
        raise ToolError("Unknown group_id; use list.") from exc
    except (OSError, ValueError) as exc:
        if isinstance(exc, (SettingsFileError, CapacityError)):
            raise ToolError(str(exc)) from exc
        raise _rejected(exc) from exc
    payload: ShowRunPayload = {"group_id": group_id}
    _add_started(payload, conversations, round_, manager_session_id)
    return payload


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
    prepared: list[GroupSeat] = []
    defaults: dict[str, list[str]] = {}
    for seat in members:
        options, defaults[seat.id] = apply_member(
            document,
            "discussion",
            seat.provider,
            seat.provider_options,
            runtime.EXCLUDED_PROVIDERS,
            restricted=isolated,
            permissions=permissions,
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
            "identity": seat_identity(state, seat.id, current=True),
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
            "defaults_applied": [
                *defaults[seat.id],
                *([READ_ONLY_APPLIED] if seat.access == "read" else []),
            ],
        }
        for seat in state.seats
    ]


async def run_round(
    prompt: Annotated[
        str | None,
        Field(
            min_length=1,
            max_length=12000,
            description=(
                "Shared task; no seats posts a public remark without starting a round; omit only to create."
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
        Field(description=("Ordered seat ids to answer once; empty posts a remark.")),
    ] = [],  # noqa: B006 - pydantic copies defaults
    order: Annotated[
        Literal["parallel", "serial"],
        Field(
            description=(
                "parallel: answers hidden from peers this round; serial: later seats see earlier answers."
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
                "Message_ids to commit before this task; requires seats; see MCP instructions."
            )
        ),
    ] = None,
    title: Annotated[
        str | None,
        Field(
            description=(
                "New meeting title: at most 48 columns (CJK=2); the first also names this workspace."
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
    retry_seats: Annotated[
        list[str] | None,
        Field(
            description=(
                "Failed/blocked seats to restart with the same input in a still-running parallel round; pass only with group_id. After it ends, use a new round."
            )
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    wait: Annotated[bool, Field(description=DISPATCH_WAIT)] = False,
    *,
    ctx: Context[Runtime],
) -> ShowRunPayload:
    """Create/continue a saved meeting; group_id equals conversation_id. Seats keep native
    sessions across rounds/restarts and automatically share committed entries.
    New: title/members required; add prompt/seats to run round 1, omit prompt to only create.
    Existing: omit creation-only arguments. One active round per meeting; wait on round_id.
    entries contains committed message_ids; unreceived flags ends/blockers/reviews for wait.
    At meeting close, post the summary you give the user as a prompt with no seats.
    """
    conversations = _conversations(ctx)
    if retry_seats is not None:
        extras = (prompt, seats, instructions, forward, title, members, cwd, review, isolated)
        if group_id is None or any(extras):
            raise ToolError("retry_seats takes only group_id. Correct the arguments and retry.")
        payload = _retry_seats(conversations, group_id, retry_seats, manager_session_id)
        return await _started(payload, wait, manager_session_id, ctx)
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
        except (MemberFileError, PermissionFileError) as exc:
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
            warnings = model_warnings(conversations.host, entry)
            if warnings:
                payload["warnings"] = warnings
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
    _add_started(payload, conversations, round_, manager_session_id)
    return await _started(payload, wait, manager_session_id, ctx)


async def _started(
    payload: ShowRunPayload, wait_now: bool, manager_session_id: str | None, ctx: Context[Runtime]
) -> ShowRunPayload:
    started = payload.get("round")
    if started is None:
        return payload
    return await waited_dispatch(payload, started["round_id"], wait_now, manager_session_id, ctx)


class ReviewReceipt(TypedDict):
    round_id: str
    decisions: dict[str, Decision]
    pending_review: list[str]
    committed: list[str]
    entries: list[CommittedEntryPayload]
    status: Literal["running", "review", "finished", "failed", "cancelled", "interrupted"]
    error: NotRequired[str]


async def decide_answers(
    round_id: Annotated[str, Field(min_length=1)],
    decisions: Annotated[
        dict[str, Decision],
        Field(min_length=1, description="seat_id -> publish or private"),
    ],
    *,
    ctx: Context[Runtime],
) -> ReviewReceipt:
    """Decide held answers atomically: publish shares with peers; private keeps between boss and
    seat. Repeating a decision is idempotent; changing it is rejected. Parallel rounds commit
    after all decisions; serial rounds then start the next seat. Returns after local commit
    when no work/review remains: finished needs no further call; running/review requires wait.
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


__all__ = [
    "_add_started",
    "_retry_seats",
    "_open_show",
    "run_round",
    "ReviewReceipt",
    "decide_answers",
    "_WORKSPACE_ID",
]
