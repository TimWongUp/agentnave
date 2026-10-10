"""Private conversation index and automatic collection of ordinary CLI tasks."""

from __future__ import annotations

import asyncio
import json
import os
import tempfile
from contextlib import suppress
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal, TypedDict
from uuid import uuid4

from agentnave.adapters import get_adapter
from agentnave.core import InvocationManager
from agentnave.dashboard import Dashboard
from agentnave.discussion import DiscussionRooms, Room, RoomSummary, RoomView, Seat, TaskRun
from agentnave.models import InvocationRequest


class DeletionResult(TypedDict):
    room_id: str
    deleted_paths: list[str]
    retained_paths: list[str]
    forgotten_invocations: int


class Workbench(DiscussionRooms):
    def __init__(self, manager: InvocationManager, directory: Path) -> None:
        super().__init__(manager)
        self.directory = directory.resolve()
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.catalog = self.directory / "catalog"
        self.catalog.mkdir(exist_ok=True, mode=0o700)
        self.home_token = uuid4().hex
        self.collectors: list[asyncio.Task[None]] = []
        self.unavailable: list[dict[str, str]] = []
        self.index_catalog()

    def index_catalog(self) -> None:
        """Index saved records read-only; another server may hold their locks."""
        known = {room.directory for room in self.rooms.values()}
        self.unavailable = []
        for entry in sorted(self.catalog.glob("*.json")):
            try:
                path = Path(json.loads(entry.read_text()))
                if path in known:
                    continue
                room = Room(path, "", [], write=False)
                if room.state.id in self.rooms:
                    raise ValueError("A copy of this room is already indexed")
            except (OSError, ValueError, TypeError) as exc:
                self.unavailable.append({"record": entry.stem, "reason": str(exc)})
                continue
            if self.dashboard is None:
                self.dashboard = Dashboard()
            self.rooms[room.state.id] = room
            self.refresh_views(room)

    def list(self, *, probe: bool = False) -> list[RoomSummary]:
        if probe:
            self.index_catalog()
        return super().list(probe=probe)

    def home(self) -> str:
        if self.dashboard is None:
            self.dashboard = Dashboard()
        self.refresh_home()
        return self.dashboard.url(self.home_token)

    def refresh_home(self) -> None:
        if self.dashboard is None:
            return
        self.dashboard.update(
            self.home_token,
            json.dumps(
                {
                    "view": "workbench",
                    "title": "工作台",
                    "id": "workbench",
                    "seats": [],
                    "messages": [],
                    "shows": [item.model_dump() for item in self.list()],
                    "storage": str(self.directory),
                    "unavailable": self.unavailable,
                },
                ensure_ascii=False,
            ).encode(),
        )

    def refresh_views(self, room: Room) -> None:
        super().refresh_views(room)
        assert self.dashboard is not None
        data = json.loads(self.dashboard.snapshots[room.director_token])
        data["workbench_url"] = self.dashboard.url(self.home_token)
        data["storage"] = str(room.directory)
        self.dashboard.update(room.director_token, json.dumps(data, ensure_ascii=False).encode())
        self.refresh_home()

    def view(self, room_id: str) -> RoomView:
        view = super().view(room_id)
        assert self.dashboard is not None
        view.workbench_url = self.dashboard.url(self.home_token)
        return view

    def open(
        self,
        directory: Path,
        title: str,
        seats: list[Seat],
        mode: Literal["discussion", "blind", "task"] = "discussion",
    ) -> RoomView:
        view = super().open(directory, title, seats, mode)
        entry = self.catalog / (view.room.id + ".json")
        if not entry.exists():
            fd, name = tempfile.mkstemp(dir=self.catalog)
            try:
                with os.fdopen(fd, "w") as stream:
                    json.dump(str(directory.resolve()), stream)
                os.replace(name, entry)
            finally:
                Path(name).unlink(missing_ok=True)
        return view

    def new_directory(self) -> Path:
        return self.directory / "conversations" / uuid4().hex

    def update(self, room_id: str, title: str | None, archived: bool | None) -> RoomView:
        room = self.get(room_id)
        if archived and (
            room.state.blind_round_id is not None
            or any(run.status == "running" for run in room.state.tasks)
            or (room.state.turn and room.state.turn.status in ("running", "ready"))
        ):
            raise ValueError("Resolve active work before ending the conversation")
        if title is not None:
            if not title.strip() or len(title) > 200:
                raise ValueError("Title must contain 1–200 characters")
            room.state.title = title
        if archived is not None:
            room.state.archived = archived
        self.changed(room)
        for opened in self.rooms.values():
            self.refresh_views(opened)
        return self.view(room_id)

    def start_task(
        self, request: InvocationRequest, title: str | None, conversation_id: str | None
    ) -> tuple[str, RoomView]:
        room = self.get(conversation_id) if conversation_id else None
        if request.session_id:

            def resumes(run: TaskRun) -> bool:
                return run.provider == request.provider and run.session_id == request.session_id

            if room is None:
                # Another server may have created or extended this session's conversation.
                self.list(probe=True)
                room = next(
                    (
                        existing
                        for existing in self.rooms.values()
                        if not existing.state.archived and any(map(resumes, existing.state.tasks))
                    ),
                    None,
                )
            if room is not None:
                # Lock before checking: never split one native session into a new conversation.
                room.acquire()
            if any(
                run.status == "running" and resumes(run)
                for existing in self.rooms.values()
                for run in existing.state.tasks
            ):
                raise ValueError("This native session already has an active task")
        # Validate provider options before creating history; prepare only once.
        prepared = get_adapter(request.provider).prepare(request)
        try:
            if room is None:
                opened = self.open(
                    self.new_directory(), title or f"{request.provider} 任务", [], "task"
                )
                room = self.get(opened.room.id)
            if room.state.mode != "task" or room.state.archived:
                raise ValueError("Ordinary tasks require an active task conversation")
            run = TaskRun(
                id=uuid4().hex,
                provider=request.provider,
                model=str(request.provider_options.get("model", "原生设置")),
                cwd=str(request.cwd),
                session_id=request.session_id,
                time=datetime.now(UTC).isoformat(),
            )
            room.state.tasks.append(run)
            self.changed(room)
            invocation_id = self.manager.start(request, prepared=prepared)
            run.invocation_id = invocation_id
            room.invocation_ids.add(invocation_id)
            self.refresh_views(room)
        except Exception:
            for path in prepared.cleanup_paths:
                Path(path).unlink(missing_ok=True)
            raise
        self.collectors.append(asyncio.create_task(self._collect_task(room, run.id, invocation_id)))
        return invocation_id, self.view(room.state.id)

    def delete(self, room_id: str) -> DeletionResult:
        room = self.get(room_id)
        if not room.state.archived:
            raise ValueError("Archive the conversation before deleting it")
        if (
            room.state.blind_round_id is not None
            or room.state.pending_answers
            or any(run.status == "running" for run in room.state.tasks)
            or (room.state.turn and room.state.turn.status in ("running", "ready"))
            or (room.task is not None and not room.task.done())
        ):
            raise ValueError("Resolve active work before deleting the conversation")
        state_path = room.directory / "room.json"
        entry = self.catalog / (room_id + ".json")
        state_path.unlink(missing_ok=True)
        try:
            entry.unlink(missing_ok=True)
        except OSError:
            # Keep the indexed history readable and the live entry available for retry.
            room.save()
            raise

        forgotten = self.manager.forget_finished(room.invocation_ids)
        self.collectors = [task for task in self.collectors if not task.done()]
        del self.rooms[room_id]
        assert self.dashboard is not None
        self.dashboard.snapshots.pop(room.token, None)
        self.dashboard.snapshots.pop(room.director_token, None)
        deleted = [str(state_path), str(entry)]
        # Only prune empty seat directories; never traverse or remove native CLI files.
        seats_path = room.directory / "seats"
        if seats_path.is_dir() and not seats_path.is_symlink():
            with suppress(OSError):
                for path in seats_path.iterdir():
                    if not path.is_symlink():
                        with suppress(OSError):
                            path.rmdir()
                seats_path.rmdir()
        # A retained directory must keep the same lock inode for future writers.
        # Unlinking it can allow two processes to lock different files at this path.
        room.release()
        for opened in self.rooms.values():
            self.refresh_views(opened)
        self.refresh_home()
        return {
            "room_id": room_id,
            "deleted_paths": deleted,
            "retained_paths": [str(room.directory)],
            "forgotten_invocations": forgotten,
        }

    async def _collect_task(self, room: Room, task_id: str, invocation_id: str) -> None:
        while True:
            result = await self.manager.wait(invocation_id, 1.5)
            run = next(run for run in room.state.tasks if run.id == task_id)
            if result is not None:
                break
            run.output = self.manager.recent_output(invocation_id)[0]
            snapshot = self.manager.snapshot(invocation_id)
            run.elapsed_ms = snapshot.elapsed_ms
            if snapshot.last_activity and snapshot.last_activity.blocking_error:
                run.error = snapshot.last_activity.blocking_error
            self.refresh_views(room)
        run.status = result.status.value
        run.output = result.output
        run.session_id = result.session_id or run.session_id
        run.elapsed_ms = result.duration_ms
        run.error = result.error.code if result.error else None
        try:
            self.changed(room)
        except OSError:
            restored = next(item for item in room.state.tasks if item.id == task_id)
            restored.status = "interrupted"
            restored.error = "history_write_failed; use wait_agent for the invocation result"
            # Keep terminal recovery state across subsequent failed edits in this process.
            room.rollback_state = room.state.model_copy(deep=True)
            self.refresh_views(room)

    async def shutdown(self) -> None:
        try:
            await asyncio.gather(*self.collectors)
        finally:
            await super().shutdown()
