from __future__ import annotations

import pytest

from agentnave.adapters import base


@pytest.fixture(autouse=True)
def no_common_provider_dirs(monkeypatch: pytest.MonkeyPatch) -> None:
    # Never let a test fall back to a real provider CLI installed on this machine.
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ())


@pytest.fixture(autouse=True)
def manager_chat(monkeypatch: pytest.MonkeyPatch) -> None:
    # start_agent and run_round need a boss chat identity; tests run as one injected chat.
    monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "test-chat")
    # Running the suite inside Claude Code must not hand tests that chat's native identity.
    monkeypatch.delenv("CLAUDE_CODE_SESSION_ID", raising=False)
