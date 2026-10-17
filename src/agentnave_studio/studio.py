"""Saved shows and task history on top of core groups: persistence, review and the dashboard."""

from __future__ import annotations

import asyncio
import itertools
import json
import os
import shutil
import sys
import time
from collections.abc import Generator
from contextlib import contextmanager, suppress
from pathlib import Path
from typing import Literal, TypedDict
from uuid import uuid4

from agentnave.adapters import get_adapter
from agentnave.adapters.antigravity import remove_restricted_profile, write_restricted_profile
from agentnave.core import InvocationManager
from agentnave.group import Entry, Group, Groups, GroupSeat, Round, RoundOrder, SeatRun, SeatState
from agentnave.host import Host, detect_host
from agentnave.models import InvocationRequest
from agentnave_studio import views
from agentnave_studio.dashboard import Dashboard
from agentnave_studio.store import (
    Conversation,
    EntryRecord,
    Record,
    Review,
    RoundRecord,
    SeatRecord,
    SeatRunRecord,
    SessionRecord,
    TaskRecord,
    Unavailable,
    now,
    scan,
    write_file,
)

type Decision = Literal["publish", "discard"]

_LIVE_SECONDS = 1.5  # also how often the dashboard re-projects running work
_STALE_LIVE_SECONDS = 3600
# The relay passes one session per host connection; a server started directly makes its own.
_RELAY_SESSION = os.environ.get("AGENTNAVE_MANAGER_SESSION", "")
_MANAGER_SESSION = (
    _RELAY_SESSION
    if len(_RELAY_SESSION) == 32 and all(c in "0123456789abcdef" for c in _RELAY_SESSION)
    else uuid4().hex
)
_MANAGER_STARTED_AT = os.environ.get("AGENTNAVE_MANAGER_STARTED_AT") or now()


def _project(record: Record) -> bytes:
    return _json(views.snapshot(record.state, str(record.directory)))


def _json(data: object) -> bytes:
    return json.dumps(data, ensure_ascii=False).encode()


Record.project = _project


class DeletionResult(TypedDict):
    conversation_id: str
    deleted_paths: list[str]
    retained_paths: list[str]
    forgotten_invocations: int


class Studio(Groups):
    """Groups whose state is saved per conversation, plus private task history.

    Writes take the conversation's OS lock; idle conversations release it once saved.
    Running processes never survive a restart.
    """

    def __init__(self, manager: InvocationManager, root: Path, *, host: Host | None = None) -> None:
        super().__init__(manager)
        self.host = host if host is not None else detect_host()
        self.manager_session = _MANAGER_SESSION
        self.manager_started_at = _MANAGER_STARTED_AT
        self.root = root.resolve()  # created only by the first saved conversation
        self.records: dict[str, Record] = {}
        self.unavailable: list[Unavailable] = []
        self.invocations: dict[str, set[str]] = {}
        self.running_tasks: dict[str, str] = {}  # task id -> invocation id
        self.collectors: list[asyncio.Task[None]] = []
        self.dashboard: Dashboard | None = None
        self.tokens: dict[str, tuple[str, str]] = {}  # conversation id -> public, director
        self.home_token = uuid4().hex
        self.refresher: asyncio.Task[None] | None = None
        self.refresh_error: str | None = None
        self.reviews: dict[str, dict[str, Review | None]] = {}  # round id -> seat -> review
        # Completed replies remain readable, but never participate in group writes again.
        self._completed_rounds: dict[str, Round] = {}
        self._unsaved: set[str] = set()
        # While work runs, its reply tails are written here for the tray app, then removed.
        self.live_path = self.root / "live" / f"{os.getpid()}-{uuid4().hex[:8]}.json"
        self.live_writer: asyncio.Task[None] | None = None

    # Conversations

    def scan(self) -> dict[str, bool]:
        """Load every saved conversation and report which ones this server may write."""
        found, self.unavailable = scan(self.root)
        for conversation_id in set(self.records) - set(found):
            if self.records[conversation_id].lock is None:
                self._forget(conversation_id)  # deleted by another server
        writable: dict[str, bool] = {}
        read_error: str | None = None
        for conversation_id, directory in found.items():
            record = self.records.get(conversation_id)
            try:
                if record is None:
                    record = Record.read(directory)
                    if record.state.id != conversation_id:
                        raise ValueError("record id does not match its directory")
                    self.records[conversation_id] = record
                writable[conversation_id] = record.refresh()
            except (OSError, ValueError) as exc:
                if isinstance(exc, OSError) and record is not None:
                    # Keep the last readable state and its URLs through transient I/O failures.
                    read_error = str(exc)
                    writable[conversation_id] = False
                else:
                    self._forget(conversation_id)
                self.unavailable.append(Unavailable(path=str(directory), reason=str(exc)))
        self.refresh_error = read_error
        for record in list(self.records.values()):
            self._publish(record, home=False)
        self._publish_home()
        return writable

    def record(self, conversation_id: str) -> Record:
        if conversation_id not in self.records:
            self.scan()
        try:
            return self.records[conversation_id]
        except KeyError:
            raise KeyError(f"unknown conversation_id: {conversation_id}") from None

    def writable(self, conversation_id: str) -> Record:
        record = self.record(conversation_id)
        try:
            record.acquire()
        except FileNotFoundError:
            self._forget(conversation_id)
            self._publish_home()
            raise KeyError(f"unknown conversation_id: {conversation_id}") from None
        return record

    @contextmanager
    def _writing(self, conversation_id: str) -> Generator[Record]:
        record = self.writable(conversation_id)
        try:
            yield record
        finally:
            self._release_idle(record)

    def _persist(self, record: Record) -> None:
        self._unsaved.add(record.state.id)
        record.save()
        self._unsaved.discard(record.state.id)

    def _invalidate_group(self, conversation_id: str) -> None:
        self._groups.pop(conversation_id, None)
        for round_id, round_ in list(self._rounds.items()):
            if round_.group.id == conversation_id:
                self._completed_rounds[round_id] = self._rounds.pop(round_id)
                self.reviews.pop(round_id, None)

    def _release_idle(self, record: Record) -> None:
        if record.lock is None or record.state.id in self._unsaved or self.busy(record):
            return
        self._invalidate_group(record.state.id)
        record.release()

    def _forget(self, conversation_id: str) -> None:
        self._invalidate_group(conversation_id)
        self._completed_rounds = {
            key: item
            for key, item in self._completed_rounds.items()
            if item.group.id != conversation_id
        }
        self.records.pop(conversation_id, None)
        self._unsaved.discard(conversation_id)
        self._unpublish(conversation_id)

    def _round(self, round_id: str) -> Round:
        if round_id in self._completed_rounds:
            return self._completed_rounds[round_id]
        return super()._round(round_id)

    def group(self, group_id: str) -> Group:
        with self._writing(group_id):
            return self._group(group_id)

    def _create(self, state: Conversation, directory: Path) -> Record:
        state.host = self.host.name
        state.host_source = self.host.source
        state.manager_session = self.manager_session
        record = Record.create(directory, state)
        self.records[state.id] = record
        self._publish(record)
        return record

    def _directory(self, conversation_id: str) -> Path:
        return self.root / "conversations" / conversation_id

    def open_show(
        self,
        title: str,
        cwd: Path | None,
        seats: list[GroupSeat],
        *,
        review: bool,
        isolated: bool,
    ) -> Record:
        conversation_id = uuid4().hex
        directory = self._directory(conversation_id)
        if isolated:
            # Own seat directories make restricted profiles possible and keep seats apart.
            cwd = directory
            seats = [
                GroupSeat(
                    seat.id,
                    seat.provider,
                    {**seat.provider_options, "restricted_tools": True},
                    directory / "seats" / seat.id,
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
                    if seat.provider == "antigravity":
                        write_restricted_profile(seat.cwd)
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
                    )
                    for seat in group.seats.values()
                ],
                sessions={seat_id: SessionRecord() for seat_id in group.seats},
            )
            record = self._create(state, directory)
            self._release_idle(record)
            return record
        except BaseException:
            self._groups.pop(conversation_id, None)
            shutil.rmtree(directory, ignore_errors=True)
            raise

    def _group(self, group_id: str) -> Group:
        record = self.writable(group_id)
        if group_id not in self._groups:
            if record.state.kind != "show":
                raise KeyError(f"conversation {group_id} is not a show")
            state = record.state
            assert state.cwd is not None
            group = Group(
                state.id,
                Path(state.cwd),
                {
                    seat.id: GroupSeat(
                        seat.id, seat.provider, dict(seat.provider_options), Path(seat.cwd)
                    )
                    for seat in state.seats
                },
                [Entry(item.round, item.kind, item.seat, item.text) for item in state.entries],
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
    ) -> Round:
        with self._writing(group_id):
            self._group(group_id)
            state = self.records[group_id].state
            if state.archived:
                raise ValueError("this conversation has ended; reopen it with update_conversation")
            if state.pending_review():
                raise ValueError("answers are waiting for review; call decide_answers first")
            return super().run(group_id, seat_ids, order, prompt, instructions)

    def post(self, conversation_id: str, text: str) -> None:
        with self._writing(conversation_id):
            group = self._group(conversation_id)
            state = self.records[conversation_id].state
            if state.archived or group.active is not None or state.pending_review():
                raise ValueError(
                    "post only in an open show with no running round or pending review"
                )
            group.entries.append(Entry(group.rounds, "host", text=text))
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
                    # Cancel discards undecided answers; a late publish must not slip in.
                    raise ValueError(
                        "this round is being cancelled; undecided answers are discarded"
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
                        Entry(item.round, item.kind, item.seat, item.text) for item in added
                    )
            self._publish(record)
            result = _find(record.state, round_id)
            assert result is not None
            return result

    def busy(self, record: Record) -> bool:
        group = self._groups.get(record.state.id)
        return (
            (group is not None and group.active is not None)
            or record.state.pending_review()
            or any(task.status == "running" for task in record.state.tasks)
        )

    def update(self, conversation_id: str, title: str | None, archived: bool | None) -> Record:
        with self._writing(conversation_id) as record:
            if archived and self.busy(record):
                raise ValueError("Resolve running work and pending reviews before ending it")
            if title is not None:
                if not title.strip() or len(title) > 200:
                    raise ValueError("Title must contain 1–200 characters")
                record.state.title = title
            if archived is not None:
                record.state.archived = archived
            group = self._groups.get(conversation_id)
            if group is not None:
                self._save(group)  # include any group changes an earlier failed save missed
            else:
                self._persist(record)
                self._publish(record)
            return record

    def delete(self, conversation_id: str) -> DeletionResult:
        with self._writing(conversation_id) as record:
            if not record.state.archived:
                raise ValueError("End the conversation with update_conversation before deleting it")
            if self.busy(record):
                raise ValueError("Resolve running work and pending reviews before deleting it")
            state_path = record.directory / "state.json"
            state_path.unlink()
            (record.directory / "views.json").unlink(missing_ok=True)
            forgotten = self.manager.forget_finished(self.invocations.pop(conversation_id, set()))
            self._forget(conversation_id)
            if record.state.isolated:
                _prune_seats(
                    record.directory / "seats",
                    {seat.id for seat in record.state.seats if seat.provider == "antigravity"},
                )
            # Keep the directory and its lock file: a removed lock inode could let two servers
            # lock different files at the same path.
            record.release()
            self._publish_home()
            return {
                "conversation_id": conversation_id,
                "deleted_paths": [str(state_path)],
                "retained_paths": [str(record.directory)],
                "forgotten_invocations": forgotten,
            }

    # Tasks

    def start_task(
        self, request: InvocationRequest, title: str | None, conversation_id: str | None
    ) -> tuple[str, str]:
        self.manager.check_start(request)
        record = self.writable(conversation_id) if conversation_id else None
        try:
            if request.session_id:

                def resumes(task: TaskRecord) -> bool:
                    return (
                        task.provider == request.provider and task.session_id == request.session_id
                    )

                if record is None:
                    # Another server may have created or extended this session's conversation.
                    self.scan()
                    record = next(
                        (
                            item
                            for item in self.records.values()
                            if item.state.kind == "task"
                            and not item.state.archived
                            and any(map(resumes, item.state.tasks))
                        ),
                        None,
                    )
                    if record is not None:
                        # Lock before checking: never split one native session across conversations.
                        record = self.writable(record.state.id)
                if any(
                    task.status == "running" and resumes(task)
                    for item in self.records.values()
                    for task in item.state.tasks
                ):
                    raise ValueError("This native session already has an active task")
            # Validate options before creating history; prepare only once.
            prepared = get_adapter(request.provider).prepare(request)
            try:
                if record is None:
                    conversation_id = uuid4().hex
                    record = self._create(
                        Conversation(
                            id=conversation_id,
                            title=title or f"{request.provider} 任务",
                            kind="task",
                        ),
                        self._directory(conversation_id),
                    )
                if record.state.kind != "task" or record.state.archived:
                    raise ValueError("Tasks need an open task conversation")
                task = TaskRecord(
                    id=uuid4().hex,
                    provider=request.provider,
                    model=str(request.provider_options.get("model", "原生设置")),
                    cwd=str(request.cwd),
                    prompt=request.prompt,
                    session_id=request.session_id,
                    time=now(),
                )
                record.state.tasks.append(task)
                self._persist(record)
                invocation_id = self.manager.start(request, prepared=prepared)
            except BaseException:
                for path in prepared.cleanup_paths:
                    Path(path).unlink(missing_ok=True)
                raise
            self.invocations.setdefault(record.state.id, set()).add(invocation_id)
            self.running_tasks[task.id] = invocation_id
            self._publish(record)
            self.collectors.append(
                asyncio.create_task(self._collect(record, task.id, invocation_id))
            )
            return invocation_id, record.state.id
        finally:
            if record is not None:
                self._release_idle(record)

    async def _collect(self, record: Record, task_id: str, invocation_id: str) -> None:
        result = await self.manager.wait(invocation_id)
        assert result is not None

        def finish(status: str, error: str | None) -> None:
            task = next(item for item in record.state.tasks if item.id == task_id)
            task.status = status
            task.output = result.output
            task.session_id = result.session_id or task.session_id
            task.elapsed_ms = result.duration_ms
            task.error = error

        finish(result.status.value, result.error.code if result.error else None)
        try:
            self._persist(record)
        except OSError:
            # The rollback dropped the result; keep it in memory, marked as not saved, so a
            # resumed native session still joins this conversation.
            finish("interrupted", "history_write_failed; use wait_agent for the invocation result")
            record.saved = record.state.model_copy(deep=True)
        self.running_tasks.pop(task_id, None)
        self._publish(record)
        self._release_idle(record)

    # Group hooks

    def _on_change(self, round_: Round) -> None:
        if round_.cancelling and round_.error:
            reviews = self.reviews.get(round_.id, {})
            for seat_id, decision in reviews.items():
                if decision == "pending":
                    reviews[seat_id] = "discard"
        self._save(round_.group)

    def _save(self, group: Group) -> None:
        """Write the whole live group, so a save after a failed one restores every round."""
        record = self.records[group.id]
        state = record.state
        state.entries.extend(
            EntryRecord(
                round=item.round, kind=item.kind, seat=item.seat_id, text=item.text, time=now()
            )
            for item in group.entries[len(state.entries) :]
        )
        state.sessions = {
            seat_id: SessionRecord(session_id=item.session_id, cursor=item.cursor)
            for seat_id, item in group.states.items()
        }
        committed = {
            (item.round, item.seat_id)
            for item in group.entries
            if item.kind in ("answer", "absent")
        }
        for round_ in [item for item in self._rounds.values() if item.group is group]:
            saved = _find(state, round_.id)
            if saved is None:
                saved = RoundRecord(
                    id=round_.id,
                    number=round_.number,
                    order=round_.order,
                    seats=[
                        SeatRunRecord(seat_id=run.seat_id, instruction=run.instruction)
                        for run in round_.runs
                    ],
                )
                state.rounds.append(saved)
            saved.status = (
                "running"
                if group.active is round_
                else "failed"
                if round_.error
                else "cancelled"
                if round_.cancelling
                else "finished"
            )
            saved.error = round_.error
            for run, seat in zip(round_.runs, saved.seats, strict=True):
                seat.status = run.status
                seat.output = run.result.output if run.result is not None else None
                error = run.result.error if run.result is not None else run.launch_error
                seat.error = f"{error.code}: {error.message}" if error is not None else None
                seat.committed = seat.committed or (round_.number, run.seat_id) in committed
                seat.review = self.reviews.get(round_.id, {}).get(run.seat_id)
        self._persist(record)
        self._publish(record)
        self._release_idle(record)

    async def _approve(self, round_: Round, runs: list[SeatRun]) -> set[str]:
        seat_ids = {run.seat_id for run in runs}
        if not self.records[round_.group.id].state.review or not seat_ids:
            return seat_ids
        # Kept in memory so a save after a failed one writes the reviews back too.
        reviews = self.reviews.setdefault(round_.id, {})
        reviews.update(dict.fromkeys(seat_ids, "pending"))
        self._changed(round_)
        round_.notify()
        while True:
            changed = round_.changed
            if round_.cancelling or all(reviews[seat] != "pending" for seat in seat_ids):
                break
            await changed.wait()
        for seat in seat_ids:
            if reviews[seat] == "pending":
                # Cancelled or stopped before a decision: discarded, never published.
                reviews[seat] = "discard"
        return {seat for seat in seat_ids if reviews[seat] == "publish"}

    def _ready(self, round_: Round) -> str | None:
        return "review_needed" if "pending" in self.reviews.get(round_.id, {}).values() else None

    # Dashboard

    def open_dashboard(self, *, history_loaded: bool = False) -> str:
        if self.dashboard is None:
            self.dashboard = Dashboard()
            try:
                if history_loaded:
                    for record in list(self.records.values()):
                        self._publish(record, home=False)
                else:
                    self.scan()  # projects every readable conversation
            except OSError:
                self.dashboard.close()
                self.dashboard = None
                self.tokens.clear()
                raise
            self.refresher = asyncio.create_task(self._refresh_running())
        self._publish_home()
        return self.dashboard.url(self.home_token)

    def urls(self, conversation_id: str) -> tuple[str, str] | None:
        if self.dashboard is None or conversation_id not in self.tokens:
            return None
        public, director = self.tokens[conversation_id]
        return self.dashboard.url(public), self.dashboard.url(director)

    def _running(self, record: Record) -> bool:
        group = self._groups.get(record.state.id)
        return (group is not None and group.active is not None) or any(
            task.id in self.running_tasks for task in record.state.tasks
        )

    def _tails(self, record: Record) -> tuple[dict[str, str], dict[str, int]]:
        tails: dict[str, str] = {}
        elapsed: dict[str, int] = {}
        group = self._groups.get(record.state.id)
        if group is not None and group.active is not None:
            for run in group.active.runs:
                if run.status == "running" and run.invocation_id is not None:
                    tails[run.seat_id] = self.manager.recent_output(run.invocation_id)[0]
        for task in record.state.tasks:
            if task.id in self.running_tasks:
                invocation_id = self.running_tasks[task.id]
                tails[task.id] = self.manager.recent_output(invocation_id)[0]
                elapsed[task.id] = self.manager.snapshot(invocation_id).elapsed_ms
        return tails, elapsed

    def _publish(self, record: Record, *, home: bool = True) -> None:
        if self.live_writer is None and self._running(record):
            self.live_writer = asyncio.create_task(self._write_live())
        if self.dashboard is None:
            return
        state = record.state
        public, director = self.tokens.setdefault(state.id, (uuid4().hex, uuid4().hex))
        tails, elapsed = self._tails(record)
        if state.kind == "show":
            self.dashboard.update(public, views.public(state, stale=self.refresh_error is not None))
        self.dashboard.update(
            director,
            views.director(
                state,
                public_url=self.dashboard.url(public) if state.kind == "show" else "",
                home_url=self.dashboard.url(self.home_token),
                storage=str(record.directory),
                tails=tails,
                elapsed=elapsed,
                refresh_error=self.refresh_error,
            ),
        )
        if home:
            self._publish_home()

    def _unpublish(self, conversation_id: str) -> None:
        tokens = self.tokens.pop(conversation_id, ())
        if self.dashboard is not None:
            for token in tokens:
                self.dashboard.snapshots.pop(token, None)

    def _publish_home(self) -> None:
        if self.dashboard is None:
            return
        self.dashboard.update(
            self.home_token,
            views.home(
                [
                    (record.state, self.dashboard.url(self.tokens[record.state.id][1]))
                    for record in self.records.values()
                    if record.state.id in self.tokens
                ],
                self.unavailable,
                str(self.root),
                refresh_error=self.refresh_error,
            ),
        )

    async def _refresh_running(self) -> None:
        # Reply tails live only in memory, so running work is re-projected while it runs;
        # saved conversations are re-read now and then for other servers' changes.
        active = False
        for tick in itertools.count(1):
            await asyncio.sleep(1.5)
            if self.dashboard is None or time.monotonic() - self.dashboard.last_access > 10:
                active = False
                continue
            if not active or tick % 5 == 0:
                previous_error = self.refresh_error
                try:
                    self.scan()
                except OSError as exc:
                    self.refresh_error = str(exc)
                    if self.refresh_error != previous_error:
                        for record in list(self.records.values()):
                            self._publish(record, home=False)
                        self._publish_home()
            active = True
            for record in list(self.records.values()):
                if self._running(record):
                    self._publish(record, home=False)

    async def _write_live(self) -> None:
        """Keep live_path current while work runs; remove it once nothing runs."""
        try:
            self._prune_live()
            while running := [r for r in self.records.values() if self._running(r)]:
                conversations: dict[str, object] = {}
                for record in running:
                    tails, elapsed = self._tails(record)
                    conversations[record.state.id] = views.snapshot(
                        record.state, str(record.directory), tails=tails, elapsed=elapsed
                    )
                payload = {
                    "version": 1,
                    "pid": os.getpid(),
                    "updated_at": now(),
                    "host": self.host.name,
                    "host_source": self.host.source,
                    "manager_session": self.manager_session,
                    "manager_started_at": self.manager_started_at,
                }
                try:
                    self.live_path.parent.mkdir(mode=0o700, exist_ok=True)
                    write_file(self.live_path, _json(payload | {"conversations": conversations}))
                except OSError as exc:
                    print(f"agentnave-studio: could not write live views: {exc}", file=sys.stderr)
                await asyncio.sleep(_LIVE_SECONDS)
        finally:
            self.live_writer = None  # first, so a failed removal cannot stop the next writer
            try:
                self.live_path.unlink(missing_ok=True)
            except OSError as exc:
                print(f"agentnave-studio: could not remove live views: {exc}", file=sys.stderr)

    def _prune_live(self) -> None:
        # A server that crashed leaves its file; the tray ignores it, and it goes after an hour.
        with suppress(OSError):
            for path in self.live_path.parent.glob("*.json"):
                if time.time() - path.stat().st_mtime > _STALE_LIVE_SECONDS:
                    path.unlink(missing_ok=True)

    async def close(self) -> None:
        """Run after rounds and the invocation manager have stopped."""
        try:
            await asyncio.gather(*self.collectors, return_exceptions=True)
        finally:
            for task in (self.refresher, self.live_writer):
                if task is not None:
                    task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
            # Write each group's final state once more: an earlier failed save may have left
            # a stale running round or undecided review on disk.
            for group in {item.group.id: item.group for item in self._rounds.values()}.values():
                if group.id in self.records:
                    try:
                        self._save(group)
                    except OSError as exc:
                        print(
                            f"agentnave-studio: could not save {group.id}: {exc}", file=sys.stderr
                        )
            if self.dashboard is not None:
                self.dashboard.close()
            for record in self.records.values():
                record.release()


def _find(state: Conversation, round_id: str) -> RoundRecord | None:
    return next((round_ for round_ in state.rounds if round_.id == round_id), None)


def _prune_seats(seats: Path, antigravity: set[str]) -> None:
    """Remove the profiles this studio wrote and empty seat folders; keep any CLI files."""
    if not seats.is_dir() or seats.is_symlink():
        return
    for seat in seats.iterdir():
        if seat.is_symlink() or not seat.is_dir():
            continue
        if seat.name in antigravity:
            with suppress(OSError, ValueError):
                remove_restricted_profile(seat)
        # os.walk does not descend into symlinked folders, and rmdir never follows one.
        for directory, _, _ in sorted(os.walk(seat), key=lambda item: -len(item[0])):
            if not Path(directory).is_symlink():
                with suppress(OSError):
                    Path(directory).rmdir()
    with suppress(OSError):
        seats.rmdir()
