"""Runs an Adapter's usage query under the process supervisor, bounded in time (ADR-0081).

The query starts the CLI the way a dispatch does, so the platform supervisor owns its process tree,
in an empty directory so it neither loads a project's configuration nor leaves a session there.
Any failure, including a timeout, means no reset time.
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


async def run_usage_query(query: UsageQuery, now: datetime) -> datetime | None:
    with tempfile.TemporaryDirectory(prefix="agentnave-usage-", ignore_cleanup_errors=True) as cwd:
        try:
            supervised = await spawn_process(query.argv, Path(cwd))
        except (OSError, ValueError):
            return None
        try:
            output = await asyncio.wait_for(_collect(supervised, query), QUERY_SECONDS)
        except (TimeoutError, OSError):
            return None
        finally:
            await _stop(supervised)
    return query.parse(output, now)


async def _collect(supervised: SupervisedProcess, query: UsageQuery) -> bytes:
    process = supervised.process
    if process.stdin is None or process.stdout is None or process.stderr is None:
        raise OSError("usage query pipes were not created")
    output = bytearray()
    reader = asyncio.create_task(_read(process.stdout, query, output))
    drain = asyncio.create_task(_discard(process.stderr))
    try:
        if query.stdin is None:
            process.stdin.close()
        else:
            process.stdin.write(query.stdin)
            await process.stdin.drain()
        await asyncio.wait({reader, supervised.completion}, return_when=asyncio.FIRST_COMPLETED)
        if not reader.done():
            # The command exited, but its supervisor holds the pipes until the tree is stopped.
            await _stop(supervised)
            await reader
        return bytes(output)
    finally:
        reader.cancel()
        drain.cancel()


async def _read(stream: asyncio.StreamReader, query: UsageQuery, output: bytearray) -> None:
    while len(output) <= _MAX_OUTPUT and (chunk := await stream.read(65536)):
        output += chunk
        if query.complete is not None and query.complete(bytes(output)):
            return


async def _discard(stream: asyncio.StreamReader) -> None:
    while await stream.read(65536):
        pass


async def _stop(supervised: SupervisedProcess) -> None:
    try:
        await terminate_process_tree(supervised.process, 0.2)
    finally:
        supervised.completion.cancel()
        await asyncio.gather(supervised.completion, return_exceptions=True)
