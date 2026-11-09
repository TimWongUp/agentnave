"""The pseudo-terminal Claude Code runs in, and the process groups its tools start."""

from __future__ import annotations

import contextlib
import os
import select
import signal
import subprocess
import sys
import time

from agentnave.claude_tui.screen import Screen

# Imported only by the POSIX run path, so importing pure helpers works on Windows too.
STRIP_ENV = (
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PROJECT_DIR",
)


def child_environment(executable: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in STRIP_ENV}
    paths = env.get("PATH", "").split(os.pathsep)
    if paths and paths[0] == os.path.dirname(sys.executable):
        paths.pop(0)
    env["PATH"] = os.pathsep.join([os.path.dirname(executable), *paths])
    env["TERM"] = "xterm-256color"
    return env


class Terminal:
    # Declared here: Windows type checking treats the POSIX-only __init__ as unreachable.
    screen: Screen
    outgoing: bytearray
    master: int
    process: subprocess.Popen[bytes]
    groups: set[int]
    groups_at: float

    def __init__(
        self,
        executable: str,
        args: list[str],
        extra_env: dict[str, str] | None = None,
        screen: type[Screen] = Screen,
    ) -> None:
        assert sys.platform != "win32"  # POSIX PTY runtime; Windows runs Claude with -p.
        import fcntl
        import struct
        import termios

        self.screen = screen()
        self.outgoing = bytearray()
        self.master, slave = os.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
            self.process = subprocess.Popen(
                [executable, *args],
                stdin=slave,
                stdout=slave,
                stderr=slave,
                env={**child_environment(executable), **(extra_env or {})},
                close_fds=True,
            )
        except BaseException:
            os.close(self.master)
            raise
        finally:
            os.close(slave)
        os.set_blocking(self.master, False)
        # Refreshed while running: on cancellation Claude may exit before close() looks.
        self.groups = set()
        self.groups_at = 0.0

    def poll(self) -> str:
        if time.monotonic() - self.groups_at >= 2:
            self.groups |= spawned_groups(self.process.pid)
            self.groups_at = time.monotonic()
        readable, writable, _ = select.select(
            [self.master],
            [self.master] if self.outgoing else [],
            [],
            0.05,
        )
        if readable:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                data = b""
            self.outgoing.extend(b"\x1b[1;1R" * self.screen.feed(data))
        if writable:
            try:
                count = os.write(self.master, self.outgoing[:65536])
                del self.outgoing[:count]
            except BlockingIOError:
                pass
        return self.screen.text()

    def close(self) -> None:
        """Stop Claude and the commands it started in their own process groups, within the
        supervisor's two-second grace: those groups would outlive the supervisor's cleanup."""
        assert sys.platform != "win32"  # POSIX PTY runtime; Windows runs Claude with -p.
        try:
            groups = self.groups | spawned_groups(self.process.pid)
            signal_groups(groups, signal.SIGTERM)
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
            signal_groups(groups, signal.SIGKILL)
        finally:
            os.close(self.master)


def spawned_groups(root: int) -> set[int]:
    """Process groups led by descendants of root, other than this helper's own group."""
    assert sys.platform != "win32"  # POSIX PTY runtime; Windows runs Claude with -p.
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=,ppid=,pgid="], capture_output=True, text=True, check=False
    ).stdout
    children: dict[int, list[int]] = {}
    groups: dict[int, int] = {}
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) == 3 and all(field.isdigit() for field in fields):
            pid, ppid, pgid = (int(field) for field in fields)
            children.setdefault(ppid, []).append(pid)
            groups[pid] = pgid
    found: set[int] = set()
    stack = [root]
    while stack:
        for child in children.get(stack.pop(), []):
            found.add(child)
            stack.append(child)
    own = os.getpgrp()
    return {pid for pid in found if groups.get(pid) == pid and pid != own}


def signal_groups(groups: set[int], signum: int) -> None:
    assert sys.platform != "win32"  # POSIX PTY runtime; Windows runs Claude with -p.
    for group in groups:
        with contextlib.suppress(OSError):
            os.killpg(group, signum)
