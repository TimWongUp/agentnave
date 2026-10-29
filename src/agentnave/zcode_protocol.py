"""One ZCode app-server session, owned by the existing provider supervisor."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, cast

from agentnave.adapters.base import resolve_executable
from agentnave.host import Host, session_model


def command() -> tuple[list[str], dict[str, str]]:
    """Prefer an installed CLI; macOS Desktop also ships its official Node bundle."""
    executable = resolve_executable("zcode")
    env = os.environ.copy()
    if Path(executable).is_file():
        return [executable, "app-server", "--stdio"], env
    if sys.platform == "darwin":
        for app in (Path("/Applications/ZCode.app"), Path.home() / "Applications/ZCode.app"):
            resources = app / "Contents/Resources"
            bundle = resources / "glm/zcode.cjs"
            if bundle.is_file():
                env.setdefault(
                    "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE",
                    str(resources / "config/provider/zcode-builtin.json"),
                )
                env.setdefault(
                    "ZCODE_PERSONAL_PROVIDER_CONFIG_FILE",
                    str(Path.home() / ".zcode/v2/provider_config.json"),
                )
                return [resolve_executable("node"), str(bundle), "app-server", "--stdio"], env
    return [executable, "app-server", "--stdio"], env


def emit(event: dict[str, Any]) -> None:
    print(json.dumps(event, ensure_ascii=False), flush=True)


class Connection:
    def __init__(self, process: subprocess.Popen[str], session_id: str) -> None:
        self.process = process
        self.session_id = session_id
        self.counter = 0
        self.response = ""
        self.partial = ""
        self.usage: dict[str, Any] = {}
        self.failed = False
        self.failure = ""
        self.finished = False

    def send(self, value: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def receive(self) -> dict[str, Any]:
        assert self.process.stdout is not None
        line = self.process.stdout.readline(1024 * 1024 + 1)
        if len(line) > 1024 * 1024:
            raise RuntimeError("ZCode protocol line exceeds 1 MiB")
        if not line:
            raise RuntimeError("ZCode app-server closed before completing the request")
        raw: object = json.loads(line)
        if not isinstance(raw, dict):
            raise RuntimeError("Invalid ZCode protocol message")
        value = cast(dict[str, Any], raw)
        if "method" in value and "id" in value:
            # Ask the runtime to use its native compatibility behavior, never approve a tool.
            self.send(
                {"id": value["id"], "error": {"code": -32601, "message": "No client handler"}}
            )
        if value.get("method") == "session/event":
            event = value.get("params", {})
            if event.get("sessionId") == self.session_id:
                kind, payload = event.get("type"), event.get("payload", {})
                if kind == "turn.completed":
                    self.response = payload.get("response", self.response)
                    self.usage = payload.get("usage", {})
                elif kind == "turn.failed":
                    self.failed = True
                    self.failure = "ZCode turn failed"
                if kind == "model.streaming" and payload.get("kind") == "text_delta":
                    self.partial = (self.partial + payload.get("delta", ""))[-1024 * 1024 :]
                    emit({"type": "text", "text": payload.get("delta", "")})
                elif kind == "tool.updated":
                    emit(
                        {
                            "type": "tool",
                            "name": payload.get("toolName"),
                            "status": payload.get("kind"),
                        }
                    )
                elif kind in ("turn.started", "turn.completed", "turn.failed"):
                    emit({"type": kind, "session_id": self.session_id})
        if value.get("method") == "state.updated":
            state = value.get("params", {})
            if state.get("sessionId") == self.session_id and state.get("reason") in (
                "prompt_completed",
                "prompt_failed",
            ):
                self.finished = True
                self.failed |= state["reason"] == "prompt_failed"
        return value

    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self.counter += 1
        request_id = f"agentnave-{self.counter}"
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self.receive()
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise RuntimeError(message["error"].get("message", "ZCode request failed"))
            return message.get("result", {})


def run(request: dict[str, Any]) -> int:
    started = datetime.now(UTC)
    argv, env = command()
    with subprocess.Popen(
        argv,
        cwd=request["cwd"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        text=True,
        encoding="utf-8",
    ) as process:
        connection = Connection(process, request["session_id"])
        try:
            params: dict[str, Any] = {
                "sessionId": request["session_id"],
                "workspace": {"workspacePath": request["cwd"], "workspaceKey": request["cwd"]},
            }
            options = request["options"]
            if not request["resume"] and options.get("permission_mode"):
                params["mode"] = options["permission_mode"]
            snapshot = connection.call(
                "session/resume" if request["resume"] else "session/create", params
            )
            emit({"type": "session", "session_id": connection.session_id})
            if request["resume"] and options.get("permission_mode"):
                connection.call(
                    "session/setMode",
                    {"sessionId": connection.session_id, "mode": options["permission_mode"]},
                )
            if model := options.get("model"):
                available = snapshot.get("settings", {}).get("model", {}).get("available", [])
                matches = [
                    item["ref"]
                    for item in available
                    if model
                    in (
                        item["ref"]["modelId"],
                        item["ref"]["providerId"] + "/" + item["ref"]["modelId"],
                    )
                ]
                if len(matches) != 1:
                    raise ValueError(
                        "ZCode model must uniquely match its native catalog; use providerId/modelId"
                    )
                selection = dict(matches[0])
                if options.get("effort"):
                    selection["options"] = {"reasoningLevel": options["effort"]}
                connection.call(
                    "session/setModel",
                    {
                        "sessionId": connection.session_id,
                        "model": selection,
                        "persistAsWorkspaceLastUsed": False,
                    },
                )
            if effort := options.get("effort"):
                connection.call(
                    "session/setThoughtLevel",
                    {
                        "sessionId": connection.session_id,
                        "thoughtLevel": effort,
                        "persistAsWorkspaceLastUsed": False,
                    },
                )
            connection.call(
                "session/subscribe",
                {"sessionId": connection.session_id, "deliveryKind": "desktop-continuous"},
            )
            connection.call(
                "session/send", {"sessionId": connection.session_id, "content": request["prompt"]}
            )
            while not connection.finished:
                connection.receive()
            emit(
                {
                    "type": "result",
                    "session_id": connection.session_id,
                    "output": connection.response,
                    "usage": connection.usage,
                    "failed": connection.failed,
                    "error": connection.failure,
                }
            )
            if model := session_model(Host("zcode"), connection.session_id, since=started):
                emit({"type": "model", "model": model})
            return int(connection.failed)
        except (OSError, ValueError, RuntimeError) as exc:
            emit(
                {
                    "type": "result",
                    "session_id": connection.session_id,
                    "output": connection.response or connection.partial,
                    "failed": True,
                    "error": str(exc),
                }
            )
            return 1
        finally:
            assert process.stdin is not None
            process.stdin.close()
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
    try:
        code = run(json.load(sys.stdin))
    except (OSError, ValueError, RuntimeError) as exc:
        emit({"type": "error", "message": str(exc)})
        code = 1
    raise SystemExit(code)


if __name__ == "__main__":
    main()
