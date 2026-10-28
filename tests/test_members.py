from __future__ import annotations

import json
from pathlib import Path

import pytest
from pydantic import ValidationError

from agentnave.members import (
    MemberFile,
    MemberFileError,
    PermissionFile,
    PermissionFileError,
    ProviderExcludedError,
    apply_member,
    read_members,
    read_permissions,
)


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


def test_missing_members_leave_parameters_native_and_ignore_legacy_profiles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "data"
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    assert read_members() is None and not root.exists()
    assert apply_member(None, "task", "claude", {"model": "explicit"}, frozenset()) == (
        {"model": "explicit"},
        [],
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
    assert apply_member(document, "task", "grok", {}, frozenset()) == ({}, [])
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps({"version": 1, "clis": {"grok": {"model": "grok-4.7", "effort": "high"}}}),
        encoding="utf-8",
    )
    # The model stays the CLI's choice; only the settings saved for it are applied.
    assert apply_member(document, "task", "grok", {}, frozenset()) == (
        {"effort": "low", "x": 1},
        ["effort", "x"],
    )
    assert apply_member(document, "task", "grok", {"model": "grok-4.6"}, frozenset()) == (
        {"model": "grok-4.6"},
        [],
    )


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
    assert apply_member(document, "task", "codex", {}, frozenset()) == (
        {"effort": "high"},
        ["effort"],
    )
    assert apply_member(document, "task", "codex", {"sandbox": "workspace-write"}, frozenset()) == (
        {"sandbox": "workspace-write", "effort": "high"},
        ["effort"],
    )
    explicit = {"model": "cached-A", "effort": "high", **permissions}
    assert apply_member(document, "task", "codex", {"model": "cached-A"}, frozenset()) == (
        explicit,
        ["effort", "dangerously_bypass_approvals_and_sandbox", "sandbox"],
    )
    document.defaults = {"task": {"codex": "cached-A"}}
    assert apply_member(document, "task", "codex", {}, frozenset()) == (
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
    assert apply_member(document, "task", "antigravity", {}, frozenset()) == ({}, [])


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
    assert apply_member(
        None, "discussion", "claude", {}, frozenset(), restricted=True, permissions=tiers
    ) == ({}, [])
    assert apply_member(None, "discussion", "claude", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "bypassPermissions"},
        ["permission_mode"],
    )
    # Pi has no approval step; native or absent CLIs keep their own permissions.
    for provider in ("pi", "grok"):
        assert apply_member(None, "task", provider, {}, frozenset(), permissions=tiers) == ({}, [])
    assert apply_member(
        None, "task", "codex", {}, frozenset(), permissions=PermissionFile(version=1)
    ) == ({}, [])
