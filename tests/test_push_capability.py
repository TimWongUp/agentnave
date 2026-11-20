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
