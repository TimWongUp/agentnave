"""Markers T3 Code injects into the environment of the sessions it hosts.

T3 Code runs its Antigravity ACP profile with a loopback `SSH_CONNECTION` (so the CLI prints its
sign-in URL instead of opening a browser), a `BROWSER` stub that captures that URL, file-backed
credential storage and its own `GEMINI_HOME`. A CLI started from that session must not inherit
them; the same names set by the user (a real SSH login, their own browser or Gemini home) stay.
Kept in sync with `apps/tray/src/temp_dir.rs`.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path

T3_SSH_CONNECTION = "127.0.0.1 0 127.0.0.1 0"
T3_BROWSER_MARKER = "__T3_ANTIGRAVITY_AUTH_URL__"


def _env_get(env: Mapping[str, str], target: str) -> str:
    if os.name != "nt":
        return env.get(target, "")
    for key, value in env.items():
        if key.upper() == target:
            return value
    return ""


def _matching_keys(env: Mapping[str, str], target: str) -> list[str]:
    if os.name != "nt":
        return [target] if target in env else []
    return [key for key in env if key.upper() == target]


def _in_t3_antigravity_profile(env: Mapping[str, str]) -> bool:
    app_data = _env_get(env, "ANTIGRAVITY_APP_DATA_DIR").strip()
    if app_data and Path(app_data).name == "antigravity-acp":
        return True
    if _env_get(env, "AGY_ACP_FORCE_FILE_STORAGE").strip():
        return True
    gemini_home = _env_get(env, "GEMINI_HOME").strip()
    if not gemini_home:
        return False
    t3_home = _env_get(env, "T3CODE_HOME").strip() or str(Path.home() / ".t3")
    profiles = Path(t3_home).expanduser() / "userdata" / "providers" / "antigravity"
    return Path(gemini_home).expanduser().is_relative_to(profiles)


def t3_injected_env(env: Mapping[str, str]) -> tuple[str, ...]:
    """Names to remove from a child's copy of `env`, judged on `env` before anything is removed."""
    names: list[str] = []
    if _env_get(env, "SSH_CONNECTION") == T3_SSH_CONNECTION:
        names.extend(_matching_keys(env, "SSH_CONNECTION") or ["SSH_CONNECTION"])
    if T3_BROWSER_MARKER in _env_get(env, "BROWSER"):
        names.extend(_matching_keys(env, "BROWSER") or ["BROWSER"])
    names.extend(
        _matching_keys(env, "AGY_ACP_FORCE_FILE_STORAGE") or ["AGY_ACP_FORCE_FILE_STORAGE"]
    )
    if _in_t3_antigravity_profile(env):
        names.extend(_matching_keys(env, "GEMINI_HOME") or ["GEMINI_HOME"])
    return tuple(names)
