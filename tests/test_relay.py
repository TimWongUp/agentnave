"""The launcher relay swaps in a reinstalled server only while nothing would be lost."""

from __future__ import annotations

import io
import json
import os
import sys
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from agentnave import relay as launcher

# A stand-in MCP server: refuses calls before the handshake, reports its pid and session, is busy
# while the file named in argv[1] exists, and fails to start while argv[1] + ".broken" exists.
FAKE_SERVER = """
import json, os, sys
if os.path.exists(sys.argv[1] + ".broken"):
    sys.exit(1)
ready = False
for line in sys.stdin:
    message = json.loads(line)
    method, ident = message.get("method"), message.get("id")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1"}}
    elif method == "notifications/initialized":
        ready = True
        continue
    elif not ready:
        result = {"isError": True, "content": [{"type": "text", "text": "not initialized"}]}
    elif message["params"]["name"] == "list_invocations":
        busy = [{"status": "succeeded", "result_read": False}] if os.path.exists(sys.argv[1]) else []
        result = {"structuredContent": {"invocations": busy, "restart_safe": True}}
    else:
        session = os.environ.get("AGENTNAVE_MANAGER_SESSION")
        result = {"structuredContent": {"pid": os.getpid(), "session": session}}
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
"""


class Host:
    def __init__(
        self, tmp_path: Path, env: dict[str, str] | None = None, *, script_text: str = FAKE_SERVER
    ) -> None:
        script = tmp_path / "server.py"
        script.write_text(script_text)
        self.busy = tmp_path / "busy"
        self.site = tmp_path / "site"
        self.install("1.0")
        to_relay, self.stdin = os.pipe()
        self.stdout, from_relay = os.pipe()
        self.reader = os.fdopen(self.stdout, "rb")
        self.writer = os.fdopen(self.stdin, "wb")
        self.code: list[int] = []
        relay_in, relay_out = os.fdopen(to_relay, "rb"), os.fdopen(from_relay, "wb")

        self.relay = launcher.Relay(
            [sys.executable, str(script), str(self.busy)],
            site=self.site,
            stdin=relay_in,
            stdout=relay_out,
            interval=0.05,
            env=env,
        )

        def run() -> None:
            self.code.append(self.relay.run())

        self.thread = threading.Thread(target=run, daemon=True)
        self.thread.start()
        self.sequence = 0

    def install(self, version: str) -> None:
        for old in self.site.glob("agentnave-*.dist-info"):
            (old / "RECORD").unlink()
            old.rmdir()
        dist = self.site / f"agentnave-{version}.dist-info"
        dist.mkdir(parents=True)
        (dist / "RECORD").write_text(version)

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.sequence += 1
        self.send({"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params or {}})
        reply = json.loads(self.reader.readline())
        assert reply["id"] == self.sequence
        return reply["result"]

    def send(self, message: dict[str, Any]) -> None:
        self.writer.write(json.dumps(message).encode() + b"\n")
        self.writer.flush()

    def whoami(self) -> dict[str, Any]:
        return self.call("tools/call", {"name": "whoami"})["structuredContent"]

    def pid(self) -> int:
        return self.whoami()["pid"]


def _settle(host: Host, seconds: float = 0.5) -> int:
    time.sleep(seconds)
    return host.pid()


def test_relay_swaps_only_when_idle_and_not_blocked_by_open_group(tmp_path: Path) -> None:
    host = Host(tmp_path)
    host.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
    host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    first = host.pid()

    # An unread result keeps the old server.
    host.busy.touch()
    host.install("1.1")
    assert _settle(host) == first

    # Once nothing would be lost, the new install takes over with the host's handshake.
    host.busy.unlink()
    deadline = time.monotonic() + 10
    while (second := host.pid()) == first and time.monotonic() < deadline:
        time.sleep(0.1)
    assert second != first

    # Opening a group no longer pins: its state is saved and reloads in the new child.
    host.call("tools/call", {"name": "open_group"})
    host.install("1.2")
    deadline = time.monotonic() + 10
    while (third := host.pid()) == second and time.monotonic() < deadline:
        time.sleep(0.1)
    assert third != second

    host.writer.close()
    host.thread.join(10)
    assert host.code == [0]


def test_relay_retries_a_failed_start_and_keeps_the_session(tmp_path: Path) -> None:
    host = Host(tmp_path, env={**os.environ, "AGENTNAVE_MANAGER_SESSION": "a" * 32})
    host.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
    host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    first = host.whoami()

    # The new install cannot start at first; once it can, the relay retries without a reinstall.
    broken = tmp_path / "busy.broken"
    broken.touch()
    host.install("1.1")
    assert _settle(host) == first["pid"]
    broken.unlink()
    deadline = time.monotonic() + 10
    while (second := host.whoami())["pid"] == first["pid"] and time.monotonic() < deadline:
        time.sleep(0.1)
    assert second["pid"] != first["pid"]
    assert second["session"] == first["session"] == "a" * 32

    host.writer.close()
    host.thread.join(10)


def test_real_server_keeps_pending_review_and_unsaved_results_during_upgrade(
    tmp_path: Path,
) -> None:
    # Use the real MCP lifecycle with a fake executable, including a failed terminal save.
    provider = tmp_path / ("claude.py" if os.name == "nt" else "claude")
    provider.write_text(
        "#!/usr/bin/env python3\nimport json,sys\n"
        "sys.stdin.read()\n"
        "print(json.dumps({'type':'result','result':'kept answer',"
        "'session_id':'kept-session','is_error':False}))\n"
    )
    provider.chmod(0o755)
    if os.name == "nt":
        (tmp_path / "claude.cmd").write_text(f'@"{sys.executable}" "%~dp0claude.py" %*\n')
    bootstrap = """
from pathlib import Path
import sys
from agentnave import mcp_server
from agentnave.nesting import NestingResult
from agentnave.store import Record
mcp_server.detect_nesting = lambda: NestingResult('not_nested', 'relay regression')
original_save = Record.save
def save(record):
    if Path(sys.argv[1] + '.write_failed').exists() and any(
        task.status != 'running' for task in record.state.tasks
    ):
        raise OSError('injected history write failure')
    original_save(record)
Record.save = save
mcp_server.main()
"""
    data = tmp_path / "data"
    host = Host(
        tmp_path,
        env={
            **os.environ,
            "PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}",
            "AGENTNAVE_HOST": "gemini",
            "AGENTNAVE_EXCLUDED_PROVIDERS": "",
            "AGENTNAVE_DATA_DIR": str(data),
        },
        script_text=bootstrap,
    )

    def call(tool: str, **arguments: Any) -> dict[str, Any]:
        result = host.call("tools/call", {"name": tool, "arguments": arguments})
        assert not result.get("isError"), result
        return result["structuredContent"]

    def changed_pid(old: int) -> int:
        deadline = time.monotonic() + 10
        while host.relay.child.process.pid == old and time.monotonic() < deadline:
            time.sleep(0.05)
        assert host.relay.child.process.pid != old
        return host.relay.child.process.pid

    try:
        host.call(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "relay regression", "version": "1"},
            },
        )
        host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        first = host.relay.child.process.pid
        group = call(
            "open_group",
            title="Review",
            cwd=str(tmp_path),
            review=True,
            seats=[{"id": seat, "provider": "claude"} for seat in ("a", "b")],
        )
        round_ = call(
            "run_round", group_id=group["group_id"], seats=["a"], order="parallel", prompt="answer"
        )
        assert call("wait_round", round_id=round_["round_id"])["reason"] == "review_needed"
        listed = call("list_invocations")
        assert all(item["result_read"] for item in listed["invocations"])
        assert listed["restart_safe"] is False
        host.install("1.1")
        time.sleep(0.5)
        assert host.relay.child.process.pid == first
        call("decide_answers", round_id=round_["round_id"], decisions={"a": "publish"})
        call("wait_round", round_id=round_["round_id"])
        second = changed_pid(first)
        saved = json.loads((data / "conversations" / group["group_id"] / "state.json").read_bytes())
        assert saved["rounds"][0]["seats"][0]["review"] == "publish"
        assert saved["rounds"][0]["seats"][0]["output"] == "kept answer"

        failure = tmp_path / "busy.write_failed"
        failure.touch()
        started = call("start_agent", provider="claude", prompt="answer", cwd=str(tmp_path))
        assert call("wait_agent", invocation_id=started["invocation_id"])["status"] == "succeeded"
        assert call("list_invocations")["restart_safe"] is False
        host.install("1.2")
        time.sleep(0.5)
        assert host.relay.child.process.pid == second
        failure.unlink()
        call("update_conversation", conversation_id=started["conversation_id"], title="Saved")
        changed_pid(second)
        saved = json.loads(
            (data / "conversations" / started["conversation_id"] / "state.json").read_bytes()
        )
        assert saved["tasks"][0]["output"] == "kept answer"
        assert saved["tasks"][0]["session_id"] == "kept-session"
    finally:
        host.writer.close()
        host.thread.join(10)
    assert host.code == [0]


def test_relay_recovers_after_long_capped_backoff(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    relay = launcher.Relay(
        [sys.executable, "-c", "import sys; sys.stdin.read()"],
        site=tmp_path,
        stdin=io.BytesIO(),
        stdout=io.BytesIO(),
        interval=5.0,
    )
    elapsed = 0.0
    attempts: list[float] = []
    fingerprints = iter([(("old", 1),)])

    def tick(seconds: float) -> bool:
        nonlocal elapsed
        elapsed += seconds
        return relay.stopped.is_set()

    def swap() -> str:
        attempts.append(elapsed)
        if len(attempts) > 1100:
            relay.stopped.set()
            return "swapped"
        return "failed"

    def installed(_site: Path) -> launcher.Fingerprint:
        return next(fingerprints, (("new", 2),))

    monkeypatch.setattr(launcher, "fingerprint", installed)
    monkeypatch.setattr(launcher.time, "monotonic", lambda: elapsed)
    monkeypatch.setattr(relay.stopped, "wait", tick)
    monkeypatch.setattr(relay, "_from_host", lambda: None)
    monkeypatch.setattr(relay, "_swap", swap)
    try:
        assert relay.run() == 0
        assert len(attempts) == 1101
        assert [b - a for a, b in zip(attempts, attempts[1:], strict=False)][:3] == [
            10.0,
            20.0,
            40.0,
        ]
        assert attempts[-1] - attempts[-2] == 300.0
    finally:
        relay.child.close()
        relay.child.process.wait(timeout=5)
