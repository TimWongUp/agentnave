"""Agent-only STDIO MCP server for local CLI subagents."""

from __future__ import annotations

import asyncio
import os
import time
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Annotated, Any, Literal, cast

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import AfterValidator, Field, ValidatorFunctionWrapHandler, WrapValidator

from agentnave import accounts, provider_health
from agentnave import mcp_runtime as runtime
from agentnave.conversations import (
    Conversations,
    TaskStart,
)
from agentnave.core import InvocationManager
from agentnave.group import Round
from agentnave.handover import handover_dispatch_warning
from agentnave.inbox_events import review_event_key
from agentnave.isolated import ISOLATED_APPLIED
from agentnave.manager_identity import model_warnings
from agentnave.mcp_followup import settle_follow_up
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
    _active_inbox_waits,
    _active_waits,
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
    PERMISSION_KEYS,
    OrchestrationRole,
    ProviderName,
    apply_seat,
    data_root,
    orchestration_decision,
    role_seat,
    validate_provider_options,
)
from agentnave.models import (
    ACCOUNT_ID,
    InvocationError,
    InvocationRequest,
    InvocationResult,
    ProviderOption,
    validate_invocation_inputs,
)
from agentnave.native_prompts import NativePrompt
from agentnave.orchestration_roles import AUTO_SEAT, ReviewSeat
from agentnave.push_capability import find_tray
from agentnave.seat_fallbacks import SeatFallback
from agentnave.seat_recommendation import Recommendation
from agentnave.task_isolation import check_resumed_cwd_not_redirected
from agentnave.task_start import (
    NativeSeat,
    StartArguments,
    add_write_warning,
    apply_task_defaults,
    effective_seat,
    is_image_seat,
    resolve_start,
    review_role,
    uses_random_candidates,
)
from agentnave.terminal_requests import read_tray


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
            description=(
                "Self-contained brief (task, context, constraints, expected output, paths); "
                "this chat is not inherited."
            ),
        ),
    ],
    provider: Annotated[
        ProviderName | None,
        Field(
            description=(
                "CLI name; omit to inherit session history or the App seat; see MCP instructions for exclusions."
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
                "Native session_id from a finished task in this workspace (never invocation_id); "
                "omit for a new thread. Cross-workspace threads are rejected; start a new thread "
                "and use forward instead. Resume inherits provider/cwd/role/seat and, without a "
                "seat model, the thread's last model/effort, never access (re-pass access). "
                "Unknown or ambiguous history needs provider and cwd; a new role without seat "
                "drops the old seat."
            ),
        ),
    ] = None,
    provider_options: Annotated[
        dict[str, ProviderOption] | None,
        Field(
            description=(
                "CLI options; omit model, effort, and permission keys unless the user named them "
                "(App/seat defaults apply; if rejected, retry with an allowed value from the "
                "error or list_members(provider=...)). With orchestration on and no matching "
                "seat/role supplying a model, pass an explicit model listed by list_members; a * "
                "row allows omitting model or passing a concrete unlisted model, never literal *."
            )
        ),
    ] = None,
    account: Annotated[
        str | None,
        Field(
            pattern=ACCOUNT_ID,
            description=(
                "Named account id of the CLI from list_members(accounts); only claude takes one. "
                "Omit for the default account (the seat's account when a seat chooses one); "
                "a resumed thread keeps its own account."
            ),
        ),
    ] = None,
    conversation_id: Annotated[
        str | None, Field(description="Private task conversation to append to.")
    ] = None,
    title: Annotated[
        str | None,
        Field(
            description=(
                "Workspace title on this chat's first dispatch (<=48 cols, CJK=2); permanently "
                "names this chat, so state its purpose; later tasks may omit it."
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
                "App task seat from the visible list_members catalog: selects CLI if omitted, "
                "ordered available choices for new threads (or a uniform random usable candidate "
                "on custom task seats when enabled; seat_fallback sets random=true when drawn, "
                "else earlier choices were skipped); explicit CLI or model wins; inherits on "
                "resume, where another seat must use the resumed thread's CLI. Disabled ordinary "
                "seats reject even with an explicit CLI. Not shown; "
                "also pass role as the desk label."
            )
        ),
    ] = None,
    forward: Annotated[
        list[str] | None,
        Field(
            description=(
                "Up to 20 unique message_ids of succeeded tasks or committed entries in this "
                "workspace or a linked handover workspace; injected as data."
            )
        ),
    ] = None,
    access: Annotated[
        Literal["read", "write"] | None,
        Field(description=ACCESS),
    ] = None,
    isolation: Annotated[
        IsolationInput | None,
        Field(
            description=(
                "New isolated Git worktree (optional ref) for write tasks or read-then-write "
                "threads; omit on session_id resume. Merge and release with isolated_dirs "
                "(untouched succeeded read tasks auto-release and rebuild on resume)."
            ),
        ),
    ] = None,
    manager_model: ManagerModel = None,
    manager_session_id: ManagerSessionId = None,
    terminal: Annotated[bool | None, Field(description="Run this task in an App terminal.")] = None,
    wait: Annotated[bool, Field(description=DISPATCH_WAIT)] = False,
    *,
    ctx: Context[Runtime],
) -> TaskStartPayload:
    """Start a private CLI task with a self-contained prompt, an existing absolute cwd, and title
    on this chat's first dispatch; invocation_id goes to wait, not session_id.
    Omit model, effort, and permission keys unless the user named them (if rejected, pick an
    allowed value from the error). Declare access="read" for investigation, pass session_id and
    re-pass access when resuming, and pass isolation for write tasks (after verifying, merge and
    release with isolated_dirs). Follow receipt next to collect results; succeeded does not mean
    done—verify artifacts and follow_up. A seat native to this host starts nothing and returns
    status="not_started", reason="native_seat", and subagent instructions in message (no wait).
    No deadline; only native prompts accept running replies via answer_prompt; cancel stops work;
    unreceived flags undelivered items.
    """
    if terminal is not None and not runtime.terminal_available():
        raise ToolError("terminal is not supported by this connection")
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
        if terminal:
            recommendation = await choose_auto_seat(
                prompt,
                access,
                provider,
                cwd,
                session_id,
                isolation is not None,
                excluded=_explicit_exclusions(),
            )
        else:
            recommendation = await choose_auto_seat(
                prompt, access, provider, cwd, session_id, isolation is not None
            )
        requested = recommendation.seat
    payload = await launch_task(
        ctx,
        TaskLaunch(
            prompt=prompt,
            provider=provider,
            cwd=cwd,
            session_id=session_id,
            provider_options=provider_options,
            account=account,
            conversation_id=conversation_id,
            title=title,
            role=role,
            seat=requested,
            forward=forward,
            access=access,
            isolation=isolation,
            manager_model=manager_model,
            manager_session_id=manager_session_id,
            terminal=terminal,
        ),
    )
    if recommendation is not None:
        payload["recommendation"] = recommendation_payload(recommendation, payload)
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
    account: str | None = None  # explicit; the seat's or the resumed thread's otherwise
    terminal: bool | None = None


# defaults_applied entry of a task whose isolated directory the service asked for.
AUTO_ISOLATION_APPLIED = "isolation (automatic: several write tasks)"


def native_seat_payload(
    seat: str,
    provider: str | None,
    model: str | None,
    effort: str | None,
    *,
    account: str | None = None,
    recommendation: Recommendation | None = None,
    fallback: SeatFallback | None = None,
    error: str | None = None,
) -> TaskStartPayload:
    """Nothing started: the seat is this host's own CLI, so the boss runs it in its subagent."""
    payload: TaskStartPayload = {
        "status": "not_started",
        "reason": "native_seat",
        "seat": seat,
        "message": runtime.native_seat_guidance(runtime.HOST, model, effort),
        "next": runtime.NATIVE_SEAT_NEXT,
    }
    if provider is not None:
        payload["provider"] = provider
    if account is not None:
        payload["account"] = account
    if model:
        payload["model"] = model
    if effort:
        payload["effort"] = effort
    if recommendation is not None:
        payload["recommendation"] = recommendation_payload(recommendation, payload)
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
    conversations: Conversations | None = None,
    account: str | None = None,
    terminal: bool = False,
) -> tuple[ReviewSeat | None, SeatFallback | None]:
    if (
        session_id is not None
        or provider is not None
        or account is not None
        or "model" in (provider_options or {})
    ):
        return None, None
    configured = role_seat(seat_name, runtime.HOST)
    if not configured or not configured.fallbacks:
        return None, None
    from agentnave.decision_seats import custom_seat
    from agentnave.seat_fallbacks import select

    custom = custom_seat(seat_name or "", runtime.HOST)
    randomize = uses_random_candidates(seat_name, runtime.HOST)
    in_use = (
        conversations.running_seat_models(seat_name)
        if randomize and conversations is not None and seat_name
        else ()
    )
    return select(
        configured,
        runtime.HOST,
        _explicit_exclusions() if terminal else runtime.task_excluded_providers(),
        image=is_image_seat(seat_name, custom),
        randomize=randomize,
        in_use=in_use,
    )


def _explicit_exclusions() -> frozenset[str]:
    return frozenset(
        value.strip().lower()
        for value in os.environ.get("AGENTNAVE_EXCLUDED_PROVIDERS", "").split(",")
        if value.strip()
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
    manager_workspace = (
        conversations.new_manager_session(manager_session_id)
        if launch.conversation_id is None and launch.session_id is None
        else None
    )
    resumed_acquired = None
    hold = None
    # Before InvocationRequest resolves cwd, which hides a link swapped in below the directory.
    check_resumed_cwd_not_redirected(Path(cwd), arguments.resumed_task)
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
            account=arguments.account,
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
                    manager_workspace=manager_workspace,
                )
            except (ValueError, OSError) as exc:
                if not launch.isolation_auto:
                    raise
                # Never fall back to an unisolated start: the directory was meant to be separate.
                raise ToolError(
                    f"automatic isolation failed (several write tasks in one call): {exc}"
                ) from exc
        tray = None
        if launch.terminal:
            tray = find_tray(data_root(), None)
            snapshot = read_tray(data_root(), tray) if tray else None
            providers = snapshot.get("terminal_providers") if snapshot else None
            if not isinstance(providers, list) or provider not in providers:
                raise ValueError("Tray does not support terminal mode for this provider")
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
            manager_workspace=manager_workspace,
            terminal_tray=tray,
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
    conversations = _conversations(ctx)
    if session_id is not None:
        try:
            previous = conversations.resume_task(session_id, provider, manager_session_id)
        except ValueError as exc:
            raise ToolError(str(exc)) from exc
        if previous is not None and previous.execution_mode == "terminal":
            if launch.terminal is False:
                raise ToolError("Cannot resume an open derived terminal in background mode")
            launch.terminal = True
    if launch.terminal and not runtime.terminal_available():
        raise ToolError("terminal is not supported by this connection")
    claim: tuple[str, str, str | None, str] | None = None
    try:
        try:
            arguments = StartArguments(
                provider,
                cwd,
                role,
                requested,
                access,
                account=launch.account,
                terminal=bool(launch.terminal),
            )
            seat_key = requested or review_role(role)
            arguments.selected, fallback = _select_fallback(
                seat_key,
                session_id,
                provider,
                provider_options,
                conversations,
                launch.account,
                bool(launch.terminal),
            )
            if (
                arguments.selected is not None
                and fallback is not None
                and fallback.get("random") is True
                and seat_key
            ):
                claim = conversations.reserve_seat_model(seat_key, arguments.selected)
            arguments = resolve_start(
                arguments,
                conversations,
                runtime.HOST,
                session_id,
                manager_session_id,
                provider_options,
            )
        except NativeSeat as exc:
            return native_seat_payload(
                exc.name,
                exc.seat.provider,
                exc.seat.model,
                exc.seat.effort,
                account=exc.seat.account,
                fallback=fallback,
            )
        except (ValueError, OSError) as exc:
            raise ToolError(str(exc)) from exc
        provider = arguments.provider
        assert provider is not None
        role, chosen_seat, access = arguments.role, arguments.seat, arguments.access
        seat_name = chosen_seat or review_role(role)
        try:
            seat = effective_seat(arguments, seat_name, runtime.HOST)
            accounts.resolve(provider, arguments.account)
            options, seat_applied = apply_seat(seat, provider, provider_options or {})
            options = arguments.apply_history(options)
            options, defaults_applied = apply_task_defaults(
                provider,
                seat_name,
                options,
                seat_applied,
                _explicit_exclusions() if launch.terminal else runtime.task_excluded_providers(),
                runtime.HOST,
                seat,
            )
            if launch.terminal:
                unsupported = (
                    set(options)
                    - PERMISSION_KEYS.get(provider, frozenset())
                    - {"model", "effort", "interactive"}
                )
                if unsupported:
                    raise ValueError(
                        "terminal mode does not support provider options: "
                        + ", ".join(sorted(unsupported))
                    )
            provider_health.check(
                conversations.root,
                [(provider, arguments.account, provider_health.model_option(options))],
                resume=session_id is not None,
            )
            request, started = await _acquire_and_start_task(
                conversations, launch, arguments, options
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
    finally:
        conversations.release_seat_model(claim)
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
    if started.invocation_id in manager.terminal_runs:
        payload["execution_mode"] = "terminal"
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
    warnings.extend(
        handover_dispatch_warning(
            conversations, record_state.manager_session, current_task=record_state.tasks[-1]
        )
    )
    warnings.extend(started.warnings)
    warnings.extend(
        accounts.dispatch_warnings(conversations.host, [(request.provider, request.account)])
    )
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
    boss_key = conversations.boss_key_of(manager_session_id)
    if not wait_now:
        ctx.request_context.lifespan_context.refresh_inbox()
        if unreceived := _unreceived(conversations, manager_session_id):
            payload["unreceived"] = unreceived
        payload["next"] = runtime.wait_guidance(ctx=ctx, boss_key=boss_key)
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
        payload["next"] = runtime.wait_guidance(again=True, ctx=ctx, boss_key=boss_key)
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
    """Receive ready task updates and meeting rounds, returning when any is ready; follow receipt
    next for this host's wait mode, and remember succeeded/all_finished do not mean done—verify
    artifacts and handle follow_up before reporting.

    Inbox: omit ids, pass this chat's manager_session_id, increment seq per call.
    Selected ids: continue with pending_ids; finished ids reread results.
    follow_up (inbox wait at all_finished): earlier non-succeeded ends and unreleased isolated
    dirs (merge and release via isolated_dirs), each listed once; handle before reporting.

    ready: handle each item's reason first. prompt_pending -> answer_prompt once (still running).
    execution_blocked -> still running, once per category. review_needed -> decide_answers before
    waiting again. seat_lost -> failed/blocked seat while peers run; retry via
    run_round.retry_seats. wait_elapsed: activity only; follow next. nothing_pending: inbox empty.
    already_waiting: another blocking inbox wait is active; collect there (wait_seconds=0 checks
    progress).

    wait_seconds=0 and include_output=false still receive ready items; only 0 includes running
    tails (<=1000 chars) and age. Inbox ends and round answers arrive once; output_returned means
    use read_conversation. Unknown/expired/duplicate ids reject the whole call.
    """
    if wait_seconds == 1500 and not runtime.long_wait_enabled():
        raise ToolError(
            "wait_seconds=1500 is only available in the Claude Code terminal interactive "
            "session with background tasks enabled; use 0 or 270."
        )
    payload = await _receive(ids, include_output, wait_seconds, manager_session_id, ctx)
    await settle_follow_up(
        _conversations(ctx), payload, inbox=ids is None, manager_session_id=manager_session_id
    )
    ctx.request_context.lifespan_context.refresh_inbox()
    if seq is not None:
        payload["seq"] = seq
        # Only blocking guidance asks for another wait; push, watch and already-waiting do not.
        next_step = payload.get("next")
        if next_step in runtime.BLOCKING_WAIT_AGAIN:
            payload["next"] = f"{next_step} Next wait: seq={seq + 1}."
    return payload


def _already_waiting_payload(pending_ids: list[str]) -> WaitPayload:
    return {
        "reason": "already_waiting",
        "pending_ids": pending_ids,
        "all_finished": False,
        "updates": [],
        "rounds": [],
        "next": runtime.ALREADY_WAITING,
    }


def _nothing_pending_payload() -> WaitPayload:
    return {
        "reason": "nothing_pending",
        "pending_ids": [],
        "all_finished": True,
        "updates": [],
        "rounds": [],
    }


def _explicit_handles(
    manager: InvocationManager, conversations: Conversations, ids: list[str]
) -> tuple[list[str], list[str]]:
    """Split explicit wait ids into task invocations and rounds, rejecting bad lists."""
    if not ids:
        raise ToolError("ids must list at least one handle, or be omitted for the inbox.")
    if len(set(ids)) != len(ids):
        raise ToolError("ids must not repeat. Correct the list and retry.")
    invocation_ids: list[str] = []
    round_ids: list[str] = []
    for handle in ids:
        if _known_invocation(manager, handle):
            invocation_ids.append(handle)
            continue
        try:
            conversations.round(handle)
        except KeyError as exc:
            raise _unknown_handle(handle) from exc
        round_ids.append(handle)
    return invocation_ids, round_ids


def _explicit_wait_workspaces(
    conversations: Conversations,
    invocation_ids: list[str],
    round_ids: list[str],
    manager_session_id: str | None,
) -> set[str | None]:
    try:
        caller = conversations.inbox_key(manager_session_id)
    except ValueError:
        caller = None
    if caller is not None:
        return {caller}
    workspaces: set[str | None] = set()
    by_invocation = conversations.invocation_workspaces()
    for handle in invocation_ids:
        if handle in by_invocation:
            workspaces.add(by_invocation[handle])
    for handle in round_ids:
        try:
            round_ = conversations.round(handle)
        except KeyError:
            continue
        record = conversations.records.get(round_.group.id)
        if record is not None:
            workspaces.add(record.state.manager_session)
    return workspaces


@contextmanager
def _track_blocking_wait(
    ctx: Context[Runtime],
    *,
    blocking_inbox: bool,
    inbox_key: str | None,
    explicit_wait_keys: set[str | None],
) -> Generator[None]:
    active_inbox_waits = _active_inbox_waits(ctx)
    active_waits = _active_waits(ctx)
    if blocking_inbox:
        active_inbox_waits.add(inbox_key)
        ctx.request_context.lifespan_context.refresh_inbox()
    elif explicit_wait_keys:
        for workspace in explicit_wait_keys:
            active_waits[workspace] += 1
        ctx.request_context.lifespan_context.refresh_inbox()
    try:
        yield
    finally:
        if blocking_inbox:
            active_inbox_waits.discard(inbox_key)
        for workspace in explicit_wait_keys:
            active_waits[workspace] -= 1
            if active_waits[workspace] <= 0:
                del active_waits[workspace]
        ctx.request_context.lifespan_context.refresh_inbox()


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
    inbox_key: str | None = None
    active_inbox_waits = _active_inbox_waits(ctx)
    blocking_inbox = ids is None and wait_seconds != 0
    explicit_wait_keys: set[str | None] = set()
    if ids is None:
        try:
            inbox_key = conversations.inbox_key(manager_session_id)
            invocation_ids, round_ids = conversations.inbox(manager_session_id)
        except ValueError as exc:
            raise ToolError(f"{exc}. Correct and retry.") from exc
        if blocking_inbox and inbox_key in active_inbox_waits:
            return _already_waiting_payload([*invocation_ids, *round_ids])
        if not invocation_ids and not round_ids:
            return _nothing_pending_payload()
    else:
        invocation_ids, round_ids = _explicit_handles(manager, conversations, ids)
        if wait_seconds != 0:
            explicit_wait_keys = _explicit_wait_workspaces(
                conversations, invocation_ids, round_ids, manager_session_id
            )
    if wait_seconds is None:
        # A long-wait session already decided this call runs in the background (ADR-0084).
        window = runtime.LONG_WAIT_SECONDS if runtime.long_wait_enabled() else runtime.WAIT_SECONDS
    else:
        window = wait_seconds
    deadline = time.monotonic() + window
    with _track_blocking_wait(
        ctx,
        blocking_inbox=blocking_inbox,
        inbox_key=inbox_key,
        explicit_wait_keys=explicit_wait_keys,
    ):
        while True:
            timeout = min(float(window), max(0.0, deadline - time.monotonic()))
            if blocking_inbox:
                # Re-check the inbox periodically so tasks dispatched mid-wait are received here.
                timeout = min(timeout, float(runtime.INBOX_WAIT_SLICE_SECONDS))
            ready_invocations, ready_rounds = await _ready_handles(
                manager,
                conversations,
                invocation_ids,
                round_ids,
                timeout,
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
                return _nothing_pending_payload()
        wait_results = _WaitResults(invocation_ids, round_ids, ready_invocations, ready_rounds)
        payload, delivered_reviews = _wait_payload(
            manager,
            conversations,
            wait_results,
            ids,
            include_output,
            wait_seconds,
            manager_session_id,
            ctx,
        )
        if payload["reason"] == "ready":
            bosses = _delivered_bosses(conversations, manager_session_id, wait_results)
            if bosses:
                ctx.request_context.lifespan_context.mark_inbox_delivered(bosses, delivered_reviews)
        if (
            ids is None
            and wait_seconds == 0
            and "next" in payload
            and inbox_key in active_inbox_waits
        ):
            payload["next"] = runtime.ALREADY_WAITING
        return payload


receive = wait


def _handle_boss_key(conversations: Conversations, handle: str) -> str | None:
    owners = conversations._bosses  # pyright: ignore[reportPrivateUsage]
    workspace = conversations.invocation_workspaces().get(handle)
    if workspace is None:
        try:
            round_ = conversations.round(handle)
        except KeyError:
            return None
        record = conversations.records.get(round_.group.id)
        workspace = record.state.manager_session if record is not None else None
    if workspace is None:
        return None
    owner = owners.get(workspace)
    return owner[0] if owner is not None else None


def _wait_boss_key(
    conversations: Conversations, manager_session_id: str | None, handles: list[str]
) -> str | None:
    if boss := conversations.boss_key_of(manager_session_id):
        return boss
    for handle in handles:
        if boss := _handle_boss_key(conversations, handle):
            return boss
    return None


def _delivered_bosses(
    conversations: Conversations,
    manager_session_id: str | None,
    results: _WaitResults,
) -> set[str]:
    bosses: set[str] = set()
    if boss := conversations.boss_key_of(manager_session_id):
        bosses.add(boss)
    for handle in (*results.invocations, *results.rounds):
        if boss := _handle_boss_key(conversations, handle):
            bosses.add(boss)
    return bosses


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
    ctx: Context[Runtime],
) -> tuple[WaitPayload, set[str]]:
    invocation_ids, round_ids = results.invocation_ids, results.round_ids
    ready_invocations, ready_rounds = results.invocations, results.rounds
    elapsed = not ready_invocations and not ready_rounds
    include_running_output = include_output and wait_seconds == 0
    delivered_reviews: set[str] = set()
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
        elif not elapsed and reason == "review_needed":
            for run in round_.runs:
                if reviews.get(run.seat_id) == "pending":
                    delivered_reviews.add(review_event_key(handle, run.seat_id, run.attempt))
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
        payload["next"] = runtime.wait_guidance(
            again=True,
            ctx=ctx,
            boss_key=_wait_boss_key(conversations, manager_session_id, pending_ids),
        )
    return payload, delivered_reviews


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
    conversations = _conversations(ctx)
    state = ctx.request_context.lifespan_context
    boss = _handle_boss_key(conversations, id)
    delivered = False
    state.cancelling.add(id)
    state.refresh_inbox()
    try:
        if _known_invocation(manager, id):
            try:
                result = await manager.cancel(id)
            except ValueError as exc:
                raise ToolError(str(exc)) from exc
            conversations.delivered_tasks.add(id)
            delivered = True
            return {"invocation": update_payload(manager, conversations, id, result)}
        try:
            round_ = await conversations.cancel(id)
        except KeyError as exc:
            raise _unknown_handle(id) from exc
        conversations.delivered_rounds.add(id)
        delivered = True
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
    finally:
        state.cancelling.discard(id)
        if delivered and boss is not None:
            state.mark_inbox_delivered({boss})
        else:
            state.refresh_inbox()


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
