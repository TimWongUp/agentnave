from pathlib import Path

import pytest


@pytest.fixture(autouse=True)
def isolated_workbench_storage(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "workbench-data"))
