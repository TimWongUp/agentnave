"""Run blocking work on a worker thread without losing what it produced on cancellation."""

from __future__ import annotations

import asyncio
from collections.abc import Callable


async def wait_uninterrupted[T](work: asyncio.Future[T]) -> None:
    """Wait for work to finish even if this task is cancelled again meanwhile."""
    while not work.done():
        try:
            await asyncio.wait({work})
        except asyncio.CancelledError:
            continue


async def to_thread_keeping[T](
    discard: Callable[[T], None], func: Callable[..., T], *args: object, **kwargs: object
) -> T:
    """asyncio.to_thread, except that cancelling the waiter lets the thread finish first and then
    hands its result to discard (on a worker thread) before the cancellation propagates."""
    work = asyncio.ensure_future(asyncio.to_thread(func, *args, **kwargs))
    try:
        return await asyncio.shield(work)
    except asyncio.CancelledError:
        await wait_uninterrupted(work)
        if not work.cancelled() and work.exception() is None:
            await wait_uninterrupted(
                asyncio.ensure_future(asyncio.to_thread(discard, work.result()))
            )
        raise


async def to_thread_uninterrupted[T](func: Callable[..., T], *args: object) -> T:
    """asyncio.to_thread that always runs to completion, then re-raises any cancellation."""
    work = asyncio.ensure_future(asyncio.to_thread(func, *args))
    cancelled = False
    while not work.done():
        try:
            await asyncio.wait({work})
        except asyncio.CancelledError:
            cancelled = True
    result = work.result()
    if cancelled:
        raise asyncio.CancelledError
    return result
