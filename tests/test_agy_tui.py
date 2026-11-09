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
from agentnave.agy_tui import STOP_GRACE, Run, Turn, hooks
from agentnave.agy_tui.screen import AgyScreen, input_box, state
from agentnave.claude_tui.terminal import Terminal
from agentnave.models import InvocationRequest, InvocationStatus

FAKE_AGY = r"""
import json, os, select, subprocess, sys, time, tty
from pathlib import Path

tty.setraw(0)
case = os.environ["FAKE_AGY_CASE"]
args = sys.argv[1:]
settings = json.loads(args[args.index("--settings") + 1])
assert os.getcwd() in settings["trustedWorkspaces"]
assert "--dangerously-skip-permissions=true" in args
cid = args[args.index("--conversation") + 1] if "--conversation" in args else "new-conversation"
home = Path(os.environ["HOME"])
entry = json.loads((home / ".gemini/config/hooks.json").read_text())["agentnave-interactive"]
transcript = home / ".gemini/antigravity-cli/brain" / cid / ".system_generated/logs/transcript_full.jsonl"
transcript.parent.mkdir(parents=True, exist_ok=True)

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

def hook(name, **fields):
    command = entry[name][0]
    command = command["hooks"][0]["command"] if "hooks" in command else command["command"]
    payload = {"conversationId": cid, "modelName": "fake-model", **fields}
    done = subprocess.run(command, shell=True, input=json.dumps(payload).encode(), capture_output=True, timeout=5)
    assert done.returncode == 0, done.stderr
    return json.loads(done.stdout)

def row(**fields):
    with transcript.open("a") as f:
        f.write(json.dumps({"status": "DONE", **fields}) + "\n")

rule = "─" * 40

def box(text):
    output("\x1b[2J\x1b[H" + "\r\n".join(["Antigravity CLI", rule, text, rule, "? for shortcuts"]))

if case == "dialog":
    # The words of a menu in the conversation above the box are not a menu.
    output("↑/↓ Navigate · enter Confirm\r\nDo you trust this folder?\r\n> Yes\r\n  No\r\n  ↑/↓ Navigate · enter Confirm")
    time.sleep(12)
box(">")
pasted = read_until(b"\x1b[201~")
assert b"\x1b[200~" in pasted and b"work" in pasted
box("> work")
read_until(b"\r")
row(step_index=10, source="USER_EXPLICIT", type="USER_INPUT", content="work")
if case != "resume":
    hook("SessionStart")
assert hook("PreToolUse", toolCall={"name": "ask_question"})["decision"] == "deny"
row(step_index=11, source="MODEL", type="PLANNER_RESPONSE", content="looking", tool_calls=[{"name": "run_command"}])
row(step_index=12, source="MODEL", type="GENERIC", content="ran")
if case != "noreply":
    row(step_index=13, source="MODEL", type="PLANNER_RESPONSE", content="finished")
# A subagent of this agy stops first; only this conversation's Stop ends the turn.
hook("Stop", conversationId="subagent", fullyIdle=True, terminationReason="NO_TOOL_CALL")
hook("Stop", fullyIdle=True, terminationReason="NO_TOOL_CALL", error="")
time.sleep(12)
"""


@pytest.mark.skipif(os.name == "nt", reason="POSIX PTY runtime")
@pytest.mark.parametrize("case", ["new", "resume", "dialog", "noreply"])
def test_fake_agy_turn_through_the_standalone_helper(
    case: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "agy"
    executable.write_text(f"#!{sys.executable}\n" + FAKE_AGY)
    executable.chmod(0o755)
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("FAKE_AGY_CASE", case)
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
    command = adapter.prepare(
        InvocationRequest("antigravity", "work", tmp_path, session_id=sid, provider_options=options)
    )
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
    if case == "noreply":
        # The last response still calls a tool: a Stop then is no finished turn.
        assert parsed.status is InvocationStatus.FAILED
        assert "without a final reply" in str(parsed.error_message)
        return
    assert parsed.status is InvocationStatus.SUCCEEDED, completed.stdout.decode()
    assert parsed.output == "finished"
    assert parsed.session_id == (sid or "new-conversation")
    assert any(adapter.event_model(event) == "fake-model" for event in events)
    assert "earlier" not in completed.stdout.decode()
    tools = [adapter.activity(event) for event in events]
    assert [a.state for a in tools if a is not None and a.kind == "tool"] == ["ACTIVE", "DONE"]
    assert any(hooks.NAME in warning for warning in parsed.warnings)


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell hook; no interactive runtime on Windows")
def test_hook_entry_keeps_other_hooks_and_acts_only_in_agentnave_runs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    path = hooks.hooks_path()
    path.parent.mkdir(parents=True)
    # A user who keeps the file elsewhere keeps the link; the target gets the entry.
    target = tmp_path / "dotfiles-hooks.json"
    target.write_text(json.dumps({"mine": {"Stop": []}}))
    path.symlink_to(target)
    assert hooks.ensure_entry() is not None
    assert path.is_symlink()
    assert hooks.ensure_entry() is None
    saved = json.loads(path.read_text())
    assert list(saved) == ["mine", hooks.NAME]
    ask = saved[hooks.NAME]["PreToolUse"][0]["hooks"][0]["command"]
    stop = saved[hooks.NAME]["Stop"][0]["command"]
    env = {k: v for k, v in os.environ.items() if k != hooks.ENV}

    def run(command: str, extra: dict[str, str]) -> dict[str, object]:
        done = subprocess.run(
            command, shell=True, input=b"{}", capture_output=True, env={**env, **extra}
        )
        return json.loads(done.stdout)

    assert run(ask, {}) == {"decision": "allow"}
    assert run(ask, {hooks.ENV: str(tmp_path)})["decision"] == "deny"
    assert run(stop, {}) == {} and not list(tmp_path.glob("*.Stop.json"))
    assert run(stop, {hooks.ENV: str(tmp_path)}) == {}
    assert len(list(tmp_path.glob("*.Stop.json"))) == 1
    saved[hooks.NAME]["enabled"] = False
    path.write_text(json.dumps(saved))
    with pytest.raises(ValueError, match="disabled"):
        hooks.ensure_entry()
    path.write_text("{broken")
    with pytest.raises(ValueError, match="not valid JSON"):
        hooks.ensure_entry()


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


def test_stalled_turns_block_on_limits_but_wait_for_background_stop() -> None:
    def run(**fields: object) -> Run:
        turn = Turn("c")
        for key, value in fields.items():
            setattr(turn, key, value)
        return Run(cast(Terminal, None), turn, Path())

    answered = run(answered=True, response="done")
    assert not answered.stalled("idle", STOP_GRACE - 1, "")
    assert answered.stalled("idle", STOP_GRACE + 1, "")
    assert "Stop hook never arrived" in str(answered.turn.error)
    assert cast(dict[str, object], answered.turn.result()["result"])["response"] == "done"
    # A Stop that left background commands running is followed by another once they end.
    assert not run(answered=True, stop_seen=True).stalled("idle", 3600, "")
    assert not run().stalled("busy", 3600, "")
    assert not run().stalled("unknown", 30, "") and run().stalled("unknown", 61, "")
    assert run().stalled("menu", 3, "")


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
        ("agent_response", "DONE", None),
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


@pytest.mark.parametrize("error", ["", "request failed"])
@pytest.mark.parametrize("reply_before_stop", [False, True])
def test_poll_reads_stop_and_final_reply_without_redisplaying_a_daemon(
    error: str, reply_before_stop: bool, tmp_path: Path, capsys: pytest.CaptureFixture[str]
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
            "content": "Tool is running as a background task with task id: c/daemon\n",
        }
    )
    turn.watch("idle")
    turn.hook("Stop", {"conversationId": "c", "fullyIdle": False})
    assert not turn.stopped
    turn.watch("busy")
    capsys.readouterr()
    # A real poll reads hooks first, then transcript rows, then watches the footer.
    # fullyIdle ignores daemons, so this task need not have an ending message.
    turn.transcript = tmp_path / "transcript.jsonl"
    turn.transcript.write_text(
        json.dumps(
            {"step_index": 4, "type": "PLANNER_RESPONSE", "source": "MODEL", "content": "reply"}
        )
        + "\n"
    )
    rule = "─" * 40
    screen = f"{rule}\n>\n{rule}\n● command running\n{rule}\n? for shortcuts · 1 task(s)"
    terminal = cast(
        Terminal,
        SimpleNamespace(poll=lambda: screen, process=SimpleNamespace(pid=123, poll=lambda: None)),
    )
    run = Run(terminal, turn, tmp_path)
    if reply_before_stop:
        turn.watch(state(run.poll()))
        assert not turn.stopped  # Text alone cannot prove this is the final reply.
    (tmp_path / "123.1.Stop.json").write_text(
        json.dumps({"conversationId": "c", "fullyIdle": True, "error": error})
    )
    run.await_stop()
    events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    expected = [
        {
            "conversation_id": "c",
            "step_index": 4,
            "step_type": "agent_response",
            "state": "DONE",
            "text_delta": "reply",
        }
    ]
    if reply_before_stop:
        # While still waiting for Stop, a tool update after text is valid. It must not
        # change adapter success/error classification or duplicate the response text.
        expected.append(
            {
                "conversation_id": "c",
                "step_index": 200,
                "step_type": "tool",
                "state": "ACTIVE",
                "tool_name": "run_command",
            }
        )
    assert [e["step_update"] for e in events] == expected
    assert turn.stopped and turn.background  # The Stop hook, not task count, ends the turn.
    events.append(turn.result())
    capture = (
        AntigravityAdapter().prepare(InvocationRequest("antigravity", "w", tmp_path)).capture_line
    )
    assert capture is not None
    retained = b"".join(capture(json.dumps(e).encode()) for e in events)
    # run_main exits nonzero on a hook error; parse() must not apply print-mode recovery.
    result = AntigravityAdapter().parse(1 if turn.error else 0, retained, b"")
    assert result.status is (InvocationStatus.FAILED if error else InvocationStatus.SUCCEEDED)
    assert result.output == "reply"
