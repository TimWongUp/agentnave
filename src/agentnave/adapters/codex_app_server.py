"""One `codex app-server` thread, turn after turn, owned by the provider supervisor (ADR-0120).

Usage: codex_app_server.py [--turn-option NAME]... -- <codex app-server argv>

stdin carries one JSON turn per line: {"session_id", "cwd", "prompt", "model", "effort"}. The
first line starts the thread (or loads `session_id`); later lines must name that thread. Only the
options named by --turn-option may be set, so a process is reused only by turns that set the same
ones: a value can change between turns, but a setting cannot be cleared once a turn set it.

stdout carries the events `codex exec --json` prints, ending every turn with exactly one
`turn.completed` or `turn.failed`; nothing is written between turns. A protocol failure ends the
current turn with `turn.failed` and exits 1, never falling back to `codex exec`.
"""

from __future__ import annotations

import json
import queue
import re
import shutil
import subprocess
import sys
import threading
from contextlib import suppress
from typing import Any, cast

from agentnave import __version__
from agentnave.windows_supervisor import popen_args

LINE_LIMIT = 64 * 1024 * 1024
TURN_OPTIONS = ("model", "effort")
# Notifications no turn event is made of; opting out keeps the pipe quiet.
_OPT_OUT = [
    "item/agentMessage/delta",
    "item/reasoning/textDelta",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/summaryPartAdded",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
    "item/plan/delta",
    "turn/diff/updated",
]
# Items `codex exec --json` does not report.
_UNREPORTED_ITEMS = frozenset({"userMessage", "hookPrompt"})
_ITEM_FIELDS = (
    ("text", "text"),
    ("command", "command"),
    ("exitCode", "exit_code"),
    ("server", "server"),
    ("tool", "tool"),
    ("query", "query"),
)
_CAMEL = re.compile(r"(?<=[a-z0-9])([A-Z])")


def emit(event: dict[str, Any]) -> None:
    sys.stdout.buffer.write(json.dumps(event, ensure_ascii=False).encode("utf-8") + b"\n")
    sys.stdout.buffer.flush()


def _snake(name: str) -> str:
    return _CAMEL.sub(r"_\1", name).lower()


def _object(value: object) -> dict[str, Any]:
    return cast(dict[str, Any], value) if isinstance(value, dict) else {}


class Connection:
    """JSON-RPC over the app-server's stdio. A reader thread answers the server's requests at
    once, even between turns, and queues everything else for the main thread."""

    def __init__(self, argv: list[str]) -> None:
        executable = shutil.which(argv[0]) or argv[0]
        args, application = popen_args([executable, *argv[1:]])
        self.process = subprocess.Popen(
            args,
            executable=application,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,  # the provider's own stderr, which the service drains
        )
        self._write_lock = threading.Lock()
        self._messages: queue.Queue[dict[str, Any] | None] = queue.Queue()
        self._backlog: list[dict[str, Any]] = []
        self._closed = False
        self._counter = 0
        threading.Thread(target=self._read, daemon=True).start()

    def send(self, value: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        data = json.dumps(value, ensure_ascii=False).encode("utf-8") + b"\n"
        with self._write_lock:
            self.process.stdin.write(data)
            self.process.stdin.flush()

    def _read(self) -> None:
        assert self.process.stdout is not None
        try:
            while line := self.process.stdout.readline(LINE_LIMIT + 1):
                if len(line) > LINE_LIMIT:
                    break
                try:
                    raw: object = json.loads(line)
                except ValueError:
                    continue  # a log line on stdout is not a protocol message
                message = _object(raw)
                if "method" in message and "id" in message:
                    self._answer(message)
                elif message:
                    self._messages.put(message)
        except (OSError, ValueError):
            pass
        finally:
            self._messages.put(None)

    def _answer(self, request: dict[str, Any]) -> None:
        """Never approve anything: these runs have nobody to ask, as `codex exec` has not."""
        method = str(request.get("method"))
        if method == "mcpServer/elicitation/request":
            reply: dict[str, Any] = {"id": request["id"], "result": {"action": "cancel"}}
        else:
            reply = {
                "id": request["id"],
                "error": {
                    "code": -32000,
                    "message": f"{method} is not supported in non-interactive AgentNave runs",
                },
            }
        with suppress(OSError):  # the server is gone; the reader sees its stdout close next
            self.send(reply)

    def _receive(self) -> dict[str, Any]:
        message = None if self._closed else self._messages.get()
        if message is None:
            self._closed = True
            raise RuntimeError("codex app-server exited")
        return message

    def request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        """Send a request and wait for its reply; what arrives first is kept for the turn."""
        self._counter += 1
        request_id = self._counter
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self._receive()
            if message.get("id") != request_id or "method" in message:
                self._backlog.append(message)
                continue
            if "error" in message:
                error = _object(message.get("error"))
                raise RuntimeError(f"codex app-server {method} failed: {error.get('message')}")
            return _object(message.get("result"))

    def notification(self) -> dict[str, Any]:
        return self._backlog.pop(0) if self._backlog else self._receive()

    def forget_idle(self) -> None:
        """Drop what arrived between turns: none of it belongs to the next one."""
        self._backlog.clear()
        while True:
            try:
                message = self._messages.get_nowait()
            except queue.Empty:
                return
            if message is None:
                self._closed = True
                return

    def close(self) -> None:
        if self.process.stdin is not None:
            with suppress(OSError):
                self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()


def handshake(connection: Connection) -> None:
    connection.request(
        "initialize",
        {
            "clientInfo": {"name": "agentnave", "title": "AgentNave", "version": __version__},
            "capabilities": {"experimentalApi": True, "optOutNotificationMethods": _OPT_OUT},
        },
    )
    connection.send({"method": "initialized"})


def open_thread(connection: Connection, turn: dict[str, Any]) -> str:
    params: dict[str, Any] = {"cwd": turn["cwd"]}
    if turn.get("model") is not None:
        params["model"] = turn["model"]
    session_id = turn.get("session_id")
    if session_id is None:
        result = connection.request("thread/start", params)
    else:
        result = connection.request(
            "thread/resume", {"threadId": session_id, "excludeTurns": True, **params}
        )
    thread_id = _object(result.get("thread")).get("id")
    if not isinstance(thread_id, str) or not thread_id:
        raise RuntimeError("codex app-server returned no thread id")
    return thread_id


def item_event(kind: str, item: dict[str, Any]) -> dict[str, Any] | None:
    """An app-server item as `codex exec --json` reports it, without bulky tool output."""
    item_type = item.get("type")
    if not isinstance(item_type, str) or item_type in _UNREPORTED_ITEMS:
        return None
    if item_type == "agentMessage" and kind != "item.completed":
        return None  # exec reports a reply once it is complete
    mapped: dict[str, Any] = {"id": item.get("id"), "type": _snake(item_type)}
    if isinstance(status := item.get("status"), str):
        mapped["status"] = _snake(status)
    for source, target in _ITEM_FIELDS:
        if source in item:
            mapped[target] = item[source]
    return {"type": kind, "item": mapped}


def _usage(token_usage: dict[str, Any]) -> dict[str, Any]:
    last = _object(token_usage.get("last"))
    return {
        "input_tokens": last.get("inputTokens"),
        "cached_input_tokens": last.get("cachedInputTokens"),
        "output_tokens": last.get("outputTokens"),
        "reasoning_output_tokens": last.get("reasoningOutputTokens"),
    }


def turn_end(turn: dict[str, Any], usage: dict[str, Any] | None) -> dict[str, Any]:
    status = turn.get("status")
    if status == "completed":
        return {"type": "turn.completed", "usage": usage}
    message = _object(turn.get("error")).get("message")
    return {"type": "turn.failed", "error": {"message": message or f"codex turn {status}"}}


def run_turn(connection: Connection, thread_id: str, turn: dict[str, Any]) -> None:
    connection.forget_idle()
    emit({"type": "thread.started", "thread_id": thread_id})
    params: dict[str, Any] = {
        "threadId": thread_id,
        "input": [{"type": "text", "text": turn["prompt"], "text_elements": []}],
    }
    for name in TURN_OPTIONS:
        if turn.get(name) is not None:
            params[name] = turn[name]
    started = connection.request("turn/start", params)
    turn_id = _object(started.get("turn")).get("id")
    usage: dict[str, Any] | None = None
    while True:
        message = connection.notification()
        method = message.get("method")
        params = _object(message.get("params"))
        if params.get("threadId") != thread_id:
            continue  # another thread's work, such as a subagent's, is reported by its items
        turn_payload = _object(params.get("turn"))
        if params.get("turnId", turn_payload.get("id", turn_id)) != turn_id:
            continue
        if method == "turn/started":
            emit({"type": "turn.started"})
        elif method in ("item/started", "item/completed"):
            kind = "item.started" if method == "item/started" else "item.completed"
            if event := item_event(kind, _object(params.get("item"))):
                emit(event)
        elif method == "error":
            emit({"type": "error", "message": _object(params.get("error")).get("message")})
        elif method == "thread/tokenUsage/updated":
            usage = _usage(_object(params.get("tokenUsage")))
        elif method == "turn/completed":
            emit(turn_end(turn_payload, usage))
            return


def read_turn(allowed: frozenset[str]) -> dict[str, Any] | None:
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    turn = _object(json.loads(line.decode("utf-8")))
    if not isinstance(turn.get("prompt"), str) or not isinstance(turn.get("cwd"), str):
        raise ValueError("a turn needs a prompt and a cwd")
    for name in TURN_OPTIONS:
        if (turn.get(name) is not None) != (name in allowed):
            raise ValueError(
                f"this Codex process was started for turns that set: {sorted(allowed)}"
            )
    return turn


def parse_args(argv: list[str]) -> tuple[frozenset[str], list[str]]:
    if "--" not in argv:
        raise ValueError("usage: codex_app_server.py [--turn-option NAME]... -- <app-server argv>")
    split = argv.index("--")
    options, command = argv[:split], argv[split + 1 :]
    names = [options[index + 1] for index, flag in enumerate(options) if flag == "--turn-option"]
    if not command or len(names) * 2 != len(options) or set(names) - set(TURN_OPTIONS):
        raise ValueError("usage: codex_app_server.py [--turn-option NAME]... -- <app-server argv>")
    return frozenset(names), command


def serve(allowed: frozenset[str], command: list[str]) -> int:
    connection: Connection | None = None
    thread_id: str | None = None
    try:
        while (turn := read_turn(allowed)) is not None:
            if connection is None:
                connection = Connection(command)
                handshake(connection)
            if thread_id is None:
                thread_id = open_thread(connection, turn)
            elif turn.get("session_id") != thread_id:
                raise ValueError(f"this Codex process serves thread {thread_id} only")
            run_turn(connection, thread_id, turn)
        return 0
    except (OSError, ValueError, RuntimeError) as exc:
        emit({"type": "turn.failed", "error": {"message": str(exc)}})
        return 1
    finally:
        if connection is not None:
            connection.close()


def main() -> None:
    try:
        allowed, command = parse_args(sys.argv[1:])
    except ValueError as exc:
        emit({"type": "turn.failed", "error": {"message": str(exc)}})
        raise SystemExit(2) from None
    raise SystemExit(serve(allowed, command))


if __name__ == "__main__":
    main()
