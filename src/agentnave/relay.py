"""Thin STDIO relay in front of an MCP server, so hosts pick up an upgraded install by themselves.

Hosts keep one MCP process for as long as they run. The launchers therefore start this relay,
which runs the real server as a child and forwards JSON-RPC lines both ways. When the installed
distribution changes on disk and the child is idle, the relay starts a new child, replays the
host's initialize handshake to it, and retires the old one; the host never sees a disconnect.

Idle means no request is in flight either way and no invocation is running or holding an
unread result. Saved conversations reload after a swap. Only the standard library is used and
everything is imported at start, so the relay keeps working after the install it came from has
been replaced.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Sequence
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, BinaryIO, cast
from uuid import uuid4

type Fingerprint = tuple[tuple[str, int], ...]
type Message = dict[str, Any]

INTERVAL = 5.0
_RETRY_LIMIT = 300.0
_BUSY_REPORT = 600.0
_PROBE_SECONDS = 15.0
_START_SECONDS = 60.0
_STOP_SECONDS = 15.0
_RELAY_ID = "agentnave-relay"
SESSION_ENV = "AGENTNAVE_MANAGER_SESSION"
STARTED_ENV = "AGENTNAVE_MANAGER_STARTED_AT"


def main() -> None:
    """Entry point of `agentnave-mcp`; one connection identity survives child upgrades."""
    env = {
        **os.environ,
        SESSION_ENV: uuid4().hex,
        STARTED_ENV: datetime.now(UTC).isoformat(),
    }
    sys.exit(serve(command("agentnave.mcp_server"), env=env))


def command(module: str) -> list[str]:
    # -P keeps the caller's cwd off sys.path, as the console scripts do.
    return [sys.executable, "-P", "-c", f"from {module} import main; main()"]


def fingerprint(site: Path) -> Fingerprint | None:
    """Name and RECORD time of the installed distribution; None while missing or mid-install."""
    try:
        found = tuple(
            sorted(
                (path.name, (path / "RECORD").stat().st_mtime_ns)
                for path in site.glob("agentnave-*.dist-info")
            )
        )
    except OSError:
        return None
    return found or None


def _site() -> Path:
    return Path(__file__).resolve().parent.parent


class _Child:
    def __init__(
        self,
        command: Sequence[str],
        on_line: Callable[[_Child, bytes], None],
        *,
        env: dict[str, str] | None = None,
        retiring: bool = False,
    ) -> None:
        # A child that is not yet serving, or no longer, may exit without ending the relay.
        self.retiring = retiring
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env
        )
        self.outbox: queue.Queue[bytes | None] = queue.Queue()
        threading.Thread(target=self._write, daemon=True).start()
        threading.Thread(target=self._read, args=(on_line,), daemon=True).start()

    def send(self, line: bytes) -> None:
        self.outbox.put(line)

    def close(self) -> None:
        self.outbox.put(None)

    def _write(self) -> None:
        stdin = self.process.stdin
        assert stdin is not None
        try:
            while (line := self.outbox.get()) is not None:
                stdin.write(line)
                stdin.flush()
        except OSError:
            pass
        finally:
            with contextlib.suppress(OSError):
                stdin.close()

    def _read(self, on_line: Callable[[_Child, bytes], None]) -> None:
        stdout = self.process.stdout
        assert stdout is not None
        for line in stdout:
            on_line(self, line)
        on_line(self, b"")


def _messages(line: bytes) -> list[Message]:
    try:
        data: Any = json.loads(line)
    except ValueError:
        return []
    items = cast(list[Any], data if isinstance(data, list) else [data])
    return [cast(Message, item) for item in items if isinstance(item, dict)]


def _object(value: Any) -> Message | None:
    return cast(Message, value) if isinstance(value, dict) else None


def _key(value: object) -> str:
    return json.dumps(value)


def _tools_hash(result: Message | None) -> str | None:
    if not result or result.get("nextCursor"):
        return None  # Partial/paginated lists cannot prove that the full interface was delivered.
    tools = result.get("tools")
    if not isinstance(tools, list) or not tools:
        return None  # In particular, the nested zero-tool service has no runtime side effects.
    entries = [_object(t) for t in cast(list[Any], tools)]
    if any(t is None for t in entries):
        return None
    body = json.dumps(
        sorted(cast(list[Message], entries), key=lambda t: str(t.get("name"))), sort_keys=True
    )
    return hashlib.sha256(body.encode()).hexdigest()


class Relay:
    def __init__(
        self,
        command: Sequence[str],
        *,
        site: Path,
        stdin: BinaryIO,
        stdout: BinaryIO,
        interval: float,
        env: dict[str, str] | None = None,
    ) -> None:
        self.command = list(command)
        self.env = env
        self.site = site
        self.stdin = stdin
        self.stdout = stdout
        self.interval = interval
        self.lock = threading.Lock()
        self.out_lock = threading.Lock()
        self.connection_lock = threading.Lock()
        self.closing = threading.Event()
        self.stopped = threading.Event()
        self.host_pending: set[str] = set()
        self.host_methods: dict[str, str] = {}
        self.child_pending: set[str] = set()
        self.paused = False
        self.held: list[bytes] = []
        self.initialize: Message | None = None
        self.initialized: bytes | None = None
        self.replies: dict[str, queue.Queue[Message]] = {}
        self.sequence = 0
        self.code = 0
        effective_env = env if env is not None else os.environ
        self.host = effective_env.get("AGENTNAVE_HOST")
        self.connection_path = (
            Path(effective_env.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
            / "connections"
            / f"{os.getpid()}-{uuid4().hex}.json"
        )
        self.started_ms = time.time_ns() // 1_000_000
        self.runtime_version: str | None = None
        self.runtime_tools_hash: str | None = None
        self.host_tools_hash: str | None = None
        self.notify_tools = False
        self.child = _Child(self.command, self._from_child, env=self.env)

    def run(self) -> int:
        threading.Thread(target=self._from_host, daemon=True).start()
        installed = fingerprint(self.site)
        candidate: Fingerprint | None = None
        retry_delay = self.interval
        retry_at = 0.0
        since = 0.0
        reported = False
        while not self.stopped.wait(self.interval):
            self._publish_connection()
            seen = fingerprint(self.site)
            if seen is None or seen == installed:
                candidate = None
                continue
            if seen != candidate:
                # Wait one more interval for the install to settle.
                candidate, retry_delay, retry_at, since, reported = (
                    seen,
                    self.interval,
                    0.0,
                    time.monotonic(),
                    False,
                )
                continue
            if time.monotonic() < retry_at:
                continue
            outcome = self._swap()
            if outcome == "swapped":
                installed = seen
            elif outcome == "failed":
                # A new install may fail to start once (e.g. still being written); back off, retry.
                retry_delay = min(_RETRY_LIMIT, retry_delay * 2)
                retry_at = time.monotonic() + retry_delay
            elif not reported and time.monotonic() - since > _BUSY_REPORT:
                reported = True
                print(f"agentnave relay: update waiting, {self._busy_reason()}", file=sys.stderr)
        with self.connection_lock, contextlib.suppress(OSError):
            self.connection_path.unlink(missing_ok=True)
        return self.code

    def _publish_connection(self) -> None:
        """Best-effort, private observation; never contains prompts, tool bodies or credentials."""
        with self.connection_lock:
            self._write_connection()

    def _write_connection(self) -> None:
        with self.lock:
            if (
                self.closing.is_set()
                or self.stopped.is_set()
                or not self.host
                or not self.host_tools_hash
            ):
                return
            payload = {
                "version": 1,
                "host": self.host,
                "started_ms": self.started_ms,
                "runtime_version": self.runtime_version,
                "runtime_tools_hash": self.runtime_tools_hash,
                "host_tools_hash": self.host_tools_hash,
            }
        path = self.connection_path
        try:
            if path.parent.is_symlink() or path.parent.parent.is_symlink():
                return
            path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
                temporary = Path(output.name)
                try:
                    output.write(json.dumps(payload).encode())
                    output.close()
                    os.replace(temporary, path)
                finally:
                    temporary.unlink(missing_ok=True)
        except OSError:
            pass  # Tray observations must never break the transport.

    def _busy_reason(self) -> str:
        with self.lock:
            if self.initialize is None or self.initialized is None:
                return "the host has not finished the MCP handshake"
            if self.host_pending or self.child_pending:
                return (
                    f"{len(self.host_pending)} host and {len(self.child_pending)} server "
                    "requests are unanswered"
                )
        return "an invocation is running or holds an unread result"

    # Host -> child

    def _from_host(self) -> None:
        for line in self.stdin:
            with self.lock:
                if self.paused:
                    self.held.append(line)
                    continue
                self._note_host(line)
                child = self.child
            child.send(line)
        with self.lock:
            self.closing.set()
            child = self.child
            child.retiring = True  # the host left; the child's exit is expected
        with self.connection_lock, contextlib.suppress(OSError):
            self.connection_path.unlink(missing_ok=True)
        child.close()
        self.code = child.process.wait()
        self.stopped.set()

    def _note_host(self, line: bytes) -> None:
        for message in _messages(line):
            method = message.get("method")
            if "id" in message and method is not None:
                self.host_pending.add(_key(message["id"]))
                self.host_methods[_key(message["id"])] = str(method)
            elif "id" in message:
                self.child_pending.discard(_key(message["id"]))
            if method == "notifications/cancelled":
                # A cancelled request may never get a reply.
                params = _object(message.get("params")) or {}
                self.host_pending.discard(_key(params.get("requestId")))
                self.host_methods.pop(_key(params.get("requestId")), None)
            elif method == "initialize":
                self.initialize = message
            elif method == "notifications/initialized":
                self.initialized = line

    # Child -> host

    def _from_child(self, child: _Child, line: bytes) -> None:
        if not line:
            if not child.retiring:
                self.code = child.process.wait() or 1  # the server quit on its own
                self.stopped.set()
            return
        messages = _messages(line)
        if len(messages) == 1 and "method" not in messages[0]:
            reply = self.replies.get(str(messages[0].get("id")))
            if reply is not None:
                reply.put(messages[0])
                return
        with self.lock:
            rewritten = False
            delivered_hash = None
            completed: list[str] = []
            for message in messages:
                if "id" in message and "method" in message:
                    self.child_pending.add(_key(message["id"]))
                elif "id" in message:
                    ident = _key(message["id"])
                    completed.append(ident)
                    method = self.host_methods.pop(ident, None)
                    result = _object(message.get("result"))
                    if method == "initialize" and result:
                        info = _object(result.get("serverInfo")) or {}
                        self.runtime_version = str(info.get("version", "")) or None
                        capabilities = _object(result.get("capabilities")) or {}
                        tools = _object(capabilities.get("tools"))
                        # Legacy STDIO notifications need no subscription. New protocol versions
                        # require subscriptions/listen, which this relay does not synthesize.
                        self.notify_tools = (
                            tools is not None
                            and str(result.get("protocolVersion", "")) < "2026-07-28"
                        )
                        if self.notify_tools and tools is not None:
                            tools["listChanged"] = True
                            rewritten = True
                    elif method == "tools/list":
                        delivered_hash = _tools_hash(result)
                        self.runtime_tools_hash = delivered_hash
                elif message.get("method") == "notifications/cancelled":
                    params = _object(message.get("params")) or {}
                    self.child_pending.discard(_key(params.get("requestId")))
        with self.out_lock:
            self.stdout.write(
                (json.dumps(messages[0] if len(messages) == 1 else messages).encode() + b"\n")
                if rewritten
                else line
            )
            self.stdout.flush()
            with self.lock:
                self.host_pending.difference_update(completed)
                if delivered_hash:
                    self.host_tools_hash = delivered_hash
        self._publish_connection()

    def _ask(self, child: _Child, method: str, params: Message) -> Message | None:
        self.sequence += 1
        ident = f"{_RELAY_ID}-{self.sequence}"
        reply: queue.Queue[Message] = queue.Queue()
        self.replies[ident] = reply
        line = json.dumps({"jsonrpc": "2.0", "id": ident, "method": method, "params": params})
        child.send(line.encode() + b"\n")
        deadline = time.monotonic() + (_START_SECONDS if method == "initialize" else _PROBE_SECONDS)
        try:
            while child.process.poll() is None and time.monotonic() < deadline:
                try:
                    return reply.get(timeout=0.2)
                except queue.Empty:
                    continue
            return None
        finally:
            self.replies.pop(ident, None)

    # Swapping

    def _quiet(self) -> bool:
        return not (self.host_pending or self.child_pending)

    def _idle(self, child: _Child) -> bool:
        reply = self._ask(child, "tools/call", {"name": "list_invocations", "arguments": {}})
        result = _object(reply.get("result")) if reply else None
        if result is None or result.get("isError"):
            return False
        listed = _object(result.get("structuredContent"))
        if listed is not None and "invocations" not in listed:
            listed = _object(listed.get("result"))
        if listed is None or listed.get("restart_safe") is not True:
            return False
        invocations: Any = listed.get("invocations") if listed else None
        if not isinstance(invocations, list):
            return False
        items = [_object(item) for item in cast(list[Any], invocations)]
        return all(
            item is not None and item.get("status") != "running" and item.get("result_read")
            for item in items
        )

    def _swap(self) -> str:
        """Return "swapped", "busy" (try again later) or "failed" (the new install did not start)."""
        with self.lock:
            if (
                self.closing.is_set()
                or self.initialize is None
                or self.initialized is None
                or not self._quiet()
            ):
                return "busy"
            self.paused = True
            old = self.child
        new: _Child | None = None
        try:
            if not self._idle(old):
                return "busy"
            with self.lock:
                if not self._quiet() or self.closing.is_set() or self.stopped.is_set():
                    return "busy"
            try:
                new = _Child(self.command, self._from_child, env=self.env, retiring=True)
            except OSError:
                return "failed"
            params = _object(self.initialize.get("params")) or {}
            reply = self._ask(new, "initialize", params)
            if not reply or "result" not in reply:
                return "failed"
            new.send(self.initialized)
            tools_reply = self._ask(new, "tools/list", {}) if self.host_tools_hash else None
            current_hash = _tools_hash(_object(tools_reply.get("result")) if tools_reply else None)
            old.retiring = True
            old.close()
            try:
                old.process.wait(_STOP_SECONDS)
            except subprocess.TimeoutExpired:
                old.process.kill()
            with self.lock:
                if self.closing.is_set():
                    return "busy"
                new.retiring = False
                self.child, new = new, None
                info = _object((_object(reply.get("result")) or {}).get("serverInfo")) or {}
                self.runtime_version = str(info.get("version", "")) or None
                self.runtime_tools_hash = current_hash
                changed = current_hash is not None and current_hash != self.host_tools_hash
            self._publish_connection()
            if changed and self.notify_tools:
                with self.out_lock:
                    self.stdout.write(
                        b'{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n'
                    )
                    self.stdout.flush()
            print("agentnave relay: switched to the updated installation", file=sys.stderr)
            return "swapped"
        finally:
            if new is not None:
                new.close()
                try:
                    new.process.wait(_STOP_SECONDS)
                except subprocess.TimeoutExpired:
                    new.process.kill()
            with self.lock:
                held, self.held, self.paused = self.held, [], False
                for line in held:
                    self._note_host(line)
                child = self.child
            for line in held:
                child.send(line)


def serve(
    command: Sequence[str],
    *,
    site: Path | None = None,
    stdin: BinaryIO | None = None,
    stdout: BinaryIO | None = None,
    interval: float = INTERVAL,
    env: dict[str, str] | None = None,
) -> int:
    """Relay one host connection until the host closes it; returns the exit code."""
    return Relay(
        command,
        site=site or _site(),
        stdin=stdin or sys.stdin.buffer,
        stdout=stdout or sys.stdout.buffer,
        interval=interval,
        env=env,
    ).run()
