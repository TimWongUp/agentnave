from __future__ import annotations

from typing import Any, cast

from agentnave.native_prompts import NativePrompt
from agentnave.store import Conversation, RoundRecord, SeatRecord, SeatRunRecord, TaskRecord
from agentnave.views import snapshot


def test_tray_snapshots_keep_prompt_audits_for_tasks_and_all_meeting_rounds() -> None:
    pending = NativePrompt("permission", "permission", "Write", '{"file_path":"/tmp/a"}')
    answered = NativePrompt("question", "question", "AskUserQuestion")
    answered.questions = [{"question": "Where?"}]
    answered.status = "answered"
    answered.answer = {
        "actor": "boss",
        "answers": {"Where?": "Here"},
        "answered_at": "2026-11-05T00:00:01Z",
    }
    audit = [pending.history() | {"invocation_id": "call"}, answered.history()]
    task = Conversation(
        id="task",
        title="task",
        kind="task",
        tasks=[
            TaskRecord(
                id="run",
                provider="claude",
                model="m",
                cwd="/tmp",
                prompt="work",
                time="2026-11-05T00:00:00Z",
                prompts=audit,
            )
        ],
    )
    projected = cast(dict[str, Any], snapshot(task)["director"])
    assert projected["tasks"][0]["prompts"] == audit
    meeting = Conversation(
        id="meeting",
        title="meeting",
        kind="show",
        seats=[SeatRecord(id="a", provider="claude", cwd="/tmp")],
        rounds=[
            RoundRecord(
                id="one",
                number=1,
                order="parallel",
                status="finished",
                seats=[SeatRunRecord(seat_id="a", status="succeeded", prompts=audit)],
            ),
            RoundRecord(
                id="two",
                number=2,
                order="parallel",
                status="running",
                seats=[SeatRunRecord(seat_id="a", status="running", prompts=audit[:1])],
            ),
        ],
    )
    projected = cast(dict[str, Any], snapshot(meeting)["director"])
    assert projected["round"]["seats"][0]["prompts"] == audit[:1]
    assert projected["rounds"][0]["seats"][0]["prompts"] == audit
    assert projected["rounds"][1]["seats"][0]["prompts"] == audit[:1]
