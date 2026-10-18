"""Saved conversations: one state.json per directory, written atomically under an OS lock."""

from __future__ import annotations

import os
import sys
import tempfile
import time
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import IO, ClassVar, Literal

from pydantic import BaseModel, Field

from agentnave.group import RoundOrder
from agentnave.host import HostName, HostSource
from agentnave.models import ProviderOption

type Review = Literal["pending", "publish", "discard"]


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


class EntryRecord(BaseModel):
    round: int
    kind: Literal["task", "host", "answer", "absent"]
    seat: str | None = None
    text: str | None = None
    time: str
    manager_model: str | None = None


class SessionRecord(BaseModel):
    session_id: str | None = None
    cursor: int = 0


class SeatRunRecord(BaseModel):
    seat_id: str
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
    cwd: str
    prompt: str | None = None  # absent in tasks saved before v0.18.0
    session_id: str | None = None
    status: str = "running"
    output: str = ""
    error: str | None = None
    elapsed_ms: int = 0
    time: str
    manager_model: str | None = None


class Conversation(BaseModel):
    version: Literal[2] = 2
    id: str
    title: str
    kind: Literal["show", "task"]
    created: str = Field(default_factory=now)
    host: HostName | None = None
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
                            seat.review = "discard"  # stopped rounds discard undecided answers
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
                        time=now(),
                    )
                )
            elif seat.review != "discard":
                added.append(
                    EntryRecord(
                        round=round_.number,
                        kind="answer",
                        seat=seat.seat_id,
                        text=seat.output,
                        time=now(),
                    )
                )
        self.entries.extend(added)
        return added


class Record:
    """One conversation directory. Only the lock holder writes; others read.

    views.json beside state.json is the tray app's projection of the same state, written with
    every save and rewritten under the probe lock whenever it differs from the current
    projection (missing, stale, or a crashed writer's running work now shown as interrupted).
    """

    project: ClassVar[Callable[[Record], bytes] | None] = None

    def __init__(self, directory: Path, state: Conversation) -> None:
        self.directory = directory
        self.state = state
        self.saved = state.model_copy(deep=True)
        self.lock: IO[bytes] | None = None

    @classmethod
    def create(cls, directory: Path, state: Conversation) -> Record:
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        record = cls(directory, state)
        record.lock = record._lock()
        try:
            record.save()
        except OSError:
            record.release()
            raise
        return record

    @classmethod
    def read(cls, directory: Path) -> Record:
        return cls(directory, cls._load(directory))

    @staticmethod
    def _load(directory: Path) -> Conversation:
        return Conversation.model_validate_json((directory / "state.json").read_bytes())

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
            state = self._load(self.directory)
            if state.id != self.state.id:
                raise ValueError("Conversation record was replaced; list conversations again")
            state.recover()
            self.state = state
            self.save()
        except Exception:
            self.release()
            raise

    def refresh(self) -> bool:
        """Reload an unheld record and report whether this process could write it."""
        if self.lock is not None:
            return True
        try:
            lock = self._lock()
        except ValueError:
            self.state = self._load(self.directory)
            return False
        # Read while holding the probe so a writer that just exited leaves no stale running view.
        try:
            self.state = self._load(self.directory)
            self.state.recover()  # shown as interrupted; acquire() persists it
            self._write_views(only_if_changed=True)
        finally:
            self._unlock(lock)
        return True

    def save(self) -> None:
        name: str | None = None
        try:
            fd, name = tempfile.mkstemp(prefix=".state-", dir=self.directory)
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                stream.write(self.state.model_dump_json(indent=1))
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(name, self.directory / "state.json")
            self.saved = self.state.model_copy(deep=True)
        except OSError:
            # Memory must keep matching disk so a failed change is not applied.
            self.state = self.saved.model_copy(deep=True)
            raise
        finally:
            if name is not None:
                Path(name).unlink(missing_ok=True)
        self._write_views()

    def _write_views(self, *, only_if_changed: bool = False) -> None:
        if Record.project is None:
            return
        path = self.directory / "views.json"
        try:
            data = Record.project(self)
            # Compared by content: timestamps can tie or run backwards.
            if only_if_changed and path.is_file() and path.read_bytes() == data:
                return
            write_file(path, data)
        except OSError as exc:
            # Derived data: the saved state stands, and the next save writes the views again.
            print(
                f"agentnave: could not write views for {self.state.id}: {exc}",
                file=sys.stderr,
            )

    def release(self) -> None:
        if self.lock is not None:
            self.lock.close()
            self.lock = None

    def _lock(self) -> IO[bytes]:
        # An OS lock keeps two MCP hosts from writing one conversation at the same time.
        return lock_file(
            self.directory / ".lock",
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


def lock_file(path: Path, busy_message: str) -> IO[bytes]:
    """Acquire a nonblocking OS lock; close the returned handle to release it."""
    lock = path.open("a+b")
    try:
        if os.name == "nt":
            import msvcrt

            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lock.close()
        raise ValueError(busy_message) from None
    return lock


class Unavailable(BaseModel):
    path: str
    reason: str


def scan(root: Path) -> tuple[dict[str, Path], list[Unavailable]]:
    """Find saved conversations by id; report legacy or unreadable records without migrating."""
    found: dict[str, Path] = {}
    unavailable: list[Unavailable] = []
    if (root / "catalog").is_dir():
        unavailable.append(
            Unavailable(
                path=str(root / "catalog"),
                reason="legacy AgentNave history (before 0.15); not migrated, safe to clear",
            )
        )
    conversations = root / "conversations"
    if not conversations.is_dir():
        return found, unavailable
    for directory in sorted(conversations.iterdir()):
        if (directory / "state.json").is_file():
            found[directory.name] = directory
        elif (directory / "room.json").is_file():
            unavailable.append(
                Unavailable(
                    path=str(directory),
                    reason="legacy AgentNave history (before 0.15); not migrated, safe to clear",
                )
            )
    return found, unavailable
