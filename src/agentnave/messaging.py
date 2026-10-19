"""Native input and acknowledgements, scoped to one invocation's process."""

from __future__ import annotations

import asyncio
import json
from typing import Literal, NotRequired, TypedDict, cast

from agentnave.adapters.base import object_dict
from agentnave.models import InvocationError, InvocationRequest, NativeTurnResult

type MessageMode = Literal["steer", "queue"]

type MessageStatus = Literal["accepted", "rejected", "unsupported", "not_delivered", "unknown"]


class MessageReceipt(TypedDict):
    invocation_id: str
    message_id: str
    status: MessageStatus
    reason: str
    session_id: str | None
    history_saved: NotRequired[bool]
    mode: NotRequired[MessageMode]


def rpc_line(payload: dict[str, object]) -> bytes:
    return (json.dumps(payload, ensure_ascii=False) + "\n").encode()


class PiRpcChannel:
    """A native success response acknowledges queuing, never execution or completion."""

    modes: tuple[MessageMode, ...] = ("steer",)

    def __init__(self) -> None:
        self.finished = asyncio.Event()
        self.active = False
        self.session_id: str | None = None
        self.pending: dict[str, asyncio.Future[tuple[MessageStatus, str]]] = {}
        self.error: InvocationError | None = None
        self.turns: list[NativeTurnResult] = []

    def bind(self, writer: asyncio.StreamWriter) -> None:
        self.writer = writer

    def message(self, message_id: str, text: str, mode: MessageMode = "steer") -> bytes:
        return rpc_line({"id": message_id, "type": "steer", "message": text})

    def activity_event(self, event: dict[str, object]) -> dict[str, object]:
        return event

    def accepts_event(self, event: dict[str, object]) -> bool:
        return True

    def observe(self, event: dict[str, object]) -> None:
        kind = event.get("type")
        if kind == "agent_start" and not self.finished.is_set():
            self.active = True
        elif kind == "agent_settled":
            self.active = False
            self.finished.set()
        elif kind == "response":
            command = event.get("command")
            if event.get("id") == "agentnave-state" and command == "get_state":
                session_id = object_dict(event.get("data")).get("sessionId")
                if isinstance(session_id, str) and session_id:
                    self.session_id = session_id
            elif event.get("id") == "agentnave-prompt" and command == "prompt":
                if (
                    event.get("success") is not True
                    or object_dict(event.get("data")).get("disposition") == "handled"
                ):
                    self.active = False
                    self.finished.set()
            elif command == "steer":
                message_id = event.get("id")
                future = self.pending.get(message_id) if isinstance(message_id, str) else None
                if future is not None and not future.done():
                    disposition = object_dict(event.get("data")).get("disposition")
                    if event.get("success") is True and disposition in ("queued", "handled"):
                        reason = (
                            "queued_by_provider"
                            if disposition == "queued"
                            else "handled_by_provider_extension"
                        )
                        future.set_result(("accepted", reason))
                    elif event.get("success") is False:
                        future.set_result(("rejected", "rejected_by_provider"))
                    else:
                        future.set_result(("unknown", "unrecognized_provider_ack"))

    def close(self) -> None:
        self.active = False
        for future in self.pending.values():
            if not future.done():
                future.set_result(("unknown", "invocation_stopped_before_ack"))


def codex_event(event: dict[str, object]) -> dict[str, object]:
    """Normalize public app-server events to the existing exec result contract."""
    params = object_dict(event.get("params"))
    method = event.get("method")
    if event.get("id") == "agentnave-thread" and "result" in event:
        return {
            "type": "thread.started",
            "thread_id": object_dict(object_dict(event["result"]).get("thread")).get("id"),
        }
    if method == "turn/started":
        return {"type": "turn.started"}
    if method in ("item/started", "item/completed"):
        item = object_dict(params.get("item"))
        kinds = {
            "agentMessage": "agent_message",
            "commandExecution": "command_execution",
            "fileChange": "file_change",
            "mcpToolCall": "mcp_tool_call",
            "webSearch": "web_search",
            "reasoning": "reasoning",
        }
        return {
            "type": "item.started" if method == "item/started" else "item.completed",
            "item": {
                "type": kinds.get(str(item.get("type")), "unknown"),
                "id": item.get("id"),
                "text": item.get("text"),
                "status": item.get("status"),
            },
        }
    if method == "turn/completed":
        turn = object_dict(params.get("turn"))
        return {
            "type": "turn.completed" if turn.get("status") == "completed" else "turn.failed",
            "error": turn.get("error"),
        }
    if method == "error":
        return {"type": "error", "message": object_dict(params.get("error")).get("message")}
    if "error" in event and str(event.get("id", "")).startswith("agentnave-"):
        return {
            "type": "turn.failed",
            "error": {"message": "Codex app-server rejected initialization or turn request"},
        }
    return {}


class CodexRpcChannel(PiRpcChannel):
    def __init__(self, request: InvocationRequest) -> None:
        super().__init__()
        self.request = request
        self.turn_id: str | None = None

    def _write(self, message_id: str, method: str, params: dict[str, object]) -> None:
        self.writer.write(rpc_line({"id": message_id, "method": method, "params": params}))

    def message(self, message_id: str, text: str, mode: MessageMode = "steer") -> bytes:
        return rpc_line(
            {
                "id": message_id,
                "method": "turn/steer",
                "params": {
                    "threadId": self.session_id,
                    "expectedTurnId": self.turn_id,
                    "input": [{"type": "text", "text": text}],
                },
            }
        )

    def activity_event(self, event: dict[str, object]) -> dict[str, object]:
        if not self.accepts_event(event):
            return {}
        if "id" in event and "method" in event:
            return {"type": "agentnave.approval_required"}
        return codex_event(event)

    def accepts_event(self, event: dict[str, object]) -> bool:
        params = object_dict(event.get("params"))
        if "threadId" in params and params["threadId"] != self.session_id:
            return False
        turn = params.get("turnId", object_dict(params.get("turn")).get("id"))
        return turn is None or self.turn_id is None or turn == self.turn_id

    def observe(self, event: dict[str, object]) -> None:
        message_id = event.get("id")
        method = event.get("method")
        result = object_dict(event.get("result"))
        params = object_dict(event.get("params"))
        if isinstance(message_id, str) and message_id in self.pending:
            future = self.pending[message_id]
            if not future.done():
                if "error" in event:
                    future.set_result(("rejected", "provider_rejected_active_turn"))
                elif result.get("turnId") == self.turn_id and self.turn_id is not None:
                    future.set_result(("accepted", "accepted_by_active_turn"))
                else:
                    future.set_result(("unknown", "unrecognized_provider_ack"))
            return
        if isinstance(message_id, str) and message_id.startswith("agentnave-") and "error" in event:
            self.active = False
            self.finished.set()
            return
        if self.finished.is_set():
            return
        if message_id == "agentnave-initialize" and "result" in event:
            self.writer.write(rpc_line({"method": "initialized"}))
            thread_params: dict[str, object] = {"cwd": str(self.request.cwd)}
            if "model" in self.request.provider_options:
                thread_params["model"] = self.request.provider_options["model"]
            if (
                self.request.provider_options.get("dangerously_bypass_approvals_and_sandbox")
                is True
            ):
                thread_params.update(approvalPolicy="never", sandbox="danger-full-access")
            if self.request.session_id is not None:
                thread_params["threadId"] = self.request.session_id
            self._write(
                "agentnave-thread",
                "thread/resume" if self.request.session_id else "thread/start",
                thread_params,
            )
        elif message_id == "agentnave-thread" and "result" in event:
            session = object_dict(result.get("thread")).get("id")
            if (
                not isinstance(session, str)
                or not session
                or (self.request.session_id and session != self.request.session_id)
            ):
                self.finished.set()
                return
            self.session_id = session
            turn_params: dict[str, object] = {
                "threadId": session,
                "input": [{"type": "text", "text": self.request.prompt}],
            }
            if "effort" in self.request.provider_options:
                turn_params["effort"] = self.request.provider_options["effort"]
            self._write("agentnave-turn", "turn/start", turn_params)
        elif (message_id == "agentnave-turn" and "result" in event) or (
            method == "turn/started" and params.get("threadId") == self.session_id
        ):
            turn = (
                object_dict(result.get("turn"))
                if "result" in event
                else object_dict(params.get("turn"))
            )
            turn_id = turn.get("id")
            if (
                isinstance(turn_id, str)
                and turn_id
                and (self.turn_id is None or self.turn_id == turn_id)
            ):
                self.turn_id = turn_id
                self.active = True
        elif method == "turn/completed" and params.get("threadId") == self.session_id:
            if object_dict(params.get("turn")).get("id") == self.turn_id:
                self.active = False
                self.finished.set()


def user_input(text: str, message_id: str, session_id: str | None) -> bytes:
    return rpc_line(
        {
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": text}]},
            "parent_tool_use_id": None,
            "uuid": message_id,
            "session_id": session_id or "default",
        }
    )


class ClaudeQueueChannel(PiRpcChannel):
    """Explicit queue mode may schedule another native turn, without interrupting one."""

    modes: tuple[MessageMode, ...] = ("queue",)

    def __init__(self, request: InvocationRequest) -> None:
        super().__init__()
        self.request = request
        self.outstanding: set[str] = set()
        self.result_seen = False
        self.queued_turns = 0

    def bind(self, writer: asyncio.StreamWriter) -> None:
        from uuid import uuid4

        super().bind(writer)
        message_id = str(uuid4())
        self.outstanding.add(message_id)
        writer.write(user_input(self.request.prompt, message_id, self.request.session_id))

    def message(self, message_id: str, text: str, mode: MessageMode = "queue") -> bytes:
        self.outstanding.add(message_id)
        return user_input(text, message_id, self.session_id)

    def accepts_event(self, event: dict[str, object]) -> bool:
        session = event.get("session_id")
        return event.get("parent_tool_use_id") is None and (
            self.session_id is None or session is None or session == self.session_id
        )

    def activity_event(self, event: dict[str, object]) -> dict[str, object]:
        if not self.accepts_event(event):
            return {}
        if event.get("type") == "control_request":
            return {"type": "agentnave.approval_required"}
        return event

    def _settle(self) -> None:
        if self.result_seen and not self.outstanding and self.queued_turns == 0:
            self.active = False
            self.finished.set()

    def observe(self, event: dict[str, object]) -> None:
        if not self.accepts_event(event):
            return
        session = event.get("session_id")
        if isinstance(session, str) and session and self.session_id is None:
            self.session_id = session
        if self.session_id is not None and isinstance(session, str) and session != self.session_id:
            return
        kind = event.get("type")
        if kind == "command_lifecycle":
            message_id = event.get("command_uuid")
            state = event.get("state")
            if not isinstance(message_id, str) or message_id not in self.outstanding:
                return
            future = self.pending.get(message_id)
            if state in ("queued", "started"):
                self.active = True
                if future is not None and not future.done():
                    future.set_result(("accepted", "queued_provider_schedules_turn"))
            elif state in ("completed", "cancelled", "discarded", "refused"):
                # completed can precede its result on another stdout read: only the
                # result's UUIDs prove that its final reply has been captured.
                if state != "completed":
                    self.outstanding.discard(message_id)
                if future is not None and not future.done():
                    future.set_result(
                        ("accepted", "completed_by_provider")
                        if state == "completed"
                        else ("rejected", "provider_" + str(state))
                    )
            self._settle()
        elif kind in ("assistant", "stream_event"):
            self.active = not self.finished.is_set()
        elif kind == "result":
            ids: list[str] = []
            message_ids = event.get("user_message_uuids")
            if isinstance(message_ids, list):
                ids.extend(
                    item for item in cast(list[object], message_ids) if isinstance(item, str)
                )
            message_id = event.get("user_message_uuid")
            if isinstance(message_id, str):
                ids.append(message_id)
            self.outstanding.difference_update(ids)
            output = event.get("result")
            self.turns.append(
                NativeTurnResult(
                    output=output if isinstance(output, str) else "",
                    status="failed" if event.get("is_error") is True else "succeeded",
                    message_ids=tuple(dict.fromkeys(ids)),
                )
            )
            queued = event.get("queued_turn_count")
            if not isinstance(queued, int) or isinstance(queued, bool) or queued < 0:
                self.error = InvocationError(
                    "protocol_unsupported",
                    "Claude queue requires queued_turn_count in native results",
                )
                self.finished.set()
                self.active = False
                return
            self.queued_turns = queued
            self.result_seen = True
            self._settle()


class CodeBuddyChannel(PiRpcChannel):
    """CodeBuddy control steer, bound to the first native conversation request."""

    def __init__(self, request: InvocationRequest) -> None:
        super().__init__()
        self.request = request
        self.request_id: str | None = None

    def bind(self, writer: asyncio.StreamWriter) -> None:
        from uuid import uuid4

        super().bind(writer)
        writer.write(user_input(self.request.prompt, str(uuid4()), self.request.session_id))

    def message(self, message_id: str, text: str, mode: MessageMode = "steer") -> bytes:
        return rpc_line(
            {
                "type": "control_request",
                "request_id": message_id,
                "request": {
                    "subtype": "steer",
                    "session_id": self.session_id,
                    "expected_request_id": self.request_id,
                    "content_blocks": [{"type": "text", "text": text}],
                },
            }
        )

    def accepts_event(self, event: dict[str, object]) -> bool:
        if event.get("parent_tool_use_id") is not None:
            return False
        session = event.get("session_id")
        request_id = event.get("_requestId")
        return (self.session_id is None or session is None or session == self.session_id) and (
            self.request_id is None or request_id is None or request_id == self.request_id
        )

    def activity_event(self, event: dict[str, object]) -> dict[str, object]:
        if not self.accepts_event(event):
            return {}
        if event.get("type") == "control_request":
            return {"type": "agentnave.approval_required"}
        return event

    def observe(self, event: dict[str, object]) -> None:
        if self.finished.is_set() or event.get("parent_tool_use_id") is not None:
            return
        kind = event.get("type")
        if kind == "control_response":
            response = object_dict(event.get("response"))
            message_id = response.get("request_id")
            future = self.pending.get(message_id) if isinstance(message_id, str) else None
            if future is not None and not future.done():
                data = object_dict(response.get("response"))
                if (
                    response.get("subtype") == "success"
                    and data.get("steered") is True
                    and data.get("session_id") == self.session_id
                ):
                    future.set_result(("accepted", "buffered_by_active_turn"))
                elif response.get("subtype") == "error" or data.get("steered") is False:
                    future.set_result(("rejected", "provider_rejected_active_turn"))
                else:
                    future.set_result(("unknown", "unrecognized_provider_ack"))
            return
        if not self.accepts_event(event):
            return
        session = event.get("session_id")
        if isinstance(session, str) and session and self.session_id is None:
            self.session_id = session
        request_id = event.get("_requestId")
        if isinstance(request_id, str) and request_id and self.request_id is None:
            self.request_id = request_id
        if self.session_id is not None and self.request_id is not None:
            self.active = True
        if kind == "result":
            self.active = False
            self.finished.set()
