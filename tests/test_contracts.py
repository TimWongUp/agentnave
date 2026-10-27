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
async def test_real_suite_reads_its_own_history_and_checks_the_recalled_token(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sys
    from types import SimpleNamespace

    from mcp import Client

    monkeypatch.setattr(sys, "path", [str(Path(__file__).parents[1]), *sys.path])
    from scripts.real_suite import Suite, ToolFailure

    calls: list[tuple[str, dict[str, object]]] = []
    reply = ""

    async def call_tool(name: str, arguments: dict[str, object], **_kwargs: object) -> object:
        calls.append((name, arguments))
        if name == "start_agent":
            content = {"conversation_id": "suite-task", "invocation_id": "invocation"}
        elif name == "wait":
            update = {"status": "succeeded", "output": reply, "session_id": "session-1"}
            content = {"updates": [update]}
        elif name == "list":
            content = {
                "dashboard_url": "http://localhost/",
                "conversations": [
                    {"conversation_id": "user-task", "title": "User history"},
                    {"conversation_id": "suite-task", "title": "Suite task"},
                ],
            }
        elif name == "read_conversation":
            content = {"conversation_id": arguments["conversation_id"], "tasks": ["saved"]}
        else:
            content = {}
        return SimpleNamespace(is_error=False, structured_content=content)

    suite = Suite(cast("Client", SimpleNamespace(call_tool=call_tool)), tmp_path, ["pi"])
    await suite.start("pi", "hello")
    assert calls[0][1]["role"] == "real-suite"
    await suite.history()
    read = [
        arguments["conversation_id"] for name, arguments in calls if name == "read_conversation"
    ]
    assert read == ["suite-task"]

    # Resume passes the session back without the answer and accepts only the exact token.
    suite.sessions["pi"] = ("session-1", "PI-1234ABCD")
    reply = "PI-1234ABCD\n"
    assert await suite.resume("pi") is None
    resumed = [arguments for name, arguments in calls if name == "start_agent"][-1]
    assert resumed["session_id"] == "session-1"
    assert "PI-1234ABCD" not in str(resumed["prompt"])
    reply = "PI-1234ABCE"
    with pytest.raises(ToolFailure, match="did not recall"):
        await suite.resume("pi")
    assert "pi" not in suite.sessions  # a failed recall leaves no session to reuse
