"""Saved conversations, reviewable shows, and tray view snapshots."""

from __future__ import annotations

import asyncio
import logging
import shutil
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path
from typing import IO
from uuid import uuid4

from agentnave import provider_health
from agentnave.adapters import get_adapter
from agentnave.adapters.base import PreparedCommand
from agentnave.conversations_store import (
    _CONNECTION_ID,
    _LIVE_SECONDS,
    _MANAGER_STARTED_AT,
    _MAX_FORWARD,
    _RELAY_SESSION,
    _ROLE_MAX_LENGTH,
    _STALE_LIVE_SECONDS,
    _UNRECEIVED_ROUND_SECONDS,
    Access,
    CommittedEntry,
    ConversationStore,
    Decision,
    ForwardError,
    TaskStart,
    _entry_source,
    _find,
    _host_label,
    _json,
    _project,
    _task_source,
    entry_message_id,
    forwarded_prompt,
    naming_peers,
    normalize_access,
    normalize_role,
    task_message_id,
)
from agentnave.group import (
    Entry,
    Forwarded,
    Group,
    GroupSeat,
    Round,
    SeatState,
)
from agentnave.isolated import IsolationLease
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.locks import release_and_remove
from agentnave.member_names import seat_identity, task_identity
from agentnave.members import CapacityError, ProviderName, read_max_running
from agentnave.models import InvocationRequest, RoundOrder
from agentnave.orchestration_roles import OrchestrationRole
from agentnave.store import (
    Conversation,
    EntryRecord,
    Record,
    Review,
    RoundRecord,
    SeatRecord,
    SessionRecord,
    TaskRecord,
    now,
)
from agentnave.task_isolation import (
    IsolationHold,
    acquire_isolation,
    may_touch_isolated_dirs,
    release_hold,
)
from agentnave.task_start import latest_session_task
from agentnave.threaded import to_thread_keeping, to_thread_uninterrupted


def _missing_forward(message_id: str) -> ForwardError:
    return ForwardError(f"forward id {message_id} does not exist")


@dataclass
class _TaskStartup:
    record: Record | None = None
    native_lock: IO[bytes] | None = None
    hold: IsolationHold | None = None
    owns_hold: bool = False
    cleanup_paths: tuple[Path, ...] = ()
    started: bool = False


class Conversations(ConversationStore):
    """Saved groups and private task history, with review and tray view snapshots.

    Writes take the conversation's OS lock; idle conversations release it once saved.
    Running processes never survive a restart.
    """

    def open_show(
        self,
        title: str,
        cwd: Path | None,
        seats: list[GroupSeat],
        *,
        review: bool,
        isolated: bool,
        manager_session_id: str | None = None,
    ) -> Record:
        manager_session = self._manager_session(manager_session_id)
        seats = [
            replace(seat, role=normalize_role(seat.role), access=normalize_access(seat.access))
            for seat in seats
        ]
        conversation_id = uuid4().hex
        directory = self._seats_directory(conversation_id)
        if isolated:
            # Own seat directories make restricted profiles possible and keep seats apart.
            cwd = directory
            seats = [
                GroupSeat(
                    seat.id,
                    seat.provider,
                    {**seat.provider_options, "restricted_tools": True},
                    directory / seat.id,
                    seat.role,
                    seat.access,
                )
                for seat in seats
            ]
        elif cwd is None:
            raise ValueError("cwd is required unless isolated is true")
        try:
            if isolated:
                directory.mkdir(parents=True, mode=0o700)
                for seat in seats:
                    seat.cwd.mkdir(parents=True, mode=0o700)
                    get_adapter(seat.provider).prepare_seat(seat.cwd)
            group = self.open(cwd, seats, conversation_id)
            state = Conversation(
                id=conversation_id,
                title=title,
                kind="show",
                cwd=str(group.cwd),
                review=review,
                isolated=isolated,
                seats=[
                    SeatRecord(
                        id=seat.id,
                        provider=seat.provider,
                        provider_options=dict(seat.provider_options),
                        cwd=str(seat.cwd),
                        role=seat.role,
                        access=normalize_access(seat.access),
                    )
                    for seat in group.seats.values()
                ],
                sessions={seat_id: SessionRecord() for seat_id in group.seats},
            )
            record = self._create(state, manager_session)
            self._release_idle(record)
            return record
        except BaseException:
            self._groups.pop(conversation_id, None)
            if isolated:
                shutil.rmtree(directory, ignore_errors=True)
            raise

    def _group(self, group_id: str) -> Group:
        record = self.writable(group_id)
        if group_id not in self._groups:
            if record.state.kind != "show":
                raise KeyError(f"conversation {group_id} is not a meeting")
            state = record.state
            assert state.cwd is not None
            group = Group(
                state.id,
                Path(state.cwd),
                {
                    seat.id: GroupSeat(
                        seat.id,
                        seat.provider,
                        dict(seat.provider_options),
                        Path(seat.cwd),
                        seat.role,
                        seat.access,
                    )
                    for seat in state.seats
                },
                [
                    Entry(
                        round=item.round,
                        kind=item.kind,
                        seat_id=item.seat,
                        text=item.text,
                        manager_host=item.manager_host,
                        manager_model=item.manager_model,
                        reported_model=item.reported_model,
                        manager_reported_model=item.manager_reported_model,
                        forward_id=item.forward_id,
                        forward_source=item.forward_source,
                    )
                    for item in state.entries
                ],
                {
                    seat_id: SeatState(saved.session_id, saved.cursor)
                    for seat_id, saved in state.sessions.items()
                },
                max((round_.number for round_ in state.rounds), default=0),
            )
            self.adopt(group)
        return super()._group(group_id)

    def run(
        self,
        group_id: str,
        seat_ids: list[str],
        order: RoundOrder,
        prompt: str,
        instructions: dict[str, str],
        *,
        manager_host: str | None = None,
        manager_model: str | None = None,
        manager_session_id: str | None = None,
        manager_reported_model: str | None = None,
        forward: list[str] | None = None,
    ) -> Round:
        with self._writing(group_id):
            group = self._group(group_id)
            state = self.records[group_id].state
            # The identity may come from the argument or the injected environment alike.
            session = self._manager_session(manager_session_id)
            if session is not None and session != state.manager_session:
                raise ValueError(self._not_yours("Group", state.manager_session))
            if state.pending_review():
                raise ValueError("answers are waiting for review; call decide_answers first")
            provider_health.check(
                self.root,
                (
                    (seat.provider, provider_health.model_option(seat.provider_options))
                    for seat in state.seats
                    if seat.id in seat_ids
                ),
            )
            # A parallel round starts every seat at once; a serial one runs a seat at a time.
            self._check_capacity(len(seat_ids) if order == "parallel" else 1)
            manager_reported = self._manager_model(state.manager_session) or manager_reported_model
            added = [
                Entry(
                    group.rounds + 1,
                    "forward",
                    text=item.text,
                    manager_host=manager_host or self.host.name,
                    manager_model=manager_model,
                    manager_reported_model=manager_reported,
                    forward_id=item.message_id,
                    forward_source=item.source,
                )
                for item in self._resolve_forwards(forward, session)
            ]
            group.entries.extend(added)
            try:
                return super().run(
                    group_id,
                    seat_ids,
                    order,
                    prompt,
                    instructions,
                    manager_host=manager_host or self.host.name,
                    manager_model=manager_model,
                    manager_reported_model=manager_reported,
                )
            except BaseException:
                # An input rejection happens before the task entry is appended; keep the rollback
                # conservative so a later failure could never drop committed entries.
                if added and group.entries[-len(added) :] == added:
                    del group.entries[-len(added) :]
                raise

    def retry(
        self, group_id: str, seat_ids: list[str], *, manager_session_id: str | None = None
    ) -> Round:
        with self._writing(group_id):
            self._group(group_id)
            state = self.records[group_id].state
            session = self._manager_session(manager_session_id)
            if session is not None and session != state.manager_session:
                raise ValueError(self._not_yours("Group", state.manager_session))
            self._validate_retry(group_id, seat_ids)
            provider_health.check(
                self.root,
                (
                    (seat.provider, provider_health.model_option(seat.provider_options))
                    for seat in state.seats
                    if seat.id in seat_ids
                ),
            )
            self._check_capacity(len(seat_ids))
            return super().retry(group_id, seat_ids)

    def post(
        self,
        conversation_id: str,
        text: str,
        *,
        manager_model: str | None = None,
        manager_session_id: str | None = None,
    ) -> None:
        with self._writing(conversation_id):
            group = self._group(conversation_id)
            state = self.records[conversation_id].state
            session = self._manager_session(manager_session_id)
            if session is not None and session != state.manager_session:
                raise ValueError(self._not_yours("Group", state.manager_session))
            if group.active is not None or state.pending_review():
                raise ValueError("post only between rounds, with no answers pending review")
            group.entries.append(
                Entry(
                    group.rounds,
                    "host",
                    text=text,
                    manager_host=self.host.name,
                    manager_model=manager_model,
                    manager_reported_model=self._manager_model(state.manager_session),
                )
            )
            try:
                self._save(group)
            except OSError:
                group.entries.pop()
                raise

    def decide(self, round_id: str, decisions: dict[str, Decision]) -> RoundRecord:
        """Validate the whole batch, save it, then let the round (or saved history) apply it."""
        record = next((item for item in self.records.values() if _find(item.state, round_id)), None)
        if record is None:
            self.scan()
            record = next(
                (item for item in self.records.values() if _find(item.state, round_id)), None
            )
        if record is None:
            raise KeyError(f"unknown round_id: {round_id}")
        with self._writing(record.state.id) as record:
            saved = _find(record.state, round_id)
            if saved is None:
                raise KeyError(f"unknown round_id: {round_id}")
            if not decisions:
                raise ValueError("decisions must name at least one seat")
            live = self._rounds.get(round_id)
            # A round of this process keeps its reviews in memory; a restored one only on disk.
            current: dict[str, Review | None] = (
                self.reviews.setdefault(round_id, {})
                if live is not None
                else {seat.seat_id: seat.review for seat in saved.seats}
            )
            for seat_id, decision in decisions.items():
                review = current.get(seat_id)
                if review is None:
                    raise ValueError(f"seat {seat_id} has no answer to review in this round")
                if review not in ("pending", decision):
                    raise ValueError(f"seat {seat_id} was already decided: {review}")
                if live is not None and live.cancelling and review == "pending":
                    # Cancel keeps undecided answers private; a late publish must not slip in.
                    raise ValueError(
                        "this round is being cancelled; undecided answers stay private"
                    )
            if live is not None:
                before = dict(current)
                current.update(decisions)
                try:
                    self._save(live.group)
                except OSError:
                    current.clear()
                    current.update(before)
                    raise
                live.notify()
            else:
                for seat in saved.seats:
                    if seat.seat_id in decisions:
                        seat.review = decisions[seat.seat_id]
                added: list[EntryRecord] = []
                if not saved.pending_review():
                    added = record.state.commit_ended(saved)  # what the stopped round left
                self._persist(record)
                group = self._groups.get(record.state.id)
                if group is not None:
                    group.entries.extend(
                        Entry(
                            round=item.round,
                            kind=item.kind,
                            seat_id=item.seat,
                            text=item.text,
                            manager_host=item.manager_host,
                            manager_model=item.manager_model,
                            reported_model=item.reported_model,
                            manager_reported_model=item.manager_reported_model,
                            forward_id=item.forward_id,
                            forward_source=item.forward_source,
                        )
                        for item in added
                    )
            self._ensure_live_writer(record)
            result = _find(record.state, round_id)
            assert result is not None
            return result

    def workspace_id(self, manager_session_id: str | None) -> str | None:
        return self._manager_session(manager_session_id)

    def reading_workspace(self, manager_session_id: str | None) -> str | None:
        """The calling boss's workspace for reading history; one it handed over still reads as
        its own. None without a boss identity."""
        return self._manager_session(manager_session_id, strict=False)

    def invocation_workspaces(self) -> dict[str, str | None]:
        """The workspace each private task or meeting seat run of this server works for."""

        def owner(conversation_id: str) -> str | None:
            record = self.records.get(conversation_id)
            return record.state.manager_session if record is not None else None

        found = {
            invocation_id: owner(conversation_id)
            for invocation_id, (conversation_id, _) in self.task_invocations.items()
        }
        for round_ in (*self._completed_rounds.values(), *self._rounds.values()):
            for run in round_.runs:
                if run.invocation_id is not None:
                    found[run.invocation_id] = owner(round_.group.id)
        return found

    def inbox(self, manager_session_id: str | None) -> tuple[list[str], list[str]]:
        """This server's task invocations and rounds whose end no caller has received yet.

        With a boss identity (argument or host_session_id) only that boss's
        conversations count; without one, every handle of this server. Oldest first.
        """
        session = self._manager_session(manager_session_id, strict=False)

        def mine(conversation_id: str) -> bool:
            if session is None:
                return True
            record = self.records.get(conversation_id)
            return record is not None and record.state.manager_session == session

        known = {listing.invocation_id for listing in self.manager.list_invocations()}
        self.delivered_tasks &= known
        tasks = [
            invocation_id
            for invocation_id, (conversation_id, _) in self.task_invocations.items()
            if invocation_id in known
            and invocation_id not in self.delivered_tasks
            and mine(conversation_id)
        ]
        rounds = {**self._completed_rounds, **self._rounds}
        self.delivered_rounds &= set(rounds)
        pending = sorted(
            (
                round_
                for round_id, round_ in rounds.items()
                if round_id not in self.delivered_rounds and mine(round_.group.id)
            ),
            key=lambda round_: round_.started_at,
        )
        return tasks, [round_.id for round_ in pending]

    # Forwards

    def _resolve_forwards(
        self, forward: list[str] | None, manager_session: str | None
    ) -> list[Forwarded]:
        """Validate and read every referenced message; reject the whole batch on any problem."""
        if not forward:
            return []
        if len(forward) > _MAX_FORWARD:
            raise ForwardError(f"forward accepts at most {_MAX_FORWARD} message ids")
        repeated = sorted({item for item in forward if forward.count(item) > 1})
        if repeated:
            raise ForwardError("forward message ids must be unique: " + ", ".join(repeated))
        if manager_session is None:
            raise ForwardError(
                "forwarding needs a known manager session; pass manager_session_id or "
                "configure AGENTNAVE_HOST_SESSION_ID"
            )
        self.scan()  # Include records another server saved.
        return [self._resolve_forward(item, manager_session) for item in forward]

    def _resolve_forward(self, message_id: str, manager_session: str) -> Forwarded:
        parts = message_id.split(":")
        if len(parts) != 3 or parts[1] not in ("t", "e"):
            raise ForwardError(f"forward id {message_id} is not a workspace message id")
        conversation_id, kind, local = parts
        record = self.records.get(conversation_id)
        if record is None or (kind == "e" and record.state.kind != "show"):
            raise _missing_forward(message_id)
        if record.state.manager_session != manager_session:
            raise ForwardError(f"forward id {message_id} is not in this manager workspace")
        if kind == "t":
            task = next((item for item in record.state.tasks if item.id == local), None)
            if task is None:
                raise _missing_forward(message_id)
            if task.status == "running" or task.id in self.running_tasks:
                raise ForwardError(f"forward id {message_id} is still running")
            if task.status != "succeeded":
                raise ForwardError(
                    f"forward id {message_id} is not a succeeded task: {task.status}"
                )
            return Forwarded(
                message_id,
                task.output,
                _task_source(task, task_identity(record.state, task, naming_peers(record))),
            )
        if not local.isascii() or not local.isdigit():
            raise _missing_forward(message_id)
        index = int(local)
        if index >= len(record.state.entries):
            raise _missing_forward(message_id)
        entry = record.state.entries[index]
        if entry.kind == "absent":
            raise ForwardError(f"forward id {message_id} is an absent entry, not a message")
        if entry.kind not in ("answer", "host", "task", "forward"):
            raise ForwardError(f"forward id {message_id} is not a committed message")
        return Forwarded(message_id, entry.text or "", _entry_source(record.state, entry))

    def task_message_id(self, invocation_id: str) -> str | None:
        """The saved task's message id, once it reached a terminal state."""
        found = self.task_invocations.get(invocation_id)
        if found is None:
            return None
        conversation_id, task_id = found
        record = self.records.get(conversation_id)
        if record is None:
            return None
        task = next((item for item in record.state.tasks if item.id == task_id), None)
        if task is None or task.status == "running":
            return None
        return task_message_id(conversation_id, task_id)

    def round_conversation(self, round_id: str) -> str | None:
        for record in self.records.values():
            if _find(record.state, round_id) is not None:
                return record.state.id
        return None

    def committed_entries(self, conversation_id: str, number: int) -> list[CommittedEntry]:
        """Entries of one round in append order; ids stay stable across restarts."""
        record = self.record(conversation_id)
        return [
            CommittedEntry(
                entry_message_id(conversation_id, index),
                entry.kind,
                entry.round,
                entry.seat,
                seat_identity(
                    record.state, entry.seat, reported=entry.reported_model, number=entry.round
                )
                if entry.seat is not None
                else None,
            )
            for index, entry in enumerate(record.state.entries)
            if entry.round == number
        ]

    # Tasks

    def _check_capacity(self, needed: int) -> None:
        """Refuse work that would run more CLI processes at once than the App allows."""
        limit = read_max_running()
        active = self.manager.active_ids()
        # A serial round between seats, or waiting on a review, keeps a slot for its next seat.
        held = sum(
            1
            for round_ in self._rounds.values()
            if round_.order == "serial"
            and not round_.finished
            and not round_.cancelling
            and any(run.status == "pending" for run in round_.runs)
            and not any(run.invocation_id in active for run in round_.runs)
        )
        running = len(active) + held
        if running + needed > limit:
            raise CapacityError(
                f"{running} CLI runs are active or held by serial rounds for their next seat, and "
                f"this would start {needed}; the AgentNave App limits this server to {limit} at "
                "once. Wait for some to finish, then retry"
            )

    def resume_task(
        self,
        session_id: str,
        provider: ProviderName | None,
        manager_session_id: str | None,
        *,
        explicit_identity: bool,
    ) -> TaskRecord | None:
        manager_session = self._manager_session(manager_session_id)
        self.scan()
        return latest_session_task(
            self.records.values(),
            session_id,
            provider,
            manager_session,
            self._not_yours,
            explicit_identity=explicit_identity,
        )

    def start_task(
        self,
        request: InvocationRequest,
        title: str | None,
        conversation_id: str | None,
        *,
        manager_model: str | None = None,
        manager_session_id: str | None = None,
        role: str | None = None,
        seat: OrchestrationRole | None = None,
        forward: list[str] | None = None,
        access: str | None = None,
        isolation_requested: bool = False,
        isolation_ref: str | None = None,
        hold: IsolationHold | None = None,
    ) -> TaskStart:
        """Start one private task.

        hold is an isolated directory the caller acquired ahead (see acquire_task_isolation);
        the caller then also gives it back when this raises. Without one, it is acquired here.
        """
        role = normalize_role(role)
        declared = normalize_access(access)
        if isolation_requested and request.session_id is not None:
            raise ValueError("isolation cannot be combined with session_id")
        manager_session = self._manager_session(manager_session_id)
        if isolation_requested and manager_session is None:
            raise ValueError(
                "isolation needs a known manager session; pass manager_session_id or "
                "configure AGENTNAVE_HOST_SESSION_ID"
            )
        forwarded = self._resolve_forwards(forward, manager_session)
        prompt = request.prompt  # Keep the manager's own text as the saved task prompt.
        self.manager.check_start(request)
        self._check_capacity(1)
        startup = _TaskStartup(hold=hold, owns_hold=hold is None)
        try:
            if conversation_id:
                startup.record = self.writable(conversation_id)
                if startup.record.state.manager_session != manager_session:
                    raise ValueError(
                        self._not_yours("Task conversation", startup.record.state.manager_session)
                    )
                if startup.record.state.kind != "task":
                    raise ValueError("Tasks need a task conversation")
            if startup.hold is None:
                startup.hold = self._acquire_hold(
                    request,
                    manager_session,
                    isolation_requested=isolation_requested,
                    isolation_ref=isolation_ref,
                )
            request = startup.hold.request
            if forwarded:
                request = replace(request, prompt=forwarded_prompt(forwarded, prompt))
            prepared = self.manager.prepare(request)
            startup.cleanup_paths = tuple(Path(path) for path in prepared.cleanup_paths)
            # Held by the run until it ends: no other server can resume this session meanwhile.
            startup.native_lock = self.manager.reserve(request)
            record = self._bind_task_record(
                startup, request, conversation_id, manager_session, title
            )
            info = startup.hold.info
            task = self._new_task_record(
                request,
                prompt,
                manager_model,
                manager_session,
                role,
                seat,
                declared,
                info,
                forwarded,
            )
            record.state.tasks.append(task)
            self._persist(record)
            invocation_id = self._launch_prepared_task(startup, record, task, request, prepared)
            startup.native_lock = None  # the run releases it when it ends
            startup.started = True
            startup.hold.consumed = True
            self.running_tasks[task.id] = invocation_id
            self.task_invocations[invocation_id] = (record.state.id, task.id)
            self._ensure_live_writer(record)
            collector = asyncio.create_task(self._collect(record, task.id, invocation_id))
            self.collectors.add(collector)
            collector.add_done_callback(self.collectors.discard)
            return TaskStart(invocation_id, record.state.id, declared, info, startup.hold.warnings)
        finally:
            self._cleanup_task_startup(startup)

    def _new_task_record(
        self,
        request: InvocationRequest,
        prompt: str,
        manager_model: str | None,
        manager_session: str | None,
        role: str | None,
        seat: OrchestrationRole | None,
        declared: Access | None,
        info: TaskIsolationInfo | None,
        forwarded: list[Forwarded],
    ) -> TaskRecord:
        return TaskRecord(
            id=uuid4().hex,
            provider=request.provider,
            model=str(request.provider_options.get("model", "")),
            effort=str(request.provider_options["effort"])
            if "effort" in request.provider_options
            else None,
            cwd=str(request.cwd),
            prompt=prompt,
            manager_host=self.host.name,
            manager_model=manager_model,
            manager_reported_model=self._manager_model(manager_session),
            session_id=request.session_id,
            time=now(),
            role=role,
            seat=seat,
            access=declared,
            isolation_id=info.id if info is not None else None,
            isolation_path=info.path if info is not None else None,
            isolation_branch=info.branch if info is not None else None,
            forward=[item.message_id for item in forwarded],
        )

    def _launch_prepared_task(
        self,
        startup: _TaskStartup,
        record: Record,
        task: TaskRecord,
        request: InvocationRequest,
        prepared: PreparedCommand,
    ) -> str:
        on_settled = self._task_settlement(startup)
        try:
            if on_settled is not None:
                return self.manager.start(
                    request,
                    prepared=prepared,
                    claim=startup.native_lock,
                    on_settled=on_settled,
                )
            return self.manager.start(
                request,
                prepared=prepared,
                claim=startup.native_lock,
            )
        except Exception:
            if startup.hold is not None and startup.hold.created is not None:
                task.isolation_id = None
                task.isolation_path = None
                task.isolation_branch = None
            task.status = "failed"
            task.error = "start_failed"
            self._persist(record)
            raise

    def _acquire_hold(
        self,
        request: InvocationRequest,
        manager_session: str | None,
        *,
        isolation_requested: bool,
        isolation_ref: str | None,
    ) -> IsolationHold:
        if not may_touch_isolated_dirs(request.cwd, requested=isolation_requested):
            return IsolationHold(request)
        hold = acquire_isolation(
            self.isolated,
            request,
            manager_session,
            requested=isolation_requested,
            ref=isolation_ref,
        )
        if (lease := hold.lease) is not None:
            self.active_lease_paths.add(lease.path)
        return hold

    async def acquire_task_isolation(
        self,
        request: InvocationRequest,
        manager_session_id: str | None,
        *,
        isolation_requested: bool,
        isolation_ref: str | None,
    ) -> IsolationHold:
        """Acquire the task's isolated directory on a worker thread, so waiting calls and other
        tasks keep running; pass the result to start_task and give it back on failure."""
        manager_session = self._manager_session(manager_session_id)
        if isolation_requested:
            if request.session_id is not None:
                raise ValueError("isolation cannot be combined with session_id")
            if manager_session is None:
                raise ValueError(
                    "isolation needs a known manager session; pass manager_session_id or "
                    "configure AGENTNAVE_HOST_SESSION_ID"
                )
        if not may_touch_isolated_dirs(request.cwd, requested=isolation_requested):
            return IsolationHold(request)
        self.manager.check_start(request)
        self._check_capacity(1)
        hold = await to_thread_keeping(
            lambda held: release_hold(self.isolated, held),
            acquire_isolation,
            self.isolated,
            request,
            manager_session,
            requested=isolation_requested,
            ref=isolation_ref,
        )
        if (lease := hold.lease) is not None:
            self.active_lease_paths.add(lease.path)
        return hold

    async def give_back_isolation(self, hold: IsolationHold) -> None:
        """Release a hold whose task never started, off the event loop."""
        if (lease := hold.lease) is not None:
            self.active_lease_paths.discard(lease.path)
        await to_thread_uninterrupted(release_hold, self.isolated, hold)

    def _task_settlement(self, startup: _TaskStartup) -> Callable[[str], None] | None:
        hold = startup.hold
        if hold is None or hold.info is None or hold.lease is None:
            return None
        info = hold.info
        lease: IsolationLease = hold.lease

        def settle(invocation_id: str) -> None:
            # Runs on a worker thread after the process tree ended; the lease goes last.
            try:
                self.isolation_summaries[invocation_id] = self.isolated.summarize_changes(
                    info.id,
                    fallback_path=info.path,
                    fallback_branch=info.branch,
                )
            finally:
                self.active_lease_paths.discard(lease.path)
                lease.release()

        return settle

    def _cleanup_task_startup(self, startup: _TaskStartup) -> None:
        """Run every cleanup step even when one fails; the error already in flight stays."""

        def drop_files() -> None:
            for path in startup.cleanup_paths:
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    logging.getLogger(__name__).exception("Removing %s failed", path)

        def give_back() -> None:
            hold = startup.hold
            if hold is None or not startup.owns_hold or startup.started:
                return
            if (lease := hold.lease) is not None:
                self.active_lease_paths.discard(lease.path)
            release_hold(self.isolated, hold)

        def release_lock() -> None:
            if startup.native_lock is not None:
                release_and_remove(startup.native_lock, Path(startup.native_lock.name))

        steps: list[Callable[[], None]] = []
        if not startup.started:
            steps.append(drop_files)
        steps.append(give_back)
        if (record := startup.record) is not None:
            steps.append(lambda: self._release_idle(record))
        steps.append(release_lock)
        for step in steps:
            try:
                step()
            except Exception:
                logging.getLogger(__name__).exception("Task startup cleanup step failed")

    def _bind_task_record(
        self,
        startup: _TaskStartup,
        request: InvocationRequest,
        conversation_id: str | None,
        manager_session: str | None,
        title: str | None,
    ) -> Record:
        if startup.record is None and conversation_id:
            startup.record = self.writable(conversation_id)
        if startup.record is not None and startup.record.state.manager_session != manager_session:
            raise ValueError(
                self._not_yours("Task conversation", startup.record.state.manager_session)
            )
        if request.session_id:
            # Under the native-session lock, include other chats' active bindings.
            self.scan()

            def resumes(task: TaskRecord) -> bool:
                return task.provider == request.provider and task.session_id == request.session_id

            if startup.record is None:
                # Another server may have created or extended this session's conversation.
                startup.record = max(
                    (
                        item
                        for item in self.records.values()
                        if item.state.kind == "task"
                        and item.state.manager_session == manager_session
                        and any(map(resumes, item.state.tasks))
                    ),
                    # Continue the most recent binding, independent of directory order.
                    key=lambda item: (
                        max(task.time for task in item.state.tasks if resumes(task)),
                        item.state.created,
                        item.state.id,
                    ),
                    default=None,
                )
                if startup.record is not None:
                    # Keep a native session together within this manager chat.
                    startup.record = self.writable(startup.record.state.id)
        if startup.record is None:
            conversation_id = uuid4().hex
            startup.record = self._create(
                Conversation(
                    id=conversation_id,
                    title=title or f"{request.provider} 任务",
                    kind="task",
                ),
                manager_session,
            )
        if startup.record.state.kind != "task":
            raise ValueError("Tasks need a task conversation")
        return startup.record

    async def _collect(self, record: Record, task_id: str, invocation_id: str) -> None:
        result = await self.manager.wait(invocation_id)
        assert result is not None

        def finish(status: str, error: str | None) -> None:
            task = next(item for item in record.state.tasks if item.id == task_id)
            task.status = status
            task.output = result.output
            task.session_id = result.session_id or task.session_id
            task.elapsed_ms = result.duration_ms
            task.reported_model = result.reported_model or task.reported_model
            task.error = error
            task.prompts = self._prompt_audit(task.prompts, invocation_id)

        error = result.error
        # Keep the provider's own message like a seat does; older records hold only the code.
        finish(result.status.value, f"{error.code}: {error.message}" if error else None)
        try:
            self._persist(record)
        except OSError:
            # The rollback dropped the result; keep it in memory, marked as not saved, so a
            # resumed native session still joins this conversation.
            finish("interrupted", "history_write_failed; use wait for the invocation result")
            record.saved = record.state.model_copy(deep=True)
        self.running_tasks.pop(task_id, None)
        self._ensure_live_writer(record)
        self._release_idle(record)


__all__ = [
    "Conversations",
    "normalize_role",
    "normalize_access",
    "TaskStart",
    "ForwardError",
    "CommittedEntry",
    "task_message_id",
    "entry_message_id",
    "forwarded_prompt",
    "_host_label",
    "_task_source",
    "_entry_source",
    "_project",
    "_json",
    "ConversationStore",
    "_find",
    "Decision",
    "Access",
    "_ROLE_MAX_LENGTH",
    "_MAX_FORWARD",
    "_UNRECEIVED_ROUND_SECONDS",
    "_LIVE_SECONDS",
    "_STALE_LIVE_SECONDS",
    "_RELAY_SESSION",
    "_CONNECTION_ID",
    "_MANAGER_STARTED_AT",
]
