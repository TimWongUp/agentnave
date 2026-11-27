from __future__ import annotations

import tempfile
from pathlib import Path
from types import SimpleNamespace

import pytest

from agentnave import temp_dir


def test_a_deleted_cached_temp_dir_is_chosen_again(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gone = tmp_path / "gone"
    gone.mkdir()
    monkeypatch.setenv("TMPDIR", str(gone))
    monkeypatch.setattr(tempfile, "tempdir", str(gone))
    gone.rmdir()

    chosen = temp_dir.usable_temp_dir()

    assert chosen.is_dir() and chosen != gone
    with tempfile.TemporaryDirectory(dir=chosen) as made:
        assert Path(made).is_dir()


def test_posix_drops_missing_temp_dirs_and_keeps_valid_ones(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(temp_dir, "os", SimpleNamespace(name="posix"))
    env = {"TMPDIR": str(tmp_path / "gone"), "TEMP": "", "TMP": str(tmp_path), "tmpdir": "x"}
    temp_dir.sanitize_temp_env(env)
    assert env == {"TMP": str(tmp_path), "tmpdir": "x"}


def test_windows_replaces_missing_temp_dirs_whatever_their_case(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(temp_dir, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(temp_dir, "usable_temp_dir", lambda: tmp_path / "fallback")
    env = {"Temp": str(tmp_path / "gone"), "TMP": str(tmp_path), "PATH": "p"}
    temp_dir.sanitize_temp_env(env)
    assert env == {"Temp": str(tmp_path / "fallback"), "TMP": str(tmp_path), "PATH": "p"}
