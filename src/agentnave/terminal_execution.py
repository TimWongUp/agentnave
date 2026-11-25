"""Track one tray-owned terminal invocation without supervising its processes."""

from __future__ import annotations

import asyncio
import logging
import os
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast
from uuid import uuid4

from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.models import InvocationError, InvocationRequest, InvocationResult, InvocationStatus
from agentnave.process_info import read_process_started
from agentnave.push_capability import TrayIdentity
from agentnave.terminal_requests import publish_request, read_tray, receipt
from agentnave.terminal_turns import read_turn


@dataclass(frozen=True)
class TerminalLaunch:
    root: Path
    tray: TrayIdentity
    boss_key: str
    workspace_id: str
    account_dir: str | None
    isolation: dict[str, object] | None = None


def make_launch(
    root: Path,
    tray: TrayIdentity,
    boss: tuple[str, str] | None,
    workspace: str | None,
    request: InvocationRequest,
    info: TaskIsolationInfo | None,
) -> TerminalLaunch:
    if boss is None or workspace is None:
        raise ValueError("terminal dispatch requires a boss session")
    isolation: dict[str, object] | None = (
        None
        if info is None
        else {
            "id": info.id,
            "path": info.path,
            "repo_common_dir": str(info.repo_common_dir),
            "branch": info.branch,
        }
    )
    return TerminalLaunch(root, tray, boss[0], workspace, None, isolation)


def _object(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


async def execute_run(
    run: TerminalRun,
    settle_session: Callable[[InvocationResult], InvocationResult],
    reconcilers: set[asyncio.Task[None]],
    on_settled: Callable[[str, InvocationStatus], None] | None,
) -> InvocationResult:
    result = await run.run()
    reconciler = asyncio.create_task(run.reconcile_cancel())
    reconcilers.add(reconciler)
    reconciler.add_done_callback(reconcilers.discard)
    result = settle_session(result)
    if on_settled:
        try:
            await asyncio.shield(asyncio.to_thread(on_settled, run.invocation_id, result.status))
        except Exception:
            logging.getLogger(__name__).exception("Post-invocation settlement failed")
    return result


@dataclass
class TerminalRun:
    launch: TerminalLaunch
    request: InvocationRequest
    invocation_id: str
    prompt: str
    accepted: Callable[[dict[str, object]], None]
    started: Callable[[dict[str, object]], None]
    ack: Callable[[str, TrayIdentity], None]
    on_blocked: Callable[[str, str], None] | None = None
    stop: asyncio.Event = field(default_factory=asyncio.Event)
    cancel_requested: asyncio.Event = field(default_factory=asyncio.Event)
    cancel_result: asyncio.Future[str] | None = None
    anchor: dict[str, object] = field(default_factory=dict[str, object])
    session_id: str | None = None
    blocked: dict[str, object] | None = None
    terminal_finished: bool = False
    _bg_reminder_emitted: bool = False
    _ended: bool = False
    _cancel_id: str | None = None
    _service_started: float = field(default_factory=lambda: read_process_started(os.getpid()))
    _time: float = field(default_factory=time.monotonic)

    def _base(self, request_id: str, kind: str) -> dict[str, object]:
        return {
            "version": 1,
            "request_id": request_id,
            "kind": kind,
            "service": {"pid": os.getpid(), "started_at": self._service_started},
            "target_tray": {"pid": self.launch.tray.pid, "started_at": self.launch.tray.started_at},
            "invocation_id": self.invocation_id,
        }

    def _run_request(self) -> dict[str, object]:
        doc = self._base(self.invocation_id, "run")
        options = dict(self.request.provider_options)
        options.pop("interactive", None)
        doc.update(
            {
                "parent": {
                    "boss_key": self.launch.boss_key,
                    "workspace_id": self.launch.workspace_id,
                },
                "session_id": self.request.session_id,
                "config": {
                    "provider": self.request.provider,
                    "account": self.request.account,
                    "account_dir": self.launch.account_dir,
                    "cwd": str(self.request.cwd),
                    "isolation": self.launch.isolation,
                    "model": options.pop("model", None),
                    "effort": options.pop("effort", None),
                    "permissions": options,
                },
                "task": {"text": self.prompt, "forward": []},
            }
        )
        return doc

    def _send_cancel(self) -> None:
        cancel_id = str(uuid4())
        doc = self._base(cancel_id, "cancel")
        doc["expected_anchor"] = {"session_id": self.session_id, **self.anchor}
        publish_request(self.launch.root, cancel_id, doc)
        self._cancel_id = cancel_id
        self.cancel_requested.clear()

    async def cancel(self) -> str:
        if self._ended:
            return "already_finished"
        if self.cancel_result is None or self.cancel_result.done():
            self.cancel_result = asyncio.get_running_loop().create_future()
            self.cancel_requested.set()
        return await asyncio.shield(self.cancel_result)

    def _fail(self, code: str, message: str) -> InvocationResult:
        return InvocationResult(
            InvocationStatus.FAILED,
            self.request.provider,
            "",
            self.session_id,
            int((time.monotonic() - self._time) * 1000),
            InvocationError(code, message),
        )

    def _finish(self, item: dict[str, object]) -> InvocationResult:
        terminal = _object(item.get("terminal"))
        status = terminal.get("status")
        raw_warnings = terminal.get("warnings")
        warnings = (
            tuple(
                entry[:500]
                for entry in cast(list[object], raw_warnings)
                if isinstance(entry, str) and entry
            )
            if isinstance(raw_warnings, list)
            else ()
        )
        if status in ("interrupted", "cancelled"):
            return InvocationResult(
                InvocationStatus.CANCELLED,
                self.request.provider,
                "",
                self.session_id,
                int((time.monotonic() - self._time) * 1000),
                warnings=warnings,
            )
        if status not in ("completed", "succeeded"):
            return self._fail(
                "terminal_failed",
                str(terminal.get("reason") or status or "unknown terminal status")[:500],
            )
        if not self.session_id or not self.anchor:
            return self._fail(
                "missing_anchor", "Terminal did not provide this turn's session ID and anchor"
            )
        try:
            output = read_turn(
                self.request.provider, self.request.account, self.session_id, self.anchor
            )
        except (OSError, ValueError) as exc:
            return self._fail("terminal_read_failed", str(exc)[:500])
        return InvocationResult(
            InvocationStatus.SUCCEEDED,
            self.request.provider,
            output,
            self.session_id,
            int((time.monotonic() - self._time) * 1000),
            warnings=warnings,
        )

    def _emit_bg_reminder_if_present(self, item: dict[str, object]) -> None:
        if self._bg_reminder_emitted or self.on_blocked is None:
            return
        reminder = _object(item.get("background_tasks_pending"))
        if not reminder:
            blocked = _object(item.get("blocked"))
            if blocked.get("kind") == "background_tasks_pending":
                reminder = blocked
        if not reminder:
            return
        self._bg_reminder_emitted = True
        code = "background_tasks_pending"
        raw_reason = reminder.get("reason")
        if isinstance(raw_reason, str) and raw_reason.strip():
            message = raw_reason.strip()[:500]
        else:
            count = reminder.get("count")
            raw_tasks = reminder.get("tasks")
            tasks = (
                [t for t in cast(list[object], raw_tasks) if isinstance(t, str) and t][:16]
                if isinstance(raw_tasks, list)
                else []
            )
            if isinstance(count, int) and not isinstance(count, bool):
                detail = f" ({', '.join(tasks)})" if tasks else ""
                message = f"{count} background task(s) still pending{detail}."
            elif tasks:
                message = f"Background tasks still pending ({', '.join(tasks)})."
            else:
                message = "Background tasks are still pending in the derived terminal."
        self.on_blocked(code, message)

    def _cancel_receipt(self, snapshot: dict[str, object]) -> None:
        if not self._cancel_id:
            return
        item = receipt(snapshot, self._cancel_id, self.invocation_id)
        if (
            item is None
            or item.get("service_pid") != os.getpid()
            or item.get("state") != "finished"
        ):
            return
        result = _object(item.get("terminal")).get("status")
        outcome = (
            result
            if result in ("interrupted", "already_finished", "not_safe_to_interrupt")
            else "not_safe_to_interrupt"
        )
        self.ack(self._cancel_id, self.launch.tray)
        self._cancel_id = None
        if self.cancel_result is not None and not self.cancel_result.done():
            self.cancel_result.set_result(cast(str, outcome))

    async def reconcile_cancel(self) -> None:
        """A late cancel receipt can arrive after its target invocation has settled."""
        while self._cancel_id is not None and not self.stop.is_set():
            snapshot = read_tray(self.launch.root, self.launch.tray)
            if snapshot is None:
                return
            self._cancel_receipt(snapshot)
            await asyncio.sleep(0.5)

    async def run(self) -> InvocationResult:
        try:
            # Stopped or cancelled before the request went out (while a kept process of the
            # session ended): nothing was started, so nothing is published for the tray.
            if self.stop.is_set():
                return self._fail(
                    "service_stopped", "Service exited before the terminal invocation started"
                )
            if self.cancel_requested.is_set():
                return InvocationResult(
                    InvocationStatus.CANCELLED,
                    self.request.provider,
                    "",
                    self.session_id,
                    int((time.monotonic() - self._time) * 1000),
                )
            publish_request(self.launch.root, self.invocation_id, self._run_request())
            last_state = ""
            while not self.stop.is_set():
                snapshot = read_tray(self.launch.root, self.launch.tray)
                if snapshot is None:
                    return self._fail(
                        "delivery_unknown", "Tray unavailable; delivery may have occurred"
                    )
                self._cancel_receipt(snapshot)
                item = receipt(snapshot, self.invocation_id, self.invocation_id)
                if item is not None and item.get("service_pid") != os.getpid():
                    item = None
                if item is not None:
                    state = str(item.get("state"))
                    if state == "accepted" and not last_state:
                        self.accepted(item)
                        last_state = state
                    if state in ("started", "finished"):
                        session_id = item.get("session_id")
                        if isinstance(session_id, str):
                            if self.session_id is not None and self.session_id != session_id:
                                return self._fail(
                                    "receipt_changed", "Native session changed during invocation"
                                )
                            self.session_id = session_id
                        new_anchor = _object(item.get("anchor"))
                        if any(
                            key in self.anchor and self.anchor[key] != value
                            for key, value in new_anchor.items()
                        ):
                            return self._fail(
                                "receipt_changed", "Native turn anchor changed during invocation"
                            )
                        self.anchor.update(new_anchor)
                        blocked = _object(item.get("blocked"))
                        reason = blocked.get("reason")
                        if isinstance(reason, str):
                            blocked = {**blocked, "reason": reason[:500]}
                        self.blocked = blocked or None
                        if state == "started":
                            self.started(item)
                            self._emit_bg_reminder_if_present(item)
                        last_state = state
                    if state == "finished":
                        self.terminal_finished = True
                        return await asyncio.to_thread(self._finish, item)
                if self.cancel_requested.is_set() and self._cancel_id is None:
                    self._send_cancel()
                await asyncio.sleep(0.1)
            return self._fail(
                "service_stopped", "Service exited while terminal invocation was in flight"
            )
        except (OSError, ValueError) as exc:
            return self._fail("terminal_transport_error", str(exc)[:500])
        finally:
            self._ended = True
            if self.cancel_result is not None and not self.cancel_result.done():
                self.cancel_result.set_result("already_finished")
