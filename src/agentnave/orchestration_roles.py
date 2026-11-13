"""Typed task seats and the shared, product-owned role catalog.

Literal aliases describe the wire contract for static consumers; catalog metadata supplies
runtime grouping, visibility and access. Contract tests check the aliases against the catalog.
"""

from __future__ import annotations

from functools import cache
from pathlib import Path
from typing import Any, Literal, cast, get_args

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    SerializerFunctionWrapHandler,
    TypeAdapter,
    field_validator,
    model_serializer,
    model_validator,
)

from agentnave.host import Host
from agentnave.models import ProviderOption

type ProviderName = Literal[
    "antigravity",
    "claude",
    "codebuddy",
    "codex",
    "deepseek",
    "devin",
    "grok",
    "pi",
    "zcode",
]
type Difficulty = Literal["easy", "hard"]
type CustomSeatDifficulty = Literal["easy", "medium", "hard"]
type DecisionModelName = Literal["gemini-decision-1"]
type OrchestrationPreset = Literal["off", "meetings", "split", "parallel", "decision", "custom"]
type OrchestrationGroup = Literal[
    "explorer",
    "worker",
    "worker_frontend",
    "worker_writing",
    "explorer_web",
    "explorer_social",
    "worker_documents",
]
type OrchestrationRole = Literal[
    "explorer-easy",
    "explorer-hard",
    "worker-easy",
    "worker-hard",
    "worker-frontend-easy",
    "worker-frontend-hard",
    "worker-writing-easy",
    "worker-writing-hard",
    "explorer-web-easy",
    "explorer-web-hard",
    "explorer-social-easy",
    "explorer-social-hard",
    "worker-documents-easy",
    "worker-documents-hard",
    "image-generator",
]


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)


class CatalogRole(StrictModel):
    group: OrchestrationGroup
    seat_prefix: str
    name_zh: str
    name_en: str
    scope_zh: str
    access: Literal["read", "write"]
    fallback_group: OrchestrationGroup | None
    legacy_enabled: bool


@cache
def role_catalog() -> tuple[CatalogRole, ...]:
    """Load the same packaged catalog that the App and rule generator use."""
    return TypeAdapter(tuple[CatalogRole, ...]).validate_json(
        Path(__file__).with_name("orchestration_roles.json").read_bytes()
    )


def seat_role(name: str) -> CatalogRole | None:
    prefix, separator, difficulty = name.rpartition("-")
    if not separator or difficulty not in ("easy", "hard"):
        return None
    return next((role for role in role_catalog() if role.seat_prefix == prefix), None)


class SeatChoice(StrictModel):
    provider: ProviderName
    model: str = Field(pattern=r"\S")
    effort: str = ""


class ReviewSeat(SeatChoice):
    """The CLI and conversational model the user picked in the App for one seat."""

    fallbacks: list[SeatChoice] = Field(
        default_factory=list[SeatChoice], max_length=3, exclude_if=lambda value: not value
    )

    @model_validator(mode="after")
    def unique_choices(self) -> ReviewSeat:
        choices = [(choice.provider, choice.model) for choice in [self, *self.fallbacks]]
        if len(choices) != len(set(choices)):
            raise ValueError("seat choices must have unique provider and model pairs")
        return self


_CUSTOM_ROLE_MAX_LENGTH = 40
_CUSTOM_DESCRIPTION_MAX_LENGTH = 200
# start_agent's seat value that asks the decision model to choose a seat of the catalog.
AUTO_SEAT = "auto"
# The kind of a seat the user named a specialty for; any other kind is a group of the role catalog.
CUSTOM_KIND = "custom"
# The kind of an image generation seat: write access, no difficulty, one card per seat.
IMAGE_KIND = "image"


# What decision-seats.json text may not hold, by code point, listed one by one so that the App
# (decision_seats.rs) checks the very same table and no Unicode category table decides: control
# characters, white space but the ordinary space, invisible format characters, private use and
# surrogates.
_REFUSED_CODES = (
    (0x0000, 0x001F), (0x007F, 0x009F), (0x00A0, 0x00A0), (0x00AD, 0x00AD), (0x061C, 0x061C),
    (0x1680, 0x1680), (0x180E, 0x180E), (0x2000, 0x200F), (0x2028, 0x202F), (0x205F, 0x205F),
    (0x2060, 0x2064), (0x2066, 0x206F), (0x3000, 0x3000), (0xD800, 0xDFFF), (0xE000, 0xF8FF),
    (0xFEFF, 0xFEFF), (0xFFF9, 0xFFFB), (0xF0000, 0xFFFFD), (0x100000, 0x10FFFD),
)  # fmt: skip
# The white space trimmed from both ends first; inside the text only the ordinary space is kept.
_TRIMMED_CODES = (
    (0x09, 0x0D), (0x20, 0x20), (0x85, 0x85), (0xA0, 0xA0), (0x1680, 0x1680), (0x2000, 0x200A),
    (0x2028, 0x2029), (0x202F, 0x202F), (0x205F, 0x205F), (0x3000, 0x3000),
)  # fmt: skip
_TRIMMED = "".join(chr(code) for low, high in _TRIMMED_CODES for code in range(low, high + 1))


def _printable_text(text: str, limit: int, what: str) -> str:
    cleaned = text.strip(_TRIMMED)
    if (
        not cleaned
        or len(cleaned) > limit
        or any(low <= ord(char) <= high for char in cleaned for low, high in _REFUSED_CODES)
    ):
        raise ValueError(
            f"{what} must contain 1–{limit} printable characters and no newlines or "
            "control characters"
        )
    return cleaned


class CustomSeat(StrictModel):
    """One seat of the user's own catalog in decision-seats.json (ADR-0087).

    The identifier is both the `seat` value and the decision candidate id. One card of the App is
    the seats sharing a kind and a role, one per difficulty.
    """

    id: str = Field(pattern=r"^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$")
    kind: str
    role: str
    description: str
    provider: ProviderName
    model: str = Field(pattern=r"\S")
    effort: str = ""
    fallbacks: list[SeatChoice] = Field(
        default_factory=list[SeatChoice], max_length=3, exclude_if=lambda value: not value
    )
    access: Literal["read", "write"]
    # Image seats have none; every other kind has one.
    difficulty: CustomSeatDifficulty | None = None

    @field_validator("kind")
    @classmethod
    def known_kind(cls, kind: str) -> str:
        if kind not in (CUSTOM_KIND, IMAGE_KIND, *(role.group for role in role_catalog())):
            raise ValueError(
                f"kind must be {CUSTOM_KIND!r}, {IMAGE_KIND!r} or a group of the role catalog"
            )
        return kind

    @field_validator("role")
    @classmethod
    def desk_role(cls, role: str) -> str:
        # The same limits as the desk role label of a task.
        return _printable_text(role, _CUSTOM_ROLE_MAX_LENGTH, "role")

    @field_validator("description")
    @classmethod
    def specialty_description(cls, description: str) -> str:
        return _printable_text(description, _CUSTOM_DESCRIPTION_MAX_LENGTH, "description")

    @model_validator(mode="after")
    def group_access(self) -> CustomSeat:
        self.review_seat()  # Validate uniqueness of the configured choices.
        group = next((role for role in role_catalog() if role.group == self.kind), None)
        if group is not None and group.access != self.access:
            raise ValueError(f"a {self.kind} seat is {group.access}-only by its group")
        if self.kind == IMAGE_KIND:
            if self.access != "write" or self.difficulty is not None:
                raise ValueError("an image seat is write access and has no difficulty")
        elif self.difficulty is None:
            raise ValueError(f"a {self.kind} seat needs a difficulty")
        return self

    def review_seat(self) -> ReviewSeat:
        return ReviewSeat(
            provider=self.provider, model=self.model, effort=self.effort, fallbacks=self.fallbacks
        )


class DecisionSeatsFile(BaseModel):
    """App-owned custom seat catalog; a boss uses it while its orchestration mode is decision."""

    model_config = ConfigDict(extra="ignore", strict=True)

    version: int = Field(ge=1, le=1)
    decision_model: DecisionModelName = "gemini-decision-1"
    # Two to 255 candidates is what the decision model accepts.
    seats: list[CustomSeat] = Field(default_factory=list[CustomSeat], max_length=255)

    @field_validator("seats")
    @classmethod
    def unique_ids(cls, seats: list[CustomSeat]) -> list[CustomSeat]:
        ids = [seat.id for seat in seats]
        if len(ids) != len(set(ids)):
            raise ValueError("seat ids must be unique")
        reserved = sorted(set(ids) & {*get_args(OrchestrationRole.__value__), AUTO_SEAT})
        if reserved:
            raise ValueError(f"seat id {reserved[0]!r} is reserved")
        cards = [(seat.kind, seat.role, seat.difficulty) for seat in seats]
        if len(cards) != len(set(cards)):
            raise ValueError("a card has one seat per difficulty")
        # A card shows one access, so the seats of one card must agree on it.
        accesses: dict[tuple[str, str], str] = {}
        for seat in seats:
            if accesses.setdefault((seat.kind, seat.role), seat.access) != seat.access:
                raise ValueError(f"the {seat.role!r} card must have one access")
        roles: dict[str, str] = {}
        for seat in seats:
            if (
                seat.kind not in (CUSTOM_KIND, IMAGE_KIND)
                and roles.setdefault(seat.kind, seat.role) != seat.role
            ):
                raise ValueError(f"a {seat.kind} seat has one role")
        return seats


class OrchestrationFile(BaseModel):
    """App-owned rule modes and saved task seats; only enabled hosts use seats."""

    model_config = ConfigDict(extra="ignore")

    version: int = Field(ge=1, le=1)
    preset: OrchestrationPreset = "off"
    apps: dict[str, OrchestrationPreset] = {}
    effective_apps: dict[str, OrchestrationPreset] | None = None
    host_cli: dict[str, Literal["subagent", "launch", "skip"]] = {}
    explorer: dict[Difficulty, ReviewSeat] = {}
    worker: dict[Difficulty, ReviewSeat] = {}
    # A key of its own, which servers that predate it ignore instead of rejecting the file.
    worker_frontend: dict[Difficulty, ReviewSeat] = {}
    worker_writing: dict[Difficulty, ReviewSeat] = {}
    explorer_web: dict[Difficulty, ReviewSeat] = {}
    explorer_social: dict[Difficulty, ReviewSeat] = {}
    worker_documents: dict[Difficulty, ReviewSeat] = {}
    enabled_roles: list[OrchestrationGroup] | None = None
    image_generation: ReviewSeat | None = None

    @field_validator("enabled_roles")
    @classmethod
    def unique_enabled_roles(
        cls, roles: list[OrchestrationGroup] | None
    ) -> list[OrchestrationGroup] | None:
        if roles is not None and len(roles) != len(set(roles)):
            raise ValueError("enabled_roles must not contain duplicate groups")
        return roles

    def image_seat(self, host: Host) -> ReviewSeat | None:
        """The optional image CLI is only advertised to a different provider's host."""
        seat = self.image_generation
        return (
            seat
            if seat is not None
            and any(choice.provider != host.provider for choice in [seat, *seat.fallbacks])
            else None
        )

    def group_enabled(self, group: OrchestrationGroup) -> bool:
        if self.enabled_roles is not None:
            return group in self.enabled_roles
        return any(role.group == group and role.legacy_enabled for role in role_catalog())

    def group_seats(self, group: OrchestrationGroup) -> dict[Difficulty, ReviewSeat]:
        if not self.group_enabled(group):
            return {}
        return cast(dict[Difficulty, ReviewSeat], getattr(self, group))

    def mode(self, host: Host) -> OrchestrationPreset | None:
        """The host's orchestration mode; None for a connection that cannot name its row."""
        key = host.app_id or host.name or ""
        if self.effective_apps is not None:
            # Old connections cannot prove which row started them, including rows whose
            # config repair failed. Reconnect rather than borrow the default row's mode.
            if host.app_id is None:
                return None
            return self.effective_apps.get(key, "off")
        return self.apps.get(key, self.preset)

    def enabled(self, host: Host) -> bool:
        return self.mode(host) not in (None, "off", "meetings")

    def host_cli_mode(self, host: Host) -> Literal["subagent", "launch", "skip"]:
        if self.effective_apps is not None and not host.app_id:
            return "subagent"
        return self.host_cli.get(host.app_id or host.name or "", "subagent")

    def decision(self, host: Host) -> bool:
        """Whether the host takes the user's own seat catalog instead of the fixed groups."""
        return self.mode(host) == "decision"


class SeatReplacement(StrictModel):
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(default_factory=dict[str, ProviderOption])
    reason: str


class FallbackStatus(SeatChoice):
    available: bool
    reason: str | None = None


class ReviewSeatStatus(StrictModel):
    provider: ProviderName
    model: str
    effort: str | None = None
    available: Literal[False] | None = None
    reason: str | None = None
    fallback_position: int | None = None
    fallbacks: list[FallbackStatus] = Field(default_factory=list[FallbackStatus])
    # The host's own CLI: dispatch to the host's built-in subagent, with this model if it can.
    native: Literal[True] | None = None
    # Other permitted task CLIs listing the same model ID.
    alternatives: list[ProviderName] = Field(default_factory=list[ProviderName])
    # A ready start_agent choice for an unavailable seat; the boss decides whether to use it.
    replacement: SeatReplacement | None = None


class CustomSeatStatus(ReviewSeatStatus):
    kind: str
    role: str
    description: str
    access: Literal["read", "write"]
    difficulty: CustomSeatDifficulty | None = None


class OrchestrationStatus(StrictModel):
    enabled: bool = False
    explorer: dict[Difficulty, ReviewSeatStatus] = {}
    worker: dict[Difficulty, ReviewSeatStatus] = {}
    worker_frontend: dict[Difficulty, ReviewSeatStatus] = {}
    worker_writing: dict[Difficulty, ReviewSeatStatus] = {}
    explorer_web: dict[Difficulty, ReviewSeatStatus] = {}
    explorer_social: dict[Difficulty, ReviewSeatStatus] = {}
    worker_documents: dict[Difficulty, ReviewSeatStatus] = {}
    image_generation: ReviewSeatStatus | None = None
    # The user's own catalog by seat id; it replaces the groups above while it is effective.
    custom_seats: dict[str, CustomSeatStatus] = {}
    # What each visible group covers, for bosses that never see the App's rules block. Plain
    # string keys keep hidden group names out of the published schema.
    scopes: dict[str, str] = {}

    @model_serializer(mode="wrap")
    def visible_groups(self, handler: SerializerFunctionWrapHandler):
        result: dict[str, Any] = handler(self)
        for role in role_catalog():
            if role.group not in self.model_fields_set:
                result.pop(role.group, None)
        if "custom_seats" not in self.model_fields_set:
            result.pop("custom_seats", None)
        return result
