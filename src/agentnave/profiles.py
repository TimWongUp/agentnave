"""Read user-owned orchestration profiles without preparing or launching a CLI."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from agentnave.adapters import get_adapter
from agentnave.adapters.base import restricted_options
from agentnave.models import InvocationRequest, ProviderOption


class Profile(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)

    id: str
    name: str
    provider: Literal["antigravity", "claude", "codebuddy", "codex", "grok", "pi"]
    provider_options: dict[str, ProviderOption]
    notes: str

    @field_validator("id", "name")
    @classmethod
    def nonblank(cls, value: str) -> str:
        if not value.strip():
            raise ValueError("must not be blank")
        return value


class ProfileFile(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    version: int = Field(ge=1, le=1)
    profiles: list[Profile]

    @model_validator(mode="after")
    def unique_ids(self) -> ProfileFile:
        ids = [profile.id for profile in self.profiles]
        if len(set(ids)) != len(ids):
            raise ValueError("profile ids must be unique")
        return self


class ProfileStatus(Profile):
    valid: bool
    error: str | None
    permitted: bool


class ProfileList(BaseModel):
    profiles: list[ProfileStatus]


def list_profiles(excluded: frozenset[str]) -> ProfileList:
    root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
    path = root / "profiles.json"
    try:
        content = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return ProfileList(profiles=[])
    document = ProfileFile.model_validate_json(content)
    profiles: list[ProfileStatus] = []
    for profile in document.profiles:
        error = None
        try:
            # The package directory exists even when the data root does not. Option validation
            # needs no task cwd, prompt file, executable discovery or restricted-seat files.
            request = InvocationRequest(
                profile.provider,
                "Validate profile options",
                Path(__file__).resolve().parent,
                provider_options=profile.provider_options,
            )
            request, _ = restricted_options(request)
            get_adapter(profile.provider).option_args(request)
        except ValueError as exc:
            error = str(exc)
        profiles.append(
            ProfileStatus(
                **profile.model_dump(),
                valid=error is None,
                error=error,
                permitted=profile.provider not in excluded,
            )
        )
    return ProfileList(profiles=profiles)
