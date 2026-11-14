import subprocess
import sys
from pathlib import Path

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
