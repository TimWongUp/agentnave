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
# Like the real one, a first request carrying the 2026-07-28 envelope makes every request need it;
# listen streams are acknowledged at once and end with an error when stdin closes.
FAKE_SERVER = """
import json, os, sys
from pathlib import Path
if os.path.exists(sys.argv[1] + ".broken"):
    sys.exit(1)
ready = False
modern = None
listens = []
version = Path(sys.argv[1] + ".version").read_text()
schema = json.loads(Path(sys.argv[1] + ".tools").read_text()) if Path(sys.argv[1] + ".tools").exists() else {"type":"object"}
for line in sys.stdin:
    message = json.loads(line)
    method, ident = message.get("method"), message.get("id")
    enveloped = "io.modelcontextprotocol/protocolVersion" in message.get("params", {}).get("_meta", {})
    if modern is None and ident is not None:
        modern = ready = method != "initialize" and enveloped
    if modern and ident is not None and not enveloped:
        result = {"isError": True, "content": [{"type": "text", "text": "missing envelope"}]}
    elif method == "server/discover":
        result = {"_meta": {"io.modelcontextprotocol/serverInfo": {"name": "fake", "version": version}}}
    elif method == "subscriptions/listen":
        listens.append(ident)
        meta = {"io.modelcontextprotocol/subscriptionId": ident}
        print(json.dumps({"jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged", "params": {"_meta": meta}}), flush=True)
        continue
    elif method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools":{}}, "serverInfo": {"name": "fake", "version": version}}
    elif method == "notifications/initialized":
        ready = True
        continue
    elif not ready:
        result = {"isError": True, "content": [{"type": "text", "text": "not initialized"}]}
    elif method == "tools/list":
        result = {"tools":[{"name":"probe", "inputSchema":schema}] if schema is not None else []}
    elif message["params"]["name"] == "list_invocations":
        busy = [{"status": "succeeded", "result_read": False}] if os.path.exists(sys.argv[1]) else []
        result = {"structuredContent": {"invocations": busy, "restart_safe": True}}
    else:
        session = os.environ.get("AGENTNAVE_MANAGER_SESSION")
        result = {"structuredContent": {"pid": os.getpid(), "session": session, "listens": len(listens)}}
    if modern:  # 2026-07-28 results carry serverInfo, as the SDK stamps them
        result.setdefault("_meta", {})["io.modelcontextprotocol/serverInfo"] = {"name": "fake", "version": version}
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
for ident in listens:
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "error": {"code": -32000, "message": "Connection closed"}}), flush=True)
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
        self.notifications: list[dict[str, Any]] = []

    def install(self, version: str) -> None:
        for old in self.site.glob("agentnave-*.dist-info"):
            (old / "RECORD").unlink()
            old.rmdir()
        dist = self.site / f"agentnave-{version}.dist-info"
        dist.mkdir(parents=True)
        (dist / "RECORD").write_text(version)
        self.busy.with_suffix(".version").write_text(version)

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.sequence += 1
        self.send({"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params or {}})
        reply = json.loads(self.reader.readline())
        while "id" not in reply:
            self.notifications.append(reply)
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


def test_relay_swaps_for_hosts_without_handshake_and_hands_over_listen_streams(
    tmp_path: Path,
) -> None:
    host = Host(tmp_path)
    meta = {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "modern", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    }

    def whoami() -> dict[str, Any]:
        result = host.call("tools/call", {"name": "whoami", "_meta": meta})
        return result["structuredContent"]

    try:
        whoami()  # Before any server/discover, the stamped result already names the runtime.
        assert host.relay.runtime_version == "1.0"
        host.call("server/discover", {"_meta": meta})
        assert host.relay.runtime_version == "1.0"
        # The listen request is answered only when its stream ends, so it must not pin the child.
        listen = {"_meta": meta, "notifications": {"toolsListChanged": True}}
        host.send(
            {"jsonrpc": "2.0", "id": "listen:0", "method": "subscriptions/listen", "params": listen}
        )
        first = whoami()
        host.install("1.1")
        deadline = time.monotonic() + 10
        while (second := whoami())["pid"] == first["pid"] and time.monotonic() < deadline:
            time.sleep(0.1)
        assert second["pid"] != first["pid"]
        assert second["listens"] == first["listens"] == 1
        assert host.relay.runtime_version == "1.1"
        # One ack from the first child; the stream's end there and the second ack stay hidden.
        assert host.notifications == [
            {
                "jsonrpc": "2.0",
                "method": "notifications/subscriptions/acknowledged",
                "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": "listen:0"}},
            }
        ]
    finally:
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


def test_relay_observes_delivered_interfaces_across_updates_and_reconnects(tmp_path: Path) -> None:
    data = tmp_path / "data"
    host = Host(
        tmp_path, env={**os.environ, "AGENTNAVE_HOST": "codex", "AGENTNAVE_DATA_DIR": str(data)}
    )

    def observed(version: str) -> dict[str, Any]:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            for path in data.glob("connections/*.json"):
                if path.name in {"dead.json", "alive.json"}:
                    continue  # Never hold a handle on the samples the relay is pruning.
                try:
                    value = json.loads(path.read_bytes())
                except OSError:  # Being pruned; Windows refuses reads of files pending deletion.
                    continue
                if value["runtime_version"] == version:
                    return value
            time.sleep(0.02)
        raise AssertionError(f"missing connection observation for {version}")

    try:
        result = host.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
        assert result["capabilities"]["tools"]["listChanged"] is True
        host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        assert not data.exists()  # Handshakes alone, including nested servers, do not create data.
        # A relay that died left its file; a live one keeps touching its own.
        (data / "connections").mkdir(parents=True)
        dead, alive = data / "connections" / "dead.json", data / "connections" / "alive.json"
        for path in (dead, alive):
            path.write_text('{"runtime_version": "other"}')
        os.utime(dead, (time.time() - 7200, time.time() - 7200))
        host.call("tools/list")
        initial = observed("1.0")
        assert not dead.exists() and alive.exists()
        alive.unlink()
        assert initial["runtime_tools_hash"] == initial["host_tools_hash"]
        assert set(initial) == {
            "version",
            "host",
            "started_ms",
            "runtime_version",
            "runtime_tools_hash",
            "host_tools_hash",
        }
        # Unchanged observations only refresh the modification time; removal rewrites it.
        path = host.relay.connection_path
        written = path.stat()
        os.utime(path, ns=(0, 0))
        host.relay._publish_connection()  # pyright: ignore[reportPrivateUsage]
        assert path.stat().st_ino == written.st_ino
        assert path.stat().st_mtime_ns > 0
        path.unlink()
        host.relay._publish_connection()  # pyright: ignore[reportPrivateUsage]
        assert json.loads(path.read_bytes()) == initial

        first = host.pid()
        host.install("1.1")
        same = observed("1.1")
        assert host.pid() != first
        assert same["runtime_tools_hash"] == same["host_tools_hash"] == initial["host_tools_hash"]
        assert host.notifications == []

        host.busy.with_suffix(".tools").write_text(
            '{"type":"object","properties":{"new":{"type":"string"}}}'
        )
        host.install("1.2")
        changed = observed("1.2")
        assert changed["runtime_tools_hash"] != changed["host_tools_hash"]
        host.call("tools/list")
        refreshed = observed("1.2")
        deadline = time.monotonic() + 5
        while (
            refreshed["runtime_tools_hash"] != refreshed["host_tools_hash"]
            and time.monotonic() < deadline
        ):
            time.sleep(0.02)
            refreshed = observed("1.2")
        assert refreshed["runtime_tools_hash"] == refreshed["host_tools_hash"]
        assert host.notifications == [
            {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}
        ]
    finally:
        host.writer.close()
        host.thread.join(10)
    assert not list(data.glob("connections/*.json"))


def test_empty_data_root_matches_service_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(launcher.Path, "home", lambda: tmp_path / "home")
    host = Host(tmp_path, env={**os.environ, "AGENTNAVE_HOST": "codex", "AGENTNAVE_DATA_DIR": ""})
    try:
        assert host.relay.connection_path.parent.parent == tmp_path / "home/.agentnave"
    finally:
        host.writer.close()
        host.thread.join(10)
    assert not (tmp_path / "home").exists()


def test_host_eof_removes_observation_and_prevents_swap_while_child_exits(tmp_path: Path) -> None:
    data = tmp_path / "data"
    gate = tmp_path / "busy.closing"
    host = Host(
        tmp_path,
        env={**os.environ, "AGENTNAVE_HOST": "codex", "AGENTNAVE_DATA_DIR": str(data)},
        script_text=FAKE_SERVER
        + '\nimport time\np = Path(sys.argv[1] + ".closing")\np.touch()\nwhile p.exists():\n    time.sleep(0.01)\n',
    )
    try:
        host.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
        host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        host.call("tools/list")
        child = host.relay.child
        host.writer.close()
        deadline = time.monotonic() + 5
        while not gate.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert gate.exists()  # Child deliberately remains alive after stdin EOF.
        assert not list(data.glob("connections/*.json"))
        host.install("1.1")
        assert host.relay._swap() == "busy"  # pyright: ignore[reportPrivateUsage]
        assert host.relay.child is child
        assert child.process.poll() is None
    finally:
        gate.unlink(missing_ok=True)
        host.thread.join(10)
    assert host.code == [0]


def test_zero_tool_relay_never_creates_connection_observations(tmp_path: Path) -> None:
    (tmp_path / "busy.tools").write_text("null")
    data = tmp_path / "data"
    host = Host(
        tmp_path, env={**os.environ, "AGENTNAVE_HOST": "codex", "AGENTNAVE_DATA_DIR": str(data)}
    )
    try:
        host.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})
        host.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        assert host.call("tools/list") == {"tools": []}
    finally:
        host.writer.close()
        host.thread.join(10)
    assert not data.exists()


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
            "run_round",
            title="Review",
            cwd=str(tmp_path),
            review=True,
            members=[{"id": seat, "provider": "claude"} for seat in ("a", "b")],
        )
        round_ = call(
            "run_round", group_id=group["group_id"], seats=["a"], order="parallel", prompt="answer"
        )["round"]
        assert call("wait", ids=[round_["round_id"]])["rounds"][0]["reason"] == "review_needed"
        listed = call("list_invocations")
        assert all(item["result_read"] for item in listed["invocations"])
        assert listed["restart_safe"] is False
        host.install("1.1")
        time.sleep(0.5)
        assert host.relay.child.process.pid == first
        call("decide_answers", round_id=round_["round_id"], decisions={"a": "publish"})
        call("wait", ids=[round_["round_id"]])
        second = changed_pid(first)
        saved = json.loads((data / "conversations" / group["group_id"] / "state.json").read_bytes())
        assert saved["rounds"][0]["seats"][0]["review"] == "publish"
        assert saved["rounds"][0]["seats"][0]["output"] == "kept answer"

        failure = tmp_path / "busy.write_failed"
        failure.touch()
        started = call("start_agent", provider="claude", prompt="answer", cwd=str(tmp_path))
        assert call("wait", ids=[started["invocation_id"]])["updates"][0]["status"] == "succeeded"
        assert call("list_invocations")["restart_safe"] is False
        host.install("1.2")
        time.sleep(0.5)
        assert host.relay.child.process.pid == second
        failure.unlink()
        # The next write to that conversation saves the held result too.
        again = call(
            "start_agent",
            provider="claude",
            prompt="again",
            cwd=str(tmp_path),
            conversation_id=started["conversation_id"],
        )
        call("wait", ids=[again["invocation_id"]])
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
