"""Saved conversation records and their single-writer OS lock (ADR-0026)."""

from __future__ import annotations

import os
import sys
import tempfile
import time
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import IO, TYPE_CHECKING, ClassVar, Literal

from pydantic import BaseModel, Field

from agentnave.core import lock_file
from agentnave.group import RoundOrder
from agentnave.host import HostName, HostSource, RetiredHostName
from agentnave.models import ProviderOption

if TYPE_CHECKING:
    from agentnave.db import Database

type Review = Literal["pending", "publish", "private"]


def now() -> str:
    return datetime.now(UTC).isoformat()


def write_file(path: Path, data: bytes) -> None:
    """Replace path atomically; readers see the old or the new bytes, never a partial file."""
    name: str | None = None
    try:
        fd, name = tempfile.mkstemp(prefix=f".{path.stem}-", dir=path.parent)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
        os.replace(name, path)
        name = None
    finally:
        if name is not None:
            Path(name).unlink(missing_ok=True)


class SeatRecord(BaseModel):
    id: str
    provider: str
    provider_options: dict[str, ProviderOption] = Field(default_factory=lambda: {})
    cwd: str
    role: str | None = None
    access: Literal["read", "write"] | None = None  # declared, never enforced


class EntryRecord(BaseModel):
    round: int
    kind: Literal["task", "host", "answer", "absent", "forward"]
    seat: str | None = None
    text: str | None = None
    time: str
    manager_model: str | None = None
    manager_reported_model: str | None = None
    reported_model: str | None = None
    # Set only on forward entries: the referenced workspace message and its origin.
    forward_id: str | None = None
    forward_source: str | None = None


class SessionRecord(BaseModel):
    session_id: str | None = None
    cursor: int = 0


class SeatRunRecord(BaseModel):
    seat_id: str
    reported_model: str | None = None
    instruction: str | None = None
    status: Literal[
        "pending",
        "running",
        "succeeded",
        "failed",
        "blocked",
        "cancelled",
        "not_started",
        "interrupted",
    ] = "pending"
    output: str | None = None
    error: str | None = None
    # When the seat's result was first saved.
    finished: str | None = None
    # Set only in review conversations, once the answer succeeded.
    review: Review | None = None
    committed: bool = False


class RoundRecord(BaseModel):
    id: str
    number: int
    order: RoundOrder
    status: Literal["running", "finished", "failed", "cancelled", "interrupted"] = "running"
    seats: list[SeatRunRecord]
    error: str | None = None

    def pending_review(self) -> bool:
        return any(seat.review == "pending" for seat in self.seats)


class TaskRecord(BaseModel):
    id: str
    provider: str
    model: str
    reported_model: str | None = None
    cwd: str
    prompt: str
    session_id: str | None = None
    status: str = "running"
    output: str = ""
    error: str | None = None
    elapsed_ms: int = 0
    time: str
    manager_model: str | None = None
    manager_reported_model: str | None = None
    role: str | None = None
    access: Literal["read", "write"] | None = None  # declared, never enforced
    # Message ids this task had injected into its prompt; the saved prompt stays the manager's.
    forward: list[str] = Field(default_factory=lambda: [])


class Conversation(BaseModel):
    id: str
    title: str
    kind: Literal["show", "task"]
    created: str = Field(default_factory=now)
    host: HostName | RetiredHostName | None = None
    host_source: HostSource = "unknown"
    manager_session: str | None = None
    cwd: str | None = None
    review: bool = False
    isolated: bool = False
    seats: list[SeatRecord] = Field(default_factory=lambda: [])
    entries: list[EntryRecord] = Field(default_factory=lambda: [])
    sessions: dict[str, SessionRecord] = Field(default_factory=lambda: {})
    rounds: list[RoundRecord] = Field(default_factory=lambda: [])
    tasks: list[TaskRecord] = Field(default_factory=lambda: [])

    def pending_review(self) -> bool:
        return any(round_.pending_review() for round_ in self.rounds)

    def recover(self) -> None:
        """Mark work left running by an earlier writer as interrupted; never resume it."""
        for round_ in self.rounds:
            if round_.status == "running":
                round_.status = "interrupted"
            for seat in round_.seats:
                if seat.status == "running":
                    # The launch may have changed the native session: start a fresh one.
                    seat.status = "interrupted"
                    self.sessions[seat.seat_id] = SessionRecord()
                elif seat.status == "pending":
                    seat.status = "not_started"
                elif self.review and seat.status == "succeeded" and not seat.committed:
                    if round_.status in ("cancelled", "failed"):
                        if seat.review in (None, "pending"):
                            seat.review = (
                                "private"  # stopped rounds never publish undecided answers
                            )
                    elif seat.review is None:
                        seat.review = "pending"  # ended before its review began
            if not round_.pending_review():
                self.commit_ended(round_)
        for task in self.tasks:
            if task.status == "running":
                task.status = "interrupted"
                task.error = "server_restarted; previous invocation cannot resume"

    def commit_ended(self, round_: RoundRecord) -> list[EntryRecord]:
        """Commit what a stopped round left uncommitted, in seat order; call once no seat
        awaits review."""
        added: list[EntryRecord] = []
        for seat in round_.seats:
            if seat.committed or seat.status in ("pending", "running", "not_started"):
                continue
            seat.committed = True
            if seat.status != "succeeded":
                added.append(
                    EntryRecord(
                        round=round_.number,
                        kind="absent",
                        seat=seat.seat_id,
                        text=seat.status,
                        reported_model=seat.reported_model,
                        time=seat.finished or now(),
                    )
                )
            elif seat.review != "private":
                added.append(
                    EntryRecord(
                        round=round_.number,
                        kind="answer",
                        seat=seat.seat_id,
                        text=seat.output,
                        reported_model=seat.reported_model,
                        time=seat.finished or now(),
                    )
                )
        self.entries.extend(added)
        return added


class Record:
    """One saved conversation. Only the holder of its OS lock writes; others read.

    Its views row is the tray app's projection of the same state, written with every save and
    rewritten under the probe lock whenever it may differ from the current projection (first
    read, another writer's change, or a crashed writer's running work now shown as interrupted).
    """

    project: ClassVar[Callable[[Record], bytes] | None] = None

    def __init__(self, db: Database, locks: Path, state: Conversation, rev: int) -> None:
        self.db = db
        self.locks = locks
        self.state = state
        # What a failed save rolls back to, kept only while this process holds the lock.
        self.saved: Conversation | None = None
        # What the database holds, the base of the next save's changes; held like saved.
        self.stored: Conversation | None = None
        self.rev = rev  # the database revision behind self.state
        # The revision whose views row matched the projection; None to check it again.
        self.view_rev: int | None = None
        # Another server held it at the last refresh: recover once the lock comes free.
        self.contended = False
        self.lock: IO[bytes] | None = None

    @classmethod
    def create(cls, db: Database, locks: Path, state: Conversation) -> Record:
        record = cls(db, locks, state, 0)
        record.lock = record._lock()
        try:
            record.rev = db.save(None, state, record._projection())
        except OSError:
            record.release()
            raise
        record.saved = state.model_copy(deep=True)
        record.stored = state.model_copy(deep=True)
        record.view_rev = record.rev
        return record

    @classmethod
    def read(cls, db: Database, locks: Path, conversation_id: str) -> Record:
        state, rev = db.load(conversation_id)
        return cls(db, locks, state, rev)

    def acquire(self) -> None:
        """Become the single writer until release, reloading what an earlier writer saved."""
        if self.lock is not None:
            return
        try:
            self.lock = self._lock()
        except ValueError:
            # A reader briefly holds the same probe lock; allow one bounded handoff.
            time.sleep(0.05)
            self.lock = self._lock()
        try:
            state, self.rev = self.db.load(self.state.id)
            self.saved = state.model_copy(deep=True)
            self.stored = state.model_copy(deep=True)
            state.recover()
            self.state = state
            self.contended = False
            if state != self.saved:
                self.save()  # persist what recovery changed
            elif self.view_rev != self.rev:
                self._write_view()
        except Exception:
            self.release()
            raise

    def refresh(self) -> bool:
        """Reload an unheld record and report whether this process could write it.

        An unchanged revision is not read again, unless another server held it at the last
        refresh and has since let go.
        """
        if self.lock is not None:
            return True
        try:
            lock = self._lock()
        except ValueError:
            self._reload()
            self.contended = True
            return False
        # Read while holding the probe so a writer that just exited leaves no stale running view.
        try:
            if self._reload() or self.contended or self.view_rev != self.rev:
                self.contended = False
                self.state.recover()  # shown as interrupted; acquire() persists it
                self._write_view()
        finally:
            self._unlock(lock)
        return True

    def _reload(self) -> bool:
        rev = self.db.revision(self.state.id)
        if rev is None:
            raise FileNotFoundError(self.state.id)
        if rev == self.rev:
            return False
        self.state, self.rev = self.db.load(self.state.id)
        return True

    def save(self) -> None:
        assert self.saved is not None and self.stored is not None, "only the lock holder saves"
        try:
            self.rev = self.db.save(self.stored, self.state, self._projection())
        except OSError:
            # Memory must keep matching the database so a failed change is not applied.
            self.state = self.saved.model_copy(deep=True)
            raise
        self.saved = self.state.model_copy(deep=True)
        self.stored = self.state.model_copy(deep=True)
        self.view_rev = self.rev

    def _projection(self) -> bytes | None:
        return None if Record.project is None else Record.project(self)

    def _write_view(self) -> None:
        view = self._projection()
        if view is None:
            return
        try:
            self.db.save_view(self.state.id, view)
            self.view_rev = self.rev
        except OSError as exc:
            # Derived data: the saved state stands; the next save or scan writes the view again.
            self.view_rev = None
            print(
                f"agentnave: could not write views for {self.state.id}: {exc}",
                file=sys.stderr,
            )

    def release(self) -> None:
        if self.lock is not None:
            self.lock.close()
            self.lock = None
        self.saved = None
        self.stored = None

    def _lock(self) -> IO[bytes]:
        # An OS lock keeps two MCP hosts from writing one conversation at the same time.
        self.locks.mkdir(parents=True, mode=0o700, exist_ok=True)
        return lock_file(
            self.locks / f"{self.state.id}.lock",
            "Conversation is held by another AgentNave server; continue it from that "
            "host or once it becomes idle",
        )

    @staticmethod
    def _unlock(lock: IO[bytes]) -> None:
        if os.name == "nt":
            import msvcrt

            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
        lock.close()


class Unavailable(BaseModel):
    path: str
    reason: str
