import json
from pathlib import Path

from agentnave.inbox_snapshot import BossInbox, InboxSnapshot


def test_snapshot_changes_only_when_state_changes(tmp_path: Path) -> None:
    snapshot = InboxSnapshot(tmp_path, 123)
    bosses: dict[str, BossInbox] = {
        "s-boss": {
            "waiting": False,
            "items": [{"key": "end:1", "kind": "task_end", "role": None, "status": "succeeded"}],
        }
    }
    snapshot.publish(bosses)
    before = snapshot.path.stat().st_mtime_ns
    assert json.loads(snapshot.path.read_text())["bosses"] == bosses
    snapshot.publish(bosses)
    assert snapshot.path.stat().st_mtime_ns == before
    bosses["s-boss"]["items"].clear()
    snapshot.publish(bosses)
    assert not snapshot.path.exists()
    bosses["s-boss"]["waiting"] = True
    snapshot.publish(bosses)
    assert snapshot.path.exists()
    snapshot.close()
    assert not snapshot.path.exists()


def test_settlement_ack_keeps_snapshot_without_inbox_events(tmp_path: Path) -> None:
    snapshot = InboxSnapshot(tmp_path, 123)
    acknowledgements: list[dict[str, object]] = [
        {"tray_pid": 456, "tray_started_at": 123.0, "request_id": "req"}
    ]
    snapshot.publish({}, acknowledgements)
    assert json.loads(snapshot.path.read_text())["settled_requests"] == acknowledgements
    snapshot.publish({}, [])
    assert not snapshot.path.exists()
