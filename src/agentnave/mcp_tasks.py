"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Annotated, Any, Literal, cast

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import AfterValidator, Field, ValidatorFunctionWrapHandler, WrapValidator

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
    refresh_unreleased_count,
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
from agentnave.mcp_recommend import choose_auto_seat, recommendation_payload
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
    orchestration_decision,
    role_seat,
    validate_provider_options,
)
from agentnave.models import (
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
    validate_invocation_inputs,
)
from agentnave.native_prompts import NativePrompt
from agentnave.orchestration_roles import AUTO_SEAT, ReviewSeat
from agentnave.seat_fallbacks import SeatFallback
from agentnave.seat_recommendation import Recommendation
from agentnave.task_start import (
    NativeSeat,
    StartArguments,
    add_write_warning,
    apply_task_defaults,
    is_image_seat,
    native_seat_guidance,
    resolve_start,
    review_role,
)


def _own_seat(value: Any, handler: ValidatorFunctionWrapHandler) -> Any:
    """While the user's own seats are effective, any identifier reaches the seat checks of
    start_agent, which name unknown ones; otherwise the published enum decides alone.

    The published schema is untouched: list_tools lists the effective seats in its enum.
    """
    if isinstance(value, str) and orchestration_decision(runtime.HOST):
        if not 1 <= len(value) <= 128:
            raise ValueError("seat must be a seat identifier from list_members")
        return value
    return handler(value)


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
        WrapValidator(_own_seat),
        Field(
            description=(
                "App task seat from the visible list_members catalog: selects CLI if omitted, ordered available choices for new threads; explicit CLI or model wins; inherits on resume. Disabled ordinary seats reject even with an explicit CLI. Not shown; also pass role as the desk label."
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
            description=(
                "New isolated Git worktree (optional ref); omit on session_id resume "
                "(read-only tasks that changed nothing auto-release it; resume rebuilds it in place)."
            ),
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    wait: Annotated[bool, Field(description=DISPATCH_WAIT)] = False,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start a private CLI task; invocation_id goes to wait, not session_id.
    A seat native to this host starts nothing and returns status not_started with reason
    native_seat, the seat, its provider/model/effort and a message: run it with the host's
    built-in subagent instead, so no wait is needed.
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
    # Any identifier may arrive while the catalog is effective, see _own_seat.
    requested = cast(str | None, seat)
    recommendation: Recommendation | None = None
    if requested == AUTO_SEAT:
        recommendation = await choose_auto_seat(prompt, access, provider, cwd, session_id)
        requested = recommendation.seat
    payload = await launch_task(
        ctx,
        TaskLaunch(
            prompt=prompt,
            provider=provider,
            cwd=cwd,
            session_id=session_id,
            provider_options=provider_options,
            conversation_id=conversation_id,
            title=title,
            role=role,
            seat=requested,
            forward=forward,
            access=access,
            isolation=isolation,
            manager_model=manager_model,
            manager_session_id=manager_session_id,
        ),
    )
    if recommendation is not None:
        payload["recommendation"] = recommendation_payload(recommendation)
    invocation_id = payload.get("invocation_id")
    if invocation_id is None:
        # A seat native to this host: nothing started, so there is nothing to wait for.
        return payload
    result = await waited_dispatch(payload, invocation_id, wait, manager_session_id, ctx)
    if wait:
        await refresh_unreleased_count(result, _conversations(ctx), manager_session_id)
    return result


@dataclass
class TaskLaunch:
    """What starts one new or resumed private task, as start_agent and start_agents give it."""

    prompt: str
    provider: ProviderName | None
    cwd: str | None
    session_id: str | None
    provider_options: dict[str, ProviderOption] | None
    conversation_id: str | None
    title: str | None
    role: str | None
    seat: str | None  # the seat to apply, already chosen (never "auto")
    forward: list[str] | None
    access: Literal["read", "write"] | None
    isolation: IsolationInput | None
    manager_model: str | None
    manager_session_id: str | None
    isolation_auto: bool = False  # isolation the service added itself, not the boss's request


# defaults_applied entry of a task whose isolated directory the service asked for.
AUTO_ISOLATION_APPLIED = "isolation (automatic: several write tasks)"


def native_seat_payload(
    seat: str,
    provider: str | None,
    model: str | None,
    effort: str | None,
    *,
    recommendation: Recommendation | None = None,
    fallback: SeatFallback | None = None,
    error: str | None = None,
) -> TaskStartPayload:
    """Nothing started: the seat is this host's own CLI, so the boss runs it in its subagent."""
    payload: TaskStartPayload = {
        "status": "not_started",
        "reason": "native_seat",
        "seat": seat,
        "message": native_seat_guidance(model, effort),
        "next": runtime.NATIVE_SEAT_NEXT,
    }
    if provider is not None:
        payload["provider"] = provider
    if model:
        payload["model"] = model
    if effort:
        payload["effort"] = effort
    if recommendation is not None:
        payload["recommendation"] = recommendation_payload(recommendation)
    if fallback is not None:
        payload["seat_fallback"] = fallback
    if error is not None:
        payload["error"] = {"code": "native_seat", "message": error}
    return payload


def _select_fallback(
    seat_name: str | None,
    session_id: str | None,
    provider: ProviderName | None,
    provider_options: dict[str, ProviderOption] | None,
) -> tuple[ReviewSeat | None, SeatFallback | None]:
    if session_id is not None or provider is not None or "model" in (provider_options or {}):
        return None, None
    configured = role_seat(seat_name, runtime.HOST)
    if not configured or not configured.fallbacks:
        return None, None
    from agentnave.decision_seats import custom_seat
    from agentnave.seat_fallbacks import select

    custom = custom_seat(seat_name or "", runtime.HOST)
    return select(
        configured,
        runtime.HOST,
        runtime.task_excluded_providers(),
        image=is_image_seat(seat_name, custom),
    )


async def _acquire_and_start_task(
    conversations: Conversations,
    launch: TaskLaunch,
    arguments: StartArguments,
    options: dict[str, Any],
) -> tuple[InvocationRequest, TaskStart]:
    provider, cwd = arguments.provider, arguments.cwd
    assert provider is not None and cwd is not None
    access = arguments.access
    isolation = launch.isolation
    manager_session_id = launch.manager_session_id
    resumed_acquired = None
    hold = None
    if not Path(cwd).is_dir() and arguments.resumed_task is not None:
        validate_invocation_inputs(
            provider, launch.prompt, Path(cwd), launch.session_id, require_cwd_dir=False
        )
        resumed_acquired = await conversations.acquire_resumed_isolation(
            Path(cwd), arguments.resumed_task, manager_session_id
        )
    try:
        request = InvocationRequest(
            provider=provider,
            prompt=launch.prompt,
            cwd=Path(cwd),
            session_id=launch.session_id,
            provider_options=options,
            read_only=access == "read",
        )
        validate_provider_options(request)
        if resumed_acquired is not None:
            assert arguments.resumed_task is not None
            hold = conversations.bind_resumed_isolation(
                request, resumed_acquired, arguments.resumed_task
            )
            resumed_acquired = None
        else:
            try:
                hold = await conversations.acquire_task_isolation(
                    request,
                    manager_session_id,
                    isolation_requested=isolation is not None,
                    isolation_ref=isolation.ref if isolation is not None else None,
                    resumed_task=arguments.resumed_task,
                )
            except (ValueError, OSError) as exc:
                if not launch.isolation_auto:
                    raise
                # Never fall back to an unisolated start: the directory was meant to be separate.
                raise ToolError(
                    f"automatic isolation failed (several write tasks in one call): {exc}"
                ) from exc
        started = conversations.start_task(
            request,
            launch.title,
            launch.conversation_id,
            manager_model=launch.manager_model,
            manager_session_id=manager_session_id,
            role=arguments.role,
            seat=arguments.seat,
            forward=launch.forward,
            access=access,
            isolation_requested=isolation is not None,
            isolation_ref=isolation.ref if isolation is not None else None,
            hold=hold,
        )
    except BaseException:
        if hold is not None:
            await conversations.give_back_isolation(hold)
        elif resumed_acquired is not None:
            await conversations.give_back_resumed(resumed_acquired)
        raise
    return request, started


async def launch_task(ctx: Context[Runtime], launch: TaskLaunch) -> TaskStartPayload:
    """Resolve the seat and defaults, acquire isolation, start the task and describe it.

    The shared start path of start_agent and start_agents; failures are ToolErrors. A seat
    native to this host starts nothing and comes back as a not_started receipt before any
    isolated directory is made."""
    manager = _manager(ctx)
    provider, cwd, session_id = launch.provider, launch.cwd, launch.session_id
    provider_options = launch.provider_options
    role, requested, access = launch.role, launch.seat, launch.access
    manager_session_id = launch.manager_session_id
    seat_applied: list[str] = []
    fallback = None
    try:
        arguments = StartArguments(provider, cwd, role, requested, access)
        arguments.selected, fallback = _select_fallback(
            requested or review_role(role), session_id, provider, provider_options
        )
        arguments = resolve_start(
            arguments,
            _conversations(ctx),
            runtime.HOST,
            session_id,
            manager_session_id,
            provider_options,
        )
    except NativeSeat as exc:
        return native_seat_payload(
            exc.name, exc.seat.provider, exc.seat.model, exc.seat.effort, fallback=fallback
        )
    except (ValueError, OSError) as exc:
        raise ToolError(str(exc)) from exc
    provider = arguments.provider
    assert provider is not None
    role, chosen_seat, access = arguments.role, arguments.seat, arguments.access
    seat_name = chosen_seat or review_role(role)
    try:
        options, seat_applied = apply_seat(
            arguments.selected or role_seat(seat_name, runtime.HOST),
            provider,
            provider_options or {},
        )
        options = arguments.apply_history(options)
        options, defaults_applied = apply_task_defaults(
            provider,
            seat_name,
            options,
            seat_applied,
            runtime.task_excluded_providers(),
            runtime.HOST,
            arguments.selected,
        )
        provider_health.check(
            _conversations(ctx).root, [(provider, provider_health.model_option(options))]
        )
        request, started = await _acquire_and_start_task(
            _conversations(ctx), launch, arguments, options
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
    payload = await _task_start_payload(
        ctx, manager, request, started, access, [*defaults_applied, *arguments.defaults]
    )
    if fallback is not None:
        payload["seat_fallback"] = fallback
    if launch.isolation_auto:
        payload["defaults_applied"] = [*payload.get("defaults_applied", []), AUTO_ISOLATION_APPLIED]
    return payload


async def _task_start_payload(
    ctx: Context[Runtime],
    manager: InvocationManager,
    request: InvocationRequest,
    started: TaskStart,
    access: Literal["read", "write"] | None,
    defaults: list[str],
) -> TaskStartPayload:
    conversations = _conversations(ctx)
    if started.isolation is not None and started.isolation.instructions_applied:
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
    warnings.extend(started.warnings)
    if started.isolation is not None:
        warnings.extend(started.isolation.warnings)
        if not started.isolation.instructions_applied:
            warnings.append(
                "Isolated directory instructions were not added: a path contains control characters."
            )
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
        # A snapshot: worker threads add and drop leases while this reads.
        own_lease_paths=tuple(conversations.active_lease_paths),
    )
    await attach_unreleased_count(payload, warnings, conversations, record_state.manager_session)
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
