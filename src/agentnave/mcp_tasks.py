"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Annotated, Any, Literal

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import AfterValidator, Field

from agentnave import mcp_runtime as runtime
from agentnave import provider_health
from agentnave.conversations import (
    Conversations,
    TaskStart,
)
from agentnave.core import InvocationManager
from agentnave.group import Round
from agentnave.isolated import ISOLATED_APPLIED
from agentnave.manager_identity import model_warnings
from agentnave.mcp_isolated import (
    IsolationInput,
    attach_unreleased_count,
    start_isolation_payload,
)
from agentnave.mcp_payloads import (
    CancelPayload,
    InvocationErrorPayload,
    InvocationListItem,
    InvocationListPayload,
    InvocationPayload,
    RoundPayload,
    ShowRunPayload,
    TaskStartPayload,
    WaitPayload,
)
from agentnave.mcp_runtime import (
    ACCESS,
    DISPATCH_WAIT,
    LONG_WAIT_SENTENCE,
    ManagerModel,
    ManagerSessionId,
    Runtime,
    _conversations,
    _manager,
    _review_map,
    _short_title,
    _unknown_handle,
    _unreceived,
    invocation_request_error,
    round_payload,
    started_payload,
    update_payload,
)
from agentnave.members import (
    OrchestrationRole,
    ProviderName,
    apply_seat,
    role_seat,
    validate_provider_options,
)
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
)
from agentnave.native_prompts import NativePrompt
from agentnave.task_start import (
    StartArguments,
    add_write_warning,
    apply_task_defaults,
    resolve_start,
    review_role,
)


async def start_agent(
    prompt: Annotated[
        str,
        Field(
            min_length=1,
            description="Task, context, constraints and expected output; this chat is not inherited.",
        ),
    ],
    provider: Annotated[
        ProviderName | None,
        Field(
            description=(
                "CLI name; omit to inherit session history or the App seat; see MCP instructions for exclusions and native seats."
            )
        ),
    ] = None,
    cwd: Annotated[
        str | None,
        Field(
            min_length=1,
            description=(
                "Existing absolute directory; required for new tasks, inherited on resume when omitted."
            ),
        ),
    ] = None,
    session_id: Annotated[
        str | None,
        Field(
            min_length=1,
            description=(
                "Native session_id from a finished task, never invocation_id; omit for a new thread. Resume inherits provider/cwd/role/seat and, without a seat model, the thread's last model/effort; explicit values win. Unknown or ambiguous history needs provider and cwd; a new role without seat drops the old seat."
            ),
        ),
    ] = None,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                "CLI options, including model/effort; keys via list_members(provider=...); "
                "defaults in MCP instructions. With orchestration on and no matching seat/role "
                "supplying a model, pass an explicit model listed for that CLI by list_members; "
                "a * row allows omitting model or passing a concrete unlisted model, never the literal *."
            )
        ),
    ] = None,
    conversation_id: Annotated[
        str | None, Field(description="Private task conversation to append to.")
    ] = None,
    title: Annotated[
        str | None,
        Field(
            description=(
                "New title: at most 48 columns (CJK=2). The first permanently names this workspace, so "
                "state the chat's purpose; later tasks may omit it."
            )
        ),
        AfterValidator(_short_title),
    ] = None,
    role: Annotated[
        str | None,
        Field(
            description=(
                "Desk label: 1–40 trimmed characters, no controls/newlines; blank clears. Review roles select CLI if omitted and App defaults if CLI matches; explicit options win."
            )
        ),
    ] = None,
    seat: Annotated[
        OrchestrationRole | None,
        Field(
            description=(
                "App task seat from the visible list_members catalog: selects CLI if omitted, defaults if CLI matches and orchestration enabled; explicit options win; inherits on resume. Disabled ordinary seats reject even with an explicit CLI. Not shown; also pass role as the desk label."
            )
        ),
    ] = None,
    forward: Annotated[
        list[str] | None,
        Field(description=("Message_ids to inject before this task; see MCP instructions.")),
    ] = None,
    access: Annotated[
        Literal["read", "write"] | None,
        Field(description=ACCESS),
    ] = None,
    isolation: Annotated[
        IsolationInput | None,
        Field(
            description="New isolated Git worktree (optional ref); omit on session_id resume.",
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    wait: Annotated[bool, Field(description=DISPATCH_WAIT)] = False,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start a private CLI task; invocation_id goes to wait, not session_id.
    No deadline; only native prompts accept running replies via answer_prompt. Cancel to stop. Reply settings are requested,
    not effective; unreceived flags undelivered ends/blockers/reviews for wait.
    """
    if isolation is not None and session_id is not None:
        raise ToolError(
            "isolation cannot be combined with session_id; omit isolation when resuming a thread."
        )
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
        arguments = resolve_start(
            StartArguments(provider, cwd, role, seat, access),
            _conversations(ctx),
            runtime.HOST,
            session_id,
            manager_session_id,
        )
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc
    provider, cwd = arguments.provider, arguments.cwd
    assert provider is not None and cwd is not None
    role, seat, access = arguments.role, arguments.seat, arguments.access
    seat_name = seat or review_role(role)
    try:
        options, seat_applied = apply_seat(
            role_seat(seat_name, runtime.HOST), provider, provider_options or {}
        )
        options = arguments.apply_history(options)
        options, defaults_applied = apply_task_defaults(
            provider, seat_name, options, seat_applied, runtime.EXCLUDED_PROVIDERS, runtime.HOST
        )
        provider_health.check(
            _conversations(ctx).root, [(provider, provider_health.model_option(options))]
        )
        request = InvocationRequest(
            provider=provider,
            prompt=prompt,
            cwd=Path(cwd),
            session_id=session_id,
            provider_options=options,
            read_only=access == "read",
        )
        validate_provider_options(request)
        started = _conversations(ctx).start_task(
            request,
            title,
            conversation_id,
            manager_model=manager_model,
            manager_session_id=manager_session_id,
            role=role,
            seat=seat,
            forward=forward,
            access=access,
            isolation_requested=isolation is not None,
            isolation_ref=isolation.ref if isolation is not None else None,
        )
    except KeyError as exc:
        raise ToolError("Unknown conversation_id; use list.") from exc
    except ValueError as exc:
        error = invocation_request_error(provider, exc)
        if seat_applied:
            raise ToolError(
                f"{error} App defaults applied: {(seat_name or '').strip()!r} seat "
                f"({', '.join(seat_applied)})."
            ) from exc
        raise error from exc
    except OSError as exc:
        raise ToolError(f"Unable to prepare or save the task: {exc}.") from exc
    payload = _task_start_payload(
        ctx, manager, request, started, access, [*defaults_applied, *arguments.defaults]
    )
    return await waited_dispatch(payload, started.invocation_id, wait, manager_session_id, ctx)


def _task_start_payload(
    ctx: Context[Runtime],
    manager: InvocationManager,
    request: InvocationRequest,
    started: TaskStart,
    access: Literal["read", "write"] | None,
    defaults: list[str],
) -> TaskStartPayload:
    conversations = _conversations(ctx)
    if started.isolation is not None:
        defaults = [*defaults, ISOLATED_APPLIED]
    payload: TaskStartPayload = {
        **started_payload(started.invocation_id, request, defaults),
        "conversation_id": started.conversation_id,
        "access": started.access,
    }
    if started.isolation is not None:
        payload["isolation"] = start_isolation_payload(started.isolation)
    if identity := conversations.task_identity(started.invocation_id):
        payload["identity"] = identity
    record_state = conversations.records[started.conversation_id].state
    warnings = model_warnings(
        conversations.host,
        record_state.tasks[-1],
        record_state.manager_session,
        conversations.model_warned,
    )
    if started.isolation is not None:
        warnings.extend(started.isolation.warnings)
    exec_request = (
        replace(request, cwd=Path(started.isolation.cwd))
        if started.isolation is not None
        else request
    )
    add_write_warning(
        warnings,
        manager,
        exec_request,
        started.invocation_id,
        access,
        isolation=started.isolation,
        own_lease_paths=conversations.active_lease_paths,
    )
    attach_unreleased_count(payload, warnings, conversations, record_state.manager_session)
    if warnings:
        payload["warnings"] = warnings
    return payload


async def waited_dispatch[P: (TaskStartPayload, ShowRunPayload)](
    payload: P,
    handle: str,
    wait_now: bool,
    manager_session_id: str | None,
    ctx: Context[Runtime],
) -> P:
    """Point a dispatch at wait, or run that wait now when asked (ADR-0064)."""
    conversations = _conversations(ctx)
    if not wait_now:
        if unreceived := _unreceived(conversations, manager_session_id):
            payload["unreceived"] = unreceived
        payload["next"] = runtime.WAIT_NEXT
        return payload
    waited = await receive(
        ids=[handle],
        wait_seconds=runtime.WAIT_SECONDS,
        manager_session_id=manager_session_id,
        ctx=ctx,
    )
    payload["wait"] = waited
    if unreceived := _unreceived(conversations, manager_session_id):
        payload["unreceived"] = unreceived
    if not waited["all_finished"] or any(conversations.inbox(manager_session_id)):
        payload["next"] = runtime.WAIT_AGAIN
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
) -> tuple[
    dict[str, InvocationResult | InvocationError | NativePrompt],
    dict[str, tuple[Round, str | None]],
]:
    """Wait until any handle is ready, then collect every handle ready at that moment."""

    def ready(found: tuple[Round, str | None]) -> bool:
        return found[0].finished or found[1] is not None

    invocations: (
        asyncio.Task[dict[str, InvocationResult | InvocationError | NativePrompt]] | None
    ) = None
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
    ready_invocations: dict[str, InvocationResult | InvocationError | NativePrompt] = {}
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


async def wait(
    ids: Annotated[
        list[str] | None,
        Field(
            description=(
                "Distinct invocation_ids/round_ids; omit for this boss chat's inbox (requires explicit or host-injected chat identity)."
            ),
        ),
    ] = None,
    include_output: Annotated[
        bool,
        Field(
            description="false omits bodies and tails but still receives ready results, including answers under review."
        ),
    ] = True,
    wait_seconds: Annotated[
        Literal[0, 270, 1500] | None,
        Field(
            description=(
                "Omit to wait up to 270s. "
                + LONG_WAIT_SENTENCE
                + "0 checks progress immediately and includes running tails; ready results are still received; no value stops work."
            ),
        ),
    ] = None,
    manager_session_id: Annotated[
        str | None,
        Field(
            description=(
                "Boss chat identity for the inbox; required unless host-injected. Ignored with ids; see MCP identity instructions."
            ),
        ),
    ] = None,
    seq: Annotated[
        int | None,
        Field(
            ge=0,
            description=(
                "Optional call sequence: start at 1 and increment on each wait so legitimate repeated waits have distinct arguments. "
                "Echoed in the reply; no deduplication, replay or ordering enforcement."
            ),
        ),
    ] = None,
    *,
    ctx: Context[Runtime],
) -> WaitPayload:
    """Receive all handles ready now in updates (tasks) and rounds; return when any is ready.

    Inbox: omit ids, pass this chat's manager_session_id; increment seq for each call.
    Selected ids: continue with pending_ids; retaining finished ids rereads their results.
    all_finished means no unreceived ends remain in that scope, not that all succeeded.

    ready: handle each item's reason first. prompt_pending requires answer_prompt, once per
    prompt, still running. execution_blocked is still running, once per category.
    review_needed requires decide_answers before waiting again. seat_lost reports a failed/blocked
    seat while peers run, once per seat, answers withheld; retry via run_round.retry_seats.
    wait_elapsed: status/activity only; wait again. nothing_pending: the inbox is empty.

    wait_seconds=0 and include_output=false still receive ready items. Only 0 includes
    running tails (up to 1000 characters) and age, subject to include_output.
    Inbox ends and round answers arrive once; output_returned means use read_conversation.
    Unknown/expired/duplicate ids reject the whole call. Ends are never pushed: wait after
    dispatch or independent work; receive every terminal result before ending your turn.
    """
    if wait_seconds == 1500 and not runtime.long_wait_enabled():
        raise ToolError(
            "wait_seconds=1500 is only available in the Claude Code terminal interactive "
            "session; use 0 or 270."
        )
    payload = await _receive(ids, include_output, wait_seconds, manager_session_id, ctx)
    if seq is not None:
        payload["seq"] = seq
        if next_step := payload.get("next"):
            payload["next"] = next_step + f" Next wait: seq={seq + 1}."
    return payload


async def _receive(
    ids: list[str] | None,
    include_output: bool,
    wait_seconds: Literal[0, 270, 1500] | None,
    manager_session_id: str | None,
    ctx: Context[Runtime],
) -> WaitPayload:
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
            return {
                "reason": "nothing_pending",
                "pending_ids": [],
                "all_finished": True,
                "updates": [],
                "rounds": [],
            }
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
    if wait_seconds is None:
        # A long-wait session already decided this call runs in the background (ADR-0084).
        window = runtime.LONG_WAIT_SECONDS if runtime.long_wait_enabled() else runtime.WAIT_SECONDS
    else:
        window = wait_seconds
    deadline = time.monotonic() + window
    while True:
        ready_invocations, ready_rounds = await _ready_handles(
            manager,
            conversations,
            invocation_ids,
            round_ids,
            min(float(window), max(0.0, deadline - time.monotonic())),
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
            return {
                "reason": "nothing_pending",
                "pending_ids": [],
                "all_finished": True,
                "updates": [],
                "rounds": [],
            }
    return _wait_payload(
        manager,
        conversations,
        _WaitResults(invocation_ids, round_ids, ready_invocations, ready_rounds),
        ids,
        include_output,
        wait_seconds,
        manager_session_id,
    )


receive = wait


@dataclass
class _WaitResults:
    invocation_ids: list[str]
    round_ids: list[str]
    invocations: dict[str, InvocationResult | InvocationError | NativePrompt]
    rounds: dict[str, tuple[Round, str | None]]


def _wait_payload(
    manager: InvocationManager,
    conversations: Conversations,
    results: _WaitResults,
    ids: list[str] | None,
    include_output: bool,
    wait_seconds: Literal[0, 270, 1500] | None,
    manager_session_id: str | None,
) -> WaitPayload:
    invocation_ids, round_ids = results.invocation_ids, results.round_ids
    ready_invocations, ready_rounds = results.invocations, results.rounds
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
        reviews = _review_map(conversations, round_)
        if reason == "seat_lost" and "pending" in reviews.values():
            reason = "review_needed"  # the round reached review after the lost-seat wake
        if round_.finished and not elapsed:
            # One ending just after the window closed is delivered by the next wait as ready.
            conversations.delivered_rounds.add(handle)
        rounds.append(
            round_payload(
                round_,
                manager,
                reason or "wait_elapsed",
                include_output=include_output,
                include_running_output=include_running_output and reason != "seat_lost",
                # A round that ended right after seat_lost woke this wait is delivered in full.
                deliver=not elapsed and (round_.finished or reason != "seat_lost"),
                reviews=reviews,
                identities=conversations.round_identities(round_),
                entries=conversations.committed_entries(round_.group.id, round_.number),
            )
        )
    if not include_output:
        for update in updates:
            update.pop("output", None)
            update.pop("output_age_ms", None)
    if ids is None:
        pending_tasks, pending_rounds = conversations.inbox(manager_session_id)
        pending_ids = [*pending_tasks, *pending_rounds]
    else:
        delivered = conversations.delivered_tasks | conversations.delivered_rounds
        pending_ids = [handle for handle in ids if handle not in delivered]
    payload: WaitPayload = {
        "reason": "wait_elapsed" if elapsed else "ready",
        "pending_ids": pending_ids,
        "all_finished": not pending_ids,
        "updates": updates,
        "rounds": rounds,
    }
    if pending_ids:
        payload["next"] = runtime.WAIT_AGAIN
    return payload


async def cancel(
    id: Annotated[  # noqa: A002 - the tool's public argument name
        str,
        Field(min_length=1, description="An invocation_id from start_agent or a round_id."),
    ],
    *,
    ctx: Context[Runtime],
) -> CancelPayload:
    """Stop a task or round; return its cancelled or already-finished result.
    Running seats become absent, unstarted seats skip, published answers stay and undecided
    answers stay private. CLI side effects are not undone.
    """
    manager = _manager(ctx)
    if _known_invocation(manager, id):
        result = await manager.cancel(id)
        _conversations(ctx).delivered_tasks.add(id)
        return {"invocation": update_payload(_manager(ctx), _conversations(ctx), id, result)}
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
            identities=conversations.round_identities(round_),
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


async def list_invocations(*, ctx: Context[Runtime]) -> InvocationListPayload:
    """Unlisted: the relay asks this before swapping servers; agents use list."""
    conversations = _conversations(ctx)
    return {
        "invocations": _invocation_items(_manager(ctx)),
        "restart_safe": (
            conversations.restart_safe()
            and conversations.isolated.restart_safe()
            and ctx.request_context.lifespan_context.retention.restart_safe()
        ),
    }


__all__ = [
    "start_agent",
    "_known_round",
    "_known_invocation",
    "_ready_handles",
    "wait",
    "cancel",
    "_invocation_items",
    "list_invocations",
]
