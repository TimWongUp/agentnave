from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from agentnave import terminal_requests
from agentnave.push_capability import MAX_PUSH_BYTES, TrayIdentity


def test_request_is_private_and_cannot_be_replaced(tmp_path: Path) -> None:
    document: dict[str, object] = {"request_id": "one", "kind": "run"}
    path = terminal_requests.publish_request(tmp_path, "one", document)
    assert json.loads(path.read_text()) == document
    if os.name != "nt":
        assert path.stat().st_mode & 0o777 == 0o600
        assert path.parent.stat().st_mode & 0o777 == 0o700
    with pytest.raises(FileExistsError):
        terminal_requests.publish_request(tmp_path, "one", document)
    assert json.loads(path.read_text()) == document
    with pytest.raises(ValueError):
        terminal_requests.publish_request(tmp_path, "../other", document)


def test_read_receipt_rejects_replaced_tray_and_wrong_invocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    folder = tmp_path / "push"
    folder.mkdir()
    path = folder / "123.json"
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "pid": 123,
                "requests": {"req": {"invocation_id": "inv", "state": "started"}},
            }
        )
    )
    started = path.stat().st_mtime - 1

    def process_started(_pid: int) -> float:
        return started

    monkeypatch.setattr(terminal_requests, "read_process_started", process_started)
    tray = TrayIdentity(123, started)
    snapshot = terminal_requests.read_tray(tmp_path, tray)
    assert snapshot is not None
    assert terminal_requests.receipt(snapshot, "req", "inv") is not None
    assert terminal_requests.receipt(snapshot, "req", "another") is None
    assert terminal_requests.read_tray(tmp_path, TrayIdentity(123, started - 2)) is None
    path.write_text("x" * (MAX_PUSH_BYTES + 1))
    assert terminal_requests.read_tray(tmp_path, tray) is None
