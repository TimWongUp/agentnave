"""Provider processes kept after a turn for the next resume of the same session (ADR-0120).

A kept process still belongs to the supervisor that started it; the pool only decides when it
ends: when its keep period runs out, when it has served its session for its lifetime, when more
processes are kept than the limit allows (the one used longest ago goes first), when it exits on
its own, or when the service closes.
"""

from __future__ import annotations

import asyncio
import os
import time
from collections import OrderedDict
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass, field

from agentnave.adapters.base import PreparedCommand
from agentnave.processes import ProviderCompletion, SupervisedProcess, terminate_process_tree

KEEP_SECONDS = 180.0
MAX_KEPT = 3
# A kept process serves its session for at most this long after it started (ADR-0120).
MAX_LIFETIME_SECONDS = 900.0

type LaunchSignature = tuple[object, ...]


def _child_env(prepared: PreparedCommand) -> dict[str, str]:
    env = {name: value for name, value in os.environ.items() if name not in prepared.unset_env}
    return {**env, **prepared.extra_env}


def session_state(prepared: PreparedCommand, session_id: str | None) -> object:
    """The native session's stored state as the Adapter reads it, or None."""
    if prepared.session_state is None or session_id is None:
        return None
    return prepared.session_state(_child_env(prepared), session_id)


def launch_signature(
    prepared: PreparedCommand, account: str | None, session_id: str | None = None
) -> LaunchSignature:
    """What a reused process must have been started with, and the state of the session it
    serves: anything else needs a new one. The session state comes last (see KeptProcess)."""
    inputs = None
    if prepared.launch_inputs is not None:
        inputs = prepared.launch_inputs(_child_env(prepared))
    return (
        prepared.argv,
        str(prepared.cwd),
        tuple(sorted(prepared.unset_env)),
        tuple(sorted(prepared.extra_env.items())),
        account,
        inputs,
        session_state(prepared, session_id),
    )


def can_keep(prepared: PreparedCommand) -> bool:
    """Files and directories made for one run are removed when it ends, so such a run's process
    cannot outlive it."""
    return prepared.keep_alive and not prepared.cleanup_paths and prepared.temp_dir_env is None


@dataclass(slots=True)
class _TurnBytes:
    limit: int
    observe: Callable[[bytes], None]
    on_exceeded: Callable[[], None]
    data: bytearray = field(default_factory=bytearray)
    exceeded: bool = False

    def take(self, chunk: bytes) -> None:
        self.observe(chunk)
        remaining = self.limit - len(self.data)
        if remaining > 0:
            self.data.extend(chunk[:remaining])
        if len(chunk) > remaining and not self.exceeded:
            self.exceeded = True
            self.on_exceeded()


class StderrDrain:
    """Reads a process's stderr for its whole life, so an idle provider never blocks on a full
    pipe; only what it writes during a turn is retained, up to that turn's limit."""

    def __init__(self, stream: asyncio.StreamReader) -> None:
        self._turn: _TurnBytes | None = None
        self._task = asyncio.create_task(self._read(stream))

    async def _read(self, stream: asyncio.StreamReader) -> None:
        while chunk := await stream.read(64 * 1024):
            if self._turn is not None:
                self._turn.take(chunk)

    def begin(
        self, limit: int, observe: Callable[[bytes], None], on_exceeded: Callable[[], None]
    ) -> None:
        self._turn = _TurnBytes(limit, observe, on_exceeded)

    def end(self) -> tuple[bytes, bool]:
        turn, self._turn = self._turn, None
        return (bytes(turn.data), turn.exceeded) if turn is not None else (b"", False)

    async def finish(self) -> tuple[bytes, bool]:
        """The turn's bytes once the stream has closed; cancelling this leaves the reader."""
        await asyncio.wait({self._task})
        return self.end()

    async def close(self) -> None:
        self._task.cancel()
        await asyncio.gather(self._task, return_exceptions=True)


@dataclass(eq=False, slots=True)
class KeptProcess:
    supervised: SupervisedProcess
    signature: LaunchSignature
    stderr: StderrDrain
    prepared: PreparedCommand
    # The process's start, which the lifetime counts from, not this record's (ADR-0120).
    started: float = field(default_factory=time.monotonic)

    def remaining(self, lifetime: float) -> float:
        """Seconds the process may still serve its session for; it never gets a new lifetime."""
        return lifetime - (time.monotonic() - self.started)

    @classmethod
    def adopt(
        cls, supervised: SupervisedProcess, signature: LaunchSignature, prepared: PreparedCommand
    ) -> KeptProcess:
        stream = supervised.process.stderr
        if stream is None:
            raise RuntimeError("provider subprocess pipes were not created")
        return cls(supervised, signature, StderrDrain(stream), prepared)

    def settle(self, session_id: str) -> None:
        """Record the session's stored state as this process leaves it, so a resume finds out
        when something else (another service's one-off run) has continued the session since;
        what the process was started with stays as it was then."""
        self.signature = (*self.signature[:-1], session_state(self.prepared, session_id))

    def alive(self) -> bool:
        return self.supervised.process.returncode is None and not self.supervised.completion.done()

    async def end(self) -> None:
        """Stop the process tree through its supervisor, as any other run's cleanup does."""
        try:
            if self.supervised.process.returncode is None:
                await terminate_process_tree(
                    self.supervised.process, completion=self.supervised.completion
                )
        finally:
            await self.stderr.close()
            # A process kept and then ended has nobody left to collect its stdin writer.
            stdin = self.supervised.process.stdin
            if stdin is not None:
                stdin.close()
                with suppress(BrokenPipeError, ConnectionResetError):
                    await stdin.wait_closed()


@dataclass(slots=True)
class _Entry:
    kept: KeptProcess
    timer: asyncio.TimerHandle
    on_exit: Callable[[asyncio.Task[ProviderCompletion]], None]

    def release(self) -> None:
        self.timer.cancel()
        self.kept.supervised.completion.remove_done_callback(self.on_exit)


class WarmPool:
    """Kept processes by (provider, native session): one per session, at most `limit`."""

    def __init__(
        self,
        keep_seconds: float = KEEP_SECONDS,
        limit: int = MAX_KEPT,
        lifetime: float = MAX_LIFETIME_SECONDS,
    ) -> None:
        self.keep_seconds = keep_seconds
        self.limit = limit
        self.lifetime = lifetime
        # Least recently used first.
        self._entries: OrderedDict[tuple[str, str], _Entry] = OrderedDict()
        self._ending: dict[asyncio.Task[None], tuple[str, str]] = {}
        self._closed = False

    def __len__(self) -> int:
        return len(self._entries)

    def take(self, provider: str, session_id: str) -> KeptProcess | None:
        """Remove and return the session's kept process; its keep period stops."""
        entry = self._entries.pop((provider, session_id), None)
        if entry is None:
            return None
        entry.release()
        return entry.kept

    def serves(self, provider: str, session_id: str, signature: LaunchSignature) -> bool:
        """Whether a kept process started this way can serve the session's next turn: same
        signature, alive, and not past its lifetime (its timer may run late when the loop is
        busy)."""
        entry = self._entries.get((provider, session_id))
        return (
            entry is not None
            and entry.kept.signature == signature
            and entry.kept.remaining(self.lifetime) > 0
            and entry.kept.alive()
        )

    async def ended(self, provider: str, session_id: str) -> None:
        """Wait until no process of the session is still ending, so a new one never shares the
        native session with an old one on its way out."""
        key = (provider, session_id)
        ending = [task for task, owner in self._ending.items() if owner == key]
        if ending:
            await asyncio.gather(*ending, return_exceptions=True)

    async def release(self, provider: str, session_id: str) -> None:
        """End the session's kept process, if any, for a run that resumes it elsewhere."""
        kept = self.take(provider, session_id)
        if kept is not None:
            await kept.end()
        await self.ended(provider, session_id)

    def keep(self, provider: str, session_id: str, kept: KeptProcess) -> None:
        """Keep a process for its session's next turn, restarting its keep period; one past its
        lifetime ends instead, and the keep period never runs past the lifetime."""
        key = (provider, session_id)
        remaining = kept.remaining(self.lifetime)
        if self._closed or remaining <= 0:
            self._end(key, kept)
            return
        previous = self.take(provider, session_id)
        if previous is not None and previous is not kept:
            self._end(key, previous)

        def on_exit(_completion: asyncio.Task[ProviderCompletion]) -> None:
            self._expire(key, kept)

        timer = asyncio.get_running_loop().call_later(
            min(self.keep_seconds, remaining), self._expire, key, kept
        )
        kept.supervised.completion.add_done_callback(on_exit)
        self._entries[key] = _Entry(kept, timer, on_exit)
        while len(self._entries) > self.limit:
            oldest_key, oldest = self._entries.popitem(last=False)
            oldest.release()
            self._end(oldest_key, oldest.kept)

    async def close(self) -> None:
        """End every kept process and wait for all of them, including those already ending."""
        self._closed = True
        while self._entries:
            key, entry = self._entries.popitem(last=False)
            entry.release()
            self._end(key, entry.kept)
        await asyncio.gather(*self._ending, return_exceptions=True)

    def _expire(self, key: tuple[str, str], kept: KeptProcess) -> None:
        entry = self._entries.get(key)
        if entry is None or entry.kept is not kept:
            return
        del self._entries[key]
        entry.release()
        self._end(key, kept)

    def _end(self, key: tuple[str, str], kept: KeptProcess) -> None:
        task = asyncio.create_task(kept.end())
        self._ending[task] = key
        task.add_done_callback(lambda done: self._ending.pop(done, None))
