"""Typed task seats and the shared, product-owned role catalog.

Literal aliases describe the wire contract for static consumers; catalog metadata supplies
runtime grouping, visibility and access. Contract tests check the aliases against the catalog.
"""

from __future__ import annotations

from functools import cache
from pathlib import Path
from typing import Any, Literal, cast

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    SerializerFunctionWrapHandler,
    TypeAdapter,
    field_validator,
    model_serializer,
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
type OrchestrationPreset = Literal["off", "split", "parallel", "custom"]
type OrchestrationGroup = Literal[
    "explorer",
    "worker",
    "worker_code",
    "worker_frontend",
    "worker_writing",
    "explorer_web",
    "explorer_social",
    "worker_data",
    "worker_documents",
]
type OrchestrationRole = Literal[
    "explorer-easy",
    "explorer-hard",
    "worker-easy",
    "worker-hard",
    "worker-code-easy",
    "worker-code-hard",
    "worker-frontend-easy",
    "worker-frontend-hard",
    "worker-writing-easy",
    "worker-writing-hard",
    "explorer-web-easy",
    "explorer-web-hard",
    "explorer-social-easy",
    "explorer-social-hard",
    "worker-data-easy",
    "worker-data-hard",
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


class ReviewSeat(StrictModel):
    """The CLI and conversational model the user picked in the App for one seat."""

    provider: ProviderName
    model: str = Field(pattern=r"\S")
    effort: str = ""


class OrchestrationFile(BaseModel):
    """App-owned rule modes and saved task seats; only enabled hosts use seats."""

    model_config = ConfigDict(extra="ignore")

    version: int = Field(ge=1, le=1)
    preset: OrchestrationPreset = "off"
    apps: dict[str, OrchestrationPreset] = {}
    effective_apps: dict[str, OrchestrationPreset] | None = None
    explorer: dict[Difficulty, ReviewSeat] = {}
    worker: dict[Difficulty, ReviewSeat] = {}
    # A key of its own, which servers that predate it ignore instead of rejecting the file.
    worker_frontend: dict[Difficulty, ReviewSeat] = {}
    worker_code: dict[Difficulty, ReviewSeat] = {}
    worker_writing: dict[Difficulty, ReviewSeat] = {}
    explorer_web: dict[Difficulty, ReviewSeat] = {}
    explorer_social: dict[Difficulty, ReviewSeat] = {}
    worker_data: dict[Difficulty, ReviewSeat] = {}
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
        return seat if seat is not None and seat.provider != host.provider else None

    def interface_workers(self) -> dict[Difficulty, ReviewSeat]:
        """The interface Worker seats, an unset one filled from the Worker seat of its difficulty."""
        if not self.group_enabled("worker_frontend"):
            return {}
        return {**(self.worker if self.group_enabled("worker") else {}), **self.worker_frontend}

    def group_enabled(self, group: OrchestrationGroup) -> bool:
        if self.enabled_roles is not None:
            return group in self.enabled_roles
        return any(role.group == group and role.legacy_enabled for role in role_catalog())

    def group_seats(self, group: OrchestrationGroup) -> dict[Difficulty, ReviewSeat]:
        if not self.group_enabled(group):
            return {}
        if group == "worker_frontend":
            return self.interface_workers()
        return cast(dict[Difficulty, ReviewSeat], getattr(self, group))

    def enabled(self, host: Host) -> bool:
        key = host.app_id or host.name or ""
        if self.effective_apps is not None:
            # Old connections cannot prove which row started them, including rows whose
            # config repair failed. Reconnect rather than borrow the default row's mode.
            if host.app_id is None:
                return False
            return self.effective_apps.get(key, "off") != "off"
        return self.apps.get(key, self.preset) != "off"


class SeatReplacement(StrictModel):
    provider: ProviderName
    provider_options: dict[str, ProviderOption] = Field(default_factory=dict[str, ProviderOption])
    reason: str


class ReviewSeatStatus(StrictModel):
    provider: ProviderName
    model: str
    effort: str | None = None
    available: Literal[False] | None = None
    reason: str | None = None
    # The host's own CLI: dispatch to the host's built-in subagent, with this model if it can.
    native: Literal[True] | None = None
    # Other permitted task CLIs listing the same model ID.
    alternatives: list[ProviderName] = Field(default_factory=list[ProviderName])
    # A ready start_agent choice for an unavailable seat; the boss decides whether to use it.
    replacement: SeatReplacement | None = None


class OrchestrationStatus(StrictModel):
    enabled: bool = False
    explorer: dict[Difficulty, ReviewSeatStatus] = {}
    worker: dict[Difficulty, ReviewSeatStatus] = {}
    worker_frontend: dict[Difficulty, ReviewSeatStatus] = {}
    worker_code: dict[Difficulty, ReviewSeatStatus] = {}
    worker_writing: dict[Difficulty, ReviewSeatStatus] = {}
    explorer_web: dict[Difficulty, ReviewSeatStatus] = {}
    explorer_social: dict[Difficulty, ReviewSeatStatus] = {}
    worker_data: dict[Difficulty, ReviewSeatStatus] = {}
    worker_documents: dict[Difficulty, ReviewSeatStatus] = {}
    image_generation: ReviewSeatStatus | None = None

    @model_serializer(mode="wrap")
    def visible_groups(self, handler: SerializerFunctionWrapHandler):
        result: dict[str, Any] = handler(self)
        for role in role_catalog():
            if role.group not in self.model_fields_set:
                result.pop(role.group, None)
        return result
