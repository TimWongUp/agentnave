"""T3 Code host connection detection and child-thread guardrails (ADR-0121)."""

from __future__ import annotations

import os
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Literal

from agentnave.host import Host, argv_host
from agentnave.process_info import read_process_argv, read_process_parent

type T3Mode = Literal["none", "main", "child"]

DELEGATED_THREAD_PREFIX = "thread:delegated-task:"
_MAX_ANCESTOR_DEPTH = 32

T3_CHILD_REFUSED_TOOLS = frozenset(
    {
        "recommend_seat",
        "start_agent",
        "start_agents",
        "run_round",
        "decide_answers",
        "handover",
    }
)

T3_CHILD_INSTRUCTIONS = (
    "T3 Code delegated child thread: complete only your assigned task and report directly to the "
    "parent thread. Do not recommend seats, dispatch subagents, hold meetings, or hand over; "
    "those AgentNave tools are disabled here. "
)

_cached_mode: T3Mode | None = None


def t3_home(env: Mapping[str, str] | None = None) -> Path:
    """T3 Code data root: T3CODE_HOME if set and non-blank, otherwise ~/.t3."""
    source = os.environ if env is None else env
    raw = source.get("T3CODE_HOME", "").strip()
    return Path(raw) if raw else Path.home() / ".t3"


def _has_second_claude_ancestor(
    parent_pid: int,
    read_parent: Callable[[int], int],
    read_argv: Callable[[int], Sequence[str]],
) -> bool:
    """Walk all the way up the ancestor chain (up to 32 levels) to spot a nested claude.

    T3's own Claude process is the first match; a second claude higher in the chain means this
    connection is a nested `claude` run that merely inherited T3's environment variables.
    """
    seen: set[int] = set()
    pid = parent_pid
    claude_count = 0
    for _ in range(_MAX_ANCESTOR_DEPTH):
        if pid <= 1 or pid in seen:
            break
        seen.add(pid)
        try:
            argv = read_argv(pid)
        except (OSError, ValueError):
            break
        if argv_host(argv) == "claude":
            claude_count += 1
            if claude_count >= 2:
                return True
        try:
            pid = read_parent(pid)
        except (OSError, ValueError):
            break
    return False


def detect_t3_mode(
    host: Host,
    *,
    env: Mapping[str, str] | None = None,
    parent_pid: int | None = None,
    read_parent: Callable[[int], int] = read_process_parent,
    read_argv: Callable[[int], Sequence[str]] = read_process_argv,
    platform: str | None = None,
) -> T3Mode:
    """Determine whether this server serves a T3 Code main thread, child thread, or neither."""
    source = os.environ if env is None else env
    thread_id = source.get("T3CODE_THREAD_ID", "").strip()
    if not thread_id:
        return "none"
    child = thread_id.startswith(DELEGATED_THREAD_PREFIX)
    if host.name == "claude":
        if source.get("CLAUDE_CODE_ENTRYPOINT") != "t3-code":
            return "none"
        if child:  # A `claude` nested in a delegated thread stays within its limits.
            return "child"
        effective_platform = sys.platform if platform is None else platform
        if effective_platform != "win32" and _has_second_claude_ancestor(
            os.getppid() if parent_pid is None else parent_pid,
            read_parent,
            read_argv,
        ):
            return "none"
    elif host.name not in ("codex", "antigravity"):
        return "none"
    return "child" if child else "main"


def current_t3_mode(host: Host) -> T3Mode:
    """Detect and cache T3 mode once per server process."""
    global _cached_mode
    if _cached_mode is None:
        _cached_mode = detect_t3_mode(host)
    return _cached_mode


def reset_t3_mode_cache() -> None:
    """Clear the cached T3 mode (for tests)."""
    global _cached_mode
    _cached_mode = None


def t3_child_refusal(tool_name: str) -> str:
    """Error text when a T3 Code delegated child thread calls a disabled orchestration tool."""
    return (
        f"Refused: this connection is a T3 Code delegated child thread, so {tool_name} is "
        "unavailable. Complete only your own assigned task and report back to the parent thread "
        "without recommending seats, dispatching subagents, holding meetings, or handing over."
    )
