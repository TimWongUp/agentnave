import tomllib
from pathlib import Path
from typing import cast

import pytest

from agentnave.models import InvocationRequest


def test_single_package_and_console_entrypoint() -> None:
    root = Path(__file__).parents[1]
    assert not (root / "src" / "agentnave_studio").is_dir()
    project = tomllib.loads((root / "pyproject.toml").read_text(encoding="utf-8"))["project"]
    assert project["scripts"] == {"agentnave-mcp": "agentnave.relay:main"}


def test_request_normalizes_provider_and_cwd(tmp_path: Path) -> None:
    request = InvocationRequest(" Claude ", "task", tmp_path)

    assert request.provider == "claude"
    assert request.cwd == tmp_path.resolve()


@pytest.mark.parametrize("cwd", [Path("relative"), Path("~"), Path("~missing-user")])
def test_request_rejects_non_absolute_cwd(cwd: Path) -> None:
    with pytest.raises(ValueError, match="absolute"):
        InvocationRequest("claude", "task", cwd)


@pytest.mark.parametrize("session_id", ["--always-approve", "valid\n--flag"])
def test_request_rejects_session_option_injection(tmp_path: Path, session_id: str) -> None:
    with pytest.raises(ValueError, match="unsafe"):
        InvocationRequest("grok", "task", tmp_path, session_id=session_id)


def test_request_copies_and_freezes_provider_options(tmp_path: Path) -> None:
    options = {"model": "first"}
    request = InvocationRequest("claude", "task", tmp_path, provider_options=options)

    options["model"] = "changed"

    assert request.provider_options["model"] == "first"
    with pytest.raises(TypeError):
        cast(dict[str, str | int | float | bool], request.provider_options)["model"] = "third"


@pytest.mark.anyio
async def test_real_suite_history_uses_created_ids_after_readable_titles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sys
    from types import SimpleNamespace

    from mcp import Client

    monkeypatch.setattr(sys, "path", [str(Path(__file__).parents[1]), *sys.path])
    from scripts.real_suite import Suite

    calls: list[tuple[str, dict[str, object]]] = []

    async def call_tool(name: str, arguments: dict[str, object], **_kwargs: object) -> object:
        calls.append((name, arguments))
        if name == "start_agent":
            content = {"conversation_id": "suite-task", "invocation_id": "invocation"}
        elif name == "list_conversations":
            content = {
                "dashboard_url": "http://localhost/",
                "conversations": [
                    {"conversation_id": "user-task", "title": "User history"},
                    {"conversation_id": "suite-task", "title": "连通检查 · pi"},
                ],
            }
        elif name == "update_conversation":
            content = {"title": arguments["title"]}
        else:
            content = {}
        return SimpleNamespace(is_error=False, structured_content=content)

    suite = Suite(cast("Client", SimpleNamespace(call_tool=call_tool)), tmp_path, ["pi"])
    await suite.start("pi", "hello")
    assert calls[0][1]["title"] == "连通检查 · pi"
    assert calls[0][1]["role"] == "real-suite"
    await suite.history()
    assert all(
        arguments["conversation_id"] == "suite-task"
        for name, arguments in calls
        if name in {"read_conversation", "update_conversation"}
    )
