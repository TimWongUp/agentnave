"""MCP Tasks (`io.modelcontextprotocol/tasks`, 2026-07-28): `wait` as a lazily collected
background task (ADR-0131).

Only `wait` becomes a task, and only when the request declares the extension. A task watches
its handles without consuming anything; a `tasks/get` that finds something ready collects it
exactly as a blocking wait would return it, so cancelling, abandoning or losing a task never
loses a result. Tasks live in this server process only.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import time
from collections.abc import Callable, Sequence
from contextvars import ContextVar
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any, Literal
from uuid import uuid4

from mcp.server.context import CallNext, HandlerResult, ServerRequestContext
from mcp.server.extension import Extension, MethodBinding
from mcp.server.mcpserver.exceptions import ToolError
from mcp.shared.exceptions import MCPError
from mcp_types import INTERNAL_ERROR, INVALID_PARAMS, CallToolRequestParams, RequestParams
from pydantic import Field

from agentnave.mcp_projection import without_empty_fields
from agentnave.wait_task_status import scan_wait, status_message, ui_language

if TYPE_CHECKING:
    from mcp.server.mcpserver.context import Context

    from agentnave.mcp_payloads import WaitPayload
    from agentnave.mcp_runtime import Runtime
    from agentnave.mcp_tasks import BlockingWaitKeys

EXTENSION = "io.modelcontextprotocol/tasks"
TASKS_PROTOCOL = "2026-07-28"
POLL_INTERVAL_MS = 2000
# No tasks/get for this long: the host stopped polling, so the inbox is not held for it.
LEASE_SECONDS = 60.0
# A task is completed by the first tasks/get after this, ready or not, so none waits forever.
ORPHAN_SECONDS = 2 * 60 * 60.0
# Ended tasks answer repeated polls this long, then their ids are unknown.
KEEP_ENDED_SECONDS = 10 * 60.0
_TICK_SECONDS = 5.0

type TaskStatus = Literal["working", "completed", "failed", "cancelled"]

_log = logging.getLogger(__name__)


@dataclass(frozen=True)
class WaitArguments:
    """The validated arguments of the wait call a task finishes."""

    ids: list[str] | None
    include_output: bool
    wait_seconds: Literal[270, 1500] | None
    manager_session_id: str | None
    seq: int | None


@dataclass
class WaitTaskRequest:
    """One tools/call from a host that accepts tasks; wait fills `task` when it starts one."""

    tool: str
    task: WaitTask | None = None


_REQUEST: ContextVar[WaitTaskRequest | None] = ContextVar(
    "agentnave_wait_task_request", default=None
)


def wait_task_request() -> WaitTaskRequest | None:
    """The task slot of a wait tool call that accepts tasks; None otherwise, including the wait
    a dispatch runs with wait=true, which stays synchronous."""
    request = _REQUEST.get()
    return request if request is not None and request.tool == "wait" else None


def wait_tasks_active() -> bool:
    """Whether receipt guidance is written for a host that accepts wait tasks."""
    return _REQUEST.get() is not None


def _now_iso() -> str:
    return datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _tool_result(payload: WaitPayload) -> dict[str, Any]:
    """The CallToolResult a synchronous wait would have returned (see _Server.call_tool)."""
    projected = without_empty_fields(payload)
    text = json.dumps(projected, ensure_ascii=False, separators=(",", ":"))
    return {
        "content": [{"type": "text", "text": text}],
        "structuredContent": projected,
        "isError": False,
    }


def _tool_error(message: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": message}], "isError": True}


@dataclass(eq=False)
class WaitTask:
    """A wait running in the background for one host request."""

    arguments: WaitArguments
    ctx: Context[Runtime]
    keys: BlockingWaitKeys
    task_id: str = field(default_factory=lambda: uuid4().hex)
    status: TaskStatus = "working"
    status_message: str = ""
    created_at: str = field(default_factory=_now_iso)
    updated_at: str = field(default_factory=_now_iso)
    started: float = field(default_factory=time.monotonic)
    last_poll: float = field(default_factory=time.monotonic)
    ended: float | None = None
    ready: bool = False
    result: dict[str, Any] | None = None
    error: dict[str, Any] | None = None
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    changed: asyncio.Event = field(default_factory=asyncio.Event)
    watcher: asyncio.Task[None] | None = None
    _release: Callable[[], None] | None = None

    # Inbox registration (already_waiting, tray badge)

    def hold(self) -> None:
        from agentnave import mcp_tasks

        if self._release is None:
            self._release = mcp_tasks.hold_blocking_wait(self.ctx, self.keys)

    def release(self) -> None:
        release, self._release = self._release, None
        if release is not None:
            release()

    def lease_live(self, now: float | None = None) -> bool:
        current = time.monotonic() if now is None else now
        return current - self.last_poll <= LEASE_SECONDS

    def overdue(self) -> bool:
        return time.monotonic() - self.started >= ORPHAN_SECONDS

    def polled(self) -> None:
        """A tasks/get renews the lease; a lapsed one holds the inbox again if it is free."""
        from agentnave import mcp_tasks

        self.last_poll = time.monotonic()
        if (
            self.status == "working"
            and self._release is None
            and not mcp_tasks.inbox_wait_active(self.ctx, self.keys)
        ):
            self.hold()

    def check_lease(self) -> None:
        if self._release is not None and not self.lease_live():
            self.release()

    # Watching and collecting

    def refresh(self) -> None:
        """Re-read readiness and the status line; never consumes anything."""
        conversations = self.ctx.request_context.lifespan_context.conversations
        found = scan_wait(conversations, self.arguments.ids, self.arguments.manager_session_id)
        self.ready = found.ready
        message = status_message(found, ui_language())
        if message != self.status_message:
            self.status_message = message
            self.updated_at = _now_iso()

    async def collect(self) -> None:
        """Receive what is ready as the blocking wait would return it, or keep waiting."""
        from agentnave import mcp_tasks

        arguments = self.arguments
        self.release()
        token = _REQUEST.set(WaitTaskRequest("tasks/get"))  # guidance for a tasks host
        try:
            payload = await mcp_tasks.probe_wait(
                arguments.ids,
                arguments.include_output,
                arguments.wait_seconds,
                arguments.manager_session_id,
                self.ctx,
            )
            if payload["reason"] == "wait_elapsed" and not self.overdue():
                self.ready = False
                self.hold()
                return
            payload = await mcp_tasks.finish_wait(
                payload,
                ids=arguments.ids,
                seq=arguments.seq,
                manager_session_id=arguments.manager_session_id,
                ctx=self.ctx,
            )
            self.end("completed", result=_tool_result(payload))
        except ToolError as exc:
            self.end("completed", result=_tool_error(str(exc)))
        except Exception:
            _log.exception("wait task %s failed to collect", self.task_id)
            self.end(
                "failed",
                error={"code": INTERNAL_ERROR, "message": "Collecting the wait results failed."},
            )
        finally:
            _REQUEST.reset(token)

    def end(
        self,
        status: TaskStatus,
        *,
        result: dict[str, Any] | None = None,
        error: dict[str, Any] | None = None,
    ) -> None:
        self.release()
        self.status, self.result, self.error = status, result, error
        self.ended = time.monotonic()
        self.updated_at = _now_iso()
        self.changed.set()  # ends the watcher

    def wire(self) -> dict[str, Any]:
        """The Task object of tasks/get and of the tools/call that created it."""
        data: dict[str, Any] = {
            "taskId": self.task_id,
            "status": self.status,
            "statusMessage": self.status_message,
            "createdAt": self.created_at,
            "lastUpdatedAt": self.updated_at,
            "ttlMs": None,
            "pollIntervalMs": POLL_INTERVAL_MS,
        }
        if self.result is not None:
            data["result"] = self.result
        if self.error is not None:
            data["error"] = self.error
        return data


class WaitTaskStore:
    """This server's wait tasks, kept in memory only."""

    def __init__(self) -> None:
        self._tasks: dict[str, WaitTask] = {}

    def start(self, task: WaitTask) -> WaitTask:
        self._prune()
        task.hold()
        task.refresh()
        self._tasks[task.task_id] = task
        task.watcher = asyncio.get_running_loop().create_task(self._watch(task))
        return task

    def notify(self) -> None:
        """The inbox changed: let every watcher re-read readiness."""
        for task in self._tasks.values():
            if task.status == "working":
                task.changed.set()

    def restart_safe(self) -> bool:
        """A new server would not know a task the host is still polling."""
        now = time.monotonic()
        return not any(
            task.status == "working" and task.lease_live(now) for task in self._tasks.values()
        )

    def lookup(self, task_id: str) -> WaitTask:
        self._prune()
        task = self._tasks.get(task_id)
        if task is None:
            raise MCPError(
                code=INVALID_PARAMS,
                message=(
                    f"Unknown or expired AgentNave wait task {task_id!r}. Call wait again (omit "
                    "ids, same manager_session_id) to collect results."
                ),
                data={"taskId": task_id},
            )
        return task

    async def get(self, task_id: str) -> dict[str, Any]:
        task = self.lookup(task_id)
        async with task.lock:
            if task.status == "working":
                task.polled()
                task.refresh()
                if task.ready or task.overdue():
                    await task.collect()
        return task.wire()

    async def cancel(self, task_id: str) -> dict[str, Any]:
        task = self.lookup(task_id)
        async with task.lock:
            if task.status == "working":
                # Stop waiting only: nothing is consumed and no CLI is stopped.
                task.end("cancelled")
        return {}

    def close(self) -> None:
        for task in self._tasks.values():
            task.release()
            if task.watcher is not None:
                task.watcher.cancel()
        self._tasks.clear()

    def _prune(self) -> None:
        cutoff = time.monotonic() - KEEP_ENDED_SECONDS
        for task_id, task in list(self._tasks.items()):
            if task.ended is not None and task.ended < cutoff:
                del self._tasks[task_id]

    async def _watch(self, task: WaitTask) -> None:
        while task.status == "working":
            with contextlib.suppress(TimeoutError):
                await asyncio.wait_for(task.changed.wait(), _TICK_SECONDS)
            task.changed.clear()
            if task.status != "working":
                return
            if task.lock.locked():
                continue  # a tasks/get is collecting right now
            try:
                task.refresh()
            except Exception:
                _log.warning("wait task %s could not refresh", task.task_id, exc_info=True)
            task.check_lease()
            if task.overdue() and not task.lease_live():
                # Nobody polls it any more: forget it, its results stay in the inbox.
                task.release()
                self._tasks.pop(task.task_id, None)
                return


def start_wait_task(ctx: Context[Runtime], arguments: WaitArguments) -> WaitTask:
    """Register a wait task for a wait whose first pass found nothing ready."""
    from agentnave import mcp_tasks

    keys = mcp_tasks.blocking_wait_keys(ctx, arguments.ids, arguments.manager_session_id)
    store = ctx.request_context.lifespan_context.wait_tasks
    return store.start(WaitTask(arguments, ctx, keys))


# Protocol


class TaskIdParams(RequestParams):
    task_id: str = Field(alias="taskId", min_length=1, max_length=128)


def _store(ctx: ServerRequestContext[Any, Any]) -> WaitTaskStore:
    return ctx.lifespan_context.wait_tasks


def _accepts_tasks(ctx: ServerRequestContext[Any, Any]) -> bool:
    """2026-07-28, this request declares the extension, and not a T3 thread (ADR-0121)."""
    from agentnave import mcp_runtime
    from agentnave.t3 import current_t3_mode

    if ctx.protocol_version != TASKS_PROTOCOL:
        return False
    capabilities = ctx.session.client_capabilities
    extensions = capabilities.extensions if capabilities is not None else None
    if not extensions or EXTENSION not in extensions:
        return False
    return current_t3_mode(mcp_runtime.HOST) == "none"


class WaitTasksExtension(Extension):
    """Serves tasks/get and tasks/cancel and turns an eligible wait into a task."""

    identifier = EXTENSION

    def methods(self) -> Sequence[MethodBinding]:
        versions = frozenset({TASKS_PROTOCOL})
        return (
            MethodBinding("tasks/get", TaskIdParams, self._get, versions),
            MethodBinding("tasks/cancel", TaskIdParams, self._cancel, versions),
        )

    async def intercept_tool_call(
        self,
        params: CallToolRequestParams,
        ctx: ServerRequestContext[Any, Any],
        call_next: CallNext,
    ) -> HandlerResult:
        if not _accepts_tasks(ctx):
            return await call_next(ctx)
        request = WaitTaskRequest(params.name)
        token = _REQUEST.set(request)
        try:
            result = await call_next(ctx)
        finally:
            _REQUEST.reset(token)
        if request.task is not None:
            return {"resultType": "task", **request.task.wire()}
        return result

    async def _get(
        self, ctx: ServerRequestContext[Any, Any], params: TaskIdParams
    ) -> HandlerResult:
        return await _store(ctx).get(params.task_id)

    async def _cancel(
        self, ctx: ServerRequestContext[Any, Any], params: TaskIdParams
    ) -> HandlerResult:
        return await _store(ctx).cancel(params.task_id)
