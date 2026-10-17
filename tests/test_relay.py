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
from agentnave.relay import serve

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
        result = {"structuredContent": {"invocations": busy}}
    else:
        session = os.environ.get("AGENTNAVE_MANAGER_SESSION")
        result = {"structuredContent": {"pid": os.getpid(), "session": session}}
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
"""


class Host:
    def __init__(self, tmp_path: Path, env: dict[str, str] | None = None) -> None:
        script = tmp_path / "server.py"
        script.write_text(FAKE_SERVER)
        self.busy = tmp_path / "busy"
        self.site = tmp_path / "site"
        self.install("1.0")
        to_relay, self.stdin = os.pipe()
        self.stdout, from_relay = os.pipe()
        self.reader = os.fdopen(self.stdout, "rb")
        self.writer = os.fdopen(self.stdin, "wb")
        self.code: list[int] = []
        relay_in, relay_out = os.fdopen(to_relay, "rb"), os.fdopen(from_relay, "wb")

        def run() -> None:
            self.code.append(
                serve(
                    [sys.executable, str(script), str(self.busy)],
                    site=self.site,
                    stdin=relay_in,
                    stdout=relay_out,
                    interval=0.05,
                    env=env,
                )
            )

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
