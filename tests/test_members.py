from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any, cast, get_args

import pytest
from pydantic import ValidationError

from agentnave.accounts import AccountsFile
from agentnave.adapters import get_adapter
from agentnave.adapters.base import AdapterHooks
from agentnave.decision_seats import DecisionSeatsFileError, custom_seats, decision_seats_for
from agentnave.host import Host
from agentnave.host_choice import default_host_cli_mode
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
    _read_app_file,  # pyright: ignore[reportPrivateUsage]
    apply_member,
    list_members,
    read_antigravity_interactive,
    read_claude_interactive,
    read_max_running,
    read_members,
    read_permissions,
    role_seat,
    validate_task_seat,
)
from agentnave.models import ProviderOption
from agentnave.orchestration_roles import (
    DecisionSeatsFile,
    OrchestrationGroup,
    OrchestrationRole,
    OrchestrationStatus,
    role_catalog,
)
from agentnave.t3_seats import T3SeatsFile
from agentnave.task_start import role_access

APP_FILES = Path(__file__).parent / "fixtures" / "app-files"
APP_MODELS: dict[
    str,
    type[
        MemberFile
        | ReviewFile
        | OrchestrationFile
        | PermissionFile
        | DecisionSeatsFile
        | AccountsFile
        | T3SeatsFile
    ],
] = {
    "accounts": AccountsFile,
    "members": MemberFile,
    "decision-seats": DecisionSeatsFile,
    "review": ReviewFile,
    "orchestration": OrchestrationFile,
    "permissions": PermissionFile,
    "t3-seats": T3SeatsFile,
}


def _installed_cli(_adapter: AdapterHooks) -> bool:
    # Configuration tests do not depend on the machine's installed CLIs.
    return True


def test_role_catalog_covers_wire_seats_and_top_level_bindings() -> None:
    catalog = role_catalog()
    assert len(catalog) == 7
    assert {role.group for role in catalog} == set(get_args(OrchestrationGroup.__value__))
    assert {
        f"{role.seat_prefix}-{difficulty}"
        for role in catalog
        for difficulty in ("easy", "medium", "hard")
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
    assert set(rules["order"]) == {"meetings", "split", "decision"}
    known_steps = (
        set(rules["sections"])
        | set(rules["switches"]) - {"lead_only"}
        | {"meeting_defaults", "enabled_roles"}
    )
    for steps in rules["order"].values():
        assert steps and set(steps) <= known_steps
    lead = rules["switches"]["lead_only"]
    assert set(lead["replaces_in"]) == {"shared"}
    for name in lead["replaces_in"]:
        assert lead["when_false"] in rules["sections"][name]
    # New split_options keys (parallel_writes) and legacy parallel preset both stay readable.
    assert OrchestrationFile.model_validate(
        {"version": 1, "preset": "split", "split_options": {"parallel_writes": True}}
    ).enabled(Host("codex", "env"))
    assert OrchestrationFile.model_validate({"version": 1, "preset": "parallel"}).enabled(
        Host("codex", "env")
    )
    # The decision text has no per-group seat lines.
    assert "enabled_roles" not in rules["order"]["decision"]
    t3_line = rules["sections"]["t3_orchestration"]
    assert t3_line.startswith(
        "\n- 在 T3 Code 主线程（有 `delegate_task` 工具）里，用户已开启 T3 编排："
    )
    assert "`recommend_seat`" in t3_line and "`delegate_task`" in t3_line
    assert "由 `delegate_task` 派出的线程只完成自己的任务，不再派发或开会。" in t3_line
    assert all("t3_orchestration" not in steps for steps in rules["order"].values())
    assert set(rules["meeting_defaults"]["provider_names"]) == set(get_args(ProviderName.__value__))

    providers_rs = (
        Path(__file__).resolve().parents[1] / "apps" / "tray" / "src" / "providers.rs"
    ).read_text("utf-8")
    match = re.search(
        r"pub const LAUNCH_HOST_CLIS:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\];", providers_rs
    )
    assert match is not None
    tray_launch_hosts = set(re.findall(r'"([^"]+)"', match.group(1)))
    adapter_launch_hosts = {
        name
        for name in get_args(ProviderName.__value__)
        if get_adapter(name).default_host_cli_mode == "launch"
    }
    assert tray_launch_hosts == adapter_launch_hosts == {"antigravity"}
    assert default_host_cli_mode(Host("antigravity", "parent")) == "launch"
    assert default_host_cli_mode(Host("antigravity", "parent", app_id="antigravity")) == "launch"
    assert default_host_cli_mode(Host("claude", "env")) == "subagent"
    assert default_host_cli_mode(Host("workbuddy", "env")) == "subagent"
    assert default_host_cli_mode(Host()) == "subagent"
    assert OrchestrationFile(version=1).host_cli_mode(Host("antigravity", "parent")) == "launch"
    assert (
        OrchestrationFile(version=1, host_cli={"antigravity": "subagent"}).host_cli_mode(
            Host("antigravity", "parent")
        )
        == "subagent"
    )


def test_host_cli_mode_falls_back_when_orchestration_unreadable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.host_choice import host_cli_mode

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "orchestration.json"
    host = Host("antigravity", "parent", app_id="antigravity")
    path.write_text('{"version": 1, "host_cli": {"antigravity": "skip"}')
    assert host_cli_mode(host) == "subagent"
    path.unlink()
    path.mkdir()
    assert host_cli_mode(host) == "subagent"


def test_host_cli_mode_old_connection_cannot_identify_saved_row() -> None:
    host = Host("antigravity", "parent")
    for choice in ("skip", "subagent"):
        roles = OrchestrationFile.model_validate(
            {
                "version": 1,
                "effective_apps": {"antigravity": "split"},
                "host_cli": {"antigravity": choice},
            }
        )
        assert roles.host_cli_mode(host) == "subagent"
    assert OrchestrationFile(version=1, effective_apps={}).host_cli_mode(host) == "subagent"
    assert (
        OrchestrationFile(version=1, effective_apps={}).host_cli_mode(
            Host("antigravity", "parent", app_id="antigravity")
        )
        == "launch"
    )


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


def test_named_accounts_are_chosen_cooled_and_hosted_apart(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health
    from agentnave.seat_fallbacks import select

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    work, personal = tmp_path / "work-home", tmp_path / "personal-home"
    (tmp_path / "accounts.json").write_text(
        json.dumps(
            {
                "version": 1,
                "accounts": [
                    {
                        "id": "work",
                        "provider": "claude",
                        "label": "工作",
                        "dir": str(work),
                        "managed": True,
                    },
                    {
                        "id": "a2",
                        "provider": "claude",
                        "label": "个人",
                        "dir": str(personal),
                        "managed": False,
                    },
                ],
            }
        )
    )
    seat = {
        "provider": "claude",
        "model": "opus",
        "account": "ghost",  # a valid file may name an account that is not registered
        "fallbacks": [
            {"provider": "claude", "model": "opus", "account": "work"},
            {"provider": "claude", "model": "opus", "account": "a2"},
            {"provider": "claude", "model": "opus"},
        ],
    }
    (tmp_path / "review.json").write_text(
        json.dumps({"version": 1, "seats": {"security-reviewer": seat}})
    )
    host = Host("codex", "env")
    listed = list_members(frozenset(), host)
    # Only an id and a display name are listed; never the directory.
    assert [(a.id, a.label) for a in listed.accounts] == [("work", "工作"), ("a2", "个人")]
    assert "dir" not in listed.model_dump_json() and str(work) not in listed.model_dump_json()
    status = listed.review["security-reviewer"]
    assert status.fallback_position == 1 and status.account == "work"
    assert [(c.account, c.available) for c in status.fallbacks] == [
        ("work", True),
        ("a2", True),
        (None, True),
    ]
    configured = role_seat("security-reviewer", host)
    assert configured is not None
    chosen, receipt = select(configured, host, frozenset())
    assert (chosen.account, chosen.alone().fallbacks) == ("work", [])
    assert receipt is not None and receipt["skipped"][0]["reason"].startswith("Account 'ghost'")

    # Cooling one account takes only that account's choice out.
    provider_health.record(tmp_path, "claude", "auth", "opus", account="work")
    chosen, receipt = select(configured, host, frozenset())
    assert chosen.account == "a2" and receipt is not None and receipt["position"] == 2
    # The same model under another account is a seat in use of its own.
    random_pick, _ = select(
        configured,
        host,
        frozenset(),
        randomize=True,
        in_use={("claude", "a2", "opus")},
        randrange=lambda _n: 0,
    )
    assert random_pick.account is None

    # A seat is the host's own only for the host's own account (decided by the inherited
    # configuration directory); the default account inherits what the host runs as.
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(work))
    claude = Host("claude", "env")
    provider_health.clear(tmp_path, "claude", account="work")
    mine = list_members(frozenset(), claude).review["security-reviewer"]
    assert mine.account == "work" and mine.native is True
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(personal))
    other = list_members(frozenset(), claude).review["security-reviewer"]
    assert other.account == "work" and other.native is None
    # The host's exclusion of its own CLI does not reach another account of it.
    excluded = list_members(frozenset({"claude"}), claude).review["security-reviewer"]
    assert excluded.account == "work" and excluded.reason is None
    # An image seat of another account of the same CLI is not the host drawing for itself.
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))
    _write_decision_seats(
        tmp_path,
        [
            {**_image_seat("draw-work", "claude", "写实"), "account": "work"},
            _image_seat("draw-own", "claude", "漫画"),
        ],
    )
    assert list(decision_seats_for(claude) or {}) == ["draw-work"]
    # Naming the CLI with that seat starts it as the seat's account, not the host's own.
    from agentnave.task_start import StartArguments

    arguments = StartArguments("claude", str(tmp_path), None, "draw-work", None)
    arguments.resolve_seat(claude)
    assert arguments.account == "work" and "account (draw-work seat)" in arguments.defaults


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


def test_cached_model_does_not_select_deepseek_route_or_credentials(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "cli-defaults.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    "deepseek": {"model": "deepseek-flash"},
                    "pi": {"model": "pi-default"},
                },
            }
        ),
        encoding="utf-8",
    )
    account = {"provider": "deepseek-account", "stored_credentials": True}
    document = MemberFile.model_validate(
        {
            "version": 1,
            "clis": {
                "deepseek": {"models": {"deepseek-flash": {"options": account}}},
                "pi": {
                    "models": {
                        "pi-default": {"options": {"provider": "openrouter", "tools": "read"}}
                    }
                },
            },
            "modes": {
                "task": {
                    "deepseek": {
                        "models": [],
                        "overrides": {
                            "deepseek-flash": {
                                "options": {**account, "permission_mode": "read-only"}
                            }
                        },
                    },
                    "pi": {"models": []},
                },
                "discussion": {
                    "deepseek": {
                        "models": [],
                        "overrides": {
                            "deepseek-flash": {"options": {"permission_mode": "read-only"}}
                        },
                    }
                },
            },
        }
    )
    assert document.apply("task", "deepseek", {}, frozenset()) == (
        {"stored_credentials": False},
        ["stored_credentials"],
    )
    assert document.apply("task", "deepseek", {"stored_credentials": True}, frozenset()) == (
        {"stored_credentials": True},
        [],
    )
    assert document.apply("task", "deepseek", {"model": "deepseek-flash"}, frozenset()) == (
        {"model": "deepseek-flash", **account, "permission_mode": "read-only"},
        ["provider", "stored_credentials", "permission_mode"],
    )
    assert document.apply("discussion", "deepseek", {"model": "deepseek-flash"}, frozenset()) == (
        {"model": "deepseek-flash", "permission_mode": "read-only"},
        ["permission_mode"],
    )
    assert document.apply(
        "task", "deepseek", {"model": "deepseek-flash", "provider": "gateway"}, frozenset()
    ) == (
        {"model": "deepseek-flash", "provider": "gateway", "permission_mode": "read-only"},
        ["permission_mode"],
    )
    for saved_options in (
        {"provider": "deepseek-account"},
        {"stored_credentials": False},
        {"provider": "deepseek-official", "stored_credentials": False},
    ):
        disabled = MemberFile.model_validate(
            {
                "version": 1,
                "clis": {"deepseek": {"models": {"deepseek-flash": {"options": saved_options}}}},
                "modes": {},
            }
        )
        assert disabled.apply("task", "deepseek", {}, frozenset()) == (
            {"stored_credentials": False},
            ["stored_credentials"],
        )
    for saved_options in (
        {"provider": "deepseek-official"},
        {"provider": "gateway", "stored_credentials": True},
        {},
    ):
        other = MemberFile.model_validate(
            {
                "version": 1,
                "clis": {"deepseek": {"models": {"deepseek-flash": {"options": saved_options}}}},
                "modes": {},
            }
        )
        assert other.apply("task", "deepseek", {}, frozenset()) == ({}, [])
    assert document.apply("task", "pi", {}, frozenset()) == (
        {"provider": "openrouter"},
        ["provider"],
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
    # Without a saved choice Claude runs interactively on supported platforms.
    assert read_claude_interactive() is True
    result, applied = apply_member(None, "task", "claude", {}, frozenset())
    assert result.get("interactive") is True
    assert "interactive" in applied
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
    until = provider_health.active(tmp_path)[("grok", None, "shared")]["until"]
    assert until is not None and until in (cooled.reason or "")
    assert cooled.alternatives == []
    assert cooled.replacement is not None and cooled.replacement.provider == "codebuddy"
    assert result.review["simplicity-reviewer"].native is True
    assert result.review["simplicity-reviewer"].available is False

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


def _custom_seat(seat_id: str, provider: str, access: str, **extra: Any) -> dict[str, Any]:
    return {
        "id": seat_id,
        "kind": "custom",
        "role": f"role {seat_id}",
        "description": f"role {seat_id}",
        "provider": provider,
        "model": "shared",
        "access": access,
        "difficulty": "hard",
        **extra,
    }


def _image_seat(seat_id: str, provider: str, specialty: str) -> dict[str, Any]:
    seat = _custom_seat(seat_id, provider, "write", kind="image", role=specialty)
    seat["description"] = specialty
    del seat["difficulty"]
    return seat


def _write_decision_seats(root: Path, seats: list[dict[str, Any]], **extra: Any) -> None:
    (root / "decision-seats.json").write_text(json.dumps({"version": 1, "seats": seats, **extra}))


def test_custom_seats_replace_the_fixed_catalog_for_the_effective_boss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from datetime import UTC, datetime, timedelta

    from agentnave import provider_health

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    seat = {"provider": "grok", "model": "shared"}
    roles: dict[str, Any] = {
        "version": 1,
        "preset": "split",
        "explorer": {"easy": seat},
        "worker": {"hard": seat},
        "image_generation": {"provider": "pi", "model": "shared"},
    }
    orchestration = tmp_path / "orchestration.json"
    orchestration.write_text(json.dumps(roles))
    host = Host("codex", "env", "codex")
    before = list_members(frozenset(), host).model_dump()
    seats = [
        _custom_seat("write-hard", "grok", "write", effort="high"),
        _custom_seat("write-cooling", "pi", "write"),
        _custom_seat("read-native", "codex", "read", difficulty="easy"),
        _image_seat("draw-comic", "grok", "漫画风格"),
    ]
    _write_decision_seats(tmp_path, seats)
    # In every other mode the file changes nothing, byte for byte in the output.
    for mode in ("split", "parallel", "off", "meetings"):
        orchestration.write_text(json.dumps(roles | {"preset": mode}))
        (tmp_path / "decision-seats.json").rename(tmp_path / "held.json")
        without = list_members(frozenset(), host).model_dump()
        (tmp_path / "held.json").rename(tmp_path / "decision-seats.json")
        assert list_members(frozenset(), host).model_dump() == without
    # Another boss on the mode does not give this one the catalog.
    orchestration.write_text(json.dumps(roles | {"apps": {"grok": "decision"}}))
    assert list_members(frozenset(), host).model_dump() == before
    assert list_members(frozenset(), Host("grok", "env", "grok")).orchestration.custom_seats

    orchestration.write_text(json.dumps(roles | {"preset": "decision"}))
    provider_health.record(
        tmp_path, "pi", "quota", "shared", datetime.now(UTC) + timedelta(hours=1)
    )
    result = list_members(frozenset(), host)
    status = result.orchestration
    assert status.enabled and result.orchestration_error is None
    dumped = status.model_dump()
    # The fixed groups, their scopes and the fixed image seat are not announced.
    assert "explorer" not in dumped and "worker" not in dumped and not dumped["scopes"]
    assert status.image_generation is None
    assert list(status.custom_seats) == ["write-hard", "write-cooling", "read-native", "draw-comic"]
    drawing = status.custom_seats["draw-comic"]
    assert (drawing.kind, drawing.role, drawing.access, drawing.difficulty) == (
        "image",
        "漫画风格",
        "write",
        None,
    )
    ready = status.custom_seats["write-hard"]
    assert (ready.kind, ready.role, ready.description, ready.access, ready.difficulty) == (
        "custom",
        "role write-hard",
        "role write-hard",
        "write",
        "hard",
    )
    assert "tags" not in ready.model_dump()
    assert ready.effort == "high" and ready.available is None and ready.native is None
    cooling = status.custom_seats["write-cooling"]
    assert cooling.available is False and "quota" in (cooling.reason or "")
    assert status.custom_seats["read-native"].native is True
    # The App's effective mode decides, row by row; an old connection has no row to read.
    orchestration.write_text(
        json.dumps(roles | {"preset": "split", "effective_apps": {"codex": "decision"}})
    )
    assert list_members(frozenset(), host).orchestration.custom_seats
    assert not list_members(frozenset(), Host("grok", "env", "grok")).orchestration.custom_seats
    assert not list_members(frozenset(), Host("codex", "env")).orchestration.custom_seats


def test_decision_seats_file_is_empty_when_missing_and_reported_when_broken(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    host = Host("codex", "env", "codex")
    assert decision_seats_for(host) is None
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))
    # In the decision mode a missing file is an empty catalog, not an error.
    assert decision_seats_for(host) == {}
    listed = list_members(frozenset(), host)
    assert listed.orchestration.enabled and listed.orchestration_error is None
    assert listed.orchestration.custom_seats == {}
    path = tmp_path / "decision-seats.json"
    for content in (
        "{broken",
        json.dumps({"version": 2}),
        # A review role name would be taken for the review seat.
        json.dumps(
            {
                "version": 1,
                "seats": [_custom_seat("security-reviewer", "grok", "read")],
            }
        ),
    ):
        path.write_text(content)
        with pytest.raises(DecisionSeatsFileError, match="Invalid decision-seats.json"):
            decision_seats_for(host)
        assert custom_seats(host) is None
        listed = list_members(frozenset(), host)
        assert "Invalid decision-seats.json" in (listed.orchestration_error or "")
        assert not listed.orchestration.enabled
        # Fixed seats and review roles keep working while the file is reported.
        assert role_seat("explorer-easy", host) is None
        with pytest.raises(DecisionSeatsFileError):
            validate_task_seat("anything", host)


def test_custom_seat_names_resolve_only_while_effective(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    host = Host("claude", "env", "claude")
    seat = {"provider": "grok", "model": "fixed"}
    roles: dict[str, Any] = {
        "version": 1,
        "preset": "split",
        "explorer": {"easy": seat},
        "image_generation": {"provider": "pi", "model": "img"},
    }
    (tmp_path / "orchestration.json").write_text(json.dumps(roles))
    _write_decision_seats(tmp_path, [_custom_seat("mine", "grok", "read", effort="low")])
    with pytest.raises(MemberFileError, match="Unknown task seat: mine"):
        validate_task_seat("mine", host)
    validate_task_seat("explorer-easy", host)
    (tmp_path / "orchestration.json").write_text(json.dumps(roles | {"preset": "decision"}))
    validate_task_seat("mine", host)
    with pytest.raises(MemberFileError, match="Unknown task seat: gone"):
        validate_task_seat("gone", host)
    # The fixed ordinary seats are replaced, not merely hidden.
    with pytest.raises(MemberFileError, match="not used in the decision-model mode"):
        validate_task_seat("explorer-easy", host)
    assert role_seat("explorer-easy", host) is None
    with pytest.raises(MemberFileError, match="replace the fixed catalog"):
        role_seat("explorer-easy", host, required=True)
    chosen = role_seat("mine", host, required=True)
    assert chosen == ReviewSeat(provider="grok", model="shared", effort="low")
    # The fixed image seat goes like the other fixed seats; image seats are the user's own.
    with pytest.raises(MemberFileError, match="not used in the decision-model mode"):
        validate_task_seat("image-generator", host)
    assert role_seat("image-generator", host) is None
    # Deleting the seat in the App makes the name unknown at once.
    _write_decision_seats(tmp_path, [])
    with pytest.raises(MemberFileError, match="Unknown task seat: mine"):
        validate_task_seat("mine", host)
    assert role_seat("mine", host) is None


@pytest.mark.parametrize(
    "orchestration",
    [None, "off", "meetings", "split", "parallel", {"codex": "off"}, {"codex": "split"}],
)
def test_custom_seats_need_the_decision_mode_for_the_boss(
    orchestration: str | dict[str, str] | None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    host = Host("codex", "env", "codex")
    fixed = {"provider": "grok", "model": "shared"}
    roles: dict[str, Any] = {"version": 1, "worker": {"hard": fixed}}
    if isinstance(orchestration, dict):
        roles |= {"preset": "off", "effective_apps": orchestration}
    elif orchestration is not None:
        roles["preset"] = orchestration
    if orchestration is not None:
        (tmp_path / "orchestration.json").write_text(json.dumps(roles))
    before = list_members(frozenset(), host).model_dump()
    _write_decision_seats(tmp_path, [_custom_seat("mine", "grok", "write")])
    # The file alone changes nothing, whatever the other content.
    assert list_members(frozenset(), host).model_dump() == before
    assert decision_seats_for(host) is None and custom_seats(host) is None
    with pytest.raises(MemberFileError, match="Unknown task seat: mine"):
        validate_task_seat("mine", host)
    assert role_seat("mine", host) is None
    # In the decision mode for the boss, the same file takes effect.
    roles |= {"preset": "off", "effective_apps": {"codex": "decision"}}
    (tmp_path / "orchestration.json").write_text(json.dumps(roles))
    assert list(list_members(frozenset(), host).orchestration.custom_seats) == ["mine"]
    validate_task_seat("mine", host)


def test_seat_ids_of_the_fixed_catalog_and_auto_are_reserved() -> None:
    for reserved in ("worker-easy", "image-generator", "auto"):
        with pytest.raises(ValidationError, match="reserved"):
            DecisionSeatsFile.model_validate(
                {"version": 1, "seats": [_custom_seat(reserved, "grok", "read")]}
            )


def test_a_card_is_the_seats_of_one_kind_and_role_one_per_difficulty() -> None:
    def seat(seat_id: str, **extra: Any) -> dict[str, Any]:
        return _custom_seat(seat_id, "grok", "read") | extra

    def file(*seats: dict[str, Any]) -> DecisionSeatsFile:
        return DecisionSeatsFile.model_validate({"version": 1, "seats": list(seats)})

    group = {"kind": "explorer", "role": "通用探索", "description": "读代码和材料"}
    assert len(file(seat("a", **group), seat("b", **group, difficulty="easy")).seats) == 2
    # One seat per difficulty on a card; a group has one role; a group keeps its access.
    with pytest.raises(ValidationError, match="one seat per difficulty"):
        file(seat("a", **group), seat("b", **group))
    with pytest.raises(ValidationError, match="has one role"):
        file(seat("a", **group), seat("b", **group | {"role": "探索"}, difficulty="easy"))
    with pytest.raises(ValidationError, match="read-only by its group"):
        file(seat("a", **group, access="write"))
    # Custom specialties are a card each, whatever their access.
    custom = {"kind": "custom", "description": "x"}
    file(seat("a", **custom, role="x"), seat("b", **custom, role="y", access="write"))
    with pytest.raises(ValidationError, match="kind must be"):
        file(seat("a", kind="gardening"))
    bad_fields: list[dict[str, Any]] = [
        {"description": " "},
        {"description": "x" * 201},
        {"role": "x" * 41},
        {"tags": []},
    ]
    for bad in bad_fields:
        with pytest.raises(ValidationError):
            file(seat("a", **bad))
    assert file(seat("a", description="x" * 200)).seats[0].description == "x" * 200


def test_image_seats_are_write_only_have_no_difficulty_and_repeat_by_specialty() -> None:
    def file(*seats: dict[str, Any]) -> DecisionSeatsFile:
        return DecisionSeatsFile.model_validate({"version": 1, "seats": list(seats)})

    comic = _image_seat("comic", "grok", "漫画风格")
    photo = _image_seat("photo", "grok", "写实")
    assert [seat.difficulty for seat in file(comic, photo).seats] == [None, None]
    with pytest.raises(ValidationError, match="one seat per difficulty"):
        file(comic, comic | {"id": "again"})
    with pytest.raises(ValidationError, match="no difficulty"):
        file(comic | {"difficulty": "easy"})
    with pytest.raises(ValidationError, match="no difficulty"):
        file(comic | {"access": "read"})
    # Every other kind needs one.
    with pytest.raises(ValidationError, match="needs a difficulty"):
        file({k: v for k, v in _custom_seat("a", "grok", "read").items() if k != "difficulty"})


@pytest.mark.parametrize(
    ("text", "accepted"),
    [
        ("写代码 and code 🚀", True),
        ("  数据库迁移　", True),  # white space at the ends is trimmed
        ("a b", True),  # an ordinary space inside stays
        ("a­b", False),  # soft hyphen
        ("ab", False),  # private use
        ("a\U000f0000b", False),
        ("a​b", False),  # zero width space
        ("a⁦b", False),
        ("a﻿b", False),
        ("a b", False),  # white space other than the ordinary space, inside
        ("a　b", False),
        ("a\nb", False),
        ("a\x1fb", False),
        ("\x1fab", False),  # control characters are not white space to trim
        ("a\x85b", False),
        ("a\ud800b", False),  # a lone surrogate
        ("a⁥b", True),  # only what the table names
    ],
)
def test_decision_seat_text_follows_the_table_the_app_checks_too(text: str, accepted: bool) -> None:
    seat = _custom_seat("s-1", "grok", "write", role=text, description=text)
    if accepted:
        assert DecisionSeatsFile.model_validate({"version": 1, "seats": [seat]}).seats
    else:
        with pytest.raises(ValueError, match="printable characters"):
            DecisionSeatsFile.model_validate({"version": 1, "seats": [seat]})


def test_the_seats_of_one_card_share_one_access() -> None:
    both = [
        _custom_seat("s-1", "grok", "read", role="迁移", difficulty="easy"),
        _custom_seat("s-2", "grok", "write", role="迁移", difficulty="hard"),
    ]
    with pytest.raises(ValueError, match="one access"):
        DecisionSeatsFile.model_validate({"version": 1, "seats": both})
    both[1]["role"] = "另一张"
    assert len(DecisionSeatsFile.model_validate({"version": 1, "seats": both}).seats) == 2


def test_an_image_seat_may_say_null_for_its_difficulty_and_others_may_not() -> None:
    image = {**_image_seat("s-1", "grok", "写实"), "difficulty": None}
    assert DecisionSeatsFile.model_validate({"version": 1, "seats": [image]}).seats
    with pytest.raises(ValueError, match="needs a difficulty"):
        DecisionSeatsFile.model_validate(
            {"version": 1, "seats": [{**_custom_seat("s-2", "grok", "write"), "difficulty": None}]}
        )


def test_seat_fallback_duplicates_invalidate_files(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    seat = {
        "provider": "grok",
        "model": "shared",
        "fallbacks": [{"provider": "grok", "model": "shared", "effort": "high"}],
    }
    (tmp_path / "review.json").write_text(
        json.dumps({"version": 1, "seats": {"correctness-reviewer": seat}})
    )
    assert list_members(frozenset(), Host()).review_error is not None
    (tmp_path / "orchestration.json").write_text(
        json.dumps({"version": 1, "preset": "split", "worker": {"easy": seat}})
    )
    assert list_members(frozenset(), Host()).orchestration_error is not None
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))
    _write_decision_seats(
        tmp_path, [_custom_seat("own", "grok", "read", fallbacks=seat["fallbacks"])]
    )
    assert list_members(frozenset(), Host()).orchestration_error is not None


def test_native_seat_respects_model_pool_before_fallback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    (tmp_path / "members.json").write_text(
        json.dumps(
            {
                "version": 1,
                "clis": {
                    "codex": {
                        "models": {"allowed": {}, "blocked": {}},
                        "allowed_models": ["allowed"],
                    }
                },
                "modes": {},
            }
        )
    )
    (tmp_path / "review.json").write_text(
        json.dumps(
            {
                "version": 1,
                "seats": {
                    "correctness-reviewer": {
                        "provider": "codex",
                        "model": "blocked",
                        "fallbacks": [{"provider": "codex", "model": "allowed", "effort": "high"}],
                    },
                },
            }
        )
    )
    status = list_members(frozenset(), Host("codex", "env")).review["correctness-reviewer"]
    assert status.model == "allowed" and status.fallback_position == 1 and status.native
    from agentnave.members import role_seat
    from agentnave.seat_fallbacks import select

    host = Host("codex", "env")
    configured = role_seat("correctness-reviewer", host)
    assert configured is not None
    chosen, receipt = select(configured, host, frozenset(), None)
    assert chosen.model == "allowed" and receipt is not None
    assert "not allowed" in receipt["skipped"][0]["reason"]


def test_decision_random_candidates_flag_and_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health
    from agentnave.decision_seats import (
        DecisionSeatsFileError,
        random_candidates_enabled,
        read_decision_seats,
    )
    from agentnave.members import role_seat
    from agentnave.seat_fallbacks import select

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(AdapterHooks, "is_installed", _installed_cli)
    host = Host("codex", "env")
    seat = _custom_seat(
        "own",
        "claude",
        "write",
        fallbacks=[
            {"provider": "grok", "model": "g-1", "effort": "low"},
            {"provider": "claude", "model": "c-2"},
        ],
    )
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))
    _write_decision_seats(tmp_path, [seat])
    assert read_decision_seats() is not None and not random_candidates_enabled(host)
    for bad in (1, "true", None):
        (tmp_path / "decision-seats.json").write_text(
            json.dumps({"version": 1, "random_candidates": bad, "seats": [seat]})
        )
        with pytest.raises(DecisionSeatsFileError):
            read_decision_seats()
    (tmp_path / "decision-seats.json").write_text(
        json.dumps({"version": 1, "random_candidates": True, "seats": [seat]})
    )
    assert random_candidates_enabled(host) is True
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "split"}))
    assert random_candidates_enabled(host) is False
    (tmp_path / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "decision"}))

    # list_members stays deterministic even while random_candidates is enabled.
    listed = list_members(frozenset({"codex"}), host)
    assert listed.orchestration.custom_seats["own"].model == "shared"
    assert listed.orchestration.custom_seats["own"].fallback_position is None

    configured = role_seat("own", host)
    assert configured is not None
    provider_health.record(tmp_path, "grok", "quota", "g-1")
    seen_counts: list[int] = []
    first, first_receipt = select(
        configured,
        host,
        frozenset({"codex"}),
        randomize=True,
        randrange=lambda n: (seen_counts.append(n), 0)[1],
    )
    second, second_receipt = select(
        configured,
        host,
        frozenset({"codex"}),
        randomize=True,
        randrange=lambda n: (seen_counts.append(n), 1)[1],
    )
    assert seen_counts == [2, 2]
    assert first.model == "shared" and second.model == "c-2"
    assert first_receipt is not None and first_receipt.get("random") is True
    assert first_receipt["position"] == 0 and first_receipt["skipped"][0]["model"] == "g-1"
    assert second_receipt is not None and second_receipt.get("random") is True
    assert second_receipt["position"] == 2

    # In-use candidates on the same seat are skipped while idle ones exist; once all usable
    # candidates are in use, selection falls back to drawing across every usable candidate.
    idle_only, idle_receipt = select(
        configured,
        host,
        frozenset({"codex"}),
        randomize=True,
        in_use={("claude", None, "shared")},
        randrange=lambda _n: 99,
    )
    assert idle_only.model == "c-2" and idle_receipt is not None
    assert idle_receipt["position"] == 2 and idle_receipt.get("random") is True

    seen_all: list[int] = []
    all_busy_0, all_busy_0_receipt = select(
        configured,
        host,
        frozenset({"codex"}),
        randomize=True,
        in_use={("claude", None, "shared"), ("claude", None, "c-2")},
        randrange=lambda n: (seen_all.append(n), 0)[1],
    )
    all_busy_1, all_busy_1_receipt = select(
        configured,
        host,
        frozenset({"codex"}),
        randomize=True,
        in_use={("claude", None, "shared"), ("claude", None, "c-2")},
        randrange=lambda n: (seen_all.append(n), 1)[1],
    )
    assert seen_all == [2, 2]
    assert all_busy_0.model == "shared" and all_busy_1.model == "c-2"
    assert all_busy_0_receipt is not None and all_busy_0_receipt.get("random") is True
    assert all_busy_1_receipt is not None and all_busy_1_receipt.get("random") is True

    # Only one usable candidate left: no random draw, so random is omitted.
    provider_health.record(tmp_path, "claude", "auth", "shared")
    sole, sole_receipt = select(
        configured, host, frozenset({"codex"}), randomize=True, randrange=lambda _n: 99
    )
    assert sole.model == "c-2" and sole_receipt is not None
    assert sole_receipt["position"] == 2 and "random" not in sole_receipt


def test_app_files_naming_a_retired_cli_are_read_without_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    claude = {"provider": "claude", "model": "opus"}
    devin = {"provider": "devin", "model": "swe"}
    files: dict[str, dict[str, Any]] = {
        "members.json": {
            "version": 1,
            "clis": {"devin": {"models": {"swe": {}}}, "codex": {"models": {}}},
            "modes": {"task": {"devin": {"models": ["swe"]}, "codex": {"models": []}}},
            "defaults": {"discussion": {"devin": "swe"}},
        },
        "permissions.json": {"version": 1, "clis": {"devin": "auto", "codex": "auto"}},
        "review.json": {
            "version": 1,
            "seats": {
                "security-reviewer": {**devin, "fallbacks": [claude]},
                "performance-reviewer": {**devin, "fallbacks": [devin | {"model": "x"}]},
            },
        },
        "orchestration.json": {
            "version": 1,
            "explorer": {"easy": devin},
            "image_generation": devin,
        },
        "decision-seats.json": {
            "version": 1,
            "seats": [
                {
                    "id": "scout",
                    "kind": "custom",
                    "role": "Scout",
                    "description": "Reads",
                    "access": "read",
                    "difficulty": "easy",
                    **devin,
                    "fallbacks": [claude],
                },
                {
                    "id": "only-devin",
                    "kind": "custom",
                    "role": "Gone",
                    "description": "Gone",
                    "access": "read",
                    "difficulty": "easy",
                    **devin,
                },
            ],
        },
    }
    for name, document in files.items():
        (tmp_path / name).write_text(json.dumps(document))

    members = read_members()
    assert members is not None
    assert set(members.clis) == {"codex"} and set(members.modes["task"]) == {"codex"}
    assert members.defaults["discussion"] == {}
    permissions = read_permissions()
    assert permissions is not None and permissions.clis == {"codex": "auto"}
    review = _read_app_file("review.json", ReviewFile, MemberFileError)
    assert review is not None
    # The next remaining choice takes the retired primary's place; a seat with none is unbound.
    assert set(review.seats) == {"security-reviewer"}
    assert review.seats["security-reviewer"].provider == "claude"
    assert review.seats["security-reviewer"].fallbacks == []
    orchestration = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    assert orchestration is not None
    assert orchestration.explorer == {} and orchestration.image_generation is None
    decision = _read_app_file("decision-seats.json", DecisionSeatsFile, DecisionSeatsFileError)
    assert decision is not None and [(s.id, s.provider) for s in decision.seats] == [
        ("scout", "claude")
    ]


def _t3_seat_dict(seat_id: str, **extra: Any) -> dict[str, Any]:
    return {
        "id": seat_id,
        "kind": "custom",
        "role": f"Role {seat_id}",
        "description": f"Role {seat_id}",
        "access": "write",
        "difficulty": "medium",
        "candidates": [
            {
                "instance": "inst-a",
                "model": "model-a",
                "options": [{"id": "effort", "value": "high"}],
            }
        ],
        **extra,
    }


def test_t3_seats_file_strict_validation_and_activation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import t3
    from agentnave.t3_seats import (
        T3SeatsFile,
        T3SeatsFileError,
        effective_t3_seats,
        read_t3_seats,
        t3_orchestration_active,
    )

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    codex = Host("codex", "env")

    # Missing file -> inactive.
    assert read_t3_seats() is None
    monkeypatch.setenv("T3CODE_THREAD_ID", "main-1")
    t3.reset_t3_mode_cache()
    assert effective_t3_seats(codex) is None
    assert not t3_orchestration_active(codex)

    # Valid file with task seat, 0-candidate draft seat, image seat, and review role.
    valid: dict[str, Any] = {
        "version": 1,
        "enabled": True,
        "random_candidates": True,
        "seats": [
            _t3_seat_dict("coder"),
            _t3_seat_dict("draft", candidates=[]),
            {
                "id": "artist",
                "kind": "image",
                "role": "画图",
                "description": "生成插画",
                "access": "write",
                "candidates": [{"instance": "inst-b", "model": "img-1", "options": []}],
            },
        ],
        "review": {
            "correctness-reviewer": [{"instance": "inst-c", "model": "rev-1", "options": []}]
        },
    }
    doc = T3SeatsFile.model_validate(valid)
    assert doc.is_active()

    # Active when only review has candidates; inactive when enabled=False or all 0 candidates.
    review_only = T3SeatsFile.model_validate(
        {
            "version": 1,
            "enabled": True,
            "seats": [_t3_seat_dict("draft", candidates=[])],
            "review": {"security-reviewer": [{"instance": "inst-c", "model": "rev-1"}]},
        }
    )
    assert review_only.is_active()
    assert not T3SeatsFile.model_validate({**valid, "enabled": False}).is_active()
    assert not T3SeatsFile.model_validate(
        {
            "version": 1,
            "enabled": True,
            "seats": [_t3_seat_dict("draft", candidates=[])],
            "review": {"security-reviewer": []},
        }
    ).is_active()

    # Forbidden option ids (case-insensitive) and option count / value rules.
    for forbidden in (
        "mode",
        "sandbox",
        "multitask",
        "permission",
        "permissionMode",
        "approvalPolicy",
        "runtimeMode",
        "fastMode",
        "ultracode",
        "serviceTier",
        "cyberAccessProgram",
        "SANDBOX",
        "PermissionMode",
    ):
        with pytest.raises(ValidationError, match="not allowed"):
            T3SeatsFile.model_validate(
                {
                    "version": 1,
                    "seats": [
                        _t3_seat_dict(
                            "bad-opt",
                            candidates=[
                                {
                                    "instance": "i",
                                    "model": "m",
                                    "options": [{"id": forbidden, "value": "x"}],
                                }
                            ],
                        )
                    ],
                }
            )

    invalid_docs: list[dict[str, Any]] = [
        {**valid, "extra_field": 1},  # StrictModel forbids extra keys
        {**valid, "version": 2},
        {**valid, "version": True},
        {"version": 1, "seats": [_t3_seat_dict("dup"), _t3_seat_dict("dup", role="Other")]},
        {"version": 1, "seats": [_t3_seat_dict("correctness-reviewer")]},  # review role clash
        {"version": 1, "review": {"unknown-reviewer": [{"instance": "i", "model": "m"}]}},
        {
            "version": 1,
            "seats": [
                _t3_seat_dict(
                    "too-many",
                    candidates=[{"instance": f"i{n}", "model": "m"} for n in range(5)],
                )
            ],
        },
        {
            "version": 1,
            "review": {
                "correctness-reviewer": [{"instance": f"i{n}", "model": "m"} for n in range(5)]
            },
        },
        {
            "version": 1,
            "seats": [_t3_seat_dict("ws-inst", candidates=[{"instance": " i ", "model": "m"}])],
        },
        {
            "version": 1,
            "seats": [_t3_seat_dict("ws-model", candidates=[{"instance": "i", "model": " m"}])],
        },
        {
            "version": 1,
            "seats": [
                _t3_seat_dict(
                    "two-opts",
                    candidates=[
                        {
                            "instance": "i",
                            "model": "m",
                            "options": [
                                {"id": "effort", "value": "high"},
                                {"id": "reasoningEffort", "value": "max"},
                            ],
                        }
                    ],
                )
            ],
        },
        {
            "version": 1,
            "seats": [
                _t3_seat_dict(
                    "empty-val",
                    candidates=[
                        {
                            "instance": "i",
                            "model": "m",
                            "options": [{"id": "effort", "value": "   "}],
                        }
                    ],
                )
            ],
        },
        # Reused CustomSeat metadata checks: explorer group is read-only; image has no difficulty.
        {"version": 1, "seats": [_t3_seat_dict("bad-group", kind="explorer", access="write")]},
        {
            "version": 1,
            "seats": [_t3_seat_dict("bad-img", kind="image", access="write", difficulty="easy")],
        },
    ]
    for bad in invalid_docs:
        with pytest.raises(ValidationError):
            T3SeatsFile.model_validate(bad)

    # Broken t3-seats.json raises T3SeatsFileError in T3 main thread, but does not affect non-T3.
    (tmp_path / "t3-seats.json").write_text("{broken json", encoding="utf-8")
    with pytest.raises(T3SeatsFileError, match="t3-seats.json"):
        effective_t3_seats(codex)
    monkeypatch.delenv("T3CODE_THREAD_ID")
    t3.reset_t3_mode_cache()
    assert effective_t3_seats(codex) is None


def test_t3_cache_filtering_unknown_passthrough_path_safety_and_privacy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from datetime import UTC, datetime

    from agentnave.t3_cache import check_instance

    home = tmp_path / "t3-home"
    caches = home / "caches"
    caches.mkdir(parents=True)
    monkeypatch.setenv("T3CODE_HOME", str(home))
    now = datetime(2026, 11, 24, 10, 0, 0, tzinfo=UTC)

    def write(name: str, payload: object) -> None:
        (caches / f"{name}.json").write_text(json.dumps(payload), encoding="utf-8")

    # Missing, corrupt JSON, non-object, wrong types -> unknown (not excluded).
    assert check_instance("missing", "m", now=now).available is True
    (caches / "corrupt.json").write_text("{not-json", encoding="utf-8")
    assert check_instance("corrupt", "m", now=now).available is True
    write("array", [1, 2, 3])
    assert check_instance("array", "m", now=now).available is True
    write(
        "wrong-types",
        {
            "driver": "claudeAgent",
            "enabled": "false",
            "installed": 0,
            "status": 123,
            "auth": {"status": 0, "email": "hidden@example.com"},
            "usageLimits": {
                "windows": [
                    {"id": "five_hour", "usedPercent": True, "resetsAt": "2026-11-24T12:00:00Z"},
                    {"id": "seven_day", "usedPercent": "100"},
                    "nope",
                    {"usedPercent": 100},
                ]
            },
        },
    )
    assert check_instance("wrong-types", "m", now=now).available is True

    # Path traversal and symlink outside caches/ -> treated as unknown, never read.
    outside = tmp_path / "escape.json"
    outside.write_text(json.dumps({"enabled": False}), encoding="utf-8")
    for bad_name in ("../escape", "a/b", "a\\b", "..", ".", "bad\x00name"):
        assert check_instance(bad_name, "m", now=now).available is True
    link = caches / "sym.json"
    link.symlink_to(outside)
    assert check_instance("sym", "m", now=now).available is True

    # Explicit exclusions (and auth.email never leaks).
    write("disabled", {"enabled": False})
    assert not check_instance("disabled", "m", now=now).available
    write("uninstalled", {"enabled": True, "installed": False})
    assert not check_instance("uninstalled", "m", now=now).available
    write("errored", {"enabled": True, "installed": True, "status": "error"})
    assert not check_instance("errored", "m", now=now).available
    write("status-disabled", {"status": "disabled"})
    assert not check_instance("status-disabled", "m", now=now).available
    write(
        "unauth",
        {
            "driver": "claudeAgent",
            "enabled": True,
            "installed": True,
            "status": "ready",
            "auth": {"status": "unauthenticated", "email": "private-user@example.com"},
        },
    )
    unauth = check_instance("unauth", "m", now=now)
    assert not unauth.available
    assert "private-user@example.com" not in (unauth.reason or "")

    # Quota rules: claudeAgent checks five_hour/seven_day; codex checks primary/secondary.
    write(
        "claude-full",
        {
            "driver": "claudeAgent",
            "enabled": True,
            "installed": True,
            "status": "ready",
            "auth": {"status": "authenticated", "email": "secret@example.com"},
            "usageLimits": {
                "windows": [
                    {"id": "five_hour", "usedPercent": 100, "resetsAt": "2026-11-24T12:00:00Z"},
                    {"id": "seven_day", "usedPercent": 105, "resetsAt": "2026-11-26T00:00:00Z"},
                ]
            },
        },
    )
    claude_full = check_instance("claude-full", "m", now=now)
    assert not claude_full.available
    assert claude_full.resets_at == datetime(2026, 11, 26, 0, 0, 0, tzinfo=UTC)
    assert "secret@example.com" not in (claude_full.reason or "")

    # Past reset or usedPercent < 100 -> not excluded.
    write(
        "claude-expired",
        {
            "driver": "claudeAgent",
            "usageLimits": {
                "windows": [
                    {"id": "five_hour", "usedPercent": 100, "resetsAt": "2026-11-24T09:59:59Z"},
                    {"id": "seven_day", "usedPercent": 99.9, "resetsAt": "2026-11-26T00:00:00Z"},
                ]
            },
        },
    )
    assert check_instance("claude-expired", "m", now=now).available is True

    write(
        "codex-full",
        {
            "driver": "codex",
            "usageLimits": {
                "windows": [
                    {"id": "primary", "usedPercent": 100, "resetsAt": "2026-11-24T11:30:00Z"},
                ]
            },
        },
    )
    codex_full = check_instance("codex-full", "m", now=now)
    assert not codex_full.available
    assert codex_full.resets_at == datetime(2026, 11, 24, 11, 30, 0, tzinfo=UTC)


@pytest.mark.parametrize(
    ("driver", "windows", "stopped", "free"),
    [
        # A scoped Claude week stops its own model; overage spend stops none.
        ("claudeAgent", ["seven_day_opus"], ["claude-opus-5-5"], ["claude-sonnet-5-5"]),
        ("claudeAgent", ["monthly_spend"], [], ["claude-opus-5-5"]),
        # The Codex credit cap only matters without plan windows.
        ("codex", ["individual"], ["gpt-6-luna"], []),
        ("codex", ["individual", "-primary"], [], ["gpt-6-luna"]),
        ("grok", ["subscription"], ["grok-4.6"], []),
        (
            "antigravity",
            ["gemini-weekly"],
            ["gemini-4-neon"],
            ["claude-opus-5-5", "gemini-4-argon"],
        ),
        ("antigravity", ["gemini-argon-5h"], ["gemini-4-argon"], ["gemini-4-neon"]),
        ("antigravity", ["3p-weekly"], ["claude-opus-5-5"], ["gemini-4-neon"]),
        ("cursor", ["apiPercentUsed"], ["gpt-6-luna"], ["auto"]),
        ("cursor", ["apiPercentUsed", "autoPercentUsed"], ["auto"], []),
        ("opencode", ["go_weekly"], ["opencode-go/x"], ["openai/y"]),
        ("muse", ["weekly"], ["anything"], []),
        ("opencode", ["scoped@a/b"], ["a/b"], ["a/c"]),
    ],
)
def test_t3_quota_stops_only_the_models_each_full_window_covers(
    driver: str, windows: list[str], stopped: list[str], free: list[str]
) -> None:
    from datetime import UTC, datetime

    from agentnave.t3_quota import exhausted_windows

    limits = {
        "windows": [
            {
                "id": name.removeprefix("-").split("@")[0],
                # A leading "-" is a window with room left; "@" scopes it to one model.
                "usedPercent": 10 if name.startswith("-") else 100,
                **({"modelIds": [name.split("@")[1]]} if "@" in name else {}),
            }
            for name in windows
        ]
    }
    now = datetime(2026, 11, 24, 10, tzinfo=UTC)
    assert all(exhausted_windows(driver, limits, model, now) for model in stopped)
    assert not any(exhausted_windows(driver, limits, model, now) for model in free)
