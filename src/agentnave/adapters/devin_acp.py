"""One Devin ACP turn, owned by the existing provider supervisor (ADR-0070)."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
from dataclasses import dataclass, field
from typing import IO, Any, cast
from uuid import uuid4

from agentnave.adapters.base import resolve_executable
from agentnave.native_prompts import mask_secrets

LINE_LIMIT = 64 * 1024 * 1024
PROTOCOL_VERSION = 1
# ACP has no `-p` auto mode; dangerous is what the App's full tier writes.
MODES = {
    "accept-edits": "accept-edits",
    "smart": "smart",
    "ask": "ask",
    "plan": "plan",
    "bypass": "bypass",
    "dangerous": "bypass",
}


def command(sandbox: bool) -> tuple[list[str], dict[str, str]]:
    executable = resolve_executable("devin")
    # Popen does not resolve Windows PATHEXT command shims.
    executable = shutil.which(executable) or executable
    env = os.environ.copy()
    if sandbox:
        # `devin acp` has no sandbox flag; its exec sandbox reads this variable.
        env["DEVIN_SANDBOX"] = "true"
    return [executable, "acp"], env


def emit(event: dict[str, Any]) -> None:
    # Bytes: Windows' locale codec would garble a Chinese answer.
    sys.stdout.buffer.write(json.dumps(event, ensure_ascii=False).encode("utf-8") + b"\n")
    sys.stdout.buffer.flush()


def _object(value: object) -> dict[str, Any]:
    return cast(dict[str, Any], value) if isinstance(value, dict) else {}


def _list(value: object) -> list[object]:
    return cast(list[object], value) if isinstance(value, list) else []


@dataclass
class Turn:
    """What this prompt produced; replayed history before it never counts."""

    live: bool = False
    segments: list[str] = field(default_factory=list[str])
    split: bool = True
    thinking: bool = False
    tools: dict[str, tuple[str, object]] = field(default_factory=dict[str, tuple[str, object]])
    statuses: dict[str, str] = field(default_factory=dict[str, str])

    def output(self) -> str:
        """The reply after the last tool call, else the last non-empty reply."""
        return next((text for text in reversed(self.segments) if text.strip()), "")


class Connection:
    def __init__(self, process: subprocess.Popen[bytes]) -> None:
        self.process = process
        self.counter = 0
        self.lock = threading.Lock()
        self.turn = Turn()
        self.session_id = ""
        # Permission requests the boss has not answered: prompt id -> (request id, options).
        self.pending: dict[str, tuple[object, list[dict[str, Any]]]] = {}

    def send(self, value: dict[str, Any]) -> None:
        stdin = cast(IO[bytes], self.process.stdin)
        with self.lock:
            stdin.write(json.dumps(value, ensure_ascii=False).encode("utf-8") + b"\n")
            stdin.flush()

    def receive(self) -> dict[str, Any]:
        stdout = cast(IO[bytes], self.process.stdout)
        while True:
            # A loaded session replays its history, so lines can be large.
            line = stdout.readline(LINE_LIMIT + 1)
            if len(line) > LINE_LIMIT:
                raise RuntimeError("Devin ACP line exceeds 64 MiB")
            if not line:
                raise RuntimeError("Devin ACP closed before completing the request")
            try:
                raw: object = json.loads(line)
            except ValueError:
                continue  # a log line on stdout is not a protocol message
            if isinstance(raw, dict):
                message = cast(dict[str, Any], raw)
                if "method" in message:
                    self.handle(message)
                    continue
                return message

    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self.counter += 1
        request_id = f"agentnave-{self.counter}"
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        while True:
            message = self.receive()
            if message.get("id") != request_id:
                continue
            if "error" in message:
                error = _object(message["error"])
                detail = error.get("data")
                raise RuntimeError(
                    str(error.get("message") or f"Devin {method} failed")
                    + (f": {detail}" if isinstance(detail, str) and detail else "")
                )
            return _object(message.get("result"))

    def handle(self, message: dict[str, Any]) -> None:
        method, params = message.get("method"), _object(message.get("params"))
        if "id" in message:
            if method == "session/request_permission" and self.turn.live:
                self.ask_boss(message["id"], params)
            else:
                # The client offers no files, terminals or other callbacks.
                self.send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "error": {"code": -32601, "message": "Method not found"},
                    }
                )
        elif (
            method == "session/update"
            and self.turn.live
            and params.get("sessionId") == self.session_id
        ):
            self.update(_object(params.get("update")))

    def update(self, update: dict[str, Any]) -> None:
        kind, turn = update.get("sessionUpdate"), self.turn
        if kind == "agent_thought_chunk":
            # Thinking only marks the state; its text never leaves the helper.
            if not turn.thinking:
                turn.thinking = True
                emit({"type": "thinking"})
            return
        if kind == "agent_message_chunk":
            text = _object(update.get("content")).get("text")
            if isinstance(text, str) and text:
                if turn.split:
                    turn.segments.append("")
                    turn.split = False
                turn.segments[-1] += text
                turn.thinking = False
                emit({"type": "text", "text": text})
            return
        if kind not in ("tool_call", "tool_call_update"):
            return
        call_id = update.get("toolCallId")
        if not isinstance(call_id, str):
            return
        if kind == "tool_call":
            meta = _object(update.get("_meta"))
            # Never the title: it carries the command itself.
            name = meta.get("cognition.ai/inferenceToolName") or update.get("kind") or "tool"
            turn.tools[call_id] = (str(name), update.get("rawInput"))
            turn.split = True
        elif "rawInput" in update and call_id in turn.tools:
            # Arguments may change after the call starts; a permission prompt shows the latest.
            turn.tools[call_id] = (turn.tools[call_id][0], update["rawInput"])
        status = update.get("status") or ("pending" if kind == "tool_call" else None)
        if isinstance(status, str) and turn.statuses.get(call_id) != status:
            turn.statuses[call_id] = status
            turn.thinking = False
            name = turn.tools.get(call_id, ("",))[0]
            emit({"type": "tool", "id": call_id, "name": name, "status": status})

    def ask_boss(self, request_id: object, params: dict[str, Any]) -> None:
        tool_call = _object(params.get("toolCall"))
        name, raw_input = self.turn.tools.get(str(tool_call.get("toolCallId")), ("tool", None))
        # The request's own arguments are the ones the answer approves.
        raw_input = tool_call.get("rawInput", raw_input)
        prompt_id = uuid4().hex
        options = [_object(option) for option in _list(params.get("options"))]
        with self.lock:
            self.pending[prompt_id] = (request_id, options)
        emit(
            {
                "type": "agentnave_prompt",
                "prompt_id": prompt_id,
                "kind": "permission",
                "tool_name": name,
                # Masked whole before any cut: a cut secret is no longer recognizable.
                "tool_input": mask_secrets(json.dumps(raw_input, ensure_ascii=False))[:8000],
            }
        )

    def answer(self, reply: dict[str, Any]) -> None:
        with self.lock:
            entry = self.pending.pop(str(reply.get("prompt_id")), None)
        if entry is None:
            return
        request_id, options = entry
        # Only this call: never a session, permanent or bypass-switch option.
        wanted = "allow_once" if reply.get("decision") == "allow" else "reject_once"
        chosen = next((o for o in options if o.get("kind") == wanted), None)
        outcome: dict[str, Any] = (
            {"outcome": "selected", "optionId": chosen.get("optionId")}
            if chosen is not None
            else {"outcome": "cancelled"}
        )
        self.send({"jsonrpc": "2.0", "id": request_id, "result": {"outcome": outcome}})

    def close_pending(self) -> None:
        with self.lock:
            closed, self.pending = list(self.pending), {}
        for prompt_id in closed:
            emit({"type": "agentnave_prompt_closed", "prompt_id": prompt_id})


def read_answers(connection: Connection, stream: IO[bytes]) -> None:
    for line in stream:
        try:
            reply: object = json.loads(line)
        except ValueError:
            continue
        answer = _object(reply)
        if answer.get("type") == "agentnave_answer":
            connection.answer(answer)


def _current(result: dict[str, Any], option: str) -> tuple[object, list[object]]:
    for raw in _list(result.get("configOptions")):
        entry = _object(raw)
        if entry.get("id") == option:
            values: list[object] = [_object(o).get("value") for o in _list(entry.get("options"))]
            return entry.get("currentValue"), values
    return None, []


def _select(connection: Connection, result: dict[str, Any], option: str, value: str) -> None:
    current, values = _current(result, option)
    if current == value:
        return
    try:
        changed = connection.call(
            "session/set_config_option",
            {"sessionId": connection.session_id, "configId": option, "value": value},
        )
    except RuntimeError as exc:
        raise ValueError(
            f"Devin {option} {value} is not available here; available: "
            + ", ".join(str(v) for v in values)
        ) from exc
    if _current(changed, option)[0] != value:
        raise ValueError(f"Devin kept {option} {_current(changed, option)[0]} instead of {value}")
    result["configOptions"] = changed.get("configOptions")


def open_session(connection: Connection, request: dict[str, Any]) -> None:
    initialized = connection.call(
        "initialize",
        {
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {"readTextFile": False, "writeTextFile": False},
                "terminal": False,
            },
            "clientInfo": {"name": "agentnave", "version": "1"},
        },
    )
    if initialized.get("protocolVersion") != PROTOCOL_VERSION:
        raise RuntimeError(f"Devin speaks ACP {initialized.get('protocolVersion')}, not 1")
    params: dict[str, Any] = {"cwd": request["cwd"], "mcpServers": []}
    if request.get("session_id"):
        connection.session_id = request["session_id"]
        result = connection.call("session/load", {"sessionId": connection.session_id, **params})
    else:
        result = connection.call("session/new", params)
        connection.session_id = str(result.get("sessionId") or "")
        if not connection.session_id:
            raise RuntimeError("Devin created no session")
    emit({"type": "session", "session_id": connection.session_id})
    options = request["options"]
    if mode := options.get("permission_mode"):
        _select(connection, result, "mode", MODES[mode])
    if model := options.get("model"):
        _select(connection, result, "model", model)
    if isinstance(current := _current(result, "model")[0], str) and current:
        emit({"type": "model", "model": current})


def run(request: dict[str, Any], answers: IO[bytes]) -> int:
    argv, env = command(bool(request["options"].get("sandbox")))
    with subprocess.Popen(
        argv,
        cwd=request["cwd"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
    ) as process:
        connection = Connection(process)
        threading.Thread(target=read_answers, args=(connection, answers), daemon=True).start()
        try:
            open_session(connection, request)
            connection.turn.live = True
            prompt = connection.call(
                "session/prompt",
                {
                    "sessionId": connection.session_id,
                    "prompt": [{"type": "text", "text": request["prompt"]}],
                },
            )
            stop = prompt.get("stopReason")
            failed = stop != "end_turn"
            emit(
                {
                    "type": "result",
                    "session_id": connection.session_id,
                    "output": connection.turn.output(),
                    "stop_reason": stop,
                    "failed": failed,
                    "error": "" if not failed else f"Devin stopped: {stop}",
                }
            )
            return int(failed)
        except Exception as exc:  # an unexpected native reply must still end with a result
            expected = isinstance(exc, OSError | ValueError | RuntimeError)
            emit(
                {
                    "type": "result",
                    "session_id": connection.session_id or None,
                    "output": connection.turn.output(),
                    "failed": True,
                    "error": str(exc) if expected else f"Unexpected Devin reply: {exc!r}",
                }
            )
            return 1
        finally:
            connection.close_pending()
            _stop(process)


def _stop(process: subprocess.Popen[bytes]) -> None:
    cast(IO[bytes], process.stdin).close()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main() -> None:
    stdin = sys.stdin.buffer
    try:
        code = run(json.loads(stdin.readline().decode("utf-8")), stdin)
    except (OSError, ValueError, RuntimeError) as exc:
        emit({"type": "error", "message": str(exc)})
        code = 1
    sys.stdout.flush()
    # The answer reader still blocks on stdin, which the service keeps open; interpreter
    # shutdown would abort on its buffer lock and turn a finished turn into a crash.
    os._exit(code)


if __name__ == "__main__":
    main()
