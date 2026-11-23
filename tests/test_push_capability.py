from __future__ import annotations

import json
import os
import time
from pathlib import Path

import pytest

from agentnave import push_capability
from agentnave.process_info import read_process_started


def test_push_requires_live_matching_ancestor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "push"
    directory.mkdir()
    path = directory / "20.json"
    path.write_text(json.dumps({"version": 1, "pid": 20, "providers": ["grok"]}))
    created = path.stat().st_mtime

    def parent(pid: int) -> int:
        return {30: 20, 20: 1}[pid]

    def started(_pid: int) -> float:
        return created - 1

    monkeypatch.setattr(push_capability.sys, "platform", "darwin")
    monkeypatch.setattr(push_capability.os, "getppid", lambda: 30)
    monkeypatch.setattr(push_capability, "read_process_parent", parent)
    monkeypatch.setattr(push_capability, "read_process_started", started)
    assert push_capability.app_can_push(tmp_path, "grok")
    assert not push_capability.app_can_push(tmp_path, "claude")

    def reused(_pid: int) -> float:
        return created + 10

    monkeypatch.setattr(push_capability, "read_process_started", reused)
    assert not push_capability.app_can_push(tmp_path, "grok")
    monkeypatch.setattr(push_capability.sys, "platform", "win32")
    assert not push_capability.app_can_push(tmp_path, "grok")


def test_push_matches_codex_app_server_wrapper_chain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "push"
    directory.mkdir()
    path = directory / "10.json"
    path.write_text(json.dumps({"version": 1, "pid": 10, "providers": ["codex"]}))
    created = path.stat().st_mtime

    def parent(pid: int) -> int:
        return {80: 50, 50: 20, 20: 10, 10: 1}[pid]

    def started(_pid: int) -> float:
        return created - 1

    # mcp_server (90) -> relay (80) -> codex app-server (50) -> wrapper (20) -> tray (10)
    monkeypatch.setattr(push_capability.sys, "platform", "darwin")
    monkeypatch.setattr(push_capability.os, "getppid", lambda: 80)
    monkeypatch.setattr(push_capability, "read_process_parent", parent)
    monkeypatch.setattr(push_capability, "read_process_started", started)
    assert push_capability.app_can_push(tmp_path, "codex")


@pytest.mark.skipif(os.name == "nt", reason="POSIX process metadata")
def test_process_start_time_is_current() -> None:
    started = read_process_started(os.getpid())
    assert time.time() - 3600 < started <= time.time()


def test_push_accepts_large_snapshot_but_not_over_limit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    folder = tmp_path / "push"
    folder.mkdir()
    path = folder / "42.json"
    path.write_text(
        json.dumps({"version": 1, "pid": 42, "providers": ["claude"], "padding": "x" * 20000})
    )
    started = path.stat().st_mtime - 1
    monkeypatch.setattr(push_capability.sys, "platform", "darwin")
    monkeypatch.setattr(push_capability.os, "getppid", lambda: 42)

    def parent(_pid: int) -> int:
        return 1

    def birth(_pid: int) -> float:
        return started

    monkeypatch.setattr(push_capability, "read_process_parent", parent)
    monkeypatch.setattr(push_capability, "read_process_started", birth)
    assert push_capability.app_can_push(tmp_path, "claude")
    path.write_text("x" * (push_capability.MAX_PUSH_BYTES + 1))
    assert not push_capability.app_can_push(tmp_path, "claude")


def test_find_tray_without_provider_ignores_push_allowlist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    folder = tmp_path / "push"
    folder.mkdir()
    path = folder / "42.json"
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "pid": 42,
                "providers": ["codex"],
                "terminal_providers": ["claude", "codex", "antigravity"],
            }
        )
    )
    started = path.stat().st_mtime - 1

    def parent(_pid: int) -> int:
        return 1

    def birth(_pid: int) -> float:
        return started

    def reused(_pid: int) -> float:
        return started + 10

    monkeypatch.setattr(push_capability.sys, "platform", "darwin")
    monkeypatch.setattr(push_capability.os, "getppid", lambda: 42)
    monkeypatch.setattr(push_capability, "read_process_parent", parent)
    monkeypatch.setattr(push_capability, "read_process_started", birth)

    assert push_capability.find_tray(tmp_path, None) == push_capability.TrayIdentity(42, started)
    assert not push_capability.app_can_push(tmp_path, None)
    assert not push_capability.app_can_push(tmp_path, "claude")
    assert push_capability.app_can_push(tmp_path, "codex")

    # PID reuse (process started after file mtime) is still rejected without provider filter.
    monkeypatch.setattr(push_capability, "read_process_started", reused)
    assert push_capability.find_tray(tmp_path, None) is None
