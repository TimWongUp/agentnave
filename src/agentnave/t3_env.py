"""Markers T3 Code injects into the environment of the sessions it hosts.

T3 Code runs its Antigravity ACP profile with a loopback `SSH_CONNECTION` (so the CLI prints its
sign-in URL instead of opening a browser), a `BROWSER` stub that captures that URL, file-backed
credential storage and its own `GEMINI_HOME`. A CLI started from that session must not inherit
them; the same names set by the user (a real SSH login, their own browser or Gemini home) stay.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path

T3_SSH_CONNECTION = "127.0.0.1 0 127.0.0.1 0"
T3_BROWSER_MARKER = "__T3_ANTIGRAVITY_AUTH_URL__"


def _in_t3_antigravity_profile(env: Mapping[str, str]) -> bool:
    app_data = env.get("ANTIGRAVITY_APP_DATA_DIR", "").strip()
    if app_data and Path(app_data).name == "antigravity-acp":
        return True
    if env.get("AGY_ACP_FORCE_FILE_STORAGE", "").strip():
        return True
    gemini_home = env.get("GEMINI_HOME", "").strip()
    if not gemini_home:
        return False
    t3_home = env.get("T3CODE_HOME", "").strip() or str(Path.home() / ".t3")
    profiles = Path(t3_home).expanduser() / "userdata" / "providers" / "antigravity"
    return Path(gemini_home).expanduser().is_relative_to(profiles)


def t3_injected_env(env: Mapping[str, str]) -> tuple[str, ...]:
    """Names to remove from a child's copy of `env`, judged on `env` before anything is removed."""
    names: list[str] = []
    if env.get("SSH_CONNECTION") == T3_SSH_CONNECTION:
        names.append("SSH_CONNECTION")
    if T3_BROWSER_MARKER in env.get("BROWSER", ""):
        names.append("BROWSER")
    names.append("AGY_ACP_FORCE_FILE_STORAGE")
    if _in_t3_antigravity_profile(env):
        names.append("GEMINI_HOME")
    return tuple(names)
