import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent


def test_meeting_skill_matches_the_manager_parts_it_shares() -> None:
    # agentnave-meeting is built from agentnave-manager; editing one without the other drifts.
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "sync_skills.py"), "--check"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_a_lost_shared_marker_fails_the_check(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import shutil

    spec = importlib.util.spec_from_file_location(
        "sync_skills", ROOT / "scripts" / "sync_skills.py"
    )
    assert spec and spec.loader
    sync = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(sync)
    target = tmp_path / "agentnave-meeting"
    shutil.copytree(sync.TARGET, target)
    skill = target / "SKILL.md"
    text = skill.read_text(encoding="utf-8")
    skill.write_text(text.replace("<!-- shared:closing -->\n", "", 1), encoding="utf-8")
    monkeypatch.setattr(sync, "TARGET", target)
    with pytest.raises(SystemExit, match="closing"):
        sync.built()
