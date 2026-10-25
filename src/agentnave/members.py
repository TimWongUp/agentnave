"""Read and enforce App-owned member defaults; never create or modify the data root."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping
from pathlib import Path
from typing import Literal, cast

from pydantic import BaseModel, ConfigDict, Field, model_validator

from agentnave.adapters import get_adapter
from agentnave.adapters.base import RESTRICTED_EXTRA_OPTIONS, restricted_options
from agentnave.host import Host, HostName, HostSource
from agentnave.models import InvocationRequest, ProviderOption

type ProviderName = Literal["antigravity", "claude", "codebuddy", "codex", "deepseek", "grok", "pi"]
type Mode = Literal["task", "discussion"]
type ReviewRole = Literal[
    "correctness-reviewer",
    "architecture-reviewer",
    "security-reviewer",
    "performance-reviewer",
    "test-reliability-reviewer",
    "simplicity-reviewer",
    "specialist-reviewer",
    "adversarial-verifier",
]


class MemberFileError(ValueError):
    """The App-owned document cannot be read or validated."""


class ProviderExcludedError(ValueError):
    """The task provider belongs to the host's exclusion set."""


class AdapterOptionsError(ValueError):
    """Adapter option validation rejected the invocation options."""


class SettingsFileError(ValueError):
    """The App-owned settings.json cannot be read or validated."""


class CapacityError(ValueError):
    """Starting would exceed the App's limit on concurrently running CLI processes."""


DEFAULT_MAX_RUNNING = 8

# Cached native identity is advisory: execution controls saved for one model must not
# be transferred to a different model if the CLI default changed since the last probe.
_MODEL_EXECUTION_OPTIONS = frozenset(
    {
        "permission_mode",
        "sandbox",
        "dangerously_bypass_approvals_and_sandbox",
        "dangerously_skip_permissions",
        "skip_git_repo_check",
        "agent",
        "tools",
        "mode",
        "project",
        "restricted_tools",
        "disable_slash_commands",
    }
)


def validate_provider_options(request: InvocationRequest) -> None:
    adapter = get_adapter(request.provider)
    try:
        normalized, _ = restricted_options(request)
        adapter.option_args(normalized)
    except ValueError as exc:
        raise AdapterOptionsError(str(exc)) from exc


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)


class MemberConfig(StrictModel):
    efforts: list[str] = Field(default_factory=list)
    effort: str = ""
    options: dict[str, ProviderOption] = Field(default_factory=dict)
    notes: str = ""

    @model_validator(mode="after")
    def option_keys(self) -> MemberConfig:
        if {"model", "effort"}.intersection(self.options):
            raise ValueError("options must not contain model or effort")
        return self

    def validate_effort(self) -> None:
        if "effort" in self.model_fields_set and self.efforts and self.effort not in self.efforts:
            raise ValueError(f"default effort {self.effort!r} must be in efforts")


class CLIConfig(StrictModel):
    models: dict[str, MemberConfig]


class ModeCLI(StrictModel):
    models: list[str]
    default: str = ""
    overrides: dict[str, MemberConfig] = Field(default_factory=dict)


class ReviewSeat(StrictModel):
    """The CLI and model the user picked in the App for one agentnave-review role."""

    provider: ProviderName
    model: str = Field(min_length=1)
    effort: str = ""


class MemberFile(StrictModel):
    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, CLIConfig]
    modes: dict[Mode, dict[ProviderName, ModeCLI]]
    # Default models for unrestricted modes, kept when the user turns the allow-list off.
    defaults: dict[Mode, dict[ProviderName, str]] = {}
    # Review seats are not checked against clis: a removed model is reported by list_members,
    # and the review Skill picks a replacement at dispatch time.
    review: dict[ReviewRole, ReviewSeat] = {}

    def effective(self, mode: Mode, provider: ProviderName, model: str) -> MemberConfig:
        cli = self.clis.get(provider)
        shared = cli.models.get(model, MemberConfig()) if cli else MemberConfig()
        selection = self.modes.get(mode, {}).get(provider)
        override = selection.overrides.get(model) if selection else None
        return MemberConfig.model_validate(
            shared.model_dump(exclude_unset=True)
            | (override.model_dump(exclude_unset=True) if override else {})
        )

    @model_validator(mode="after")
    def references(self) -> MemberFile:
        for cli in self.clis.values():
            for config in cli.models.values():
                config.validate_effort()
        for mode, providers in self.modes.items():
            for provider, selection in providers.items():
                cli = self.clis.get(provider)
                models = cli.models if cli else {}
                for model in [*selection.models, *selection.overrides]:
                    if model not in models:
                        raise ValueError(f"{mode}.{provider}: unknown model {model!r}")
                if "default" in selection.model_fields_set and selection.default not in (
                    selection.models or models
                ):
                    raise ValueError(f"{mode}.{provider}: default must be an allowed model")
                for model in models:
                    self.effective(mode, provider, model).validate_effort()
        for mode, providers in self.defaults.items():
            if mode in self.modes:
                raise ValueError(f"defaults.{mode}: only unrestricted modes keep defaults here")
            for provider, model in providers.items():
                cli = self.clis.get(provider)
                if model not in (cli.models if cli else {}):
                    raise ValueError(f"defaults.{mode}.{provider}: unknown model {model!r}")
        return self

    def apply(
        self,
        mode: Mode,
        provider: ProviderName,
        options: Mapping[str, ProviderOption],
        excluded: frozenset[str],
        *,
        restricted: bool = False,
    ) -> tuple[dict[str, ProviderOption], list[str]]:
        allowed = self.modes.get(mode)
        if allowed is not None and provider not in allowed:
            members = [
                f"{cli}/{model}"
                for cli, config in allowed.items()
                for model in (config.models or ["*"])
            ]
            raise ValueError(
                f"Provider {provider!r} is not allowed in {mode}; allowed members: "
                + (", ".join(members) or "none")
            )
        check_excluded(mode, provider, excluded)
        result = dict(options)
        applied: list[str] = []
        selection = allowed.get(provider) if allowed is not None else None
        if "model" not in result and selection and "default" in selection.model_fields_set:
            result["model"] = selection.default
            applied.append("model")
        if "model" not in result and allowed is None and provider in self.defaults.get(mode, {}):
            result["model"] = self.defaults[mode][provider]
            applied.append("model")
        if "model" not in result and selection and selection.models:
            raise ValueError(
                f"Model is required in {mode}.{provider}; "
                f"allowed models: {', '.join(selection.models)}"
            )
        if (
            "model" in result
            and selection
            and selection.models
            and result["model"] not in selection.models
        ):
            raise ValueError(
                f"Model {result['model']!r} is not allowed in {mode}.{provider}; "
                f"allowed models: {', '.join(selection.models)}"
            )
        # Without a model the CLI runs its own default; the settings saved for that model
        # still apply, and the model is left for the CLI to choose (ADR-0028).
        model = result.get("model")
        config = self.effective(
            mode, provider, str(model) if model is not None else cli_default_model(provider) or ""
        )
        if "effort" not in result and "effort" in config.model_fields_set:
            result["effort"] = config.effort
            applied.append("effort")
        if "effort" in result and config.efforts and result["effort"] not in config.efforts:
            raise ValueError(
                f"Effort {result['effort']!r} is not allowed; allowed efforts: "
                + ", ".join(config.efforts)
            )
        for key, value in config.options.items():
            if model is None and key in _MODEL_EXECUTION_OPTIONS:
                continue
            if restricted and key not in RESTRICTED_EXTRA_OPTIONS.get(provider, set()):
                continue
            if key not in result:
                result[key] = value
                applied.append(key)
        return result, applied


def check_excluded(mode: Mode, provider: str, excluded: frozenset[str]) -> None:
    if mode == "task" and provider in excluded:
        raise ProviderExcludedError(f"Provider '{provider}' is excluded by this host")


def read_max_running() -> int:
    """The App's limit on CLI runs (tasks plus seats) this server keeps active at once."""
    root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
    try:
        document = json.loads((root / "settings.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return DEFAULT_MAX_RUNNING
    except (OSError, ValueError) as exc:
        raise SettingsFileError(
            f"Unable to read settings.json: {exc}. Repair it in the AgentNave App."
        ) from exc
    settings = cast(dict[str, object], document) if isinstance(document, dict) else {}
    version = settings.get("version")
    value = settings.get("max_running", DEFAULT_MAX_RUNNING)
    # Same rules as the App: integers only, so true or 1.0 are not accepted as 1.
    if type(version) is not int or version != 1 or type(value) is not int or not 1 <= value <= 32:
        raise SettingsFileError(
            "Invalid settings.json: needs version 1 and max_running between 1 and 32. "
            "Repair it in the AgentNave App."
        )
    return value


def cli_default_model(provider: str) -> str | None:
    """The default model the App last detected for one CLI, if it published one."""
    root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
    try:
        document = json.loads((root / "cli-defaults.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None

    def field(value: object, key: str) -> object:
        return cast(dict[str, object], value).get(key) if isinstance(value, dict) else None

    model = field(field(field(document, "clis"), provider), "model")
    return model if isinstance(model, str) and model else None


def read_members() -> MemberFile | None:
    root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
    try:
        content = (root / "members.json").read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        raise MemberFileError(
            f"Unable to read members.json: {exc}. Repair it in the AgentNave App."
        ) from exc
    try:
        return MemberFile.model_validate_json(content)
    except ValueError as exc:
        raise MemberFileError(
            f"Invalid members.json: {exc}. Repair it in the AgentNave App."
        ) from exc


def apply_member(
    document: MemberFile | None,
    mode: Mode,
    provider: ProviderName,
    options: Mapping[str, ProviderOption],
    excluded: frozenset[str],
    *,
    restricted: bool = False,
) -> tuple[dict[str, ProviderOption], list[str]]:
    if document is not None:
        return document.apply(mode, provider, options, excluded, restricted=restricted)
    check_excluded(mode, provider, excluded)
    return dict(options), []


class MemberStatus(StrictModel):
    provider: ProviderName
    model: str
    default: bool
    efforts: list[str]
    effort: str | None
    options: dict[str, ProviderOption]
    notes: str
    permitted: bool
    valid: bool
    error: str | None


class ModeStatus(StrictModel):
    restricted: bool
    members: list[MemberStatus]


class ReviewSeatStatus(StrictModel):
    provider: ProviderName
    model: str
    effort: str | None
    available: bool
    reason: str | None
    # Other permitted task CLIs listing the same model ID.
    alternatives: list[ProviderName]


class MemberList(StrictModel):
    configured: bool
    error: str | None = None
    host: HostName | None
    host_source: HostSource
    modes: dict[Mode, ModeStatus]
    review: dict[ReviewRole, ReviewSeatStatus] = {}


def list_members(excluded: frozenset[str], host: Host) -> MemberList:
    document = read_members()
    modes: dict[Mode, ModeStatus] = {}
    for mode in ("task", "discussion"):
        items: list[MemberStatus] = []
        if document is not None:
            providers = document.modes.get(mode)
            for provider in providers if providers is not None else document.clis:
                selection = providers.get(provider) if providers is not None else None
                cli = document.clis.get(provider)
                models = (
                    selection.models
                    if selection and selection.models
                    else (cli.models if cli else {})
                )
                for model in dict.fromkeys(models):
                    config = document.effective(mode, provider, model)
                    effort = config.effort if "effort" in config.model_fields_set else None
                    options = dict(config.options)
                    options["model"] = model
                    if effort is not None:
                        options["effort"] = effort
                    error = None
                    try:
                        request = InvocationRequest(
                            provider,
                            "Validate member options",
                            Path(__file__).resolve().parent,
                            provider_options=options,
                        )
                        validate_provider_options(request)
                    except ValueError as exc:
                        error = str(exc)
                    items.append(
                        MemberStatus(
                            provider=provider,
                            model=model,
                            default=(
                                selection is not None
                                and "default" in selection.model_fields_set
                                and selection.default == model
                            )
                            or (
                                providers is None
                                and document.defaults.get(mode, {}).get(provider) == model
                            ),
                            efforts=config.efforts,
                            effort=effort,
                            options=config.options,
                            notes=config.notes,
                            permitted=mode == "discussion" or provider not in excluded,
                            valid=error is None,
                            error=error,
                        )
                    )
                if selection is not None and not selection.models:
                    items.append(
                        MemberStatus(
                            provider=provider,
                            model="*",
                            default=False,
                            efforts=[],
                            effort=None,
                            options={},
                            notes="不限模型",
                            permitted=mode == "discussion" or provider not in excluded,
                            valid=True,
                            error=None,
                        )
                    )
        modes[mode] = ModeStatus(
            restricted=document is not None and mode in document.modes, members=items
        )
    return MemberList(
        configured=document is not None,
        host=host.name,
        host_source=host.source,
        modes=modes,
        review=_review_status(document, modes["task"].members) if document else {},
    )


def _review_status(
    document: MemberFile, task: list[MemberStatus]
) -> dict[ReviewRole, ReviewSeatStatus]:
    seats: dict[ReviewRole, ReviewSeatStatus] = {}
    for role, seat in document.review.items():
        member = next(
            (m for m in task if m.provider == seat.provider and m.model in (seat.model, "*")),
            None,
        )
        if member is None:
            reason = "model is not among this CLI's task members"
        elif not member.permitted:
            reason = "CLI is excluded by this host"
        elif not member.valid:
            reason = member.error
        elif seat.effort and member.efforts and seat.effort not in member.efforts:
            reason = f"effort {seat.effort!r} is not allowed for this model"
        else:
            reason = None
        alternatives: list[ProviderName] = [
            m.provider
            for m in task
            if m.provider != seat.provider and m.model == seat.model and m.permitted and m.valid
        ]
        seats[role] = ReviewSeatStatus(
            provider=seat.provider,
            model=seat.model,
            effort=seat.effort or None,
            available=reason is None,
            reason=reason,
            alternatives=list(dict.fromkeys(alternatives)),
        )
    return seats
