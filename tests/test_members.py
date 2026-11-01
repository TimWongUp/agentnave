from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from agentnave.host import Host
from agentnave.members import (
    CLIOptionsFile,
    CLIOptionsFileError,
    MemberFile,
    MemberFileError,
    OrchestrationFile,
    PermissionFile,
    PermissionFileError,
    ProviderExcludedError,
    ReviewFile,
    SettingsFileError,
    apply_member,
    list_members,
    read_cli_options,
    read_max_running,
    read_members,
    read_permissions,
    role_seat,
)
from agentnave.models import ProviderOption

APP_FILES = Path(__file__).parent / "fixtures" / "app-files"
APP_MODELS: dict[
    str, type[MemberFile | ReviewFile | OrchestrationFile | PermissionFile | CLIOptionsFile]
] = {
    "members": MemberFile,
    "review": ReviewFile,
    "orchestration": OrchestrationFile,
    "permissions": PermissionFile,
    "cli-options": CLIOptionsFile,
}


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
    }

    def check(host: Host, enabled: bool) -> None:
        path.write_text(json.dumps(document))
        status = list_members(frozenset(), host).orchestration
        assert status.enabled is enabled
        assert bool(status.explorer) is enabled
        assert bool(status.worker) is enabled
        assert (role_seat("explorer-easy", host) is not None) is enabled
        assert (role_seat("worker-hard", host) is not None) is enabled

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
    check(Host("grok", "env"), True)
    check(claude, False)  # no connected row in the resolved map
    document["effective_apps"] = {"codex": "split", "codex@work": "off"}
    check(codex, False)  # an old connection cannot borrow a different account's mode
    check(Host("codex", "env", "codex"), True)
    check(Host("codex", "env", "codex@work"), False)
    document["effective_apps"] = {}
    check(codex, False)
    # Turning orchestration off preserves the saved picks for the next enable.
    assert json.loads(path.read_text())["worker"]["hard"] == seat


@pytest.mark.parametrize(
    "sample", sorted(APP_FILES.glob("*/*.json")), ids=lambda path: f"{path.parent.name}/{path.stem}"
)
def test_app_file_samples_agree_with_the_tray(
    sample: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The tray checks the same samples (apps/tray/src/members/tests.rs), so a file the App
    saves is one the server reads, and one the App refuses the server refuses too."""
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))

    def check() -> object:
        if sample.parent.name == "settings":
            (tmp_path / "settings.json").write_bytes(sample.read_bytes())
            return read_max_running()
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
    # Without any App file only the default "auto" permission tier applies.
    assert apply_member(None, "task", "claude", {"model": "explicit"}, frozenset()) == (
        {"model": "explicit", "permission_mode": "auto"},
        ["permission_mode"],
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


def test_cli_options_apply_to_every_model_of_the_cli(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    assert read_cli_options() is None
    for clis in ({"codex": {"sandbox": "read-only"}}, {"codex": {"model": "m"}}):
        (tmp_path / "cli-options.json").write_text(json.dumps({"version": 1, "clis": clis}))
        with pytest.raises(CLIOptionsFileError, match="must not contain"):
            read_cli_options()
    (tmp_path / "cli-options.json").write_text(
        json.dumps({"version": 1, "clis": {"codex": {"skip_git_repo_check": True}}})
    )
    saved = read_cli_options()
    assert saved is not None
    (tmp_path / "cli-defaults.json").write_text(json.dumps({"clis": {"codex": {"model": "m"}}}))
    document = MemberFile.model_validate(
        {"version": 1, "clis": {"codex": {"models": {"m": {}}}}, "modes": {}}
    )
    # Listed, unlisted, and the CLI's own default model all carry the CLI setting,
    # unlike execution controls saved on one model.
    for options in ({"model": "m"}, {"model": "unlisted"}, {}):
        for members in (document, None):
            result, applied = apply_member(
                members, "task", "codex", options, frozenset(), cli_options=saved
            )
            assert result["skip_git_repo_check"] is True
            assert applied[-2:] == ["skip_git_repo_check", "approve_for_me"]
    # The model's own option and the caller both win on the same key.
    document.clis["codex"].models["m"].options = {"skip_git_repo_check": False}
    for options in ({"model": "m"}, {"model": "x", "skip_git_repo_check": False}):
        result, applied = apply_member(
            document, "task", "codex", options, frozenset(), cli_options=saved
        )
        assert result["skip_git_repo_check"] is False
        assert applied.count("skip_git_repo_check") == ("skip_git_repo_check" not in options)
    # Restricted runs accept no tool-granting extras.
    assert apply_member(
        None, "discussion", "codex", {}, frozenset(), restricted=True, cli_options=saved
    ) == ({}, [])
    listed = list_members(frozenset(), Host())
    assert listed.cli_options == {"codex": {"skip_git_repo_check": True}}
    # A restricted model stays valid: list_members checks what a dispatch would send.
    document.clis["codex"].models["m"].options = {"restricted_tools": True}
    (tmp_path / "members.json").write_text(document.model_dump_json(exclude_unset=True))
    [member] = list_members(frozenset(), Host()).modes["task"].members
    assert (member.valid, member.options) == (True, {"restricted_tools": True})
    assert apply_member(
        document, "task", "codex", {"model": "m"}, frozenset(), cli_options=saved
    ) == ({"model": "m", "restricted_tools": True}, ["restricted_tools"])
    (tmp_path / "cli-options.json").write_text("{")
    assert "cli-options.json" in str(list_members(frozenset(), Host()).cli_options_error)


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
    assert listed["grok"] == "auto"
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
        {"permission_mode": "bypassPermissions"},
        ["permission_mode"],
    )
    # Pi has no approval step; a CLI missing from the file, or no file, runs "auto".
    assert apply_member(None, "task", "pi", {}, frozenset(), permissions=tiers) == ({}, [])
    assert apply_member(None, "task", "grok", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "auto"},
        ["permission_mode"],
    )
    assert apply_member(
        None, "task", "codex", {}, frozenset(), permissions=PermissionFile(version=1)
    ) == ({"approve_for_me": True}, ["approve_for_me"])
