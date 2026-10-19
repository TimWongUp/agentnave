from __future__ import annotations

import pytest

from agentnave.adapters import base


@pytest.fixture(autouse=True)
def no_common_provider_dirs(monkeypatch: pytest.MonkeyPatch) -> None:
    # Never let a test fall back to a real provider CLI installed on this machine.
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ())
