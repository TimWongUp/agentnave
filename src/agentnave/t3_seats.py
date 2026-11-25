"""App-owned T3 Code seat catalog (`t3-seats.json`, ADR-0121).

Separate from `decision-seats.json` and `review.json`: T3 orchestration follows the host's
orchestration preset (split/parallel, decision, custom) for the bosses the App selected in
`bosses`, and applies only to T3 Code main threads, never affecting non-T3 connections. A selected
boss whose preset is off dispatches like split inside T3 only; meetings has no T3 mode.
"""

from __future__ import annotations

from typing import Annotated, Literal, get_args

from pydantic import Field, field_validator

from agentnave.host import Host
from agentnave.members import (
    MemberFileError,
    ReviewRole,
    _read_app_file,  # pyright: ignore[reportPrivateUsage]
)
from agentnave.orchestration_roles import (
    OrchestrationFile,
    SeatMetadata,
    StrictModel,
    role_catalog,
    validate_seat_cards,
)
from agentnave.t3 import current_t3_mode

T3_SEATS_FILE = "t3-seats.json"
type T3OrchestrationMode = Literal["split", "decision", "custom"]
# The CLIs both T3 Code and AgentNave support (claudeAgent, codex, antigravity, acpRegistry's
# CodeBuddy, grok, pi); mirrored by `T3_BOSSES` in apps/tray/src/rules.rs.
type T3Boss = Literal["claude", "codex", "antigravity", "codebuddy", "grok", "pi"]
T3_BOSS_IDS: frozenset[str] = frozenset(get_args(T3Boss.__value__))

# Options that control permissions, sandbox, execution mode, or service tiers must never be stored
# in T3 seat candidates (ADR-0121).
FORBIDDEN_T3_OPTION_IDS = frozenset(
    {
        "mode",
        "sandbox",
        "multitask",
        "permission",
        "permissionmode",
        "approvalpolicy",
        "runtimemode",
        "fastmode",
        "ultracode",
        "servicetier",
        "cyberaccessprogram",
    }
)

REVIEW_ROLES: frozenset[str] = frozenset(get_args(ReviewRole.__value__))


class T3SeatsFileError(MemberFileError):
    """t3-seats.json cannot be read or validated."""


def _non_empty_trimmed(value: str, what: str) -> str:
    if not value or value != value.strip():
        raise ValueError(f"{what} must be non-empty without leading or trailing whitespace")
    return value


class T3Option(StrictModel):
    """One T3 reasoning option passed in delegate_task target.options."""

    id: str
    value: str

    @field_validator("id")
    @classmethod
    def allowed_id(cls, value: str) -> str:
        _non_empty_trimmed(value, "option id")
        if value.lower() in FORBIDDEN_T3_OPTION_IDS:
            raise ValueError(f"option id {value!r} is not allowed in T3 seat candidates")
        return value

    @field_validator("value")
    @classmethod
    def non_empty_value(cls, value: str) -> str:
        if not value or not value.strip():
            raise ValueError("option value must be a non-empty string")
        return value


class T3Candidate(StrictModel):
    """One ordered T3 target candidate: provider instance, model, and optional reasoning option."""

    instance: str
    model: str
    options: list[T3Option] = Field(default_factory=list[T3Option], max_length=1)

    @field_validator("instance")
    @classmethod
    def valid_instance(cls, value: str) -> str:
        return _non_empty_trimmed(value, "instance")

    @field_validator("model")
    @classmethod
    def valid_model(cls, value: str) -> str:
        return _non_empty_trimmed(value, "model")


class T3Seat(SeatMetadata):
    """One T3 task seat; 0 candidates means saved without models yet and excluded from selection."""

    candidates: list[T3Candidate] = Field(default_factory=list[T3Candidate], max_length=4)


type T3ReviewCandidates = Annotated[list[T3Candidate], Field(max_length=4)]


class T3SeatsFile(StrictModel):
    """Strict schema of data_root()/t3-seats.json (ADR-0121)."""

    version: int = Field(ge=1, le=1)
    enabled: bool = False
    random_candidates: bool = False
    # None: follow the orchestration page (every supported CLI with any row that is not off).
    bosses: list[T3Boss] | None = None
    seats: list[T3Seat] = Field(default_factory=list[T3Seat], max_length=255)
    review: dict[ReviewRole, T3ReviewCandidates] = {}

    @field_validator("bosses")
    @classmethod
    def unique_bosses(cls, bosses: list[T3Boss] | None) -> list[T3Boss] | None:
        if bosses is not None and len(set(bosses)) != len(bosses):
            raise ValueError("bosses must be unique")
        return bosses

    @field_validator("seats")
    @classmethod
    def valid_seats(cls, seats: list[T3Seat]) -> list[T3Seat]:
        validate_seat_cards(seats)
        for seat in seats:
            if seat.id in REVIEW_ROLES:
                raise ValueError(f"seat id {seat.id!r} is reserved for a review role")
        return seats

    def has_candidates(self) -> bool:
        return any(seat.candidates for seat in self.seats) or any(
            candidates for candidates in self.review.values()
        )

    def is_active(self) -> bool:
        return self.enabled and self.has_candidates()

    def selects(self, boss: str | None, followed: frozenset[str]) -> bool:
        """Whether T3 orchestration is on for this CLI; `followed` are the CLIs with a row not off."""
        if boss not in T3_BOSS_IDS:
            return False
        if self.bosses is not None:
            return boss in self.bosses
        return boss in followed


def _row_cli(key: str) -> str:
    """The CLI of an orchestration row key: `claude`, `claude@a1`, `claude-desktop`, `codex@dir`."""
    cli = key.split("@", 1)[0]
    return "claude" if cli == "claude-desktop" else cli


def _followed_clis(roles: OrchestrationFile | None) -> frozenset[str]:
    """The supported CLIs with any row not off, as the App derives them while `bosses` is unsaved."""
    if roles is None:
        return frozenset()
    rows = (
        roles.effective_apps
        if roles.effective_apps is not None
        else {**dict.fromkeys(T3_BOSS_IDS, roles.preset), **roles.apps}
    )
    return frozenset(_row_cli(key) for key, mode in rows.items() if mode != "off")


def _orchestration_state(host: Host) -> tuple[str | None, frozenset[str]]:
    """The host's mode outside T3 (a missing orchestration.json reads as off, unreadable None) and
    the CLIs that follow the orchestration page."""
    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError:
        return None, frozenset()
    return ("off" if roles is None else roles.mode(host)), _followed_clis(roles)


def _preset_mode(general: str | None) -> T3OrchestrationMode | None:
    """The T3 mode a preset has by itself ('parallel' -> 'split'); off and meetings have none."""
    if general in ("split", "parallel"):
        return "split"
    if general in ("decision", "custom"):
        return general
    return None


def _t3_mode(host: Host, document: T3SeatsFile) -> T3OrchestrationMode | None:
    general, followed = _orchestration_state(host)
    if not document.selects(host.name, followed):
        return None
    # A selected boss that is off outside T3 dispatches like split inside it.
    return "split" if general == "off" else _preset_mode(general)


def t3_host_mode(host: Host) -> T3OrchestrationMode | None:
    """The T3 mode this host would run in, or None. A broken file reads as the preset's own mode so
    effective_t3_seats can report it on a boss that has one."""
    try:
        document = read_t3_seats()
    except T3SeatsFileError:
        return _preset_mode(_orchestration_state(host)[0])
    return None if document is None else _t3_mode(host, document)


def t3_seat_name(seat: T3Seat) -> str:
    """Canonical seat name exposed to bosses: `<seat_prefix>-<difficulty>` for standard roles, else `seat.id`."""
    if seat.difficulty is not None:
        role = next((entry for entry in role_catalog() if entry.group == seat.kind), None)
        if role is not None:
            return f"{role.seat_prefix}-{seat.difficulty}"
    return seat.id


def read_t3_seats() -> T3SeatsFile | None:
    """Read and validate t3-seats.json afresh; returns None when missing."""
    return _read_app_file(T3_SEATS_FILE, T3SeatsFile, T3SeatsFileError)


def effective_t3_seats(host: Host) -> T3SeatsFile | None:
    """Return the active T3 catalog for a T3 main thread the App selected, else None.

    Non-T3 connections, T3 child threads, unselected bosses and meetings/unreadable modes never touch
    t3-seats.json. A broken file on a split/decision/custom T3 main thread raises T3SeatsFileError;
    for an off boss it reads as not selected.
    """
    if current_t3_mode(host) != "main":
        return None
    general = _orchestration_state(host)[0]
    if general not in ("off", "split", "parallel", "decision", "custom"):
        return None
    try:
        document = read_t3_seats()
    except T3SeatsFileError:
        if general == "off":
            return None
        raise
    if document is None or not document.is_active() or _t3_mode(host, document) is None:
        return None
    return document


def t3_active_mode(host: Host) -> T3OrchestrationMode | None:
    """Active T3 orchestration mode for this connection; broken files read as inactive here."""
    try:
        if effective_t3_seats(host) is None:
            return None
    except T3SeatsFileError:
        return None
    return t3_host_mode(host)


def t3_orchestration_active(host: Host) -> bool:
    """Whether T3 orchestration is active on this connection; broken files read as inactive here."""
    return t3_active_mode(host) is not None
