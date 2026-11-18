from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

from agentnave.adapters.antigravity import AntigravityAdapter
from agentnave.agy_tui import Run, Turn, settings
from agentnave.agy_tui.screen import AgyScreen, busy_activity, input_box, state
from agentnave.claude_tui.terminal import Terminal
from agentnave.models import InvocationRequest, InvocationStatus

FAKE_AGY = r"""
import json, os, select, subprocess, sys, time, tty
from pathlib import Path

if "--version" in sys.argv:
    print("1.5.11")
    sys.exit(0)
tty.setraw(0)
case = os.environ["FAKE_AGY_CASE"]
args = sys.argv[1:]
settings = json.loads(args[args.index("--settings") + 1])
assert os.getcwd() in settings["trustedWorkspaces"]
assert "--dangerously-skip-permissions=true" in args
cid = args[args.index("--conversation") + 1] if "--conversation" in args else "11111111-2222-3333-4444-555555555555"
home = Path(os.environ["HOME"])
assert os.environ["AGY_CLI_OSC7501"] == "on"
assert os.environ["AGY_CLI_OSC7501_MESSAGES"] == "brief"
transcript = home / ".gemini/antigravity-cli/brain" / cid / ".system_generated/logs/transcript_full.jsonl"
transcript.parent.mkdir(parents=True, exist_ok=True)
log = Path(args[args.index("--log-file") + 1])
log.write_text("Experiments refreshed after login\n")

def output(text):
    os.write(1, text.encode())

def read_until(marker):
    data = b""
    deadline = time.monotonic() + 12
    while marker not in data and time.monotonic() < deadline:
        if select.select([0], [], [], 0.1)[0]:
            data += os.read(0, 65536)
    assert marker in data, (marker, data)
    return data

def row(**fields):
    with transcript.open("a") as f:
        f.write(json.dumps({"status": "DONE", **fields}) + "\n")

rule = "─" * 40

def box(text):
    output("\x1b[2J\x1b[H" + "\r\n".join(["Antigravity CLI", rule, text, rule, "? for shortcuts"]))

if case in ("trust", "trust_mismatch"):
    folder = os.getcwd() if case == "trust" else str(Path(os.getcwd()).parent)
    output("Accessing workspace: " + folder + "\r\nDo you trust the contents of this project?\r\n> Yes, I trust this folder\r\n  No, exit\r\n↑/↓ Navigate · enter Confirm")
    read_until(b"\r")
elif case == "dialog":
    # The words of a menu in the conversation above the box are not a menu.
    output("↑/↓ Navigate · enter Confirm\r\nDo you trust this folder?\r\n> Yes\r\n  No\r\n  ↑/↓ Navigate · enter Confirm")
    time.sleep(12)
box(">")
pasted = read_until(b"\x1b[201~")
assert b"\x1b[200~" in pasted and b"work" in pasted
text = pasted.split(b"\x1b[200~")[1].split(b"\x1b[201~")[0].decode()
if case == "resume":
    # A redraw can leave the last request, with the same start, as the last box on screen.
    output("\x1b[2J\x1b[H" + "\r\n".join(["Antigravity CLI", rule, "> " + text[:40] + " on the previous task", rule]))
    early = b""
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        if select.select([0], [], [], 0.1)[0]:
            early += os.read(0, 65536)
    assert b"\r" not in early, early
if case == "scrolled":
    # agy scrolls an unfolded paste taller than the box to its end; wrapping splits a line.
    lines = text.split("\n")
    shown = [*lines[-6:-1], lines[-1][:10], lines[-1][10:]]
    box("> ↑ 3 more lines\r\n" + "\r\n".join("  " + line for line in shown))
else:
    box("> " + " ".join(text.split()))
read_until(b"\r")
row(step_index=10, source="USER_EXPLICIT", type="USER_INPUT", content=text)
if case != "resume":
    with log.open("a") as f:
        f.write(f"Created conversation {cid}\n")
output("\x1b]7501;state=working:app=antigravity\x07")
row(step_index=11, source="MODEL", type="PLANNER_RESPONSE", content="looking", tool_calls=[{"name": "run_command"}])
row(step_index=12, source="MODEL", type="GENERIC", content="ran")
if case != "noreply":
    row(step_index=13, source="MODEL", type="PLANNER_RESPONSE", content="finished", model="fake-model")
output("\x1b]7501;state=done:app=antigravity\x1b\\")
time.sleep(12)
"""
PROMPTS = {
    "resume": "work through the same start as every turn on the resumed task",
    "scrolled": "work begins on a line that scrolls away\n"
    + "\n".join(f"step {i}" for i in range(7))
    + "\n最后这段中文在输入框里折成两行显示出来",
}


@pytest.mark.skipif(os.name == "nt", reason="POSIX PTY runtime")
@pytest.mark.parametrize(
    "case", ["new", "resume", "scrolled", "dialog", "noreply", "trust", "trust_mismatch"]
)
def test_fake_agy_turn_through_the_standalone_helper(
    case: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "agy"
    executable.write_text(f"#!{sys.executable}\n" + FAKE_AGY)
    executable.chmod(0o755)
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("FAKE_AGY_CASE", case)
    hook = home / ".gemini/config/hooks.json"
    if case == "new":
        hook.parent.mkdir(parents=True)
        hook.write_text("{unparseable existing config")
    sid = None
    if case == "resume":
        sid = "old-conversation"
        old = home / ".gemini/antigravity-cli/brain" / sid / ".system_generated/logs"
        old.mkdir(parents=True)
        reply = {"type": "PLANNER_RESPONSE", "source": "MODEL", "content": "earlier"}
        (old / "transcript_full.jsonl").write_text(json.dumps(reply) + "\n")
    adapter = AntigravityAdapter()
    monkeypatch.setattr(adapter, "executable_path", lambda: str(executable))
    options = {"interactive": True, "dangerously_skip_permissions": True}
    prompt = PROMPTS.get(case, "work")
    command = adapter.prepare(
        InvocationRequest("antigravity", prompt, tmp_path, session_id=sid, provider_options=options)
    )
    assert command.model_from_result_only
    completed = subprocess.run(
        command.argv, input=command.stdin, capture_output=True, cwd=tmp_path, timeout=60
    )
    assert completed.stderr == b"", completed.stderr.decode()
    capture = command.capture_line
    assert capture is not None
    retained = b"".join(capture(line) for line in completed.stdout.splitlines())
    parsed = adapter.parse(completed.returncode, retained, completed.stderr)
    events = [cast(dict[str, object], json.loads(line)) for line in completed.stdout.splitlines()]
    if case == "dialog":
        assert parsed.status is InvocationStatus.BLOCKED
        assert "unexpected startup dialog" in str(parsed.error_message)
        return
    if case == "trust_mismatch":
        assert parsed.status is InvocationStatus.BLOCKED
        assert "does not match cwd" in str(parsed.error_message)
        return
    if case == "noreply":
        # The last response still calls a tool: a Stop then is no finished turn.
        assert parsed.status is InvocationStatus.FAILED
        assert "without a final reply" in str(parsed.error_message)
        return
    assert parsed.status is InvocationStatus.SUCCEEDED, completed.stdout.decode()
    assert parsed.output == "finished"
    assert parsed.session_id == (sid or "11111111-2222-3333-4444-555555555555")
    assert any(adapter.event_model(event) == "fake-model" for event in events)
    assert "earlier" not in completed.stdout.decode()
    tools = [adapter.activity(event) for event in events]
    assert [a.state for a in tools if a is not None and a.kind == "tool"] == ["ACTIVE", "DONE"]
    if case == "new":
        assert hook.read_text() == "{unparseable existing config"
    else:
        assert not hook.exists()


def test_settings_trust_the_pwd_spelling_of_the_same_folder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    folder = tmp_path / "Workbuddy"
    folder.mkdir()
    # agy reads $PWD, which can name this folder with another case than the disk does.
    alias = tmp_path / "WorkBuddy"
    try:
        alias.symlink_to(folder)
    except OSError:
        pytest.skip("symlinks need extra privileges here")

    def trusted() -> list[str]:
        saved = cast(dict[str, list[str]], json.loads(settings(str(folder))))
        return saved["trustedWorkspaces"]

    monkeypatch.setenv("PWD", str(alias))
    assert str(alias) in trusted()
    monkeypatch.setenv("PWD", str(tmp_path))
    assert str(tmp_path) not in trusted()
    monkeypatch.delenv("PWD", raising=False)
    assert sorted({str(folder), os.path.realpath(folder)}) == trusted()


@pytest.mark.parametrize(
    ("line", "expected"),
    [
        ("⣷  Generating...", "thinking"),
        ("⢿  Initial concern about the use of a shell `sleep` command arose ...", "thinking"),
        ("⣻  Running command...", "command_execution"),
        ("⢿  Reading file...", "read_file"),
    ],
)
def test_busy_activity_reads_recorded_status_only(line: str, expected: str) -> None:
    rule = "─" * 40
    screen = f"● Running (sleep 6)\n○ Thought for 3.7s\n{line}\n└ Tip: something\n{rule}\n>\n{rule}\nesc to cancel"
    assert busy_activity(screen) == expected
    assert busy_activity(screen.replace(line, "● Running (sleep 6)")) is None
    assert busy_activity(screen.replace("esc to cancel", "? for shortcuts")) is None
    assert (
        busy_activity(
            screen.replace("esc to cancel", f"background task\n{rule}\nesc to cancel · 1 task(s)")
        )
        == expected
    )


def test_busy_activity_updates_only_when_screen_changes(capsys: pytest.CaptureFixture[str]) -> None:
    turn = Turn("c", user_seen=True)
    turn.watch("busy", "thinking")
    turn.watch("busy", "thinking")
    turn.watch("busy", "command_execution")
    turn.row({"type": "GENERIC", "source": "MODEL"})
    turn.watch("busy", "command_execution")
    # An interim reply while a background call runs does not hide what agy does next.
    turn.answered = True
    turn.watch("busy", "read_file")
    turn.stopped = True
    turn.watch("busy", "thinking")
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_type"], s.get("tool_name")) for s in steps] == [
        ("thinking", None),
        ("tool", "command_execution"),
        ("tool", "read_file"),
    ]


def test_transcript_calls_take_precedence_over_the_status_line(
    capsys: pytest.CaptureFixture[str],
) -> None:
    turn = Turn("c", user_seen=True)
    turn.row({"type": "PLANNER_RESPONSE", "step_index": 3, "tool_calls": [{"name": "view_file"}]})
    turn.watch("busy", "thinking")  # the screen lags behind the transcript
    turn.row({"type": "GENERIC", "source": "MODEL"})
    turn.watch("busy", "thinking")  # still the same line: no news
    turn.watch("busy", "command_execution")
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_type"], s["state"], s.get("tool_name")) for s in steps] == [
        ("tool", "ACTIVE", "view_file"),
        ("tool", "DONE", None),
        ("tool", "ACTIVE", "command_execution"),
    ]


def test_busy_screen_tool_is_not_overridden_by_background_footer(
    capsys: pytest.CaptureFixture[str],
) -> None:
    turn = Turn("c", user_seen=True, background={"task": 200}, tool_names={200: "run_command"})
    turn.watch("busy", "command_execution")
    turn.watch("busy", "command_execution")
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_type"], s.get("tool_name")) for s in steps] == [("tool", "command_execution")]


@pytest.mark.parametrize("matches", [True, False])
def test_startup_trust_requires_same_directory(
    matches: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import agentnave.agy_tui as agy

    monkeypatch.chdir(tmp_path)
    folder = tmp_path if matches else tmp_path.parent
    menu = f"Accessing workspace: {folder}\nDo you trust the contents of this project?\n> Yes, I trust this folder\n  No, exit\n↑/↓ Navigate · enter Confirm"
    terminal = SimpleNamespace(outgoing=bytearray())
    turn = Turn(None)
    run = Run(cast(Terminal, terminal), turn, tmp_path)
    ticks = iter(range(100))
    monkeypatch.setattr(agy.time, "monotonic", lambda: float(next(ticks)))
    monkeypatch.setattr(run, "exited", lambda: False)
    monkeypatch.setattr(
        run,
        "poll",
        lambda: "─" * 40 + "\n>\n" + "─" * 40 + "\n? for shortcuts" if terminal.outgoing else menu,
    )
    assert run.await_ready() is matches
    assert bytes(terminal.outgoing) == (b"\r" if matches else b"")
    if not matches:
        assert "does not match cwd" in str(turn.error)


def test_trust_menu_is_only_the_bottom_dialog() -> None:
    from agentnave.agy_tui.screen import trust_menu

    real = (
        "Accessing workspace:\n/work\nDo you trust the contents of this project?\n"
        "Antigravity CLI requires permission to read, edit, and execute files here.\n"
        "> Yes, I trust this folder\n  No, exit\n  ↑/↓ Navigate · enter ConfirmGemini 3.8 Flash · high"
    )
    assert trust_menu(real) == "\n".join(line.strip() for line in real.splitlines())
    # The same words quoted above another menu do not make that menu a trust dialog.
    quoted = real.rsplit("\n", 1)[0] + "\n> Allow always\n  Deny\n↑/↓ Navigate · enter Confirm"
    assert trust_menu(quoted) is None


def test_interactive_needs_permissions_skipped_and_no_print_options(tmp_path: Path) -> None:
    adapter = AntigravityAdapter()
    for options in (
        {"interactive": True},
        {"interactive": True, "dangerously_skip_permissions": True, "print_timeout": "5m"},
    ):
        with pytest.raises(ValueError):
            adapter.prepare(
                InvocationRequest("antigravity", "w", tmp_path, provider_options=options)
            )


def test_screen_applies_agy_edits_and_reads_only_the_bottom_lines() -> None:
    screen = AgyScreen()
    # Erase and delete characters, then a reverse index back onto an earlier row.
    screen.feed(b"Ran ing (ls)\r\x1b[4C\x1b[4X\r\nSTARTED-----\x1b[5D\x1b[5P\x1bMtop")
    assert screen.text().splitlines()[:2] == ["Ran    tops)", "STARTED"]
    rule = "─" * 40
    idle = "\n".join(
        ["Do you trust this folder? ↑/↓ Navigate · enter Confirm", rule, "> work", rule]
    )
    assert state(idle + "\n? for shortcuts") == "idle"
    assert input_box(idle + "\n? for shortcuts") == "> work"
    assert state(idle + "\nesc to cancel") == "busy"
    assert state(idle + "\n  ↑/↓ Navigate · enter Confirm") == "menu"
    assert state("Sign in to continue") == "unknown"
    # A running background task draws its line and a rule between the box and the footer.
    tasks = f"\n● [22:43:06] sleep 30 running\n{rule}\n? for shortcuts   model · 1 task(s) · /tasks"
    assert state(f"{rule}\n>\n{rule}{tasks}") == "idle"


def test_experiment_wait_isolates_process_log_and_proceeds_on_timeout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import agentnave.agy_tui as agy

    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    logs = tmp_path / ".gemini/antigravity-cli/log"
    logs.mkdir(parents=True)
    # The shared log must not satisfy the private log's wait, even with a matching PID.
    (logs / "cli-123.log").write_text("Experiments refreshed after login\n")
    idle = "─" * 40 + "\n>\n" + "─" * 40 + "\n? for shortcuts"
    terminal = cast(
        Terminal,
        SimpleNamespace(
            poll=lambda: idle, process=SimpleNamespace(poll=lambda: None), screen=AgyScreen()
        ),
    )
    clock = [0.0]

    def advance() -> float:
        clock[0] += 2.0
        return clock[0]

    monkeypatch.setattr(agy.time, "monotonic", advance)
    own_log = tmp_path / "private.log"
    run = Run(terminal, Turn(None), own_log)
    assert run.await_experiments()
    assert run.turn.error is None and clock[0] >= agy.EXPERIMENT_WAIT
    own_log.write_text("Experiments refreshed after login\n")
    clock[0] = 0.0
    assert run.await_experiments() and clock[0] < agy.EXPERIMENT_WAIT


@pytest.mark.parametrize(
    "shown,steady,blocked",
    [
        ("idle", 30, False),
        ("idle", 61, True),
        ("unknown", 30, False),
        ("unknown", 61, True),
        ("menu", 1, False),
        ("menu", 3, True),
        ("busy", 3600, False),
    ],
)
def test_stall_requires_native_completion(shown: str, steady: float, blocked: bool) -> None:
    turn = Turn("c", response="partial", answered=True)
    run = Run(cast(Terminal, None), turn, Path())
    assert run.stalled(shown, steady, "") is blocked
    assert bool(turn.error) is blocked
    assert not turn.stopped and turn.response == "partial"


@pytest.mark.parametrize("ending", ["finished", "was canceled", "failed"])
def test_background_calls_stay_open_and_the_footer_tells_who_works(
    ending: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Rows as agy 1.5.3 writes them for a command it moves to the background.
    turn = Turn("c", expected="task", user_seen=True)
    turn.row(
        {
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "source": "MODEL",
            "tool_calls": [{"name": "run_command"}],
        }
    )
    turn.row(
        {
            "step_index": 3,
            "type": "GENERIC",
            "source": "MODEL",
            "status": "RUNNING",
            "content": "Tool is running as a background task with task id: c/task-3\n",
        }
    )
    # The footer may become idle one poll before the planner's text is written.
    turn.watch("idle")
    turn.row({"step_index": 5, "type": "PLANNER_RESPONSE", "source": "MODEL", "content": "waiting"})
    turn.watch("idle")
    turn.watch("idle")
    turn.watch("busy")
    turn.row(
        {
            "step_index": 7,
            "type": "SYSTEM_MESSAGE",
            "source": "SYSTEM",
            "content": f'sender=c/task-3 content=Task id "c/task-3" {ending}',
        }
    )
    turn.watch("idle")
    steps = [
        cast(dict[str, object], json.loads(line)["step_update"])
        for line in capsys.readouterr().out.splitlines()
    ]
    shown = [(step["step_type"], step["state"], step.get("tool_name")) for step in steps]
    assert shown == [
        ("tool", "ACTIVE", "run_command"),
        ("tool", "ACTIVE", "run_command"),
        ("tool", "ACTIVE", "run_command"),
        ("thinking", "ACTIVE", None),
        ("tool", "DONE", None),
    ]
    assert not turn.background
    assert not turn.stopped and turn.error is None  # A task's end is not the turn's end.


@pytest.mark.parametrize("footer", ["busy", "idle"])
def test_footer_does_not_override_a_foreground_call(
    footer: str, capsys: pytest.CaptureFixture[str]
) -> None:
    turn = Turn("c", user_seen=True)
    turn.row(
        {
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "source": "MODEL",
            "tool_calls": [{"name": "run_command"}],
        }
    )
    turn.row(
        {
            "type": "GENERIC",
            "source": "MODEL",
            "status": "RUNNING",
            "content": "Tool is running as a background task with task id: c/task-3\n",
        }
    )
    turn.watch("idle" if footer == "busy" else "busy")
    capsys.readouterr()
    # await_stop reads the transcript in poll() before watching the same poll's footer.
    turn.row(
        {
            "step_index": 4,
            "type": "PLANNER_RESPONSE",
            "source": "MODEL",
            "tool_calls": [{"name": "read_file"}],
        }
    )
    turn.watch(footer)
    turn.watch("idle" if footer == "busy" else "busy")
    turn.watch(footer)
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_index"], s["step_type"], s["state"], s.get("tool_name")) for s in steps] == [
        (400, "tool", "ACTIVE", "read_file")
    ]
    # An unchanged footer becomes relevant again once the foreground call ends.
    turn.row({"type": "GENERIC", "source": "MODEL", "content": "read"})
    turn.watch(footer)
    turn.watch(footer)
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    expected = (
        (0, "thinking", "ACTIVE", None)
        if footer == "busy"
        else (200, "tool", "ACTIVE", "run_command")
    )
    assert [(s["step_index"], s["step_type"], s["state"], s.get("tool_name")) for s in steps] == [
        (400, "tool", "DONE", None),
        expected,
    ]


def test_idle_footer_redisplays_a_remaining_background_call_after_another_finishes(
    capsys: pytest.CaptureFixture[str],
) -> None:
    turn = Turn("c", user_seen=True)
    turn.row(
        {
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "source": "MODEL",
            "tool_calls": [{"name": "run_command"}, {"name": "run_command"}],
        }
    )
    for task in ("first", "second"):
        turn.row(
            {
                "type": "GENERIC",
                "source": "MODEL",
                "status": "RUNNING",
                "content": f"Tool is running as a background task with task id: c/{task}\n",
            }
        )
    turn.watch("idle")
    capsys.readouterr()
    turn.row(
        {
            "type": "SYSTEM_MESSAGE",
            "source": "SYSTEM",
            "content": 'sender=c/second content=Task id "c/second" finished with result:',
        }
    )
    turn.watch("idle")
    turn.watch("idle")
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_index"], s["state"], s.get("tool_name")) for s in steps] == [
        (201, "DONE", None),
        (200, "ACTIVE", "run_command"),
    ]


def test_multiple_tools_restore_active_after_first_finishes(
    capsys: pytest.CaptureFixture[str],
) -> None:
    turn = Turn("c", user_seen=True)
    turn.row(
        {
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "tool_calls": [{"name": "read_file"}, {"name": "run_command"}],
        }
    )
    capsys.readouterr()
    turn.row({"type": "GENERIC", "source": "MODEL", "content": "read"})
    steps = [json.loads(line)["step_update"] for line in capsys.readouterr().out.splitlines()]
    assert [(s["step_index"], s["state"], s.get("tool_name")) for s in steps] == [
        (200, "DONE", None),
        (201, "ACTIVE", "run_command"),
    ]


def test_native_status_fragments_and_current_turn_gate() -> None:
    screen = AgyScreen()
    screen.feed(b"\x1b]7501;state=done:app=antig")
    screen.feed(b"ravity\x07\x1b]7501;state=working:app=other\x1b\\")
    assert screen.events == ["done"]
    turn = Turn("resumed")
    turn.native("working")
    turn.native("done")
    turn.expected = "task"
    turn.native("done")
    turn.user_seen = True
    turn.response = "stale"
    turn.answered = True
    assert not turn.complete()
    turn.native("working")
    turn.background["daemon"] = 42
    turn.row({"type": "PLANNER_RESPONSE", "content": "final", "model": "actual"})
    turn.native("done")
    assert not turn.complete()
    assert turn.done_at is not None
    assert turn.complete(turn.done_at + 5) and turn.model == "actual"


def test_native_empty_final_does_not_reuse_previous_answer_or_model() -> None:
    turn = Turn("c", expected="task", user_seen=True)
    turn.row({"type": "PLANNER_RESPONSE", "content": "old", "model": "old-model"})
    turn.row({"type": "PLANNER_RESPONSE", "content": "", "model": "new-model"})
    turn.native("working")
    turn.native("done")
    assert not turn.complete()
    assert not turn.response


def test_native_background_reply_is_not_exposed_before_done(
    capsys: pytest.CaptureFixture[str],
) -> None:
    turn = Turn("c", expected="task", user_seen=True)
    turn.native("working")
    turn.row({"type": "PLANNER_RESPONSE", "step_index": 2, "tool_calls": [{"name": "run"}]})
    turn.row(
        {
            "type": "GENERIC",
            "status": "RUNNING",
            "content": "running as a background task with task id: task",
        }
    )
    turn.row({"type": "PLANNER_RESPONSE", "content": "interim", "model": "old"})
    turn.native("done")
    assert turn.done_at is not None
    assert not turn.complete(turn.done_at + 2)
    assert "interim" not in capsys.readouterr().out
    turn.row({"type": "SYSTEM_MESSAGE", "content": 'Task id "task" finished'})
    assert not turn.complete()
    turn.row({"type": "PLANNER_RESPONSE", "content": "final", "model": "actual"})
    turn.native("done")
    assert turn.complete() and turn.model == "actual"
    assert "final" in capsys.readouterr().out


def test_native_question_and_error_preserve_session_and_partial_output() -> None:
    for event, blocked in (("waiting_for_input", True), ("error", False)):
        turn = Turn("c", expected="task", user_seen=True, response="partial")
        turn.native("working")
        turn.native(event)
        result = cast(dict[str, object], turn.result()["result"])
        assert result["conversation_id"] == "c" and result["response"] == "partial"
        assert ("permission required" in str(result["error"])) is blocked


def test_native_done_deadline_survives_screen_and_transcript_churn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import agentnave.agy_tui as agy

    ticks = iter(float(i) for i in range(100))
    monkeypatch.setattr(agy.time, "monotonic", lambda: next(ticks))
    turn = Turn("c", expected="task", user_seen=True, active=True, done_at=0)
    terminal = cast(Terminal, SimpleNamespace(process=SimpleNamespace(poll=lambda: None)))
    run = Run(terminal, turn, tmp_path / "private.log")

    def changing() -> str:
        turn.offset += 1
        return str(turn.offset)

    monkeypatch.setattr(run, "poll", changing)
    run.await_stop()
    assert "without a final reply" in str(turn.error)


def test_native_completion_restarts_grace_on_coalesced_working_and_done(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import agentnave.agy_tui as agy

    turn = Turn("c", expected="task", user_seen=True, active=True, done_at=1)
    screen = AgyScreen()
    screen.events.extend(("working", "done"))
    terminal = cast(Terminal, SimpleNamespace(poll=lambda: "", screen=screen))
    monkeypatch.setattr(agy.time, "monotonic", lambda: 10.0)
    Run(terminal, turn, tmp_path / "private.log").poll()
    assert turn.done_at == 10.0


def test_old_agy_requires_upgrade_without_starting_tui(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import agentnave.agy_tui as agy

    def version(*args: object, **kwargs: object) -> SimpleNamespace:
        return SimpleNamespace(returncode=0, stdout="1.5.9")

    monkeypatch.setattr(agy.subprocess, "run", version)
    assert agy.run_main("/fake/agy", None, [], "task") == 1
    result = json.loads(capsys.readouterr().out)["result"]
    assert "1.5.10 or later" in result["error"]


def test_private_logs_bind_identical_concurrent_tasks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    ids = ["11111111-2222-3333-4444-555555555555", "aaaaaaaa-2222-3333-4444-555555555555"]
    runs: list[Run] = []
    for index, cid in enumerate(ids):
        log = tmp_path / f"private-{index}.log"
        log.write_text(f"Created conversation {cid}\n")
        screen = AgyScreen()
        terminal = cast(Terminal, SimpleNamespace(screen=screen, poll=lambda: ""))
        turn = Turn(None, expected="same task")
        run = Run(terminal, turn, log)
        run.poll()
        assert turn.conversation_id is None
        transcript = (
            tmp_path
            / ".gemini/antigravity-cli/brain"
            / cid
            / ".system_generated/logs/transcript_full.jsonl"
        )
        transcript.parent.mkdir(parents=True)
        request = {
            "type": "USER_INPUT",
            "source": "USER_EXPLICIT",
            "content": "compare with: same task",
        }
        transcript.write_text(json.dumps(request) + "\n")
        run.poll()
        assert turn.conversation_id is None
        request["content"] = "<USER_REQUEST>same task</USER_REQUEST>\n<ADDITIONAL_METADATA>"
        transcript.write_text(json.dumps(request) + "\n")
        run.poll()
        assert turn.user_seen
        runs.append(run)
    assert [run.turn.conversation_id for run in runs] == ids


@pytest.mark.parametrize("reply_before_done", [False, True])
def test_native_done_can_finish_with_daemon(
    reply_before_done: bool, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    turn = Turn("c", expected="task", user_seen=True)
    turn.native("working")
    turn.row({"type": "PLANNER_RESPONSE", "step_index": 2, "tool_calls": [{"name": "run_command"}]})
    turn.row(
        {
            "type": "GENERIC",
            "status": "RUNNING",
            "content": "Tool is running as a background task with task id: c/daemon",
        }
    )
    final: dict[str, object] = {
        "step_index": 4,
        "type": "PLANNER_RESPONSE",
        "content": "reply",
        "model": "actual",
    }
    if reply_before_done:
        turn.row(final)
        assert not turn.complete()
    turn.native("done")
    if not reply_before_done:
        assert not turn.complete()
        turn.row(final)
    assert not turn.complete()
    assert turn.done_at is not None
    assert turn.complete(turn.done_at + 5) and turn.background
    events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    events.append(turn.result())
    adapter = AntigravityAdapter()
    capture = adapter.prepare(InvocationRequest("antigravity", "w", tmp_path)).capture_line
    assert capture is not None
    retained = b"".join(capture(json.dumps(e).encode()) for e in events)
    result = adapter.parse(0, retained, b"")
    assert result.status is InvocationStatus.SUCCEEDED and result.output == "reply"
    assert adapter.event_model(events[-1]) == "actual"
