import ast
from pathlib import Path
from typing import cast

import pytest

from agentnave.models import InvocationRequest


def test_core_never_imports_studio() -> None:
    core = Path(__file__).parents[1] / "src" / "agentnave"
    for path in core.rglob("*.py"):
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            names = (
                [alias.name for alias in node.names]
                if isinstance(node, ast.Import)
                else [node.module or ""]
                if isinstance(node, ast.ImportFrom)
                else []
            )
            assert not any(name.split(".")[0] == "agentnave_studio" for name in names), path


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
