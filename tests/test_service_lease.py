"""The shared runtime lease excludes collectors in another OS process."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from agentnave.service_lease import hold


def test_shared_lease_blocks_exclusive_collector(tmp_path: Path) -> None:
    root = tmp_path / "service" / "0.58.0"
    env = root / "tools" / "agentnave"
    env.mkdir(parents=True)
    (root / "gc-v1").write_text("1")
    (root.parent / ".leases").mkdir()
    lease = root.parent / ".leases" / "0.58.0.lock"
    lease.touch()
    script = """
import os, sys
with open(sys.argv[1], 'r+b') as lock:
    try:
        if os.name == 'nt':
            import msvcrt
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        sys.exit(2)
"""

    def collect() -> int:
        return subprocess.run(
            [sys.executable, "-c", script, str(lease)], check=False, timeout=10
        ).returncode

    with hold(env), hold(env):  # Multiple simultaneous relays share the same version.
        assert collect() == 2
    assert collect() == 0


def test_marked_install_with_missing_lock_fails_closed(tmp_path: Path) -> None:
    env = tmp_path / "service" / "0.58.0" / "tools" / "agentnave"
    env.mkdir(parents=True)
    (env.parent.parent / "gc-v1").touch()
    with pytest.raises(FileNotFoundError), hold(env):
        pass


def test_legacy_install_does_not_require_a_lease(tmp_path: Path) -> None:
    with hold(tmp_path / "legacy"):
        pass
