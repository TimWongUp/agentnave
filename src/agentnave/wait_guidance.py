"""Host-specific receipt `next` guidance and wait capability checks (ADR-0114)."""

from __future__ import annotations

import math
import os
import re
import shlex
import subprocess
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from agentnave.models import Host
from agentnave.process_info import (
    read_process_argv,
    read_process_executable,
    read_process_parent,
)
from agentnave.push_capability import PUSH_AGAIN, PUSH_NEXT, app_can_push

WAIT_SECONDS = 270
LONG_WAIT_SECONDS = 1500
LONG_WAIT_SENTENCE = (
    "In the Claude Code terminal interactive session with background tasks enabled, omitting "
    "waits up to 1500s (25 minutes) instead; pass 270 for the short window. "
)

WAIT_NEXT = (
    "Ends are not pushed: before ending your turn, call wait (omit ids, same "
    "manager_session_id) until all_finished."
)
WAIT_AGAIN = (
    "Still pending: reply to any new user message first, and handle any prompt_pending or "
    "review_needed, then call wait again "
    "before ending your turn; increment seq on each wait to distinguish repeated calls."
)

CLAUDE_LONG_WAIT_NEXT = (
    "Before ending your turn, call wait (omit ids and wait_seconds, same manager_session_id): "
    "Claude Code moves this long wait to the background and starts a new turn when results arrive."
)
CLAUDE_LONG_WAIT_AGAIN = (
    "Still pending: reply to any new user message first, and handle any prompt_pending or "
    "review_needed, then call wait again (omit ids and wait_seconds, same manager_session_id) "
    "before ending your turn — Claude Code moves it to the background and starts a new turn when "
    "results arrive; increment seq on each wait to distinguish repeated calls."
)

CLAUDE_SHORT_WAIT_NEXT = (
    "Background tasks are disabled in this Claude Code session: before ending your turn, call "
    f"wait(wait_seconds={WAIT_SECONDS}) (omit ids, same manager_session_id) until all_finished."
)
CLAUDE_SHORT_WAIT_AGAIN = (
    "Still pending (background tasks disabled): reply to any new user message first, and handle "
    f"any prompt_pending or review_needed, then call wait(wait_seconds={WAIT_SECONDS}) again "
    "before ending your turn; increment seq on each wait to distinguish repeated calls."
)

BLOCKING_WAIT_AGAIN = frozenset({WAIT_AGAIN, CLAUDE_LONG_WAIT_AGAIN, CLAUDE_SHORT_WAIT_AGAIN})

type WatchHostName = Literal["antigravity", "grok"]

VERSION_PROBE_TIMEOUT_SECONDS = 2.0


@dataclass(frozen=True)
class WatchCliRule:
    """Declarative eligibility rule for a host CLI that self-wakes on background commands."""

    host_name: WatchHostName
    basename_pattern: re.Pattern[str]
    min_version: tuple[int, ...]
    non_interactive_flags: frozenset[str]
    non_interactive_subcommands: frozenset[str] = frozenset()
    reject_path_parts: frozenset[str] = frozenset()
    reject_argv_pairs: tuple[tuple[str, str], ...] = ()


WATCH_CLI_RULES: dict[str, WatchCliRule] = {
    "antigravity": WatchCliRule(
        host_name="antigravity",
        basename_pattern=re.compile(r"^agy(?:-\d+(?:\.\d+)*)?\Z", re.IGNORECASE),
        min_version=(1, 5, 14),
        non_interactive_flags=frozenset(
            {"-p", "--print", "--prompt", "--input-format", "--output-format"}
        ),
        non_interactive_subcommands=frozenset({"acp"}),
        reject_path_parts=frozenset({"antigravity.app"}),
        reject_argv_pairs=(("--subclient_type", "hub"),),
    ),
    "grok": WatchCliRule(
        host_name="grok",
        basename_pattern=re.compile(r"^grok(?:-\d+(?:\.\d+)*)?\Z", re.IGNORECASE),
        min_version=(1, 0, 92),
        non_interactive_flags=frozenset({"-p", "--print", "--prompt", "--prompt-file"}),
    ),
}

_TRUTHY_ENV = frozenset({"1", "true", "yes", "on"})
_VERSION_RE = re.compile(r"\b(\d+(?:\.\d+)+)\b")
_WRAPPER_NAMES = frozenset(
    {
        "agentnave-mcp",
        "node",
        "bun",
        "deno",
        "uv",
        "uvx",
        "npx",
        "env",
        "sh",
        "bash",
        "zsh",
        "fish",
        "dash",
    }
)
_PYTHON_NAME = re.compile(r"python(?:3(?:\.\d+)?)?\Z")
_WATCH_HOST_CACHE: dict[str, bool] = {}


def _env_truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in _TRUTHY_ENV


def claude_background_disabled() -> bool:
    """True when Claude Code's environment disables automatic backgrounding of long MCP waits."""
    if _env_truthy("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS") or _env_truthy(
        "CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND"
    ):
        return True
    raw = os.environ.get("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "").strip()
    if raw:
        try:
            ms = float(raw)
        except ValueError:
            return False
        if math.isfinite(ms) and ms > LONG_WAIT_SECONDS * 1000:
            return True
    return False


def long_wait_enabled_for_host(host: Host) -> bool:
    """1500s waits only where Claude Code backgrounds long calls: its terminal session
    with background tasks enabled (ADR-0082, ADR-0114).
    """
    return (
        host.name == "claude"
        and os.environ.get("CLAUDE_CODE_ENTRYPOINT") == "cli"
        and not claude_background_disabled()
    )


def _is_wrapper_executable(executable: str) -> bool:
    name = Path(executable).name.lower()
    return name in _WRAPPER_NAMES or _PYTHON_NAME.fullmatch(name) is not None


def _parse_version(raw: str) -> tuple[int, ...] | None:
    match = _VERSION_RE.search(raw)
    if match is None:
        return None
    return tuple(int(part) for part in match.group(1).split("."))


def _read_cli_version(executable: str) -> str | None:
    try:
        completed = subprocess.run(
            [executable, "--version"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=VERSION_PROBE_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout or completed.stderr


def _find_host_process(
    parent_pid: int,
    read_parent: Callable[[int], int],
    read_executable: Callable[[int], str],
    read_argv: Callable[[int], Sequence[str]],
) -> tuple[str, tuple[str, ...]] | None:
    """Walk up the parent chain, skipping relay/shell/runtime wrappers, to the host binary."""
    seen: set[int] = set()
    pid = parent_pid
    for _ in range(32):
        if pid <= 1 or pid in seen:
            break
        seen.add(pid)
        try:
            executable = read_executable(pid)
        except (OSError, ValueError):
            executable = ""
        if executable and not _is_wrapper_executable(executable):
            try:
                argv = tuple(read_argv(pid))
            except (OSError, ValueError):
                return None
            return executable, argv
        try:
            pid = read_parent(pid)
        except (OSError, ValueError):
            break
    return None


def _has_rejected_pair(args: Sequence[str], pairs: tuple[tuple[str, str], ...]) -> bool:
    for index, arg in enumerate(args):
        for flag, value in pairs:
            if arg == f"{flag}={value}":
                return True
            if arg == flag and index + 1 < len(args) and args[index + 1] == value:
                return True
    return False


def _has_non_interactive_args(
    args: Sequence[str], flags: frozenset[str], subcommands: frozenset[str]
) -> bool:
    for arg in args:
        if arg == "--":
            break
        if arg in flags or any(arg.startswith(f"{flag}=") for flag in flags):
            return True
        if subcommands and arg in subcommands:
            return True
    return False


def probe_watch_host_cli(
    host_name: str,
    *,
    parent_pid: int | None = None,
    read_parent: Callable[[int], int] = read_process_parent,
    read_executable: Callable[[int], str] = read_process_executable,
    read_argv: Callable[[int], Sequence[str]] = read_process_argv,
    read_version: Callable[[str], str | None] = _read_cli_version,
    platform: str | None = None,
) -> bool:
    """Verify via parent executable path, argv, and `<exe> --version` that the calling host
    is an interactive CLI version that self-wakes on background commands (ADR-0114).

    Never short-circuited by AGENTNAVE_HOST; fails closed on Windows, unreadable ancestors,
    GUI apps, non-interactive flags/subcommands, or unverified versions.
    """
    rule = WATCH_CLI_RULES.get(host_name)
    if rule is None:
        return False
    effective_platform = sys.platform if platform is None else platform
    if effective_platform == "win32":
        return False
    found = _find_host_process(
        os.getppid() if parent_pid is None else parent_pid,
        read_parent,
        read_executable,
        read_argv,
    )
    if found is None:
        return False
    executable, argv = found
    path = Path(executable)
    if any(part.lower() in rule.reject_path_parts for part in path.parts):
        return False
    args = argv[1:]
    if _has_rejected_pair(args, rule.reject_argv_pairs):
        return False
    if rule.basename_pattern.fullmatch(path.name) is None:
        return False
    if _has_non_interactive_args(
        args, rule.non_interactive_flags, rule.non_interactive_subcommands
    ):
        return False
    raw_version = read_version(executable)
    if not raw_version:
        return False
    version = _parse_version(raw_version)
    return version is not None and version >= rule.min_version


def watch_host_cli_supported(host_name: str) -> bool:
    """Lazily probe once per server process whether `host_name` supports inbox_watch."""
    if host_name not in WATCH_CLI_RULES:
        return False
    cached = _WATCH_HOST_CACHE.get(host_name)
    if cached is not None:
        return cached
    result = probe_watch_host_cli(host_name)
    _WATCH_HOST_CACHE[host_name] = result
    return result


def watch_command(root: Path, pid: int, started_at: float, boss_key: str) -> str:
    """Format the verbatim read-only inbox_watch command for this server instance."""
    argv = [
        sys.executable,
        "-I",
        "-m",
        "agentnave.inbox_watch",
        str(root),
        str(pid),
        f"{started_at:.6f}",
        boss_key,
    ]
    if os.name == "nt":
        return subprocess.list2cmdline(argv)
    return shlex.join(argv)


def watch_guidance(command: str, *, cli: WatchHostName = "antigravity", again: bool = False) -> str:
    outcomes = (
        "On `ready` or `expired`, call inbox wait(wait_seconds=0) with the same "
        "manager_session_id (if work remains pending, follow its next); on "
        "`already_watching` or `all_done`, end this turn; on `service_gone`, check this "
        "connection with wait(wait_seconds=0) or list."
    )
    if cli == "grok":
        body = (
            "run this command verbatim via run_terminal_command with block_until_ms: 0 so it "
            f"moves to the background immediately (do not call a blocking wait): {command}. "
            f"When it exits, handle its output immediately. {outcomes}"
        )
    else:
        body = (
            "run this command verbatim (it moves to the background automatically after 10s; do "
            f"not call a blocking wait): {command}. If it returns in the foreground within 10s, "
            f"handle its output immediately without waiting for a background wake-up. {outcomes}"
        )
    if again:
        return (
            "Still pending: reply to any new user message first, and handle any prompt_pending or "
            f"review_needed, then {body}"
        )
    return body[0].upper() + body[1:]


def build_wait_guidance(
    host: Host,
    *,
    root: Path,
    pid: int,
    started_at: float,
    boss_key: str | None = None,
    again: bool = False,
    can_push: Callable[[Path, str | None], bool] = app_can_push,
    watch_supported: Callable[[str], bool] = watch_host_cli_supported,
) -> str:
    """Select receipt `next` text according to host self-wake capabilities (ADR-0114)."""
    if host.name == "claude":
        if os.environ.get("CLAUDE_CODE_ENTRYPOINT") == "cli":
            if claude_background_disabled():
                return CLAUDE_SHORT_WAIT_AGAIN if again else CLAUDE_SHORT_WAIT_NEXT
            return CLAUDE_LONG_WAIT_AGAIN if again else CLAUDE_LONG_WAIT_NEXT
        return WAIT_AGAIN if again else WAIT_NEXT
    if host.name in WATCH_CLI_RULES:
        rule = WATCH_CLI_RULES[host.name]
        if boss_key is not None and watch_supported(host.name):
            cmd = watch_command(root, pid, started_at, boss_key)
            return watch_guidance(cmd, cli=rule.host_name, again=again)
        return WAIT_AGAIN if again else WAIT_NEXT
    if host.name == "codex" and can_push(root, "codex"):
        return PUSH_AGAIN if again else PUSH_NEXT
    return WAIT_AGAIN if again else WAIT_NEXT
