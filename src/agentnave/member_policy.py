"""CLI model boundaries and legacy scenario selection (ADR-0063)."""

from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING

from agentnave.models import ProviderOption

if TYPE_CHECKING:
    from agentnave.members import CLIConfig, MemberFile, Mode, ModeCLI, ProviderName


def has_cli_pool(cli: CLIConfig | None) -> bool:
    # Explicit null is the user's new unrestricted pool; omission alone is legacy.
    return cli is not None and "allowed_models" in cli.model_fields_set


def model_pool(cli: CLIConfig | None, selection: ModeCLI | None) -> list[str] | None:
    if has_cli_pool(cli):
        assert cli is not None
        return cli.allowed_models
    return (selection.models or None) if selection else None


def check_cli_model(cli: CLIConfig | None, provider: ProviderName, model: object) -> None:
    if cli is not None and cli.allowed_models is not None and model not in cli.allowed_models:
        raise ValueError(
            f"Model {model!r} is not allowed in CLI {provider}; "
            f"allowed models: {', '.join(cli.allowed_models)}"
        )


def select_model(
    document: MemberFile,
    mode: Mode,
    provider: ProviderName,
    options: Mapping[str, ProviderOption],
    seat: frozenset[str],
    *,
    scenario: bool,
    seat_selected: bool,
) -> tuple[dict[str, ProviderOption], list[str]]:
    allowed = document.modes.get(mode) if scenario else None
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
    cli = document.clis.get(provider)
    selection = allowed.get(provider) if allowed is not None else None
    result = dict(options)
    applied: list[str] = []
    if scenario and not seat_selected:
        default = None
        if selection is not None and "default" in selection.model_fields_set:
            default = selection.default
        elif allowed is None:
            default = document.defaults.get(mode, {}).get(provider)
        if "model" not in result and default is not None:
            result["model"] = default
            applied.append("model")
    pool = model_pool(cli, selection)
    if "model" not in result and pool:
        raise ValueError(
            f"Model is required in {mode}.{provider}; allowed models: {', '.join(pool)}"
        )
    if "model" in result:
        check_cli_model(cli, provider, result["model"])
        if not has_cli_pool(cli) and "model" not in seat and pool and result["model"] not in pool:
            raise ValueError(
                f"Model {result['model']!r} is not allowed in {mode}.{provider}; "
                f"allowed models: {', '.join(pool)}"
            )
    return result, applied
