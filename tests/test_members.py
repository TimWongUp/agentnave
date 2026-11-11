from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, cast, get_args

import pytest
from pydantic import ValidationError

from agentnave.adapters.base import AdapterHooks
from agentnave.host import Host
from agentnave.members import (
    MemberFile,
    MemberFileError,
    OrchestrationFile,
    PermissionFile,
    PermissionFileError,
    ProviderExcludedError,
    ProviderName,
    ReviewFile,
    ReviewSeat,
    ReviewSeatStatus,
    SettingsFileError,
    apply_member,
    list_members,
    read_antigravity_interactive,
    read_claude_interactive,
    read_max_running,
    read_members,
    read_permissions,
    role_seat,
)
from agentnave.models import ProviderOption
from agentnave.orchestration_roles import (
    OrchestrationGroup,
    OrchestrationRole,
    OrchestrationStatus,
    role_catalog,
)
from agentnave.task_start import role_access

APP_FILES = Path(__file__).parent / "fixtures" / "app-files"
APP_MODELS: dict[str, type[MemberFile | ReviewFile | OrchestrationFile | PermissionFile]] = {
    "members": MemberFile,
    "review": ReviewFile,
    "orchestration": OrchestrationFile,
    "permissions": PermissionFile,
}


def _installed_cli(_adapter: AdapterHooks) -> bool:
    # Configuration tests do not depend on the machine's installed CLIs.
    return True


def test_role_catalog_covers_wire_seats_and_top_level_bindings() -> None:
    catalog = role_catalog()
    assert len(catalog) == 7
    assert {role.group for role in catalog} == set(get_args(OrchestrationGroup.__value__))
    assert {
        f"{role.seat_prefix}-{difficulty}" for role in catalog for difficulty in ("easy", "hard")
    } | {"image-generator"} == set(get_args(OrchestrationRole.__value__))
    bindings = {
        difficulty: {"provider": "claude", "model": difficulty} for difficulty in ("easy", "hard")
    }
    document = OrchestrationFile.model_validate(
        {
            "version": 1,
            "enabled_roles": [role.group for role in catalog],
            **{role.group: bindings for role in catalog},
        }
    )
    for role in catalog:
        assert role.group in OrchestrationStatus.model_fields
        assert set(document.group_seats(role.group)) == {"easy", "hard"}
        for difficulty in ("easy", "hard"):
            assert document.group_seats(role.group)[difficulty].model == difficulty
            assert role_access(f"{role.seat_prefix}-{difficulty}") == role.access

    from agentnave import orchestration_roles

    rules = json.loads(
        Path(orchestration_roles.__file__).with_name("orchestration_rules.json").read_text("utf-8")
    )
    assert set(rules["order"]) == {"meetings", "split", "parallel"}
    known_steps = set(rules["sections"]) | {
        "split_meetings",
        "parallel_meetings",
        "meeting_defaults",
        "enabled_roles",
    }
    for steps in rules["order"].values():
        assert steps and set(steps) <= known_steps
    assert rules["switches"]["lead_only"]["when_false"] in rules["sections"]["shared"]
    assert set(rules["meeting_defaults"]["provider_names"]) == set(get_args(ProviderName.__value__))


@pytest.mark.parametrize("switches", [{}, {"enabled_roles": None}, {"enabled_roles": []}])
def test_role_switch_defaults_hide_fields_and_preserve_bindings(
    switches: dict[str, Any], tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    binding = {"provider": "claude", "model": "saved"}
    raw = {
        "version": 1,
        "preset": "split",
        **{role.group: {"easy": binding} for role in role_catalog()},
        **switches,
    }
    path = tmp_path / "orchestration.json"
    path.write_text(json.dumps(raw))
    expected: set[str] = (
        {"explorer", "worker", "worker_frontend"}
        if switches.get("enabled_roles") is None
        else set()
    )
    host = Host("codex", "env")
    status = list_members(frozenset(), host).orchestration.model_dump()
    assert {role.group for role in role_catalog() if role.group in status} == expected
    assert set(status["scopes"]) == expected
    saved = OrchestrationFile.model_validate(raw).model_dump()
    for role in role_catalog():
        assert saved[role.group] == {"easy": {**binding, "effort": ""}}
        assert (role_seat(f"{role.seat_prefix}-easy", host) is not None) is (role.group in expected)
    assert json.loads(path.read_text()) == raw


def test_disabled_orchestration_hides_seats_and_does_not_apply_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    seat = {"provider": "claude", "model": "sonnet"}
    path = tmp_path / "orchestration.json"
    document: dict[str, Any] = {
        "version": 1,
        "preset": "off",
        "explorer": {"easy": seat},
        "worker": {"hard": seat},
        "worker_frontend": {"hard": seat},
        "image_generation": seat,
    }

    def check(host: Host, enabled: bool) -> None:
        path.write_text(json.dumps(document))
        status = list_members(frozenset(), host).orchestration
        assert status.enabled is enabled
        assert bool(status.explorer) is enabled
        assert bool(status.worker) is enabled
        assert (status.image_generation is not None) is (enabled and host.provider != "claude")
        assert (role_seat("explorer-easy", host) is not None) is enabled
        assert (role_seat("worker-hard", host) is not None) is enabled
        assert (role_seat("worker-frontend-hard", host) is not None) is enabled
        assert (role_seat("image-generator", host) is not None) is (
            enabled and host.provider != "claude"
        )

    assert (
        OrchestrationFile.model_validate(
            {"version": 1, "image_generation": {"provider": "codebuddy", "model": "chat"}}
        ).image_seat(Host("workbuddy"))
        is None
    )
    codex, claude = Host("codex", "env"), Host("claude", "env")
    check(codex, False)
    document["apps"] = {"codex": "split"}
    check(codex, True)
    check(claude, False)
    document["preset"] = "parallel"
    document["apps"] = {"codex": "off"}
    check(codex, False)
    check(claude, True)
    # The App's resolved groups win over individual raw choices, and accounts stay separate.
    document["effective_apps"] = {"codex": "off", "codex@work": "split", "grok": "parallel"}
    check(codex, False)
    check(Host("codex", "env", "codex@work"), True)
    check(Host("grok", "env", "grok"), True)
    check(claude, False)  # no connected row in the resolved map
    document["effective_apps"] = {"codex": "meetings", "codex@work": "split"}
    document["meeting"] = {
        "count": 3,
        "seats": [{"provider": "codex", "model": "gpt-6.1-sol", "effort": "high"}, None],
    }
    check(Host("codex", "env", "codex"), False)
    check(Host("codex", "env", "codex@work"), True)
    assert list_members(frozenset(), Host("codex", "env", "codex")).orchestration_error is None
    document["effective_apps"] = {"codex": "split", "codex@work": "off"}
    check(codex, False)  # an old connection cannot borrow a different account's mode
    check(Host("codex", "env", "codex"), True)
    check(Host("codex", "env", "codex@work"), False)
    document["effective_apps"] = {"codex": "split"}
    check(codex, False)  # another account may be missing because its repair failed
    check(Host("codex", "env", "codex"), True)
    document["effective_apps"] = {}
    check(codex, False)
    # Turning orchestration off preserves the saved picks for the next enable.
    assert json.loads(path.read_text())["worker"]["hard"] == seat


def test_interface_seat_never_borrows_the_worker_seat(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    general = {"provider": "codex", "model": "general"}
    interface = {"provider": "antigravity", "model": "interface"}
    document = {
        "version": 1,
        "preset": "split",
        "worker": {"hard": general},
        "worker_frontend": {"easy": interface},
    }
    (tmp_path / "orchestration.json").write_text(json.dumps(document))
    host = Host("claude", "env")
    status = list_members(frozenset(), host).orchestration
    assert set(status.worker) == {"hard"}
    assert set(status.worker_frontend) == {"easy"}
    assert status.worker_frontend["easy"].model == "interface"
    assert role_seat("worker-frontend-hard", host) is None
    assert cast(ReviewSeat, role_seat("worker-frontend-easy", host)).model == "interface"
    assert role_seat("worker-easy", host) is None
    assert status.image_generation is None
    assert role_seat("image-generator", host) is None
    with pytest.raises(MemberFileError, match="not configured"):
        role_seat("image-generator", host, required=True)
    with pytest.raises(ValidationError):
        OrchestrationFile.model_validate({**document, "image_generation": {"hard": general}})
    with pytest.raises(ValidationError):
        OrchestrationFile.model_validate(
            {**document, "image_generation": {"provider": "codex", "model": " "}}
        )


@pytest.mark.parametrize(
    "sample",
    sorted(APP_FILES.glob("*/*.json")),
    ids=lambda path: f"{path.parent.name}/{path.stem}",
)
def test_app_file_samples_agree_with_the_tray(
    sample: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    """The tray checks the same samples (apps/tray/src/members/tests.rs), so a file the App
    saves is one the server reads, and one the App refuses the server refuses too."""
    from agentnave.members import read_history_retention_days

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))

    def check() -> object:
        if sample.parent.name == "settings":
            (tmp_path / "settings.json").write_bytes(sample.read_bytes())
            caplog.clear()
            days = read_history_retention_days(tmp_path)
            if "History cleanup is disabled" in caplog.text:
                raise SettingsFileError("invalid history_retention_days")
            return (
                read_max_running(),
                read_claude_interactive(),
                read_antigravity_interactive(),
                days,
            )
        return APP_MODELS[sample.parent.name].model_validate_json(sample.read_bytes())

    if sample.stem.startswith("valid-"):
        check()
    else:
        with pytest.raises((ValidationError, SettingsFileError)):
            check()


def test_member_structure_references_and_effective_overrides() -> None:
    shared = {"efforts": ["medium", "high"], "effort": "high", "options": {"agent": "shared"}}
    document: dict[str, object] = {
        "version": 1,
        "clis": {"claude": {"models": {"chosen": shared}}},
        "modes": {
            "task": {
                "claude": {
                    "models": ["chosen"],
                    "default": "chosen",
                    "overrides": {"chosen": {"effort": "medium", "options": {}}},
                }
            }
        },
    }
    parsed = MemberFile.model_validate(document)
    options, defaults = parsed.apply("task", "claude", {}, frozenset())
    assert options == {"model": "chosen", "effort": "medium"}
    assert defaults == ["model", "effort"]
    # A missing mode is unrestricted, and shared defaults still apply to a known model.
    assert parsed.apply("discussion", "claude", {"model": "chosen"}, frozenset({"claude"})) == (
        {"model": "chosen", "effort": "high", "agent": "shared"},
        ["effort", "agent"],
    )
    assert parsed.apply("discussion", "grok", {}, frozenset({"grok"})) == ({}, [])
    # Turning an allow-list off keeps each CLI's default model for that unrestricted mode.
    kept = MemberFile.model_validate(dict(document, defaults={"discussion": {"claude": "chosen"}}))
    assert kept.apply("discussion", "claude", {}, frozenset()) == (
        {"model": "chosen", "effort": "high", "agent": "shared"},
        ["model", "effort", "agent"],
    )
    with pytest.raises(ProviderExcludedError, match="excluded by this host"):
        parsed.apply("task", "claude", {}, frozenset({"claude"}))
    # Fresh copies cover structural rules without depending on Adapter semantics.
    invalid: list[dict[str, object]] = [
        dict(document, unknown=True),
        dict(document, version=True),
        dict(document, clis={"unknown": {"models": {}}}),
        dict(document, modes={"unknown": {}}),
        dict(document, modes={"task": {"claude": {"models": ["missing"]}}}),
        dict(document, modes={"task": {"claude": {"models": [], "default": "missing"}}}),
        dict(document, modes={"task": {"claude": {"models": [], "extra": True}}}),
        dict(document, defaults={"task": {"claude": "chosen"}}),
        dict(document, defaults={"discussion": {"claude": "missing"}}),
        dict(document, modes={"task": {"claude": {"models": [], "overrides": {"missing": {}}}}}),
        dict(document, clis={"claude": {"models": {"chosen": {"options": {"model": "x"}}}}}),
        dict(document, clis={"claude": {"models": {"chosen": {"options": {"effort": "high"}}}}}),
        dict(document, clis={"claude": {"models": {"chosen": {"options": {"agent": []}}}}}),
        dict(
            document,
            clis={"claude": {"models": {"chosen": {"efforts": ["high"], "effort": "low"}}}},
        ),
        dict(
            document,
            modes={
                "task": {"claude": {"models": [], "overrides": {"chosen": {"efforts": ["low"]}}}}
            },
        ),
    ]
    invalid.extend(
        {key: value for key, value in document.items() if key != missing}
        for missing in ("version", "clis", "modes")
    )
    for value in invalid:
        with pytest.raises(ValidationError):
            MemberFile.model_validate_json(json.dumps(value))


def test_missing_members_apply_only_the_default_tier_and_ignore_legacy_profiles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "data"
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    assert read_members() is None and not root.exists()
    # Without any App file the default "full" permission tier applies.
    assert apply_member(None, "task", "claude", {"model": "explicit"}, frozenset()) == (
        {
            "model": "explicit",
            "permission_mode": "bypassPermissions",
            "permission_fallback": "auto",
        },
        ["permission_mode", "permission_fallback"],
    )
    with pytest.raises(ValueError, match="excluded"):
        apply_member(None, "task", "claude", {}, frozenset({"claude"}))
    root.mkdir()
    (root / "profiles.json").write_text("broken legacy data", encoding="utf-8")
    assert read_members() is None
    assert list(root.iterdir()) == [root / "profiles.json"]


def test_model_allowlist_requires_a_model_and_restricted_defaults_preserve_pi_backend() -> None:
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {
                "pi": {
                    "models": {
                        "chosen": {
                            "effort": "high",
                            "options": {"provider": "backend", "tools": "read"},
                        }
                    }
                }
            },
            "modes": {
                "task": {"pi": {"models": ["chosen"]}},
                "discussion": {"pi": {"models": ["chosen"]}},
            },
        }
    )
    for mode in ("task", "discussion"):
        with pytest.raises(ValueError, match="Model is required.*allowed models: chosen"):
            apply_member(document, mode, "pi", {}, frozenset())
    assert apply_member(
        document, "discussion", "pi", {"model": "chosen"}, frozenset(), restricted=True
    ) == ({"model": "chosen", "effort": "high", "provider": "backend"}, ["effort", "provider"])
    assert apply_member(
        document,
        "discussion",
        "pi",
        {"model": "chosen", "provider": "explicit"},
        frozenset(),
        restricted=True,
    ) == ({"model": "chosen", "provider": "explicit", "effort": "high"}, ["effort"])


def test_member_file_errors_are_typed(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "members.json"
    path.write_text("{", encoding="utf-8")
    with pytest.raises(MemberFileError, match="Invalid members.json"):
        read_members()
    path.unlink()
    path.mkdir()
    with pytest.raises(MemberFileError, match="Unable to read members.json"):
        read_members()


def test_without_a_model_the_cli_default_model_keeps_its_saved_settings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {"grok": {"models": {"grok-4.7": {"effort": "low", "options": {"x": 1}}}}},
            "modes": {},
        }
    )
    assert document.apply("task", "grok", {}, frozenset()) == ({}, [])
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps({"version": 1, "clis": {"grok": {"model": "grok-4.7", "effort": "high"}}}),
        encoding="utf-8",
    )
    # The model stays the CLI's choice; only the settings saved for it are applied.
    assert document.apply("task", "grok", {}, frozenset()) == (
        {"effort": "low", "x": 1},
        ["effort", "x"],
    )
    assert document.apply("task", "grok", {"model": "grok-4.6"}, frozenset()) == (
        {"model": "grok-4.6"},
        [],
    )


def test_a_call_without_a_level_passes_the_cli_default_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    "grok": {
                        "model": "grok-4.7",
                        "effort": "high",
                        "efforts": {"grok-4.7": "high", "grok-4.6": "medium"},
                    }
                },
            }
        ),
        encoding="utf-8",
    )

    def effort(options: dict[str, ProviderOption], document: MemberFile | None = None) -> object:
        result, applied = apply_member(document, "task", "grok", options, frozenset())
        assert ("effort (CLI default)" in applied) == (
            result.get("effort") != options.get("effort")
        )
        return result.get("effort")

    assert effort({}) == "high"
    assert effort({"model": "grok-4.6"}) == "medium"
    # An explicit level wins; an unknown model or a level the App disallows stays unset.
    assert effort({"effort": "low"}) == "low"
    assert effort({"model": "other"}) is None
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {"grok": {"models": {"grok-4.6": {"efforts": ["low"]}}}},
            "modes": {},
        }
    )
    assert effort({"model": "grok-4.6"}, document) is None


def test_cached_native_model_does_not_transfer_execution_permissions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    permissions = {
        "dangerously_bypass_approvals_and_sandbox": True,
        "sandbox": "danger-full-access",
    }
    configured = {"effort": "high", "options": permissions}
    document = MemberFile.model_validate(
        {"version": 1, "clis": {"codex": {"models": {"cached-A": configured}}}, "modes": {}}
    )
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps({"version": 1, "clis": {"codex": {"model": "cached-A"}}}), encoding="utf-8"
    )
    # The CLI may now use model B. A cache entry cannot transfer A's execution authority.
    assert document.apply("task", "codex", {}, frozenset()) == (
        {"effort": "high"},
        ["effort"],
    )
    assert document.apply("task", "codex", {"sandbox": "workspace-write"}, frozenset()) == (
        {"sandbox": "workspace-write", "effort": "high"},
        ["effort"],
    )
    explicit = {"model": "cached-A", "effort": "high", **permissions}
    assert document.apply("task", "codex", {"model": "cached-A"}, frozenset()) == (
        explicit,
        ["effort", "dangerously_bypass_approvals_and_sandbox", "sandbox"],
    )
    document.defaults = {"task": {"codex": "cached-A"}}
    assert document.apply("task", "codex", {}, frozenset()) == (
        explicit,
        ["model", "effort", "dangerously_bypass_approvals_and_sandbox", "sandbox"],
    )

    # false can enable slash commands; it is execution control, not a harmless default.
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {
                "antigravity": {
                    "models": {"cached-A": {"options": {"disable_slash_commands": False}}}
                }
            },
            "modes": {},
        }
    )
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps({"clis": {"antigravity": {"model": "cached-A"}}}), encoding="utf-8"
    )
    assert document.apply("task", "antigravity", {}, frozenset()) == ({}, [])


def test_cli_permission_tier_applies_to_any_model_unless_the_caller_chose(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    assert read_permissions() is None
    (tmp_path / "permissions.json").write_text('{"version": 1, "clis": {"x": "auto"}}')
    with pytest.raises(PermissionFileError):
        read_permissions()
    (tmp_path / "permissions.json").write_text(
        json.dumps({"version": 1, "clis": {"codex": "auto", "claude": "full", "pi": "auto"}})
    )
    tiers = read_permissions()
    assert tiers is not None
    # ZCode advertises auto but rejects every tool in that unimplemented native mode.
    assert apply_member(None, "task", "zcode", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "yolo"},
        ["permission_mode"],
    )
    # Antigravity has no automatic reviewer worth dispatching under: auto runs it unasked too.
    assert apply_member(None, "task", "antigravity", {}, frozenset(), permissions=tiers) == (
        {"dangerously_skip_permissions": True},
        ["dangerously_skip_permissions"],
    )
    # The App shows such a CLI as full whatever was saved; one with a real choice keeps its tier.
    listed = list_members(frozenset(), Host()).permissions
    assert listed["antigravity"] == "full" and listed["zcode"] == "full"
    assert listed["grok"] == "full"
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {"codex": {"models": {"m": {"options": {"sandbox": "read-only"}}}}},
            "modes": {"task": {"codex": {"models": []}}},
        }
    )
    # No model (CLI default) and a model the App never listed both get the tier.
    for options in ({}, {"model": "unlisted"}):
        assert apply_member(document, "task", "codex", options, frozenset(), permissions=tiers)[
            1
        ] == ["approve_for_me"]
    # The tier replaces a saved model permission option rather than mixing with it.
    assert apply_member(
        document, "task", "codex", {"model": "m"}, frozenset(), permissions=tiers
    ) == ({"model": "m", "approve_for_me": True}, ["approve_for_me"])
    # A caller's own permission option keeps the tier out; restricted runs take none.
    assert apply_member(
        document, "task", "codex", {"sandbox": "workspace-write"}, frozenset(), permissions=tiers
    ) == ({"sandbox": "workspace-write"}, [])
    # A caller's read-only sandbox does not pick up a saved model approve_for_me either.
    document.clis["codex"].models["m"].options = {"approve_for_me": True}
    assert apply_member(
        document, "task", "codex", {"model": "m", "sandbox": "read-only"}, frozenset()
    ) == ({"model": "m", "sandbox": "read-only"}, [])
    # Restricted saved in model options keeps the tier out too.
    document.clis["codex"].models["m"].options = {"restricted_tools": True}
    assert apply_member(document, "task", "codex", {"model": "m"}, frozenset()) == (
        {"model": "m", "restricted_tools": True},
        ["restricted_tools"],
    )
    assert apply_member(
        None, "discussion", "claude", {}, frozenset(), restricted=True, permissions=tiers
    ) == ({}, [])
    assert apply_member(None, "discussion", "claude", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "bypassPermissions", "permission_fallback": "auto"},
        ["permission_mode", "permission_fallback"],
    )
    # Pi has no approval step; a CLI missing from the file, or no file, runs "full".
    assert apply_member(None, "task", "pi", {}, frozenset(), permissions=tiers) == ({}, [])
    assert apply_member(None, "task", "grok", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "bypassPermissions"},
        ["permission_mode"],
    )
    assert apply_member(
        None, "task", "codex", {}, frozenset(), permissions=PermissionFile(version=1)
    ) == (
        {"dangerously_bypass_approvals_and_sandbox": True},
        ["dangerously_bypass_approvals_and_sandbox"],
    )


def test_unavailable_seat_offers_a_replacement_the_boss_can_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    "grok": {"models": {"shared": {}}},
                    "pi": {"models": {"shared": {}, "other": {}}},
                },
                "modes": {
                    "task": {
                        "grok": {"models": ["shared"], "default": "shared"},
                        "pi": {"models": ["shared", "other"], "default": "other"},
                    }
                },
            }
        )
    )
    (tmp_path / "review.json").write_text(
        json.dumps(
            {
                "version": 1,
                "seats": {
                    "correctness-reviewer": {"provider": "grok", "model": "shared"},
                    "security-reviewer": {"provider": "grok", "model": "only-here"},
                    "simplicity-reviewer": {"provider": "pi", "model": "shared"},
                    "performance-reviewer": {"provider": "codex", "model": "chosen"},
                },
            }
        )
    )
    seats = list_members(frozenset({"grok"}), Host("claude")).review

    same_model = seats["correctness-reviewer"].replacement
    assert same_model is not None
    assert same_model.provider == "pi" and same_model.provider_options == {"model": "shared"}
    other_model = seats["security-reviewer"].replacement
    assert other_model is not None
    assert other_model.provider == "pi" and other_model.provider_options == {}
    assert seats["simplicity-reviewer"].available is None
    assert seats["simplicity-reviewer"].replacement is None
    # A CLI disabled for private tasks runs for no seat, though seats may pick unlisted models.
    disabled = seats["performance-reviewer"]
    assert disabled.available is False and disabled.reason == "CLI is disabled in the App"
    assert disabled.replacement is not None and disabled.replacement.provider == "pi"
    document = read_members()
    assert document is not None
    with pytest.raises(ValueError, match="not allowed in task"):
        document.apply("task", "codex", {"model": "chosen"}, frozenset(), seat=frozenset({"model"}))
    options, _ = document.apply(
        "task", "pi", {"model": "unlisted"}, frozenset(), seat=frozenset({"model"})
    )
    assert options["model"] == "unlisted"


def test_missing_clis_mark_members_and_seats_without_replacing_native_seats(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    providers = ("grok", "pi", "claude", "codebuddy")
    missing = {"grok", "pi", "claude"}
    calls: dict[str, int] = {}

    def installed(adapter: AdapterHooks) -> bool:
        calls[adapter.name] = calls.get(adapter.name, 0) + 1
        return adapter.name not in missing

    monkeypatch.setattr(AdapterHooks, "is_installed", installed)
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {p: {"models": {"shared": {}}} for p in providers},
                "modes": {
                    "task": {
                        p: {"models": ["shared"] if p == "grok" else [], "default": "shared"}
                        for p in providers
                    }
                },
            }
        )
    )
    unlisted = {"provider": "grok", "model": "unlisted"}
    shared = {"provider": "pi", "model": "shared"}
    native = {"provider": "claude", "model": "shared"}
    (tmp_path / "review.json").write_text(
        json.dumps(
            {
                "version": 1,
                "seats": {
                    "correctness-reviewer": unlisted,
                    "security-reviewer": shared,
                    "simplicity-reviewer": native,
                },
            }
        )
    )
    (tmp_path / "orchestration.json").write_text(
        json.dumps(
            {
                "version": 1,
                "preset": "split",
                "explorer": {"easy": unlisted, "hard": native},
                "worker": {"hard": shared},
                "worker_frontend": {"easy": shared},
                "image_generation": shared,
            }
        )
    )
    result = list_members(frozenset({"claude"}), Host("claude"))
    assert calls == dict.fromkeys(providers, 1)
    assert all(m.modes == ["discussion"] for m in result.members)
    for member in result.members:
        assert member.error == ("CLI is not installed" if member.provider in missing else None)
    unavailable = [
        result.review["correctness-reviewer"],
        result.review["security-reviewer"],
        result.orchestration.explorer["easy"],
        result.orchestration.worker["hard"],
        result.orchestration.worker_frontend["easy"],
        cast(ReviewSeatStatus, result.orchestration.image_generation),
    ]
    for seat in unavailable:
        assert seat.available is False and seat.reason == "CLI is not installed"
        # Hidden task defaults cannot be offered as a fallback for a missing CLI.
        if seat.model == "shared":
            assert seat.replacement is not None and seat.replacement.provider == "codebuddy"
        else:
            assert seat.replacement is None
        assert not missing.intersection(seat.alternatives)
    for seat in (result.review["simplicity-reviewer"], result.orchestration.explorer["hard"]):
        assert seat.native is True and seat.available is None and seat.reason is None
        assert seat.replacement is None
    # A later call resolves afresh, so installing a CLI repairs members and seats immediately.
    missing.clear()
    calls.clear()
    refreshed = list_members(frozenset(), Host())
    assert all(member.error is None for member in refreshed.members)
    assert all(seat.available is None for seat in refreshed.review.values())
    assert refreshed.orchestration.image_generation is not None
    assert refreshed.orchestration.image_generation.available is None
    assert calls == dict.fromkeys(providers, 1)


def test_member_list_merges_modes_defaults_and_wildcards_in_member_order(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    "claude": {"models": {"shared": {}}},
                    "pi": {"models": {"shared": {}}},
                    "grok": {"models": {"alone": {}}},
                    "codex": {"models": {"last": {}}},
                },
                "modes": {
                    "task": {
                        "claude": {"models": ["shared"], "default": "shared"},
                        "pi": {"models": [], "default": "shared"},
                        "grok": {"models": ["alone"]},
                    },
                    "discussion": {
                        "claude": {"models": ["shared"], "default": "shared"},
                        "pi": {"models": []},
                        "codex": {"models": ["last"]},
                    },
                },
            }
        )
    )
    result = list_members(frozenset({"claude", "grok"}), Host("claude"))
    assert result.restricted == ["task", "discussion"]
    assert [(member.provider, member.model) for member in result.members] == [
        ("claude", "shared"),
        ("pi", "shared"),
        ("pi", "*"),
        ("grok", "alone"),
        ("codex", "last"),
    ]
    claude, pi, wildcard, denied, discussion_only = result.members
    assert claude.modes == ["discussion"] and claude.default is True
    assert pi.modes is None and pi.default == ["task"]
    assert wildcard.modes is None and wildcard.default is None
    assert denied.modes == [] and denied.default is None
    assert discussion_only.modes == ["discussion"]
    # The same wildcard becomes discussion-only when its CLI is excluded from tasks.
    wildcard = next(
        member for member in list_members(frozenset({"pi"}), Host()).members if member.model == "*"
    )
    assert wildcard.modes == ["discussion"]


@pytest.mark.parametrize(
    ("restricted_mode", "override"),
    [
        ("task", {"effort": "medium"}),
        ("discussion", {"effort": "medium"}),
        ("discussion", {"options": {"unknown": True}}),
    ],
)
def test_member_list_keeps_mode_settings_and_expands_only_missing_modes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    restricted_mode: str,
    override: dict[str, object],
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    providers = ("pi", "claude")
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    provider: {"models": {"chosen": {"effort": "high"}}} for provider in providers
                },
                "modes": {
                    restricted_mode: {
                        provider: {
                            "models": [],
                            "default": "chosen",
                            "overrides": {"chosen": override},
                        }
                        for provider in providers
                    }
                },
            }
        )
    )
    listed = list_members(frozenset({"claude"}), Host("claude"))
    assert listed.restricted == [restricted_mode]
    assert len(listed.members) == 6  # Each CLI has two effective model settings and a wildcard.
    for provider in providers:
        task, discussion, wildcard = [
            member for member in listed.members if member.provider == provider
        ]
        assert task.modes == ([] if provider == "claude" else ["task"])
        assert discussion.modes == ["discussion"]
        assert task.error is None
        assert (discussion.error is not None) == ("options" in override)
        configured, other = (task, discussion) if restricted_mode == "task" else (discussion, task)
        assert configured.model_dump().items() >= override.items()
        assert configured.default == [restricted_mode] and other.default is None
        # Only the wildcard lacks an entry in the unrestricted mode, so it can expand.
        assert wildcard.model == "*"
        assert wildcard.modes == (["discussion"] if provider == "claude" else None)
        assert wildcard.default is None


def test_claude_interactive_is_on_unless_turned_off_and_explicit_values_win(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr("agentnave.members.read_claude_interactive", read_claude_interactive)

    def no_effort(*_: object) -> None:
        return None

    monkeypatch.setattr("agentnave.members.cli_default_effort", no_effort)
    # Without a saved choice macOS and Linux run Claude interactively; Windows has no runtime.
    assert read_claude_interactive() is True
    result, applied = apply_member(None, "task", "claude", {}, frozenset())
    assert (result.get("interactive") is True) is (os.name != "nt")
    assert ("interactive" in applied) is (os.name != "nt")
    (tmp_path / "settings.json").write_text('{"version": 1, "claude_interactive": false}')
    assert read_claude_interactive() is False
    assert "interactive" not in apply_member(None, "task", "claude", {}, frozenset())[0]
    (tmp_path / "settings.json").write_text('{"version": 1, "claude_interactive": true}')
    cases: list[tuple[ProviderName, dict[str, ProviderOption], bool]] = [
        ("claude", {"interactive": False}, False),
        ("claude", {"restricted_tools": True}, False),
        ("claude", {}, True),
        ("pi", {}, False),
    ]
    for provider, options, restricted in cases:
        result, applied = apply_member(
            None, "discussion", provider, options, frozenset(), restricted=restricted
        )
        assert result.get("interactive") is not True and "interactive" not in applied


def test_antigravity_interactive_applies_only_with_permissions_skipped(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(
        "agentnave.members.read_antigravity_interactive", read_antigravity_interactive
    )

    def no_effort(*_: object) -> None:
        return None

    monkeypatch.setattr("agentnave.members.cli_default_effort", no_effort)
    yolo: dict[str, ProviderOption] = {"dangerously_skip_permissions": True}

    def applied(options: dict[str, ProviderOption]) -> bool:
        result, names = apply_member(None, "task", "antigravity", options, frozenset())
        assert (result.get("interactive") is True) is ("interactive" in names)
        return "interactive" in names

    assert applied(yolo) is (os.name != "nt")
    assert not applied({"dangerously_skip_permissions": False})
    assert not applied({**yolo, "print_timeout": "5m"})
    (tmp_path / "settings.json").write_text('{"version": 1, "antigravity_interactive": false}')
    assert not applied(yolo)


def test_cooling_clis_disable_seats_and_filter_replacements(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    models = {"grok": "shared", "pi": "shared", "codebuddy": "other", "claude": "shared"}
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {p: {"models": {model: {}}} for p, model in models.items()},
                "modes": {
                    "task": {
                        p: {"models": [model], "default": model} for p, model in models.items()
                    }
                },
            }
        )
    )
    (tmp_path / "review.json").write_text(
        json.dumps(
            {
                "version": 1,
                "seats": {
                    "correctness-reviewer": {"provider": "grok", "model": "shared"},
                    "simplicity-reviewer": {"provider": "claude", "model": "shared"},
                },
            }
        )
    )
    from datetime import UTC, datetime, timedelta

    for provider in ("grok", "pi", "claude"):
        provider_health.record(
            tmp_path, provider, "quota", "shared", datetime.now(UTC) + timedelta(hours=1)
        )
    host = Host("claude")
    result = list_members(frozenset(), host)
    cooled = result.review["correctness-reviewer"]
    assert cooled.available is False and "quota" in (cooled.reason or "")
    until = provider_health.active(tmp_path)[("grok", "shared")]["until"]
    assert until is not None and until in (cooled.reason or "")
    assert cooled.alternatives == []
    assert cooled.replacement is not None and cooled.replacement.provider == "codebuddy"
    assert result.review["simplicity-reviewer"].native is True
    assert result.review["simplicity-reviewer"].available is None

    (tmp_path / "orchestration.json").write_text(
        json.dumps(
            {
                "version": 1,
                "preset": "split",
                "worker": {
                    "easy": {"provider": "grok", "model": "shared"},
                    "hard": {"provider": "grok", "model": "other"},
                },
                "image_generation": {"provider": "grok", "model": "shared"},
            }
        )
    )
    document = json.loads((tmp_path / "members.json").read_text())
    document["clis"]["grok"]["models"]["other"] = {}
    document["modes"]["task"]["grok"]["models"].append("other")
    (tmp_path / "members.json").write_text(json.dumps(document))
    result = list_members(frozenset(), host)
    assert result.orchestration.worker["easy"].available is False
    assert result.orchestration.worker["hard"].available is None
    image = result.orchestration.image_generation
    assert image is not None and image.available is False and image.replacement is None
    assert all(m.modes == ["discussion"] for m in result.members)
    provider_health.clear(tmp_path, "pi")
    image = list_members(frozenset(), host).orchestration.image_generation
    assert image is not None and image.replacement is not None
    assert image.replacement.provider == "pi"
    assert image.replacement.provider_options == {"model": "shared"}
