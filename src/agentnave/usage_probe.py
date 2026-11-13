"""Runs an Adapter's usage query under the process supervisor, bounded in time (ADR-0081).

The query starts the CLI the way a dispatch does, so the platform supervisor owns its process tree,
in an empty directory so it neither loads a project's configuration nor leaves a session there.
Any failure, including a timeout, a failed exit or an output nobody can read, means no reset time.
"""

from __future__ import annotations

import asyncio
import tempfile
from datetime import datetime
from pathlib import Path

from agentnave.adapters.base import UsageQuery
from agentnave.processes import SupervisedProcess, spawn_process, terminate_process_tree

QUERY_SECONDS = 20.0
_MAX_OUTPUT = 1 << 20
_DRAIN_SECONDS = 2.0


async def run_usage_query(query: UsageQuery, now: datetime) -> datetime | None:
    with tempfile.TemporaryDirectory(prefix="agentnave-usage-", ignore_cleanup_errors=True) as cwd:
        try:
            supervised = await spawn_process(query.argv, Path(cwd))
        except (OSError, ValueError):
            return None
        process = supervised.process
        if process.stdin is None or process.stdout is None or process.stderr is None:
            await _stop(supervised, [])
            return None
        output = bytearray()
        answered = asyncio.Event()
        readers = [
            asyncio.create_task(_read(process.stdout, query, output, answered)),
            asyncio.create_task(_discard(process.stderr)),
        ]
        try:
            ok = await asyncio.wait_for(_ask(supervised, query, answered), QUERY_SECONDS)
        except Exception:  # a timeout, a broken pipe, a supervisor that failed to report
            ok = False
        finally:
            await _stop(supervised, readers)
    if not ok:
        return None
    try:
        return query.parse(bytes(output), now)
    except Exception:  # an output nobody foresaw must not turn a quota failure into another one
        return None


async def _ask(supervised: SupervisedProcess, query: UsageQuery, answered: asyncio.Event) -> bool:
    """Whether the command answered: its output holds the answer, or it exited cleanly."""
    stdin = supervised.process.stdin
    assert stdin is not None
    if query.stdin is None:
        stdin.close()
    else:
        stdin.write(query.stdin)
        await stdin.drain()
    waiter = asyncio.create_task(answered.wait())
    try:
        await asyncio.wait({waiter, supervised.completion}, return_when=asyncio.FIRST_COMPLETED)
        if waiter.done():
            return True
        completion = supervised.completion.result()
        return completion.kind == "exit" and completion.returncode == 0
    finally:
        waiter.cancel()


async def _read(
    stream: asyncio.StreamReader, query: UsageQuery, output: bytearray, answered: asyncio.Event
) -> None:
    # Keeps consuming past the cap: a command that floods its pipe must not block being stopped.
    while chunk := await stream.read(65536):
        if len(output) <= _MAX_OUTPUT:
            output += chunk
        if query.complete is not None and query.complete(bytes(output)):
            answered.set()


async def _discard(stream: asyncio.StreamReader) -> None:
    while await stream.read(65536):
        pass


async def _stop(supervised: SupervisedProcess, readers: list[asyncio.Task[None]]) -> None:
    try:
        # The readers keep draining while the tree stops, then reach the closed pipes' end.
        await terminate_process_tree(supervised.process, 0.2)
        if readers:
            await asyncio.wait(readers, timeout=_DRAIN_SECONDS)
    finally:
        for task in readers:
            task.cancel()
        supervised.completion.cancel()
        await asyncio.gather(*readers, return_exceptions=True)
        await asyncio.gather(supervised.completion, return_exceptions=True)
