"""Resolve private-task arguments before preparing or locking a native session."""

from __future__ import annotations

from collections.abc import Callable, Collection, Iterable
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Literal, cast, get_args

from agentnave.decision_seats import custom_seat
from agentnave.host import Host
from agentnave.isolated import other_active_leases, shares_cwd_tree
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.members import (
    ProviderName,
    ReviewRole,
    apply_member,
    orchestration_enabled,
    read_members,
    read_permissions,
    role_seat,
    validate_task_seat,
)
from agentnave.models import InvocationRequest, ProviderOption
from agentnave.orchestration_roles import ReviewSeat, seat_role
from agentnave.store import Record, TaskRecord

if TYPE_CHECKING:
    from agentnave.conversations import Conversations
    from agentnave.core import InvocationManager


READ_ONLY_APPLIED = "read-only instruction (access=read)"


def latest_session_task(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
    manager_session: str | None,
    not_yours: Callable[[str, str | None], str],
    *,
    explicit_identity: bool,
) -> TaskRecord | None:
    """Only the owning boss's history may supply defaults or identify a provider."""
    matches = [
        (record.state, index, task)
        for record in records
        if record.state.kind == "task"
        for index, task in enumerate(record.state.tasks)
        if task.session_id == session_id
    ]
    owned = [
        (state, index, task)
        for state, index, task in matches
        if state.manager_session == manager_session
    ]
    candidates = [item for item in owned if provider is None or item[2].provider == provider]
    if not candidates:
        foreign = next(
            (state for state, _, task in matches if provider is None or task.provider == provider),
            None,
        )
        if foreign is not None:
            if explicit_identity:
                return None  # Preserve explicit cross-boss resume without borrowing its metadata.
            raise ValueError(not_yours("Task conversation", foreign.manager_session))
        if owned:
            raise ValueError("A native session cannot change provider; use its original provider")
        return None
    if len({task.provider for _, _, task in candidates}) > 1:
        raise ValueError("session_id matches multiple providers; pass provider and cwd explicitly")
    return max(
        candidates,
        key=lambda item: (item[2].time, item[0].created, item[0].id, item[1]),
    )[2]


def review_role(role: str | None) -> str:
    """Only review role labels select a seat; orchestration names require seat."""
    name = (role or "").strip()
    return name if name in get_args(ReviewRole.__value__) else ""


def role_access(role: str | None) -> Literal["read", "write"] | None:
    """Explorers and reviewers read; Workers and the image generator write."""
    name = (role or "").strip()
    if definition := seat_role(name):
        return definition.access
    if name == "image-generator":
        return "write"
    return "read" if name in get_args(ReviewRole.__value__) else None


def native_seat_guidance(model: str | None, effort: str | None) -> str:
    """How to run a native seat instead: the host's own subagent, at the saved settings."""
    settings = ", ".join(
        f"{name} {value}" for name, value in (("model", model), ("effort", effort)) if value
    )
    return "Dispatch it with the host's built-in subagent" + (
        f" using {settings}." if settings else "."
    )


class NativeSeat(Exception):
    """The chosen seat belongs to this host's CLI, so the service can start nothing for it."""

    def __init__(self, name: str, seat: ReviewSeat) -> None:
        super().__init__(name, seat)
        self.name = name
        self.seat = seat


@dataclass
class StartArguments:
    provider: ProviderName | None
    cwd: str | None
    role: str | None
    seat: str | None  # a catalog seat, or one of the user's own seats
    access: Literal["read", "write"] | None
    defaults: list[str] = field(default_factory=lambda: [])
    # Model and effort the resumed thread last ran with; see apply_history.
    history: dict[str, str] = field(default_factory=lambda: {})
    selected: ReviewSeat | None = None

    def inherit(self, task: TaskRecord) -> None:
        self.provider = self._inherited(
            self.provider, cast(ProviderName, task.provider), "provider"
        )
        self.cwd = self._inherited(self.cwd, task.cwd, "cwd")
        role = (self.role or "").strip()
        self.role = self._inherited(self.role, task.role, "role")
        # A new role is a new duty: the old seat would otherwise outrank it. Blank only clears the label.
        if self.seat is not None or role in ("", task.role):
            self.seat = self._inherited(self.seat, task.seat, "seat")
        if task.model:
            self.history["model"] = task.model
            if task.effort:
                self.history["effort"] = task.effort

    def apply_history(self, options: dict[str, ProviderOption]) -> dict[str, ProviderOption]:
        """Keep a resumed thread on its last model unless the caller or a seat chose one.

        Otherwise a resume without options falls back to the App or CLI default model, which
        may be a different and costlier one than the thread was started with.
        """
        if "model" in options or "model" not in self.history:
            return options
        result = {**options, "model": self.history["model"]}
        self.defaults.append("model (session_id history)")
        if "effort" in self.history and "effort" not in result:
            result["effort"] = self.history["effort"]
            self.defaults.append("effort (session_id history)")
        return result

    def _inherited[T](self, explicit: T | None, saved: T | None, key: str) -> T | None:
        if explicit is not None or saved is None:
            return explicit
        self.defaults.append(f"{key} (session_id history)")
        return saved

    def resolve_seat(self, host: Host) -> None:
        if self.seat is not None:
            validate_task_seat(
                self.seat, host, inherited="seat (session_id history)" in self.defaults
            )
        name = self.seat or review_role(self.role)
        if self.selected is not None and self.selected.provider == host.provider:
            raise NativeSeat(name, self.selected)
        if self.provider is None:
            if not name:
                raise ValueError("Pass provider, or seat / a review role to select an App seat")
            seat = self.selected or role_seat(name, host, required=True)
            assert seat is not None
            if seat.provider == host.provider:
                raise NativeSeat(name, seat)
            self.provider = seat.provider
            self.defaults.append(f"provider ({name} seat)")
        custom = custom_seat(name, host)
        implied = custom.access if custom is not None else role_access(name)
        if implied is None:
            return
        if self.access is None:
            self.access = implied
            self.defaults.append(f"access ({name} seat)")
        elif self.access != implied:
            raise ValueError(
                f"{name} declares access={implied}; omit access or pick a matching seat"
            )


def resolve_start(
    arguments: StartArguments,
    conversations: Conversations,
    host: Host,
    session_id: str | None,
    manager_session_id: str | None,
) -> StartArguments:
    if session_id is not None:
        task = conversations.resume_task(
            session_id,
            arguments.provider,
            manager_session_id,
            explicit_identity=arguments.provider is not None and arguments.cwd is not None,
        )
        if task is not None:
            arguments.inherit(task)
        elif arguments.provider is None or arguments.cwd is None:
            raise ValueError("Unknown session_id; pass provider and cwd explicitly")
    if arguments.cwd is None:
        raise ValueError("cwd is required for a new task; pass an absolute existing directory")
    arguments.resolve_seat(host)
    if arguments.access == "read":
        arguments.defaults.append(READ_ONLY_APPLIED)
    return arguments


def apply_task_defaults(
    provider: ProviderName,
    seat_name: str | None,
    options: dict[str, ProviderOption],
    seat_applied: list[str],
    excluded: frozenset[str],
    host: Host,
    selected: ReviewSeat | None = None,
) -> tuple[dict[str, ProviderOption], list[str]]:
    selected = selected or role_seat(seat_name, host)
    options, applied = apply_member(
        read_members(),
        "task",
        provider,
        options,
        excluded,
        permissions=read_permissions(),
        seat=frozenset(seat_applied),
        scenario=not orchestration_enabled(host),
        seat_selected=selected is not None and selected.provider == provider,
    )
    if seat_applied:
        applied = [f"{(seat_name or '').strip()} seat", *seat_applied, *applied]
    return options, applied


def add_write_warning(
    warnings: list[str],
    manager: InvocationManager,
    request: InvocationRequest,
    invocation_id: str,
    access: Literal["read", "write"] | None,
    *,
    isolation: TaskIsolationInfo | None = None,
    own_lease_paths: Collection[Path] = (),
) -> None:
    if access == "read":
        return
    sharing = [
        listing.invocation_id
        for listing in manager.list_invocations()
        if listing.status is None
        and not listing.read_only
        and shares_cwd_tree(listing.cwd, request.cwd)
        and listing.invocation_id != invocation_id
    ]
    if sharing:
        warnings.append(
            f"{len(sharing)} other CLI run(s) active in this cwd ({', '.join(sharing)}); "
            "parallel writers need separate worktrees. Declare access=read for read-only work."
        )
    if isolation is not None:
        try:
            external = other_active_leases(
                isolation.repo_common_dir,
                isolation.id,
                exclude_paths=own_lease_paths,
            )
        except (OSError, ValueError):
            external = 0
        if external > 0:
            warnings.append(
                f"{external} active lease(s) held by another process in isolated directory "
                f"{isolation.id}; parallel writers need separate worktrees. "
                "Declare access=read for read-only work."
            )
