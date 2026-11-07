"""Read and enforce App-owned member defaults; never create or modify the data root."""

from __future__ import annotations

import json
import logging
import os
from collections.abc import Callable, Mapping
from functools import cache
from pathlib import Path
from typing import Literal, cast, get_args

from pydantic import BaseModel, Field, model_validator

from agentnave.adapters import get_adapter
from agentnave.adapters.base import restricted_options
from agentnave.host import Host, HostName, HostSource
from agentnave.member_policy import (
    check_cli_model,
    cli_confirmed,
    has_cli_pool,
    model_pool,
    select_model,
)
from agentnave.models import InvocationRequest, ProviderOption
from agentnave.orchestration_roles import (
    Difficulty as Difficulty,
)
from agentnave.orchestration_roles import (
    OrchestrationFile as OrchestrationFile,
)
from agentnave.orchestration_roles import (
    OrchestrationGroup,
    StrictModel,
    role_catalog,
    seat_role,
)
from agentnave.orchestration_roles import (
    OrchestrationRole as OrchestrationRole,
)
from agentnave.orchestration_roles import (
    OrchestrationStatus as OrchestrationStatus,
)
from agentnave.orchestration_roles import (
    ProviderName as ProviderName,
)
from agentnave.orchestration_roles import (
    ReviewSeat as ReviewSeat,
)
from agentnave.orchestration_roles import (
    ReviewSeatStatus as ReviewSeatStatus,
)
from agentnave.orchestration_roles import (
    SeatReplacement as SeatReplacement,
)

type Mode = Literal["task", "discussion"]
type ReviewRole = Literal[
    "correctness-reviewer",
    "correctness-reviewer-easy",
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


class PermissionFileError(ValueError):
    """The App-owned permissions.json cannot be read or validated."""


class CapacityError(ValueError):
    """Starting would exceed the App's limit on concurrently running CLI processes."""


DEFAULT_MAX_RUNNING = 8

# Cached native identity is advisory: execution controls saved for one model must not
# be transferred to a different model if the CLI default changed since the last probe.
_MODEL_EXECUTION_OPTIONS = frozenset(
    {
        "permission_mode",
        "sandbox",
        "approve_for_me",
        "dangerously_bypass_approvals_and_sandbox",
        "dangerously_skip_permissions",
        "agent",
        "tools",
        "mode",
        "project",
        "restricted_tools",
        "disable_slash_commands",
        "interactive",
    }
)


type PermissionTier = Literal["auto", "full"]

# The native options each App permission tier stands for, per CLI; a CLI without a saved tier
# runs "auto", since a dispatched CLI cannot ask anyone and native defaults deny tool calls.
# "auto" lets the CLI's own reviewer or classifier approve tool calls; CodeBuddy's auto denies
# shell under -p, and DeepSeek, ZCode and Antigravity have no usable reviewer, so their auto
# grants full access and the App offers them no choice. Pi has no approval step.
PERMISSION_TIERS: dict[str, dict[PermissionTier, dict[str, ProviderOption]]] = {
    # ZCode's "auto" is reserved and rejects every tool; it has no automatic reviewer yet.
    "zcode": {"auto": {"permission_mode": "yolo"}, "full": {"permission_mode": "yolo"}},
    # When the chosen mode is unavailable Claude Code starts in manual mode; it then reruns
    # in the other tier's mode (ADR-0059).
    "claude": {
        "auto": {"permission_mode": "auto", "permission_fallback": "bypassPermissions"},
        "full": {"permission_mode": "bypassPermissions", "permission_fallback": "auto"},
    },
    "codebuddy": {
        "auto": {"permission_mode": "bypassPermissions"},
        "full": {"permission_mode": "bypassPermissions"},
    },
    "grok": {
        "auto": {"permission_mode": "auto"},
        "full": {"permission_mode": "bypassPermissions"},
    },
    "antigravity": {
        "auto": {"dangerously_skip_permissions": True},
        "full": {"dangerously_skip_permissions": True},
    },
    "codex": {
        "auto": {"approve_for_me": True},
        "full": {"dangerously_bypass_approvals_and_sandbox": True},
    },
    "deepseek": {
        "auto": {"permission_mode": "danger-full-access"},
        "full": {"permission_mode": "danger-full-access"},
    },
}
# Options that decide what a CLI may run unasked. A caller passing any of them keeps the
# CLI's tier out; a tier replaces saved model options among them.
PERMISSION_KEYS: dict[str, frozenset[str]] = {
    "zcode": frozenset({"permission_mode"}),
    "claude": frozenset({"permission_mode", "permission_fallback"}),
    "codebuddy": frozenset({"permission_mode"}),
    "grok": frozenset({"permission_mode", "sandbox"}),
    "antigravity": frozenset({"mode", "sandbox", "dangerously_skip_permissions"}),
    "codex": frozenset({"approve_for_me", "dangerously_bypass_approvals_and_sandbox", "sandbox"}),
    "deepseek": frozenset({"permission_mode"}),
}


def validate_provider_options(request: InvocationRequest) -> None:
    adapter = get_adapter(request.provider)
    try:
        normalized, _ = restricted_options(request, adapter)
        adapter.option_args(normalized)
    except ValueError as exc:
        raise AdapterOptionsError(str(exc)) from exc


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
    allowed_models: list[str] | None = None

    @model_validator(mode="after")
    def model_pool(self) -> CLIConfig:
        if self.allowed_models is not None:
            if not self.allowed_models or len(set(self.allowed_models)) != len(self.allowed_models):
                raise ValueError("allowed_models must be nonempty and unique")
            if any(model not in self.models for model in self.allowed_models):
                raise ValueError("allowed_models must reference known CLI models")
        return self


class ModeCLI(StrictModel):
    models: list[str]
    default: str = ""
    overrides: dict[str, MemberConfig] = Field(default_factory=dict)


class ReviewFile(StrictModel):
    """review.json, apart from members.json so older servers keep reading their members."""

    version: int = Field(ge=1, le=1)
    seats: dict[ReviewRole, ReviewSeat] = {}


class MemberFile(StrictModel):
    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, CLIConfig]
    modes: dict[Mode, dict[ProviderName, ModeCLI]]
    # Default models for unrestricted modes, kept when the user turns the allow-list off.
    defaults: dict[Mode, dict[ProviderName, str]] = {}

    def effective(
        self, mode: Mode, provider: ProviderName, model: str, *, scenario: bool = True
    ) -> MemberConfig:
        cli = self.clis.get(provider)
        shared = cli.models.get(model, MemberConfig()) if cli else MemberConfig()
        selection = self.modes.get(mode, {}).get(provider) if scenario else None
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
                    models if has_cli_pool(cli) else selection.models or models
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
        seat: frozenset[str] = frozenset(),
        scenario: bool = True,
        seat_selected: bool = False,
    ) -> tuple[dict[str, ProviderOption], list[str]]:
        """Apply shared CLI boundaries, then the visible scenario's defaults."""
        check_excluded(mode, provider, excluded)
        result, applied = select_model(
            self, mode, provider, options, seat, scenario=scenario, seat_selected=seat_selected
        )
        # Without a model the CLI runs its own default; the settings saved for that model
        # still apply, and the model is left for the CLI to choose (ADR-0028).
        model = result.get("model")
        config = self.effective(
            mode,
            provider,
            str(model) if model is not None else cli_default_model(provider) or "",
            scenario=scenario and not seat_selected,
        )
        if "effort" not in result and "effort" in config.model_fields_set:
            result["effort"] = config.effort
            applied.append("effort")
        if (
            "effort" in result
            and "effort" not in seat
            and config.efforts
            and result["effort"] not in config.efforts
        ):
            raise ValueError(
                f"Effort {result['effort']!r} is not allowed; allowed efforts: "
                + ", ".join(config.efforts)
            )
        for key, value in config.options.items():
            if model is None and key in _MODEL_EXECUTION_OPTIONS:
                continue
            if restricted and key not in get_adapter(provider).restricted_extra_options:
                continue
            if key not in result:
                result[key] = value
                applied.append(key)
        return result, applied


class PermissionFile(StrictModel):
    """permissions.json, apart from members.json so older servers keep reading their members."""

    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, PermissionTier] = {}


def data_root() -> Path:
    """Where the App keeps its settings and the server its history."""
    return Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")


def _repair(name: str) -> str:
    """Hosts without the App (Linux) have no App to repair a file in."""
    return f"Repair it in the AgentNave App, or fix or remove {data_root() / name}."


def _read_app_file[T: BaseModel](name: str, model: type[T], error: type[ValueError]) -> T | None:
    """One App settings file of the data root; None when missing. A broken file raises `error`
    with the App repair hint, never a fallback that would run with other settings."""
    try:
        content = (data_root() / name).read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        raise error(f"Unable to read {name}: {exc}. {_repair(name)}") from exc
    try:
        return model.model_validate_json(content)
    except ValueError as exc:
        raise error(f"Invalid {name}: {exc}. {_repair(name)}") from exc


def read_permissions() -> PermissionFile | None:
    return _read_app_file("permissions.json", PermissionFile, PermissionFileError)


def apply_permission(
    permissions: PermissionFile | None,
    provider: ProviderName,
    explicit: Mapping[str, ProviderOption],
    result: dict[str, ProviderOption],
    applied: list[str],
) -> None:
    """Fill the options of the CLI's App permission tier unless the caller chose its own.

    Either way saved model permission options are dropped: the tier or the caller decides.
    """
    tier = permissions.clis.get(provider, "auto") if permissions is not None else "auto"
    granted = PERMISSION_TIERS.get(provider, {}).get(tier)
    keys = PERMISSION_KEYS.get(provider, frozenset())
    # Saved model options must not mix with the tier, nor with a caller's own permissions.
    for key in keys & set(applied):
        del result[key]
        applied.remove(key)
    if not granted or keys & set(explicit):
        return
    for key, value in granted.items():
        result[key] = value
        applied.append(key)


# How defaults_applied names a level taken from the CLI's own settings, apart from App ones.
CLI_EFFORT_APPLIED = "effort (CLI default)"


def check_excluded(mode: Mode, provider: str, excluded: frozenset[str]) -> None:
    if mode == "task" and provider in excluded:
        raise ProviderExcludedError(f"Provider '{provider}' is excluded by this host")


def _read_settings() -> dict[str, object]:
    try:
        document = json.loads((data_root() / "settings.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as exc:
        raise SettingsFileError(
            f"Unable to read settings.json: {exc}. {_repair('settings.json')}"
        ) from exc
    settings = cast(dict[str, object], document) if isinstance(document, dict) else {}
    version = settings.get("version")
    value = settings.get("max_running", DEFAULT_MAX_RUNNING)
    # Same rules as the App: integers only, so true or 1.0 are not accepted as 1.
    if (
        type(version) is not int
        or version != 1
        or type(value) is not int
        or not 1 <= value <= 32
        or type(settings.get("claude_interactive", False)) is not bool
    ):
        raise SettingsFileError(
            "Invalid settings.json: needs version 1, max_running between 1 and 32 "
            "and an optional boolean claude_interactive. " + _repair("settings.json")
        )
    return settings


def read_max_running() -> int:
    """The App's limit on CLI runs (tasks plus seats) this server keeps active at once."""
    return cast(int, _read_settings().get("max_running", DEFAULT_MAX_RUNNING))


def read_claude_interactive() -> bool:
    """The interactive Claude runtime is on unless the App turned it off (ADR-0056)."""
    return cast(bool, _read_settings().get("claude_interactive", True))


def read_history_retention_days(root: Path | None = None) -> int | None:
    """Invalid or unreadable settings disable deletion, never prevent service startup.

    As with read_max_running, require version 1 and a strict integer; retention is
    fail-closed because a broken configuration must not authorize deleting history.
    """
    path = (root or data_root()) / "settings.json"
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
        settings = cast(dict[str, object], document) if isinstance(document, dict) else {}
        version = settings.get("version")
        value = settings.get("history_retention_days")
        limit = settings.get("max_running", DEFAULT_MAX_RUNNING)
        if (
            type(version) is not int
            or version != 1
            or type(limit) is not int
            or not 1 <= limit <= 32
            or (value is not None and (type(value) is not int or value not in (30, 90, 180)))
        ):
            raise ValueError("invalid retention settings")
        return cast(int | None, value)
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        logging.getLogger(__name__).warning(
            "History cleanup is disabled: invalid settings at %s; use version 1, "
            "max_running 1..32 and history_retention_days 30, 90 or 180 "
            "(omit it or use null to keep history forever)",
            path,
        )
        return None


def _field(value: object, key: str) -> object:
    return cast(dict[str, object], value).get(key) if isinstance(value, dict) else None


def _cli_defaults(provider: str) -> object:
    """One CLI's entry in the defaults the App last detected; None when missing or broken."""
    try:
        document = json.loads((data_root() / "cli-defaults.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return _field(_field(document, "clis"), provider)


def cli_default_model(provider: str) -> str | None:
    """The default model the App last detected for one CLI, if it published one."""
    model = _field(_cli_defaults(provider), "model")
    return model if isinstance(model, str) and model else None


def cli_default_effort(provider: str, model: str | None) -> str | None:
    """The level the CLI's own settings run a model at, or its default model when none is
    named, as the App last detected it (ADR-0042)."""
    entry = _cli_defaults(provider)
    effort = _field(entry, "effort") if model is None else _field(_field(entry, "efforts"), model)
    return effort if isinstance(effort, str) and effort else None


def read_members() -> MemberFile | None:
    return _read_app_file("members.json", MemberFile, MemberFileError)


def orchestration_enabled(host: Host) -> bool:
    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError:
        return False  # list_members reports a broken file; never invent an enabled mode.
    return roles is not None and roles.enabled(host)


def visible_role_groups(host: Host) -> frozenset[OrchestrationGroup]:
    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError:
        return frozenset()
    if roles is None or not roles.enabled(host):
        return frozenset()
    return frozenset(role.group for role in role_catalog() if roles.group_enabled(role.group))


def validate_task_seat(name: str, host: Host) -> None:
    """Recheck new dispatches after history inheritance, including explicit replacements.

    Image replacement behavior is independent of ordinary role switches.
    """
    if name not in get_args(OrchestrationRole.__value__):
        raise MemberFileError(f"Unknown task seat: {name}")
    if name == "image-generator":
        return
    definition = seat_role(name)
    assert definition is not None
    roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    enabled = roles is not None and roles.enabled(host) and roles.group_enabled(definition.group)
    if not enabled:
        raise MemberFileError(
            f"{name} seat is not enabled in orchestration.json; choose a visible seat or set a new role "
            "to continue without the inherited seat. Enable this seat in the App only if intended."
        )


def apply_member(
    document: MemberFile | None,
    mode: Mode,
    provider: ProviderName,
    options: Mapping[str, ProviderOption],
    excluded: frozenset[str],
    *,
    restricted: bool = False,
    permissions: PermissionFile | None = None,
    seat: frozenset[str] = frozenset(),
    scenario: bool = True,
    seat_selected: bool = False,
) -> tuple[dict[str, ProviderOption], list[str]]:
    if document is not None:
        result, applied = document.apply(
            mode,
            provider,
            options,
            excluded,
            restricted=restricted,
            seat=seat,
            scenario=scenario,
            seat_selected=seat_selected,
        )
    else:
        check_excluded(mode, provider, excluded)
        result, applied = dict(options), []
    # Restricted runs take no tools, so no permission tier either.
    if not restricted and result.get("restricted_tools") is not True:
        apply_permission(permissions, provider, options, result, applied)
        if (
            provider == "claude"
            and "interactive" not in result
            and os.name != "nt"
            and read_claude_interactive()
        ):
            result["interactive"] = True
            applied.append("interactive")
    if "effort" not in result:
        add_cli_effort(
            document, mode, provider, result, applied, scenario=scenario and not seat_selected
        )
    return result, applied


def add_cli_effort(
    document: MemberFile | None,
    mode: Mode,
    provider: ProviderName,
    result: dict[str, ProviderOption],
    applied: list[str],
    *,
    scenario: bool = True,
) -> None:
    """Pass the level the CLI would pick by itself, so every run names and records one
    (ADR-0042). A level the App does not allow for the model is left to the CLI."""
    model = result.get("model")
    effort = cli_default_effort(provider, str(model) if model is not None else None)
    if effort is None:
        return
    if document is not None:
        key = str(model) if model is not None else cli_default_model(provider) or ""
        allowed = document.effective(mode, provider, key, scenario=scenario).efforts
        if allowed and effort not in allowed:
            return
    result["effort"] = effort
    applied.append(CLI_EFFORT_APPLIED)


def role_seat(role: str | None, host: Host, *, required: bool = False) -> ReviewSeat | None:
    """The App seat a task role names: an agentnave-review role from review.json, or a
    task seat from orchestration.json. A missing or broken file names none;
    list_members reports it. Provider inference requires a usable seat instead."""
    role = (role or "").strip()
    seat = None
    try:
        if role in get_args(ReviewRole.__value__):
            review = _read_app_file("review.json", ReviewFile, MemberFileError)
            seat = review.seats.get(cast(ReviewRole, role)) if review else None
        elif role in get_args(OrchestrationRole.__value__):
            roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
            if required and roles is not None and not roles.enabled(host):
                raise MemberFileError(
                    f"{role} seat is not enabled for this host in orchestration.json; "
                    "enable it in the App or pass provider explicitly"
                )
            if (
                required
                and roles is not None
                and (definition := seat_role(role)) is not None
                and not roles.group_enabled(definition.group)
            ):
                raise MemberFileError(
                    f"{role} seat is not enabled in orchestration.json; enable it in the App"
                )
            if not roles or not roles.enabled(host):
                seat = None
            elif role == "image-generator":
                seat = roles.image_seat(host)
            else:
                definition = seat_role(role)
                assert definition is not None
                seat = roles.group_seats(definition.group).get(
                    cast(Difficulty, role.rsplit("-", 1)[1])
                )
    except MemberFileError:
        if required:
            raise
        return None
    if required and seat is None:
        raise MemberFileError(
            f"{role or 'Requested'} seat is not configured in review.json or orchestration.json; "
            "set it in the App or pass provider explicitly"
        )
    return seat


def apply_seat(
    seat: ReviewSeat | None, provider: ProviderName, options: Mapping[str, ProviderOption]
) -> tuple[dict[str, ProviderOption], list[str]]:
    """Fill the model and effort the user picked for the seat, when it uses this CLI. Explicit
    options win; the seat's effort only goes with its own model."""
    result = dict(options)
    if seat is None or seat.provider != provider:
        return result, []
    applied: list[str] = []
    if "model" not in result:
        result["model"] = seat.model
        applied.append("model")
    if seat.effort and "effort" not in result and result["model"] == seat.model:
        result["effort"] = seat.effort
        applied.append("effort")
    return result, applied


class MemberDetails(StrictModel):
    provider: ProviderName
    model: str = ""
    efforts: list[str] = Field(default_factory=list)
    effort: str | None = None
    options: dict[str, ProviderOption] = Field(default_factory=dict[str, ProviderOption])
    notes: str = ""
    error: str | None = None


class MemberStatus(MemberDetails):
    default: bool
    permitted: bool


class ListedMember(MemberDetails):
    # None means all modes; [] means configured but not permitted in any mode.
    modes: list[Mode] | None = None
    default: Literal[True] | list[Mode] | None = None


class ModeStatus(StrictModel):
    restricted: bool
    members: list[MemberStatus]


class MemberList(StrictModel):
    configured: bool
    error: str | None = None
    host: HostName | None = None
    host_source: HostSource
    restricted: list[Mode] = Field(default_factory=list[Mode])
    members: list[ListedMember]
    review: dict[ReviewRole, ReviewSeatStatus] = {}
    review_error: str | None = None
    orchestration: OrchestrationStatus = Field(default_factory=OrchestrationStatus)
    orchestration_error: str | None = None
    # The App permission tier of each CLI that has one, "auto" unless the user picked "full".
    permissions: dict[ProviderName, PermissionTier] = {}
    permissions_error: str | None = None


def _member_modes(
    document: MemberFile | None,
    excluded: frozenset[str],
    installed: Callable[[str], bool],
    *,
    cli_only: bool = False,
) -> dict[Mode, ModeStatus]:
    modes: dict[Mode, ModeStatus] = {}
    for mode in ("task", "discussion"):
        items: list[MemberStatus] = []
        if document is not None:
            providers = document.modes.get(mode) if not cli_only else None
            candidates = list(document.clis)
            for entries in document.modes.values():
                candidates.extend(p for p in entries if p not in candidates)
            for provider in providers if providers is not None else candidates:
                if cli_only and not cli_confirmed(document, provider):
                    continue
                selection = providers.get(provider) if providers is not None else None
                cli = document.clis.get(provider)
                pool_selection = document.modes.get(mode, {}).get(provider)
                if cli_only and pool_selection is None:
                    pool_selection = document.modes.get("discussion", {}).get(provider)
                pool = model_pool(cli, pool_selection)
                models = pool or (cli.models if cli else {})
                for model in dict.fromkeys(models):
                    config = document.effective(mode, provider, model, scenario=not cli_only)
                    effort = config.effort if "effort" in config.model_fields_set else None
                    options = dict(config.options)
                    options["model"] = model
                    if effort is not None:
                        options["effort"] = effort
                    # Validate what a dispatch would send; options below stay the model's own.
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
                    if not installed(provider):
                        error = "CLI is not installed"
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
                                not cli_only
                                and providers is None
                                and document.defaults.get(mode, {}).get(provider) == model
                            ),
                            efforts=config.efforts,
                            effort=effort,
                            options=config.options,
                            notes=config.notes,
                            permitted=mode == "discussion" or provider not in excluded,
                            error=error,
                        )
                    )
                if pool is None and (selection is not None or has_cli_pool(cli) or cli_only):
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
                            error=None if installed(provider) else "CLI is not installed",
                        )
                    )
        modes[mode] = ModeStatus(
            restricted=document is not None and (cli_only or mode in document.modes), members=items
        )
    return modes


def _compact_members(modes: dict[Mode, ModeStatus], excluded: frozenset[str]) -> list[ListedMember]:
    grouped: dict[tuple[ProviderName, str], list[tuple[Mode, MemberStatus]]] = {}
    for mode, status in modes.items():
        for member in status.members:
            grouped.setdefault((member.provider, member.model), []).append((mode, member))
    result: list[ListedMember] = []
    for entries in grouped.values():
        entry_modes = {mode for mode, _ in entries}
        details = [member.model_dump(exclude={"default", "permitted"}) for _, member in entries]
        groups: list[list[tuple[Mode, MemberStatus]]] = (
            [entries] if all(item == details[0] for item in details) else [[e] for e in entries]
        )
        for group in groups:
            member = group[0][1]
            permitted = {mode for mode, item in group if item.permitted}
            available: list[Mode] = [
                mode
                for mode, status in modes.items()
                if mode in permitted
                or (
                    len(groups) == 1
                    and mode not in entry_modes
                    and not status.restricted
                    and (mode == "discussion" or member.provider not in excluded)
                )
            ]
            defaults: list[Mode] = [mode for mode, member in group if member.default]
            result.append(
                ListedMember(
                    **member.model_dump(exclude={"default", "permitted"}),
                    modes=None if len(available) == len(modes) else available,
                    default=True if len(defaults) == len(modes) else defaults or None,
                )
            )
    return result


def list_members(excluded: frozenset[str], host: Host) -> MemberList:
    @cache
    def installed(provider: str) -> bool:
        return get_adapter(provider).is_installed()

    document = read_members()
    roles = None
    orchestration_error = None
    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError as exc:
        orchestration_error = str(exc)
    enabled = roles is not None and roles.enabled(host)
    modes = _member_modes(document, excluded, installed)
    task_mode = (
        _member_modes(document, excluded, installed, cli_only=True)["task"]
        if enabled
        else modes["task"]
    )
    review: dict[ReviewRole, ReviewSeatStatus] = {}
    review_error = None
    try:
        if saved_review := _read_app_file("review.json", ReviewFile, MemberFileError):
            review = _review_status(
                saved_review.seats, task_mode, excluded, host, installed, document
            )
    except MemberFileError as exc:
        review_error = str(exc)
    orchestration = OrchestrationStatus()
    if roles is not None and enabled:
        orchestration = OrchestrationStatus(
            enabled=True,
            **{
                definition.group: _review_status(
                    roles.group_seats(definition.group),
                    task_mode,
                    excluded,
                    host,
                    installed,
                    document,
                )
                for definition in role_catalog()
                if roles.group_enabled(definition.group)
            },
            image_generation=_review_status(
                {"image-generator": image} if (image := roles.image_seat(host)) else {},
                task_mode,
                excluded,
                host,
                installed,
                document,
            ).get("image-generator"),
        )
    permissions: dict[ProviderName, PermissionTier] = {}
    permissions_error = None
    try:
        saved = read_permissions()
    except PermissionFileError as exc:
        permissions_error = str(exc)
    else:
        for provider in cast(tuple[ProviderName, ...], get_args(ProviderName.__value__)):
            if provider in PERMISSION_TIERS:
                tier = saved.clis.get(provider, "auto") if saved else "auto"
                # A CLI whose auto is full access runs full whatever is saved.
                tiers = PERMISSION_TIERS[provider]
                permissions[provider] = "full" if tiers["auto"] == tiers["full"] else tier
    if enabled:
        modes["task"] = ModeStatus(restricted=True, members=[])
    return MemberList(
        configured=document is not None,
        host=host.name,
        host_source=host.source,
        restricted=[mode for mode, status in modes.items() if status.restricted],
        members=_compact_members(modes, excluded),
        review=review,
        review_error=review_error,
        orchestration=orchestration,
        orchestration_error=orchestration_error,
        permissions=permissions,
        permissions_error=permissions_error,
    )


def _review_status[Role: str](
    review: dict[Role, ReviewSeat],
    mode: ModeStatus,
    excluded: frozenset[str],
    host: Host,
    installed: Callable[[str], bool],
    document: MemberFile | None,
) -> dict[Role, ReviewSeatStatus]:
    task = mode.members
    seats: dict[Role, ReviewSeatStatus] = {}
    for role, seat in review.items():
        member = next(
            (m for m in task if m.provider == seat.provider and m.model in (seat.model, "*")),
            None,
        )
        # The host's own CLI is never dispatched through AgentNave; its built-in subagent does it.
        native = seat.provider == host.provider
        pool_error = None
        try:
            check_cli_model(
                document.clis.get(seat.provider) if document else None, seat.provider, seat.model
            )
        except ValueError as exc:
            pool_error = str(exc)
        if pool_error is not None:
            reason = pool_error
        elif native:
            reason = None
        elif seat.provider in excluded:
            reason = "CLI is excluded by this host"
        elif mode.restricted and all(m.provider != seat.provider for m in task):
            reason = "CLI is disabled in the App"
        elif not installed(seat.provider):
            reason = "CLI is not installed"
        elif member is not None and member.error is not None:
            reason = member.error
        else:
            reason = None
        alternatives: list[ProviderName] = [
            m.provider
            for m in task
            if m.provider != seat.provider
            and m.model == seat.model
            and m.permitted
            and m.error is None
        ]
        seats[role] = ReviewSeatStatus(
            provider=seat.provider,
            model=seat.model,
            effort=seat.effort or None,
            available=False if reason is not None else None,
            reason=reason,
            native=True if native else None,
            alternatives=list(dict.fromkeys(alternatives)),
            replacement=(None if reason is None else _seat_replacement(seat, alternatives, task)),
        )
    return seats


def _seat_replacement(
    seat: ReviewSeat, alternatives: list[ProviderName], task: list[MemberStatus]
) -> SeatReplacement | None:
    """The same model on another CLI, else another CLI's App default model."""
    if alternatives:
        return SeatReplacement(
            provider=alternatives[0],
            provider_options={"model": seat.model},
            reason=f"{alternatives[0]} offers the same model",
        )
    fallback = next(
        (
            m
            for m in task
            if m.default and m.permitted and m.error is None and m.provider != seat.provider
        ),
        None,
    )
    if fallback is None:
        return None
    return SeatReplacement(
        provider=fallback.provider,
        provider_options={},
        reason=f"no other CLI offers {seat.model}; {fallback.provider} uses its App default model",
    )
