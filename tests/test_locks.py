"""Disposable locks exclude concurrent holders without keeping detached inodes alive."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from agentnave.core import InvocationManager
from agentnave.locks import lock_file, release_and_remove
from agentnave.models import InvocationRequest


def test_native_start_failure_and_repeated_release_preserve_next_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    manager.session_locks = tmp_path
    request = InvocationRequest("claude", "resume", tmp_path, "existing-session")
    held = manager.reserve(request)
    assert held is not None
    path = Path(held.name)

    def fail(_request: InvocationRequest) -> None:
        raise ValueError("prepare failed")

    monkeypatch.setattr(manager, "prepare", fail)
    with pytest.raises(ValueError, match="prepare failed"):
        manager.start(request, claim=held)
    assert held.closed and not path.exists()
    with lock_file(path, "busy") as next_holder:
        release_and_remove(held, path)
        assert path.exists()
        with pytest.raises(ValueError, match="busy"):
            lock_file(path, "busy")
        release_and_remove(next_holder, path)
    assert not path.exists()


@pytest.mark.skipif(os.name == "nt", reason="Windows cannot replace another open handle's path")
@pytest.mark.parametrize("replace", [False, True])
def test_lock_retries_when_the_path_disappears_or_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, replace: bool
) -> None:
    path = tmp_path / "session.lock"
    stat = Path.stat
    checked = False

    def swap(self: Path, *, follow_symlinks: bool = True) -> os.stat_result:
        nonlocal checked
        if self == path and not checked:
            checked = True
            self.unlink()
            if replace:
                self.write_bytes(b"replacement")
        return stat(self, follow_symlinks=follow_symlinks)

    monkeypatch.setattr(Path, "stat", swap)
    with lock_file(path, "busy") as held:
        assert checked
        assert os.fstat(held.fileno()).st_ino == path.stat().st_ino
        with pytest.raises(ValueError, match="busy"):
            lock_file(path, "busy")


@pytest.mark.skipif(os.name == "nt", reason="Windows cannot unlink another open handle's path")
def test_lock_retry_limit_reports_busy(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    path = tmp_path / "session.lock"
    attempts = 0
    stat = Path.stat

    def remove(self: Path, *, follow_symlinks: bool = True) -> os.stat_result:
        nonlocal attempts
        if self == path:
            attempts += 1
            self.unlink()
        return stat(self, follow_symlinks=follow_symlinks)

    monkeypatch.setattr(Path, "stat", remove)
    with pytest.raises(ValueError, match="busy"):
        lock_file(path, "busy")
    assert attempts == 5


def test_existing_only_lock_does_not_recreate_a_missing_file(tmp_path: Path) -> None:
    path = tmp_path / "gone.lock"
    with pytest.raises(FileNotFoundError):
        lock_file(path, "busy", create=False)
    assert not path.exists()
