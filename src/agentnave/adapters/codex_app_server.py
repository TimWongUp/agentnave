"""One `codex app-server` thread, turn after turn, owned by the provider supervisor (ADR-0120).

Usage: codex_app_server.py --permissions MODE [--turn-option NAME]... -- <app-server argv>

stdin carries one JSON turn per line: {"session_id", "cwd", "prompt", "model", "effort"}. The
first line starts a thread; later lines must name it (a cold resume runs `codex exec resume`
instead, see ADR-0120). Only the options named by --turn-option may be set, so a process is reused
only by turns that set the same ones: a value can change between turns, but a setting cannot be
cleared once a turn set it.

Requests carry what `codex exec` 0.171 sends its embedded app-server (codex-rs/exec/src/lib.rs):
no cwd on thread/start (the process's own directory is the default, so starting a thread never
records project trust), MODE's harness overrides (see _THREAD_OVERRIDES) and nothing else about
permissions, so the app-server resolves them from the same config layers exec would; each
turn/start carries the cwd and exec's approval policy. Config warnings are reported as exec
reports them, on every turn.

stdout carries the events `codex exec --json` prints, ending every turn with exactly one
`turn.completed` or `turn.failed`; nothing is written between turns. A protocol failure ends the
current turn with `turn.failed` and exits 1, never falling back to `codex exec`. An app-server that
exits between turns ends the bridge too, so the service sees the provider gone.
"""

from __future__ import annotations

import json
import os
import queue
import re
import shutil
import subprocess
import sys
import threading
from contextlib import suppress
from dataclasses import dataclass, field
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
# The harness overrides `codex exec` builds its config with besides the --config overrides
# (codex-rs/exec/src/lib.rs, build_exec_config): --dangerously-bypass-approvals-and-sandbox sets
# never and full access; --approve-for-me only adds --config overrides, which the app-server is
# started with.
_THREAD_OVERRIDES: dict[str, dict[str, Any]] = {
    "approve-for-me": {},
    "bypass": {"approvalPolicy": "never", "sandbox": "danger-full-access"},
}
_AUTO_REVIEWERS = frozenset({"auto_review", "guardian_subagent"})
_PAGINATED_UNSUPPORTED = "paginated threads require thread/turns/list and thread/items/list support"
_CAMEL = re.compile(r"(?<=[a-z0-9])([A-Z])")
_USAGE = (
    "usage: codex_app_server.py --permissions MODE [--turn-option NAME]... -- <app-server argv>"
)


def emit(event: dict[str, Any]) -> None:
    sys.stdout.buffer.write(json.dumps(event, ensure_ascii=False).encode("utf-8") + b"\n")
    sys.stdout.buffer.flush()


def _snake(name: str) -> str:
    return _CAMEL.sub(r"_\1", name).lower()


def _object(value: object) -> dict[str, Any]:
    return cast(dict[str, Any], value) if isinstance(value, dict) else {}


class RequestError(RuntimeError):
    def __init__(self, method: str, code: object, message: object) -> None:
        super().__init__(f"codex app-server {method} failed: {message}")
        self.code = code
        self.message = message


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
        # A connection is opened for a turn; between turns the reader alone can see it end.
        self._state = threading.Lock()
        self._in_turn = True
        self._gone = False
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
            with self._state:
                self._gone = True
                if not self._in_turn:
                    # Idle, with no turn to fail: end, so the kept process is not left dead.
                    os._exit(1)
            self._messages.put(None)

    def begin_turn(self) -> None:
        with self._state:
            self._in_turn = True

    def end_turn(self) -> bool:
        """Whether the app-server is still there to serve the next turn."""
        with self._state:
            self._in_turn = False
            return not self._gone

    def _answer(self, request: dict[str, Any]) -> None:
        """Never approve anything: these runs have nobody to ask, as `codex exec` has not."""
        method = str(request.get("method"))
        if method == "mcpServer/elicitation/request":
            reply: dict[str, Any] = {
                "id": request["id"],
                "result": cancelled_elicitation(_object(request.get("params"))),
            }
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
                raise RequestError(method, error.get("code"), error.get("message"))
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


def cancelled_elicitation(params: dict[str, Any]) -> dict[str, Any]:
    """Cancel as exec does, marking a tool-call approval as cancelled for want of a user."""
    result: dict[str, Any] = {"action": "cancel", "content": None}
    meta = _object(params.get("_meta"))
    if params.get("mode") == "form" and meta.get("codex_approval_kind") == "mcp_tool_call":
        result["_meta"] = {"codex_approval_cancellation_reason": "non_interactive"}
    return result


def handshake(connection: Connection) -> None:
    connection.request(
        "initialize",
        {
            "clientInfo": {"name": "agentnave", "title": "AgentNave", "version": __version__},
            "capabilities": {"experimentalApi": True, "optOutNotificationMethods": _OPT_OUT},
        },
    )
    connection.send({"method": "initialized"})


@dataclass(slots=True)
class Thread:
    id: str
    # What exec sends with every turn: the thread's resolved policy under auto review or
    # `--dangerously-bypass-approvals-and-sandbox` (whose never it was resolved from), else never.
    approval: object
    warnings: list[str] = field(default_factory=list[str])


def open_thread(connection: Connection, turn: dict[str, Any], mode: str) -> Thread:
    """thread/start as exec sends it (thread_start_params_from_config)."""
    if turn.get("session_id") is not None:
        raise ValueError("a Codex app-server process starts its own thread; resume runs exec")
    params: dict[str, Any] = {
        **_THREAD_OVERRIDES[mode],
        "ephemeral": False,
        "historyMode": "paginated",
        "threadSource": "user",
    }
    if turn.get("model") is not None:
        params["model"] = turn["model"]
    try:
        result = connection.request("thread/start", params)
    except RequestError as exc:
        if exc.code != -32600 or exc.message != _PAGINATED_UNSUPPORTED:
            raise
        del params["historyMode"]
        result = connection.request("thread/start", params)
    thread_id = _object(result.get("thread")).get("id")
    if not isinstance(thread_id, str) or not thread_id:
        raise RuntimeError("codex app-server returned no thread id")
    # The policy the thread resolved is already the one requirements allow, as exec's is.
    resolved = result.get("approvalPolicy")
    auto_review = result.get("approvalsReviewer") in _AUTO_REVIEWERS
    return Thread(thread_id, resolved if mode == "bypass" or auto_review else "never")


def warning(message: dict[str, Any], thread_id: str) -> str | None:
    """A warning exec reports as an error item (event_processor_with_jsonl_output.rs)."""
    method = message.get("method")
    params = _object(message.get("params"))
    if method in ("configWarning", "deprecationNotice"):
        summary, details = params.get("summary"), params.get("details")
        if not isinstance(summary, str):
            return None
        return f"{summary} ({details})" if isinstance(details, str) and details else summary
    if method == "warning" and params.get("threadId") in (None, thread_id):
        text = params.get("message")
        return text if isinstance(text, str) else None
    return None


def emit_warning(text: str) -> None:
    emit({"type": "item.completed", "item": {"type": "error", "message": text}})


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


def run_turn(connection: Connection, thread: Thread, turn: dict[str, Any]) -> None:
    """turn/start as exec sends it: the cwd and approval policy, nothing else on permissions."""
    emit({"type": "thread.started", "thread_id": thread.id})
    # A new exec process reports the config's warnings again on every run.
    for text in thread.warnings:
        emit_warning(text)
    params: dict[str, Any] = {
        "threadId": thread.id,
        "turnTrigger": "exec",
        "input": [{"type": "text", "text": turn["prompt"], "text_elements": []}],
        "cwd": turn["cwd"],
        "approvalPolicy": thread.approval,
    }
    for name in TURN_OPTIONS:
        if turn.get(name) is not None:
            params[name] = turn[name]
    started = connection.request("turn/start", params)
    turn_id = _object(started.get("turn")).get("id")
    if not isinstance(turn_id, str) or not turn_id:
        raise RuntimeError("codex app-server returned no turn id")
    usage: dict[str, Any] | None = None
    while True:
        message = connection.notification()
        if (text := warning(message, thread.id)) is not None:
            emit_warning(text)
            if message.get("method") != "warning":
                thread.warnings.append(text)
            continue
        method = message.get("method")
        params = _object(message.get("params"))
        if params.get("threadId") != thread.id:
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


def parse_args(argv: list[str]) -> tuple[str, frozenset[str], list[str]]:
    if "--" not in argv:
        raise ValueError(_USAGE)
    split = argv.index("--")
    options, command = argv[:split], argv[split + 1 :]
    pairs = list(zip(options[::2], options[1::2], strict=False))
    names = [value for flag, value in pairs if flag == "--turn-option"]
    modes = [value for flag, value in pairs if flag == "--permissions"]
    if (
        not command
        or len(options) != 2 * (len(names) + len(modes))
        or set(names) - set(TURN_OPTIONS)
        or len(modes) != 1
        or modes[0] not in _THREAD_OVERRIDES
    ):
        raise ValueError(_USAGE)
    return modes[0], frozenset(names), command


def serve(mode: str, allowed: frozenset[str], command: list[str]) -> int:
    connection: Connection | None = None
    thread: Thread | None = None
    try:
        while (turn := read_turn(allowed)) is not None:
            if connection is None:
                connection = Connection(command)
                handshake(connection)
            connection.begin_turn()
            if thread is not None:
                connection.forget_idle()
            if thread is None:
                thread = open_thread(connection, turn, mode)
            elif turn.get("session_id") != thread.id:
                raise ValueError(f"this Codex process serves thread {thread.id} only")
            run_turn(connection, thread, turn)
            if not connection.end_turn():
                return 1
        return 0
    except (OSError, ValueError, RuntimeError) as exc:
        emit({"type": "turn.failed", "error": {"message": str(exc)}})
        return 1
    finally:
        if connection is not None:
            connection.close()


def main() -> None:
    try:
        mode, allowed, command = parse_args(sys.argv[1:])
    except ValueError as exc:
        emit({"type": "turn.failed", "error": {"message": str(exc)}})
        raise SystemExit(2) from None
    raise SystemExit(serve(mode, allowed, command))


if __name__ == "__main__":
    main()
