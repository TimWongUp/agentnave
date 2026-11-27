"""Temporary-directory variables that may name a directory someone else already deleted.

A host can hand the service a TMPDIR it removes later (a T3 Code or Antigravity turn ends);
tempfile caches the first answer for the process's whole life, and children inherit the variable
as it stands. This module has no agentnave imports so adapters, git and the supervisor can share it.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

TEMP_ENV_NAMES = ("TMPDIR", "TEMP", "TMP")


def usable_temp_dir() -> Path:
    """tempfile's directory, chosen again when the cached one no longer exists.

    os.environ stays untouched: gettempdir skips candidates it cannot write and falls back to
    the platform defaults. Racing callers at worst both reset the cache and pick the same answer.
    """
    cached = tempfile.tempdir
    if cached is not None and not Path(cached).is_dir():
        tempfile.tempdir = None
    return Path(tempfile.gettempdir())


def sanitize_temp_env(env: dict[str, str]) -> None:
    """Drop (POSIX) or replace (Windows) temp variables that name no existing directory.

    POSIX children then fall back to their own default; Windows programs expect %TEMP% and %TMP%
    to be set, so they get a directory that exists. A valid custom value is kept as is.
    """
    windows = os.name == "nt"
    for key in list(env):
        name = key.upper() if windows else key
        if name not in TEMP_ENV_NAMES:
            continue
        value = env[key]
        if value and Path(value).is_dir():
            continue
        if windows:
            env[key] = str(usable_temp_dir())
        else:
            del env[key]
