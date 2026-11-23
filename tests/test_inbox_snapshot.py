import json
from pathlib import Path

from agentnave.inbox_snapshot import BossInbox, InboxSnapshot


def test_snapshot_changes_only_when_state_changes(tmp_path: Path) -> None:
    snapshot = InboxSnapshot(tmp_path, 123, started_at=1700000000.5)
    bosses: dict[str, BossInbox] = {
        "s-boss": {
            "waiting": False,
            "in_flight": 1,
            "event_seq": 1,
            "delivered_seq": 0,
            "items": [{"key": "end:1", "kind": "task_end", "role": None, "status": "succeeded"}],
        }
    }
    snapshot.publish(bosses)
    before = snapshot.path.stat().st_mtime_ns
    document = json.loads(snapshot.path.read_text())
    assert document["started_at"] == 1700000000.5
    assert document["bosses"] == bosses
    snapshot.publish(bosses)
    assert snapshot.path.stat().st_mtime_ns == before
    # Clearing items keeps the snapshot alive while work is still in flight.
    bosses["s-boss"]["items"].clear()
    bosses["s-boss"]["delivered_seq"] = 1
    snapshot.publish(bosses)
    assert snapshot.path.exists()
    assert json.loads(snapshot.path.read_text())["bosses"]["s-boss"]["in_flight"] == 1
    # Dropping in_flight to 0 with no waiting and no items removes the file.
    bosses["s-boss"]["in_flight"] = 0
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
