from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import cast
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import pytest

from agentnave.adapters import get_adapter
from agentnave.adapters.base import PreparedCommand
from agentnave.core import InvocationManager
from agentnave.discussion import DiscussionRooms, Room, RoomState, Seat, TaskRun
from agentnave.models import InvocationRequest, InvocationResult, InvocationStatus
from agentnave.workbench import Workbench


def seats() -> list[Seat]:
    return [
        Seat(id=p, label=p.title(), provider=p, model="explicit-model", effort="low")
        for p in ("claude", "grok")
    ]


def fetch(url: str, *, headers: dict[str, str] | None = None, method: str = "GET") -> bytes:
    with urlopen(Request(url, headers=headers or {}, method=method), timeout=3) as response:
        return response.read()


def cache_control(url: str) -> str | None:
    with urlopen(url, timeout=3) as response:
        return response.headers["Cache-Control"]


@pytest.mark.asyncio
async def test_delete_preserves_native_files_and_retries_metadata_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    rooms = Workbench(manager, tmp_path / "data")
    external = tmp_path / "external"
    try:
        opened = rooms.open(external, "Keep native files", seats(), "blind")
        rid = opened.room.id
        room = rooms.get(rid)
        native = external / "seats" / "old-session"
        native.mkdir(parents=True)
        (native / "history.txt").write_text("native history")
        empty = external / "seats" / "empty-session"
        empty.mkdir()
        room.state.archived = True
        room.state.blind_round_id = "unresolved"
        with pytest.raises(ValueError, match="Resolve active work"):
            rooms.delete(rid)
        room.state.blind_round_id = None
        rooms.changed(room)
        entry = rooms.catalog / (rid + ".json")
        unlink = Path.unlink

        def fail_catalog(path: Path, missing_ok: bool = False) -> None:
            if path == entry:
                raise OSError("catalog unavailable")
            unlink(path, missing_ok=missing_ok)

        with monkeypatch.context() as patch:
            patch.setattr(Path, "unlink", fail_catalog)
            with pytest.raises(OSError, match="catalog unavailable"):
                rooms.delete(rid)
        assert rooms.get(rid).state.archived
        assert entry.exists() and (external / "room.json").exists()
        deleted = rooms.delete(rid)
        assert deleted["retained_paths"] == [str(external)]
        assert (native / "history.txt").read_text() == "native history"
        assert not empty.exists()
        assert not entry.exists() and not (external / "room.json").exists()
        assert external.exists()
        with pytest.raises(HTTPError) as error:
            await asyncio.to_thread(fetch, opened.public_url + "state")
        assert error.value.code == 404
    finally:
        await manager.shutdown()
        await rooms.shutdown()
    restored = Workbench(InvocationManager(), tmp_path / "data")
    try:
        assert restored.list() == []
        assert restored.unavailable == []
    finally:
        await restored.shutdown()


@pytest.mark.asyncio
async def test_delete_keeps_lock_identity_when_another_writer_opens(tmp_path: Path) -> None:
    manager = InvocationManager()
    rooms = Workbench(manager, tmp_path / "data")
    contenders: list[Room] = []
    try:
        opened = rooms.open(tmp_path / "external", "Old room", seats())
        room = rooms.get(opened.room.id)
        rooms.update(room.state.id, None, True)
        lock = room.lock
        assert lock is not None
        close = lock.close

        def open_after_unlock() -> None:
            close()
            contenders.append(Room(room.directory, "New room", seats()))

        # Reproduce a new writer taking the lock immediately after deletion releases it.
        with pytest.MonkeyPatch.context() as patch:
            patch.setattr(lock, "close", open_after_unlock)
            rooms.delete(room.state.id)
        with pytest.raises(ValueError, match="held by another AgentNave server"):
            Room(room.directory, "New room", seats())
        assert RoomState.model_validate_json((room.directory / "room.json").read_bytes()).title == (
            "New room"
        )
    finally:
        for contender in contenders:
            contender.release()
        await manager.shutdown()
        await rooms.shutdown()


@pytest.mark.asyncio
async def test_shared_data_directory_indexes_history_and_locks_on_first_write(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = Workbench(InvocationManager(), tmp_path / "data")
    second: Workbench | None = None
    try:
        held = first.open(first.new_directory(), "Held", [], "task").room.id
        room = first.get(held)
        room.state.tasks.append(
            TaskRun(id="run", provider="claude", cwd=str(tmp_path), session_id="s1", time="t")
        )
        room.state.tasks[0].status = "succeeded"
        first.changed(room)
        free = first.open(first.new_directory(), "Free", [], "task").room.id
        # Simulate history the first server indexed but never wrote in this process.
        first.find(free).release()
        second = Workbench(InvocationManager(), tmp_path / "data")
        assert {item.room_id: item.writable for item in second.list(probe=True)} == {
            held: False,
            free: True,
        }
        with pytest.raises(ValueError, match="held by another AgentNave server"):
            second.start_task(
                InvocationRequest(provider="claude", prompt="go", cwd=tmp_path, session_id="s1"),
                None,
                None,
            )
        with pytest.raises(ValueError, match="held by another AgentNave server"):
            second.update(held, "Taken", None)
        assert second.update(free, "Renamed", None).room.title == "Renamed"
        listed = {item.room_id: item for item in first.list(probe=True)}
        # The first response already reflects the other writer's saved fields.
        assert (listed[free].writable, listed[free].title) == (False, "Renamed")
        assert (await first.read(free)).room.title == "Renamed"
        late = first.open(first.new_directory(), "Late", [], "task").room.id
        late_room = first.get(late)
        late_room.state.tasks.append(
            TaskRun(id="late-run", provider="claude", cwd=str(tmp_path), session_id="s2", time="t")
        )
        late_room.state.tasks[0].status = "succeeded"
        first.changed(late_room)
        late_room.release()

        async def finished(invocation_id: str, timeout: float) -> InvocationResult:
            return InvocationResult(InvocationStatus.SUCCEEDED, "claude", "ok", "s2", {}, 1)

        def start(request: InvocationRequest, *, prepared: PreparedCommand | None = None) -> str:
            return "resumed"

        monkeypatch.setattr(second.manager, "start", start)
        monkeypatch.setattr(second.manager, "wait", finished)
        # Resuming a session created after this server started keeps its original conversation.
        _, resumed = second.start_task(
            InvocationRequest(provider="claude", prompt="go", cwd=tmp_path, session_id="s2"),
            None,
            None,
        )
        assert resumed.room.id == late
        await second.collectors[-1]
    finally:
        await first.shutdown()
        if second is not None:
            await second.shutdown()


@pytest.mark.asyncio
async def test_shared_board_private_desk_and_independent_session_resume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    requests: list[InvocationRequest] = []

    def start(request: InvocationRequest) -> str:
        requests.append(request)
        return "private-invocation"

    async def wait(invocation_id: str) -> InvocationResult:
        provider = requests[-1].provider
        return InvocationResult(
            InvocationStatus.SUCCEEDED,
            provider,
            '{"public_text":"<script>alert(1)</script> 公开台词"}',
            f"private-{provider}",
            {},
            1,
        )

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "wait", wait)
    rooms = DiscussionRooms(manager)
    other = DiscussionRooms(manager)
    try:
        opened = rooms.open(tmp_path, "隔离测试", seats())
        rid, url, desk = opened.room.id, opened.public_url, opened.director_url
        assert url != desk
        assert (await asyncio.to_thread(fetch, url + "avatars.png")).startswith(b"\x89PNG")
        assert (
            await asyncio.to_thread(cache_control, url + "avatars.png")
        ) == "private, max-age=86400"
        assert (await asyncio.to_thread(cache_control, url)) == "no-store"
        assert (await asyncio.to_thread(cache_control, url + "state")) == "no-store"
        with pytest.raises(ValueError, match="held by another AgentNave server"):
            other.open(tmp_path, "隔离测试", seats())
        rooms.post(rid, "FIRST_PUBLIC_MESSAGE")
        rooms.post(rid, "SECOND_PUBLIC_MESSAGE")
        assert rooms.view(rid).room.messages[0].speaker == "director"
        begun = rooms.start(rid, "claude", "PRIVATE_CLAUDE_INSTRUCTION")
        assert begun.room.turn is not None
        turn_id = begun.room.turn.id
        with pytest.raises(ValueError, match="current turn"):
            rooms.start(rid, "grok", "out of turn")
        task = rooms.get(rid).task
        assert task is not None
        await task
        view = await rooms.read(rid)
        assert view.room.turn is not None and view.room.turn.status == "ready"
        assert (
            "FIRST_PUBLIC_MESSAGE" in requests[0].prompt
            and "SECOND_PUBLIC_MESSAGE" in requests[0].prompt
        )
        assert (
            requests[0].session_id is None
            and requests[0].provider_options["discussion_mode"] is True
        )
        assert requests[0].cwd != tmp_path and requests[0].cwd.exists()
        assert url in requests[0].prompt and desk not in requests[0].prompt
        public = (await asyncio.to_thread(fetch, url + "state")).decode()
        for private in (
            "PRIVATE_CLAUDE_INSTRUCTION",
            "private-invocation",
            "private-claude",
            "candidate",
            "公开台词",
            "error",
            "status",
            "cwd",
            desk,
        ):
            assert private not in public
        private_view = (await asyncio.to_thread(fetch, desk + "state")).decode()
        assert "PRIVATE_CLAUDE_INSTRUCTION" in private_view and "公开台词" in private_view
        assert "private-claude" not in private_view

        def fail_replace(source: str, destination: Path) -> None:
            raise OSError("disk unavailable")

        with monkeypatch.context() as disk_failure:
            disk_failure.setattr(os, "replace", fail_replace)
            with pytest.raises(OSError, match="disk unavailable"):
                rooms.decide(rid, turn_id, "publish")
        assert len(rooms.view(rid).room.messages) == 2
        published = rooms.decide(rid, turn_id, "publish")
        assert len(rooms.decide(rid, turn_id, "publish").room.messages) == 3
        assert published.room.messages[-1].text == view.room.turn.candidate
        public_state = cast(
            dict[str, object], json.loads(await asyncio.to_thread(fetch, url + "state"))
        )
        desk_state = cast(
            dict[str, object], json.loads(await asyncio.to_thread(fetch, desk + "state"))
        )
        assert public_state["messages"] == desk_state["messages"]
        transcript = (await asyncio.to_thread(fetch, url + "transcript.jsonl")).decode()
        assert len(transcript.splitlines()) == 3 and "PRIVATE_CLAUDE_INSTRUCTION" not in transcript

        rooms.start(rid, "grok", "PRIVATE_GROK_INSTRUCTION")
        task = rooms.get(rid).task
        assert task is not None
        await task
        draft = rooms.view(rid).room.turn
        assert draft is not None
        rooms.decide(rid, draft.id, "discard")
        assert (
            "公开台词" in requests[-1].prompt
            and "PRIVATE_CLAUDE_INSTRUCTION" not in requests[-1].prompt
        )
        assert requests[-1].session_id is None and requests[-1].cwd != requests[0].cwd
        rooms.post(rid, "NEW_PUBLIC_MESSAGE")
        rooms.start(rid, "claude", "continue")
        task = rooms.get(rid).task
        assert task is not None
        await task
        resumed = requests[-1]
        assert resumed.session_id == "private-claude" and resumed.cwd == requests[0].cwd
        assert "NEW_PUBLIC_MESSAGE" in resumed.prompt
        for old in (
            "FIRST_PUBLIC_MESSAGE",
            "SECOND_PUBLIC_MESSAGE",
            "公开台词",
            "PRIVATE_GROK_INSTRUCTION",
        ):
            assert old not in resumed.prompt
        assert '"previous_reply_status": "published"' in resumed.prompt
        draft = rooms.view(rid).room.turn
        assert draft is not None
        rooms.decide(rid, draft.id, "discard")
        for route in ("room.json", "../room.json", "bad/state", "director"):
            with pytest.raises(HTTPError) as error:
                await asyncio.to_thread(fetch, url + route)
            assert error.value.code == 404
        for headers in ({"Host": "evil.example"}, {"Origin": "https://evil.example"}):
            with pytest.raises(HTTPError) as error:
                await asyncio.to_thread(fetch, url + "state", headers=headers)
            assert error.value.code == 403
        with pytest.raises(HTTPError) as error:
            await asyncio.to_thread(fetch, url + "state", method="POST")
        assert error.value.code == 501
    finally:
        await rooms.shutdown()
    try:
        reopened = other.open(tmp_path, "隔离测试", seats())
        assert reopened.room.id == rid and len(reopened.room.messages) == 4
        assert reopened.public_url != url
        other.start(rid, "claude", "after restart")
        task = other.get(rid).task
        assert task is not None
        await task
        assert requests[-1].session_id == "private-claude"
        assert '"new_public_messages": []' in requests[-1].prompt
        assert '"previous_reply_status": "discarded"' in requests[-1].prompt
        draft = other.view(rid).room.turn
        assert draft is not None
        other.decide(rid, draft.id, "discard")
        other.reset(rid, "claude")
        assert other.view(rid).room.sessions["grok"].session_id == "private-grok"
        other.start(rid, "claude", "new identity")
        task = other.get(rid).task
        assert task is not None
        await task
        assert requests[-1].session_id is None and requests[-1].cwd != requests[0].cwd
        assert (
            "FIRST_PUBLIC_MESSAGE" in requests[-1].prompt
            and "NEW_PUBLIC_MESSAGE" in requests[-1].prompt
        )
        draft = other.view(rid).room.turn
        assert draft is not None
        other.decide(rid, draft.id, "discard")
        for seat in seats():
            other.reset(rid, seat.id)
    finally:
        await other.shutdown()


@pytest.mark.asyncio
async def test_invalid_candidate_cancel_and_uncertain_history(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    results = [
        InvocationResult(
            InvocationStatus.SUCCEEDED,
            "claude",
            '{"public_text":"hello","thinking":"PRIVATE"}',
            None,
            {},
            1,
        ),
        InvocationResult(InvocationStatus.CANCELLED, "claude", "PARTIAL_PRIVATE", None, {}, 1),
        InvocationResult(
            InvocationStatus.SUCCEEDED, "claude", '{"public_text":"hello"}', "session", {}, 1
        ),
    ]

    def start(request: InvocationRequest) -> str:
        return "invocation"

    async def wait(invocation_id: str) -> InvocationResult:
        return results.pop(0)

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "wait", wait)
    rooms = DiscussionRooms(manager)
    try:
        rid = rooms.open(tmp_path, "failure", seats()).room.id
        for expected in ("failed", "cancelled"):
            rooms.start(rid, "claude", "speak")
            with pytest.raises(ValueError, match="current turn"):
                rooms.reset(rid, "claude")
            task = rooms.get(rid).task
            assert task is not None
            await task
            view = await rooms.read(rid)
            assert view.room.turn is not None and view.room.turn.status == expected
            with pytest.raises(ValueError, match="ready"):
                rooms.decide(rid, view.room.turn.id, "publish")
            assert view.room.messages == [] and view.room.turn.candidate is None
            with pytest.raises(ValueError, match="uncertain"):
                rooms.start(rid, "claude", "retry")
            rooms.reset(rid, "claude")

        rooms.start(rid, "claude", "result cannot be saved")
        task = rooms.get(rid).task
        assert task is not None

        def fail_replace(source: str, destination: Path) -> None:
            raise OSError("disk unavailable")

        with monkeypatch.context() as disk_failure:
            disk_failure.setattr(os, "replace", fail_replace)
            await task
            with pytest.raises(OSError, match="disk unavailable"):
                rooms.reset(rid, "claude")
        recovered = await rooms.read(rid)
        assert recovered.room.turn is not None
        assert recovered.room.turn.status == "failed"
        assert recovered.room.turn.error == "room_write_failed"
        assert recovered.room.sessions["claude"].needs_reset
        desk = json.loads(await asyncio.to_thread(fetch, recovered.director_url + "state"))
        assert desk["status"] == recovered.room.turn.status
        rooms.reset(rid, "claude")
        rooms.post(rid, "Disk recovered; continue without restarting the server")
    finally:
        await rooms.shutdown()


@pytest.mark.asyncio
async def test_task_collection_stays_terminal_after_repeated_write_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()

    def start(request: InvocationRequest, *, prepared: PreparedCommand | None = None) -> str:
        return "invocation"

    async def wait(invocation_id: str, timeout: float) -> InvocationResult:
        return InvocationResult(InvocationStatus.SUCCEEDED, "claude", "result", "session", {}, 1)

    def fail_replace(source: str, destination: Path) -> None:
        raise OSError("disk unavailable")

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "wait", wait)
    rooms = Workbench(manager, tmp_path)
    try:
        _, opened = rooms.start_task(
            InvocationRequest(provider="claude", prompt="task", cwd=tmp_path), None, None
        )
        rid = opened.room.id
        with monkeypatch.context() as disk_failure:
            disk_failure.setattr(os, "replace", fail_replace)
            await rooms.collectors[-1]
            with pytest.raises(OSError, match="disk unavailable"):
                rooms.update(rid, "Rename while disk unavailable", None)
        recovered = await rooms.read(rid)
        assert recovered.room.tasks[0].status == "interrupted"
        assert "history_write_failed" in (recovered.room.tasks[0].error or "")
        assert recovered.room.title == opened.room.title
        desk = json.loads(await asyncio.to_thread(fetch, recovered.director_url + "state"))
        assert desk["tasks"][0]["status"] == "interrupted"
        rooms.update(rid, "Recovered", True)
        assert rooms.view(rid).room.archived
    finally:
        await rooms.shutdown()


@pytest.mark.asyncio
async def test_restart_marks_running_turn_interrupted(
    tmp_path: Path,
) -> None:
    state = RoomState.model_validate(
        {
            "id": "room",
            "title": "restart",
            "seats": [s.model_dump() for s in seats()],
            "turn": {
                "id": "turn",
                "seat_id": "claude",
                "status": "running",
                "invocation_id": "expired",
            },
            "sessions": {
                "claude": {
                    "cwd": str(tmp_path / "seat"),
                    "session_id": "private-claude",
                    "needs_reset": True,
                }
            },
        }
    )
    (tmp_path / "room.json").write_text(state.model_dump_json())
    rooms = DiscussionRooms(InvocationManager())
    try:
        view = rooms.open(tmp_path, "restart", seats())
        assert view.room.turn is not None
        assert view.room.turn.status == "interrupted" and view.room.turn.invocation_id is None
        with pytest.raises(ValueError, match="uncertain"):
            rooms.start(view.room.id, "claude", "continue")
    finally:
        await rooms.shutdown()


@pytest.mark.parametrize("provider", ["claude", "grok", "codebuddy", "codex", "antigravity", "pi"])
def test_native_discussion_profiles_resume_without_enabling_tools(
    tmp_path: Path, provider: str
) -> None:
    adapter = get_adapter(provider)
    command = adapter.prepare(
        InvocationRequest(
            provider,
            "speak",
            tmp_path,
            session_id="own-seat-session",
            provider_options={"discussion_mode": True, "model": "explicit"},
        )
    )
    try:
        argv = command.argv
        if provider in ("claude", "grok", "codebuddy"):
            assert "--resume=own-seat-session" in argv
            assert argv[argv.index("--tools") + 1] == ""
        assert "--no-session-persistence" not in argv
        if provider == "claude":
            assert {"--safe-mode", "--strict-mcp-config", "--disable-slash-commands"} <= set(argv)
        elif provider == "grok":
            assert argv[argv.index("--deny") + 1] == "*"
            assert {"--no-subagents", "--disable-web-search"} <= set(argv)
        elif provider == "codebuddy":
            assert "--safe-mode" not in argv
            assert "--strict-mcp-config" in argv
            assert argv[argv.index("--mcp-config") + 1] == '{"mcpServers":{}}'
            assert argv[argv.index("--disallowedTools") + 1] == "mcp__*"
        elif provider == "codex":
            assert "resume" in argv and "own-seat-session" in argv
            assert {
                "--ignore-user-config",
                "--skip-git-repo-check",
                "shell_tool",
                "unified_exec",
                "view_image",
                "apps",
                "multi_agent",
            } <= set(argv)
            assert "--ephemeral" not in argv
        elif provider == "pi":
            assert argv[argv.index("--session") + 1] == "own-seat-session"
            assert {
                "--no-tools",
                "--no-extensions",
                "--no-mcp",
                "--no-skills",
                "--no-prompt-templates",
            } <= set(argv)
        else:
            assert argv[argv.index("--conversation") + 1] == "own-seat-session"
            assert {"--sandbox=true", "--disable-slash-commands=true"} <= set(argv)
            profile = tmp_path / ".agents" / "agents" / "agentnave-discussion.md"
            assert "tools: []" in profile.read_text() and "mcpServers: []" in profile.read_text()
    finally:
        for path in command.cleanup_paths:
            path.unlink()
    with pytest.raises(ValueError, match="only model and effort"):
        adapter.prepare(
            InvocationRequest(
                provider,
                "speak",
                tmp_path,
                provider_options={"discussion_mode": True, "permission_mode": "bypassPermissions"},
            )
        )


@pytest.mark.asyncio
async def test_shows_keep_public_history_sessions_and_director_navigation_separate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    requests: list[InvocationRequest] = []

    def start(request: InvocationRequest) -> str:
        requests.append(request)
        return str(len(requests))

    async def wait(invocation_id: str) -> InvocationResult:
        request = requests[int(invocation_id) - 1]
        return InvocationResult(
            InvocationStatus.SUCCEEDED,
            request.provider,
            '{"public_text":"published reply"}',
            request.cwd.name,
            {},
            1,
        )

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "wait", wait)
    rooms = DiscussionRooms(manager)
    try:
        first = rooms.open(tmp_path / "one", "FIRST_SHOW", seats())
        second = rooms.open(tmp_path / "two", "SECOND_SHOW", seats())
        rooms.post(first.room.id, "ONLY_FIRST_PUBLIC")
        rooms.post(second.room.id, "ONLY_SECOND_PUBLIC")
        for view in (first, second, first):
            rooms.start(view.room.id, "claude", "own private instruction")
            task = rooms.get(view.room.id).task
            assert task is not None
            await task
            turn = rooms.view(view.room.id).room.turn
            assert turn is not None
            rooms.decide(view.room.id, turn.id, "publish")
        assert requests[0].session_id is requests[1].session_id is None
        assert requests[0].cwd != requests[1].cwd
        assert requests[2].session_id == requests[0].cwd.name
        assert requests[2].cwd == requests[0].cwd
        assert "ONLY_SECOND_PUBLIC" not in requests[0].prompt + requests[2].prompt
        assert "ONLY_FIRST_PUBLIC" not in requests[1].prompt
        assert len(rooms.list()) == 2
        for view, other in ((first, second), (second, first)):
            public = (await asyncio.to_thread(fetch, view.public_url + "state")).decode()
            private = (await asyncio.to_thread(fetch, view.director_url + "state")).decode()
            for forbidden in (
                other.room.title,
                other.public_url,
                other.director_url,
                "shows",
                "directory",
            ):
                assert forbidden not in public
            assert other.director_url in private
            assert str(tmp_path) not in private
            exported = (
                await asyncio.to_thread(fetch, view.public_url + "transcript.jsonl")
            ).decode()
            assert ("ONLY_SECOND_PUBLIC" if view is first else "ONLY_FIRST_PUBLIC") not in exported
    finally:
        await rooms.shutdown()


@pytest.mark.asyncio
async def test_blind_answers_auto_seal_survive_restart_and_reveal_atomically(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = InvocationManager()
    requests: list[InvocationRequest] = []

    def start(request: InvocationRequest) -> str:
        requests.append(request)
        return str(len(requests))

    async def wait(invocation_id: str) -> InvocationResult:
        request = requests[int(invocation_id) - 1]
        return InvocationResult(
            InvocationStatus.SUCCEEDED,
            request.provider,
            json.dumps({"public_text": f"SEALED_{request.provider}"}),
            f"session-{request.provider}",
            {},
            1,
        )

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "wait", wait)
    rooms = DiscussionRooms(manager)
    try:
        opened = rooms.open(tmp_path, "blind test", seats(), "blind")
        rid = opened.room.id
        rooms.post(rid, "PUBLIC_QUESTION")
        rooms.start(rid, "claude", "PRIVATE_FIRST_INSTRUCTION")
        task = rooms.get(rid).task
        assert task is not None
        await task
        state = rooms.view(rid).room
        assert state.turn is not None and state.turn.status == "held"
        assert state.pending_answers[0].text == "SEALED_claude"
        round_id = state.blind_round_id
        assert round_id is not None
        for path in ("state", "transcript.jsonl"):
            public = (await asyncio.to_thread(fetch, opened.public_url + path)).decode()
            assert "PUBLIC_QUESTION" in public
            assert "SEALED_claude" not in public and "pending_answers" not in public
        desk = (await asyncio.to_thread(fetch, opened.director_url + "state")).decode()
        assert "SEALED_claude" in desk
        with pytest.raises(ValueError, match="individually"):
            rooms.decide(rid, state.turn.id, "publish")
        with pytest.raises(ValueError, match="All seats"):
            rooms.decide_round(rid, round_id, "publish")
        with pytest.raises(ValueError, match="already submitted"):
            rooms.start(rid, "claude", "submit twice")
        with pytest.raises(ValueError, match="sealed round"):
            rooms.reset(rid, "claude")
        with pytest.raises(ValueError, match="blind round"):
            rooms.post(rid, "changed question")
    finally:
        await rooms.shutdown()

    rooms = DiscussionRooms(manager)
    try:
        with pytest.raises(ValueError, match="configuration"):
            rooms.open(tmp_path, "blind test", seats())
        reopened = rooms.open(tmp_path, "blind test", seats(), "blind")
        assert reopened.room.blind_round_id == round_id
        assert reopened.room.pending_answers == state.pending_answers
        rooms.start(rid, "grok", "PRIVATE_SECOND_INSTRUCTION")
        task = rooms.get(rid).task
        assert task is not None
        await task
        assert "PUBLIC_QUESTION" in requests[-1].prompt
        for forbidden in (
            "SEALED_claude",
            "PRIVATE_FIRST_INSTRUCTION",
            "pending_answers",
            reopened.director_url,
        ):
            assert forbidden not in requests[-1].prompt
        assert requests[0].session_id is requests[-1].session_id is None
        assert len(rooms.view(rid).room.messages) == 1
        assert len(rooms.view(rid).room.pending_answers) == 2

        def fail_replace(source: str, destination: Path) -> None:
            raise OSError("disk unavailable")

        with monkeypatch.context() as disk_failure:
            disk_failure.setattr(os, "replace", fail_replace)
            with pytest.raises(OSError, match="disk unavailable"):
                rooms.decide_round(rid, round_id, "publish")
        assert len(rooms.view(rid).room.pending_answers) == 2
        assert len(rooms.view(rid).room.messages) == 1
        public = (await asyncio.to_thread(fetch, reopened.public_url + "state")).decode()
        assert "SEALED_" not in public
        published = rooms.decide_round(rid, round_id, "publish")
        assert [m.text for m in published.room.messages] == [
            "PUBLIC_QUESTION",
            "SEALED_claude",
            "SEALED_grok",
        ]
        assert published.room.messages[1].time == published.room.messages[2].time
        assert published.room.pending_answers == [] and published.room.blind_round_id is None
        with pytest.raises(ValueError, match="round_id"):
            rooms.decide_round(rid, round_id, "publish")
        assert len(rooms.view(rid).room.messages) == 3
        public = json.loads(await asyncio.to_thread(fetch, reopened.public_url + "state"))
        assert [m["text"] for m in public["messages"]] == [m.text for m in published.room.messages]
        rooms.post(rid, "NEXT_QUESTION")
        rooms.start(rid, "claude", "next question")
        task = rooms.get(rid).task
        assert task is not None
        await task
        assert requests[-1].session_id == "session-claude"
        assert "NEXT_QUESTION" in requests[-1].prompt and "SEALED_grok" in requests[-1].prompt
        assert "SEALED_claude" not in requests[-1].prompt
        next_id = rooms.view(rid).room.blind_round_id
        assert next_id is not None and next_id != round_id
        discarded = rooms.decide_round(rid, next_id, "discard")
        assert len(discarded.room.messages) == 4 and not discarded.room.pending_answers
        assert discarded.room.sessions["claude"].last_reply_status == "discarded"
        rooms.post(rid, "FINAL_QUESTION")
        rooms.start(rid, "claude", "answer independently")
        task = rooms.get(rid).task
        assert task is not None
        with pytest.raises(ValueError, match="Cancel/wait"):
            rooms.mark_absent(rid, "claude", "cannot skip a live invocation")
        await task
        rooms.mark_absent(rid, "grok", "director decided to finish without this seat")
        with pytest.raises(ValueError, match="absent"):
            rooms.start(rid, "grok", "late answer")
        final_round = rooms.view(rid).room.blind_round_id
        assert final_round is not None
        partial = rooms.decide_round(rid, final_round, "publish")
        assert "本轮结果不完整" in partial.room.messages[-2].text
        assert "Grok" in partial.room.messages[-2].text
        switched = rooms.continue_discussion(rid)
        assert switched.room.mode == "discussion"
        rooms.start(rid, "grok", "discuss the revealed replies")
        task = rooms.get(rid).task
        assert task is not None
        await task
        assert requests[-1].session_id == "session-grok"
        assert "SEALED_claude" in requests[-1].prompt
        assert rooms.view(rid).room.turn is not None

    finally:
        await rooms.shutdown()
