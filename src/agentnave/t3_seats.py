"""App-owned T3 Code seat catalog (`t3-seats.json`, ADR-0121).

Separate from `decision-seats.json` and `review.json`: T3 orchestration is an independent switch
that applies only to T3 Code main threads and never affects non-T3 connections.
"""

from __future__ import annotations

from typing import Annotated, get_args

from pydantic import Field, field_validator

from agentnave.host import Host
from agentnave.members import (
    MemberFileError,
    ReviewRole,
    _read_app_file,  # pyright: ignore[reportPrivateUsage]
)
from agentnave.orchestration_roles import SeatMetadata, StrictModel, validate_seat_cards
from agentnave.t3 import current_t3_mode

T3_SEATS_FILE = "t3-seats.json"

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
    seats: list[T3Seat] = Field(default_factory=list[T3Seat], max_length=255)
    review: dict[ReviewRole, T3ReviewCandidates] = {}

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


def read_t3_seats() -> T3SeatsFile | None:
    """Read and validate t3-seats.json afresh; returns None when missing."""
    return _read_app_file(T3_SEATS_FILE, T3SeatsFile, T3SeatsFileError)


def effective_t3_seats(host: Host) -> T3SeatsFile | None:
    """Return the active T3 catalog for a T3 main thread, else None.

    Non-T3 connections and T3 child threads never touch t3-seats.json.
    A broken file on a T3 main thread raises T3SeatsFileError.
    """
    if current_t3_mode(host) != "main":
        return None
    document = read_t3_seats()
    if document is None or not document.is_active():
        return None
    return document


def t3_orchestration_active(host: Host) -> bool:
    """Whether T3 orchestration is active on this connection; broken files read as inactive here."""
    try:
        return effective_t3_seats(host) is not None
    except T3SeatsFileError:
        return False
