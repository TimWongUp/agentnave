"""Read and enforce App-owned member defaults; never create or modify the data root."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping
from pathlib import Path
from typing import Literal, cast, get_args

from pydantic import BaseModel, ConfigDict, Field, model_validator

from agentnave.adapters import get_adapter
from agentnave.adapters.base import RESTRICTED_EXTRA_OPTIONS, restricted_options
from agentnave.host import Host, HostName, HostSource
from agentnave.models import InvocationRequest, ProviderOption

type ProviderName = Literal[
    "antigravity", "claude", "codebuddy", "codex", "deepseek", "grok", "pi", "zcode"
]
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
type Difficulty = Literal["easy", "hard"]
type OrchestrationPreset = Literal["off", "split", "parallel", "custom"]
# Task roles naming an Explorer or Worker difficulty in orchestration.json.
type OrchestrationRole = Literal["explorer-easy", "explorer-hard", "worker-easy", "worker-hard"]


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


class CLIOptionsFileError(ValueError):
    """The App-owned cli-options.json cannot be read or validated."""


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
        "skip_git_repo_check",
        "agent",
        "tools",
        "mode",
        "project",
        "restricted_tools",
        "disable_slash_commands",
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
    "claude": {
        "auto": {"permission_mode": "auto"},
        "full": {"permission_mode": "bypassPermissions"},
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
    "claude": frozenset({"permission_mode"}),
    "codebuddy": frozenset({"permission_mode"}),
    "grok": frozenset({"permission_mode", "sandbox"}),
    "antigravity": frozenset({"mode", "sandbox", "dangerously_skip_permissions"}),
    "codex": frozenset({"approve_for_me", "dangerously_bypass_approvals_and_sandbox", "sandbox"}),
    "deepseek": frozenset({"permission_mode"}),
}


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
    model: str = Field(pattern=r"\S")
    effort: str = ""


class ReviewFile(StrictModel):
    """review.json, apart from members.json so older servers keep reading their members."""

    version: int = Field(ge=1, le=1)
    seats: dict[ReviewRole, ReviewSeat] = {}


class OrchestrationFile(BaseModel):
    """App-owned rule modes and saved Explorer/Worker seats; only enabled hosts use seats."""

    model_config = ConfigDict(extra="ignore")

    version: int = Field(ge=1, le=1)
    preset: OrchestrationPreset = "off"
    apps: dict[str, OrchestrationPreset] = {}
    effective_apps: dict[str, OrchestrationPreset] | None = None
    explorer: dict[Difficulty, ReviewSeat] = {}
    worker: dict[Difficulty, ReviewSeat] = {}

    def enabled(self, host: Host) -> bool:
        key = host.app_id or host.name or ""
        if self.effective_apps is not None:
            # Old connections cannot prove which row started them, including rows whose
            # config repair failed. Reconnect rather than borrow the default row's mode.
            if host.app_id is None:
                return False
            return self.effective_apps.get(key, "off") != "off"
        return self.apps.get(key, self.preset) != "off"


class MemberFile(StrictModel):
    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, CLIConfig]
    modes: dict[Mode, dict[ProviderName, ModeCLI]]
    # Default models for unrestricted modes, kept when the user turns the allow-list off.
    defaults: dict[Mode, dict[ProviderName, str]] = {}

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
        seat: frozenset[str] = frozenset(),
    ) -> tuple[dict[str, ProviderOption], list[str]]:
        """Fill the mode's defaults; `seat` names the keys an App seat set, which the mode's
        member lists do not restrict (ADR-0038)."""
        allowed = self.modes.get(mode)
        if allowed is not None and provider not in allowed and "model" not in seat:
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
            and "model" not in seat
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
            if restricted and key not in RESTRICTED_EXTRA_OPTIONS.get(provider, set()):
                continue
            if key not in result:
                result[key] = value
                applied.append(key)
        return result, applied


class PermissionFile(StrictModel):
    """permissions.json, apart from members.json so older servers keep reading their members."""

    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, PermissionTier] = {}


class CLIOptionsFile(StrictModel):
    """cli-options.json: settings that hold for every model of a CLI. Apart from members.json
    so older servers keep reading their members."""

    version: int = Field(ge=1, le=1)
    clis: dict[ProviderName, dict[str, ProviderOption]] = {}

    @model_validator(mode="after")
    def option_keys(self) -> CLIOptionsFile:
        for provider, options in self.clis.items():
            # Permission keys belong to the tier, never to saved defaults.
            reserved = {"model", "effort", "restricted_tools"} | PERMISSION_KEYS.get(
                provider, frozenset()
            )
            if found := sorted(reserved.intersection(options)):
                raise ValueError(f"clis.{provider} must not contain {', '.join(found)}")
        return self


def data_root() -> Path:
    """Where the App keeps its settings and the server its history."""
    return Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")


def _read_app_file[T: BaseModel](name: str, model: type[T], error: type[ValueError]) -> T | None:
    """One App settings file of the data root; None when missing. A broken file raises `error`
    with the App repair hint, never a fallback that would run with other settings."""
    try:
        content = (data_root() / name).read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        raise error(f"Unable to read {name}: {exc}. Repair it in the AgentNave App.") from exc
    try:
        return model.model_validate_json(content)
    except ValueError as exc:
        raise error(f"Invalid {name}: {exc}. Repair it in the AgentNave App.") from exc


def read_cli_options() -> CLIOptionsFile | None:
    return _read_app_file("cli-options.json", CLIOptionsFile, CLIOptionsFileError)


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


def read_max_running() -> int:
    """The App's limit on CLI runs (tasks plus seats) this server keeps active at once."""
    try:
        document = json.loads((data_root() / "settings.json").read_text(encoding="utf-8"))
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


def add_cli_options(
    provider: str,
    saved: Mapping[str, ProviderOption],
    result: dict[str, ProviderOption],
    restricted: bool,
) -> list[str]:
    """Fill CLI-wide settings, which hold whichever model runs, the CLI's own default included.

    The caller and the model's own options win on the same key; restricted runs take only
    the extras that grant no tools. Returns the keys added.
    """
    unrestricted = not restricted and result.get("restricted_tools") is not True
    added = [
        key
        for key in saved
        if key not in result
        and (unrestricted or key in RESTRICTED_EXTRA_OPTIONS.get(provider, set()))
    ]
    result.update({key: saved[key] for key in added})
    return added


def apply_member(
    document: MemberFile | None,
    mode: Mode,
    provider: ProviderName,
    options: Mapping[str, ProviderOption],
    excluded: frozenset[str],
    *,
    restricted: bool = False,
    permissions: PermissionFile | None = None,
    cli_options: CLIOptionsFile | None = None,
    seat: frozenset[str] = frozenset(),
) -> tuple[dict[str, ProviderOption], list[str]]:
    if document is not None:
        result, applied = document.apply(
            mode, provider, options, excluded, restricted=restricted, seat=seat
        )
    else:
        check_excluded(mode, provider, excluded)
        result, applied = dict(options), []
    if cli_options is not None:
        applied += add_cli_options(provider, cli_options.clis.get(provider, {}), result, restricted)
    # Restricted runs take no tools, so no permission tier either.
    if not restricted and result.get("restricted_tools") is not True:
        apply_permission(permissions, provider, options, result, applied)
    if "effort" not in result:
        add_cli_effort(document, mode, provider, result, applied)
    return result, applied


def add_cli_effort(
    document: MemberFile | None,
    mode: Mode,
    provider: ProviderName,
    result: dict[str, ProviderOption],
    applied: list[str],
) -> None:
    """Pass the level the CLI would pick by itself, so every run names and records one
    (ADR-0042). A level the App does not allow for the model is left to the CLI."""
    model = result.get("model")
    effort = cli_default_effort(provider, str(model) if model is not None else None)
    if effort is None:
        return
    if document is not None:
        key = str(model) if model is not None else cli_default_model(provider) or ""
        allowed = document.effective(mode, provider, key).efforts
        if allowed and effort not in allowed:
            return
    result["effort"] = effort
    applied.append(CLI_EFFORT_APPLIED)


def role_seat(role: str | None, host: Host) -> ReviewSeat | None:
    """The App seat a task role names: an agentnave-review role from review.json, or an
    Explorer/Worker difficulty from orchestration.json. A missing or broken file names none;
    list_members reports it."""
    role = (role or "").strip()
    try:
        if role in get_args(ReviewRole.__value__):
            review = _read_app_file("review.json", ReviewFile, MemberFileError)
            return review.seats.get(cast(ReviewRole, role)) if review else None
        if role in get_args(OrchestrationRole.__value__):
            kind, difficulty = role.split("-")
            roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
            seats = (
                (roles.explorer if kind == "explorer" else roles.worker)
                if roles and roles.enabled(host)
                else {}
            )
            return seats.get(cast(Difficulty, difficulty))
    except MemberFileError:
        return None
    return None


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
    # The host's own CLI: dispatch to the host's built-in subagent, with this model if it can.
    native: bool = False
    # Other permitted task CLIs listing the same model ID.
    alternatives: list[ProviderName]


class OrchestrationStatus(StrictModel):
    enabled: bool = False
    explorer: dict[Difficulty, ReviewSeatStatus] = {}
    worker: dict[Difficulty, ReviewSeatStatus] = {}


class MemberList(StrictModel):
    configured: bool
    error: str | None = None
    host: HostName | None
    host_source: HostSource
    modes: dict[Mode, ModeStatus]
    review: dict[ReviewRole, ReviewSeatStatus] = {}
    review_error: str | None = None
    orchestration: OrchestrationStatus = Field(default_factory=OrchestrationStatus)
    orchestration_error: str | None = None
    # The App permission tier of each CLI that has one, "auto" unless the user picked "full".
    permissions: dict[ProviderName, PermissionTier] = {}
    permissions_error: str | None = None
    # Settings the user saved for a whole CLI; they apply to every model of it and are not
    # repeated in each member's options.
    cli_options: dict[ProviderName, dict[str, ProviderOption]] = {}
    cli_options_error: str | None = None


def list_members(excluded: frozenset[str], host: Host) -> MemberList:
    document = read_members()
    cli_options: dict[ProviderName, dict[str, ProviderOption]] = {}
    cli_options_error = None
    try:
        saved_options = read_cli_options()
    except CLIOptionsFileError as exc:
        cli_options_error = str(exc)
    else:
        cli_options = saved_options.clis if saved_options else {}
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
                    # Validate what a dispatch would send; options below stay the model's own.
                    add_cli_options(provider, cli_options.get(provider, {}), options, False)
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
    # Review seats are not checked against clis: a removed model is reported here, and the
    # review Skill picks a replacement at dispatch time. A broken review.json leaves members alone.
    review: dict[ReviewRole, ReviewSeatStatus] = {}
    review_error = None
    try:
        if saved_review := _read_app_file("review.json", ReviewFile, MemberFileError):
            review = _review_status(saved_review.seats, modes["task"], excluded, host)
    except MemberFileError as exc:
        review_error = str(exc)
    orchestration = OrchestrationStatus()
    orchestration_error = None
    try:
        if (
            roles := _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
        ) and roles.enabled(host):
            orchestration = OrchestrationStatus(
                enabled=True,
                explorer=_review_status(roles.explorer, modes["task"], excluded, host),
                worker=_review_status(roles.worker, modes["task"], excluded, host),
            )
    except MemberFileError as exc:
        orchestration_error = str(exc)
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
    return MemberList(
        configured=document is not None,
        host=host.name,
        host_source=host.source,
        modes=modes,
        review=review,
        review_error=review_error,
        orchestration=orchestration,
        orchestration_error=orchestration_error,
        permissions=permissions,
        permissions_error=permissions_error,
        cli_options=cli_options,
        cli_options_error=cli_options_error,
    )


def _review_status[Role: str](
    review: dict[Role, ReviewSeat], mode: ModeStatus, excluded: frozenset[str], host: Host
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
        # The seat is the user's own pick for the role: the task member lists do not restrict it.
        if native:
            reason = None
        elif seat.provider in excluded:
            reason = "CLI is excluded by this host"
        elif member is not None and not member.valid:
            reason = member.error
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
            native=native,
            alternatives=list(dict.fromkeys(alternatives)),
        )
    return seats
