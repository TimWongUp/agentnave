from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from mcp import Client

from agentnave import mcp_runtime
from agentnave.adapters.base import AdapterHooks
from agentnave.host import Host
from agentnave.mcp_server import mcp
from agentnave.members import (
    MemberFile,
    MemberFileError,
    list_members,
    read_members,
    validate_task_seat,
)
from agentnave.task_start import apply_task_defaults


def _installed_cli(_adapter: AdapterHooks) -> bool:
    return True


def test_cli_pool_presence_replaces_only_legacy_model_restrictions() -> None:
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {"pi": {"models": {"old": {}, "new": {}}}},
            "modes": {
                mode: {"pi": {"models": ["old"], "default": "old"}}
                for mode in ("task", "discussion")
            },
        }
    )
    for mode in ("task", "discussion"):
        with pytest.raises(ValueError, match="not allowed"):
            document.apply(mode, "pi", {"model": "new"}, frozenset())
        assert document.apply(mode, "pi", {}, frozenset())[0] == {"model": "old"}
    # Null explicitly removes the legacy restriction; even unrecorded models are allowed.
    document.clis["pi"].allowed_models = None
    for mode in ("task", "discussion"):
        assert document.apply(mode, "pi", {"model": "unrecorded"}, frozenset())[0] == {
            "model": "unrecorded"
        }
    document.clis["pi"].allowed_models = ["new"]
    for mode in ("task", "discussion"):
        assert document.apply(mode, "pi", {"model": "new"}, frozenset())[0] == {"model": "new"}
        # A stale default fails at use, never silently switches to the pool's first model.
        with pytest.raises(ValueError, match="'old'.*not allowed in CLI"):
            document.apply(mode, "pi", {}, frozenset())
        with pytest.raises(ValueError, match="not allowed in CLI"):
            document.apply(mode, "pi", {"model": "old"}, frozenset(), seat=frozenset({"model"}))


def test_stale_scene_default_does_not_break_other_clis() -> None:
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {
                "pi": {"models": {"old": {}, "new": {}}, "allowed_models": ["new"]},
                "grok": {"models": {"healthy": {}}},
            },
            "modes": {
                "task": {
                    "pi": {"models": ["new"], "default": "old"},
                    "grok": {"models": ["healthy"], "default": "healthy"},
                }
            },
        }
    )
    assert document.apply("task", "grok", {}, frozenset())[0]["model"] == "healthy"
    with pytest.raises(ValueError, match="'old'.*not allowed"):
        document.apply("task", "pi", {}, frozenset())


def test_orchestration_hides_task_catalogue_and_resolves_seats_from_clis(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    host = Host("claude", "env")
    document: dict[str, Any] = {
        "version": 1,
        "clis": {
            "pi": {
                "models": {
                    "task-only": {},
                    "shared": {"effort": "high"},
                },
                "allowed_models": ["task-only", "shared"],
            },
            "grok": {"models": {"seat-only": {}}, "allowed_models": ["seat-only"]},
        },
        "modes": {
            "task": {
                "pi": {
                    "models": ["task-only"],
                    "default": "task-only",
                    "overrides": {
                        "task-only": {"options": {"tools": "hidden-task"}},
                        "shared": {"effort": "low"},
                    },
                }
            },
            "discussion": {
                "pi": {"models": ["shared"], "default": "shared"},
                "grok": {"models": ["seat-only"]},
            },
        },
    }
    path = tmp_path / "members.json"
    path.write_text(json.dumps(document))
    roles: dict[str, Any] = {
        "version": 1,
        "preset": "split",
        "enabled_roles": ["explorer", "worker", "worker_writing"],
        "worker": {"easy": {"provider": "grok", "model": "seat-only"}},
        "worker_writing": {"hard": {"provider": "pi", "model": "shared"}},
        "explorer": {"easy": {"provider": "pi", "model": "shared"}},
    }
    role_path = tmp_path / "orchestration.json"
    role_path.write_text(json.dumps(roles))
    (tmp_path / "review.json").write_text(
        json.dumps({"version": 1, "seats": {"correctness-reviewer": roles["worker"]["easy"]}})
    )
    listed = list_members(frozenset(), host)
    assert listed.orchestration.worker["easy"].available is None
    assert listed.orchestration.worker_writing["hard"].available is None
    assert not listed.orchestration.worker_writing.get("easy")
    native = list_members(frozenset({"pi"}), Host("pi", "env")).orchestration.worker_writing["hard"]
    assert native.native and native.available is None and native.replacement is None
    assert listed.review["correctness-reviewer"].available is None
    assert all(member.modes == ["discussion"] for member in listed.members)
    default = next(member for member in listed.members if member.default)
    assert default.model == "shared" and default.default == ["discussion"]
    # Shared model records may be shown for discussion, but task overrides stay hidden.
    assert "hidden-task" not in listed.model_dump_json()
    assert {member.provider for member in listed.members} == {"pi", "grok"}

    # A disabled task CLI still serves the configured seat and explicit plain dispatch.
    options, _ = apply_task_defaults(
        "grok", "worker-easy", {"model": "seat-only"}, ["model"], frozenset(), host
    )
    assert options["model"] == "seat-only"
    options, _ = apply_task_defaults("pi", None, {"model": "shared"}, [], frozenset(), host)
    assert options == {"model": "shared", "effort": "high"}
    # The hidden task default is not borrowed when a model is omitted.
    with pytest.raises(ValueError, match="Model is required"):
        apply_task_defaults("pi", None, {}, [], frozenset(), host)
    with pytest.raises(ValueError, match="excluded"):
        apply_task_defaults(
            "grok", "worker-easy", {"model": "seat-only"}, ["model"], frozenset({"grok"}), host
        )

    # Removing the employee from both scenes keeps its model settings, but disables service use.
    removed = json.loads(path.read_text())
    del removed["modes"]["discussion"]["grok"]
    path.write_text(json.dumps(removed))
    unavailable = list_members(frozenset(), host).orchestration.worker["easy"]
    assert unavailable.available is False
    assert unavailable.replacement is None or unavailable.replacement.provider != "grok"
    with pytest.raises(ValueError, match="disabled in the App"):
        apply_task_defaults("grok", None, {"model": "seat-only"}, [], frozenset(), host)
    path.write_text(json.dumps(document))

    # Turning orchestration off or to meetings-only restores ordinary candidates and scenario overrides.
    for preset in ("off", "meetings"):
        roles["preset"] = preset
        role_path.write_text(json.dumps(roles))
        restored = list_members(frozenset(), host)
        assert not restored.orchestration.enabled
        assert any(
            member.model == "task-only" and member.default == ["task"]
            for member in restored.members
        )
        assert apply_task_defaults("pi", None, {}, [], frozenset(), host)[0]["model"] == "task-only"
        assert (
            apply_task_defaults("pi", None, {"model": "shared"}, [], frozenset(), host)[0]["effort"]
            == "low"
        )
        with pytest.raises(MemberFileError, match="not enabled"):
            validate_task_seat("worker-easy", host)
    # A seat uses CLI shared settings even when all its options were explicit.
    (tmp_path / "review.json").write_text(
        json.dumps({"version": 1, "seats": {"correctness-reviewer": roles["explorer"]["easy"]}})
    )
    assert (
        apply_task_defaults(
            "pi", "correctness-reviewer", {"model": "shared"}, [], frozenset(), host
        )[0]["effort"]
        == "high"
    )
    assert json.loads(path.read_text()) == document


@pytest.mark.asyncio
async def test_seats_and_meetings_reject_the_cli_pool_without_rebinding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {"pi": {"models": {"old": {}, "new": {}}, "allowed_models": ["new"]}},
                "modes": {"task": {}, "discussion": {"pi": {"models": ["old"], "default": "old"}}},
            }
        )
    )
    binding = {"provider": "pi", "model": "old"}
    roles = {
        "version": 1,
        "preset": "split",
        "enabled_roles": ["worker", "worker_writing"],
        "worker": {"easy": binding},
        "worker_writing": {"hard": binding},
        "image_generation": binding,
    }
    (tmp_path / "orchestration.json").write_text(json.dumps(roles))
    (tmp_path / "review.json").write_text(
        json.dumps({"version": 1, "seats": {"correctness-reviewer": binding}})
    )
    listed = list_members(frozenset(), Host("claude", "env"))
    assert listed.orchestration.image_generation is not None
    for status in (
        listed.orchestration.worker["easy"],
        listed.orchestration.worker_writing["hard"],
        listed.orchestration.image_generation,
        listed.review["correctness-reviewer"],
    ):
        assert status.model == "old" and status.available is False
        assert status.reason is not None and "not allowed in CLI" in status.reason
    # An invalid native binding also reports the CLI boundary.
    native = list_members(frozenset({"pi"}), Host("pi", "env")).review["correctness-reviewer"]
    assert native.native and native.available is False
    native_specialist = list_members(frozenset({"pi"}), Host("pi", "env")).orchestration
    assert native_specialist.worker_writing["hard"].native
    assert native_specialist.worker_writing["hard"].available is False
    assert native_specialist.image_generation is None
    async with Client(mcp) as client:
        for selector in (
            {"seat": "worker-easy"},
            {"seat": "worker-writing-hard"},
            {"seat": "image-generator"},
            {"role": "correctness-reviewer"},
        ):
            result = await client.call_tool(
                "start_agent", {"prompt": "test", "cwd": str(tmp_path), **selector}
            )
            assert result.is_error and "not allowed in CLI" in str(result.content)
        for options in ({}, {"model": "old"}):
            result = await client.call_tool(
                "run_round",
                {
                    "title": "池外会议",
                    "cwd": str(tmp_path),
                    "members": [
                        {"id": name, "provider": "pi", "provider_options": options}
                        for name in ("a", "b")
                    ],
                },
            )
            assert result.is_error and "not allowed in CLI" in str(result.content)
    assert json.loads((tmp_path / "orchestration.json").read_text()) == roles
    assert read_members() is not None

    # Without a new CLI pool, hiding task defaults does not remove legacy model boundaries.
    legacy: dict[str, Any] = {
        "version": 1,
        "clis": {"pi": {"models": {"old": {}, "new": {}}}},
        "modes": {"task": {"pi": {"models": ["old"], "default": "old"}}},
    }
    (tmp_path / "members.json").write_text(json.dumps(legacy))
    with pytest.raises(ValueError, match="not allowed"):
        apply_task_defaults("pi", None, {"model": "new"}, [], frozenset(), Host("claude", "env"))
    assert (
        apply_task_defaults("pi", None, {"model": "old"}, [], frozenset(), Host("claude", "env"))[
            0
        ]["model"]
        == "old"
    )
    with pytest.raises(ValueError, match="disabled in the App"):
        apply_task_defaults("grok", None, {"model": "new"}, [], frozenset(), Host("claude", "env"))
    legacy["modes"] = {"task": {}, "discussion": {"pi": {"models": ["old"]}}}
    (tmp_path / "members.json").write_text(json.dumps(legacy))
    with pytest.raises(ValueError, match="not allowed"):
        apply_task_defaults("pi", None, {"model": "new"}, [], frozenset(), Host("claude", "env"))
    with pytest.raises(ValueError, match="Model is required") as missing:
        apply_task_defaults("pi", None, {}, [], frozenset(), Host("claude", "env"))
    assert "old" not in str(missing.value)


@pytest.mark.asyncio
async def test_custom_seats_follow_the_cli_pool_without_rebinding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env", "claude"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {"pi": {"models": {"old": {}, "new": {}}, "allowed_models": ["new"]}},
                "modes": {"task": {}, "discussion": {"pi": {"models": ["new"], "default": "new"}}},
            }
        )
    )
    seats = [
        {
            "id": id_,
            "kind": "custom",
            "role": id_,
            "description": id_,
            "provider": "pi",
            "model": model,
            "access": "write",
            "difficulty": "easy",
        }
        for id_, model in (("outside", "old"), ("inside", "new"))
    ]
    document = {"version": 1, "seats": seats}
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))
    (tmp_path / "decision-seats.json").write_text(json.dumps(document))
    listed = list_members(frozenset(), Host("claude", "env", "claude")).orchestration.custom_seats
    assert listed["inside"].available is None
    assert listed["outside"].available is False
    assert "not allowed in CLI" in (listed["outside"].reason or "")
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agent", {"prompt": "test", "cwd": str(tmp_path), "seat": "outside"}
        )
        assert result.is_error and "not allowed in CLI" in str(result.content)
    assert json.loads((tmp_path / "decision-seats.json").read_text()) == document
