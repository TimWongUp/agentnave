from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import cast

import pytest

from agentnave import claude_tui as tui
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codebuddy import CodeBuddyAdapter
from agentnave.models import InvocationRequest, InvocationStatus


@pytest.mark.parametrize("prefix", ["/", "!", "#", "&"])
def test_paste_sanitizes_controls_and_protects_commands(prefix: str) -> None:
    assert tui.clean_paste(" \t" + prefix + "task\r\nnext\rend\x00\x1b\x7f") == (
        "\n \t" + prefix + "task\nnext\nend"
    )
    assert tui.clean_paste("中文\ttext\nline") == "中文\ttext\nline"


@pytest.mark.parametrize(
    ("text", "kind"),
    [
        ("\x1b[32m❯\x1b[0m", "ready"),
        ("❯ Do you trust this folder?", "trust"),
        ("❯ Resume this conversation?", "cold_resume"),
        ("❯ Resume from summary", "summary"),
        ("Resume full session as-is", "summary"),
        ("❯ Select login method", "login"),
        ("Choose the text style\n❯ ✔ Dark mode", "onboarding_enter"),
        ("Security notes:\nPress Enter to continue…", "onboarding_enter"),
        ("Use Claude Code's terminal setup?\n❯ 1. Yes", "onboarding_skip"),
        ("Bypass Permissions mode\n❯ No, exit\n  Yes, I accept", "mode_consent"),
        ("Do you want to use this API key?\n  Yes\n❯ No (recommended)", "api_key"),
        ("❯ Some new menu\nEnter to confirm · Esc to cancel", "unknown"),
        ("❯ Do you want to proceed?", "permission"),
        ("Unrecognized welcome screen", "unknown"),
    ],
)
def test_screen_dialogs_take_precedence_over_prompt(text: str, kind: str) -> None:
    assert tui.classify_screen(text) == kind


@pytest.mark.parametrize(
    ("extra", "consent"),
    [
        (["--permission-mode", "bypassPermissions"], {"skipDangerousModePermissionPrompt": True}),
        (["--model", "haiku", "--permission-mode", "auto"], {"skipAutoPermissionPrompt": True}),
        (["--permission-mode", "default"], {}),
        (["--permission-mode"], {}),
    ],
)
def test_settings_carry_the_tier_consent_only(
    extra: list[str], consent: dict[str, object], tmp_path: Path
) -> None:
    settings = json.loads(tui.write_settings(tmp_path, extra).read_text())
    assert {k: v for k, v in settings.items() if k != "hooks"} == consent


def test_menu_selection_matches_the_whole_choice() -> None:
    assert tui.selected("  No, exit\n❯ Yes, I trust this folder", "yes, i trust this folder")
    assert not tui.selected("❯ No, exit\n  Yes, I trust this folder", "yes, i trust this folder")
    assert tui.selected("❯ Yes\n  No (recommended)", "yes")
    assert not tui.selected("  Yes\n❯ No (recommended)", "yes")


def test_screen_erases_dismissed_dialog_and_handles_split_cursor_query() -> None:
    screen = tui.Screen()
    screen.feed("Resume this conversation?\r\n❯ 1. Resume".encode())
    assert tui.classify_screen(screen.text()) == "cold_resume"
    screen.feed(b"\x1b[1A\r\x1b[2K\x1b[1B\r\x1b[2K")
    screen.feed("❯".encode())
    assert tui.classify_screen(screen.text()) == "ready"
    assert screen.feed(b"\x1b[") == 0
    assert screen.feed(b"6n") == 1


def test_screen_skips_short_escapes_claude_emits_at_startup() -> None:
    screen = tui.Screen()
    screen.feed(b"\x1b7\x1b[r\x1b8\x1b=\x1b(B\x1bP>|xterm\x1b\\\x1b[?2004h")
    screen.feed("❯ ready".encode())
    assert screen.text() == "❯ ready"


def test_transcript_projection_skips_metadata_and_sidechains() -> None:
    message = {
        "role": "assistant",
        "model": "main-model",
        "content": [{"type": "text", "text": "ok"}],
    }
    row: dict[str, object] = {"type": "assistant", "message": message}
    event = tui.transcript_event(row, "sid")
    assert event is not None
    assert event == {
        "type": "assistant",
        "message": message,
        "session_id": "sid",
        "parent_tool_use_id": None,
    }
    assert ClaudeAdapter().event_model(event) == "main-model"
    assert ClaudeAdapter().activity(event) is not None
    for extra in ({"isSidechain": True}, {"isMeta": True}, {"type": "progress"}):
        assert tui.transcript_event(row | extra, "sid") is None


def test_result_keeps_last_answer_denials_stop_failure_and_warnings() -> None:
    turn = tui.Turn(
        "sid",
        assistant={"content": [{"type": "text", "text": "partial"}], "stop_reason": "end_turn"},
    )
    turn.hook({"hook_event_name": "PermissionRequest", "tool_name": "Bash"})
    turn.hook({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"})
    turn.hook({"hook_event_name": "PermissionDenied", "tool_name": "Edit"})
    warning = tui.cold_resume_warning(
        "Resume this conversation?\nThis conversation has been inactive for 3d 19h 47m "
        "and is 293.4k tokens long. Resuming it will use about 5% of your 5-hour usage limit."
    )
    turn.warnings.append(warning)
    payload = turn.result()
    assert payload["stop_reason"] == "end_turn"
    parsed = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")
    assert parsed.output == "partial" and parsed.status is InvocationStatus.SUCCEEDED
    assert "3d 19h 47m" in parsed.warnings[0]
    assert "293.4k" in parsed.warnings[0] and "5%" in parsed.warnings[0]
    assert "denied 1 tool call(s)" in parsed.warnings[1]
    turn.hook({"hook_event_name": "StopFailure", "error": "authentication_failed"})
    # Check StopFailure independently of permission denials (which also block failures).
    turn.denials.clear()
    parsed = ClaudeAdapter().parse(1, json.dumps(turn.result()).encode(), b"")
    assert parsed.status is InvocationStatus.BLOCKED
    assert parsed.output == "partial" and parsed.warnings == (warning,)
    assert "authentication_failed" in str(parsed.error_message)
    turn.hook({"hook_event_name": "StopFailure", "error": "rate_limit"})
    assert (
        ClaudeAdapter().parse(1, json.dumps(turn.result()).encode(), b"").status
        is InvocationStatus.FAILED
    )


def test_interactive_adapter_arguments_and_fallbacks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    adapter = ClaudeAdapter()
    monkeypatch.setattr(adapter, "executable_path", lambda: "/fake/claude")
    request = InvocationRequest(
        "claude",
        "work",
        tmp_path,
        session_id="sid",
        provider_options={"interactive": True, "model": "sonnet"},
    )
    # Scope the os.name patch so pathlib never constructs WindowsPath on POSIX.
    with monkeypatch.context() as platform:
        platform.setattr("agentnave.adapters.claude.os.name", "posix")
        command = adapter.prepare(request)
    assert command.argv[:2] == (sys.executable, "-I")
    assert Path(command.argv[2]).name == "claude_tui.py"
    assert command.argv[3:] == (
        "run",
        "--claude",
        "/fake/claude",
        "--resume",
        "sid",
        "--",
        "--model",
        "sonnet",
    )
    assert command.stdin == b'{"prompt": "work"}\n' and command.capture_line is not None
    assert command.keep_stdin_open
    with monkeypatch.context() as platform:
        platform.setattr("agentnave.adapters.claude.os.name", "nt")
        assert "--print" in adapter.prepare(request).argv
    restricted = InvocationRequest(
        "claude", "work", tmp_path, provider_options={"interactive": True, "restricted_tools": True}
    )
    assert "--print" in adapter.prepare(restricted).argv
    assert "--interactive" not in adapter.prepare(restricted).argv
    assert adapter.option_args(request) == ("--model", "sonnet")
    with pytest.raises(ValueError, match="boolean"):
        adapter.prepare(
            InvocationRequest("claude", "work", tmp_path, provider_options={"interactive": "yes"})
        )
    with pytest.raises(ValueError, match="unsupported"):
        CodeBuddyAdapter().option_args(request)


def test_child_environment_only_changes_the_interactive_child(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for name in tui.STRIP_ENV:
        monkeypatch.setenv(name, "host-value")
    monkeypatch.setenv("PATH", os.pathsep.join([os.path.dirname(sys.executable), "/usr/bin"]))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/custom/config")
    env = tui.child_environment("/provider/bin/claude")
    assert not set(tui.STRIP_ENV).intersection(env)
    assert env["PATH"].split(os.pathsep) == ["/provider/bin", "/usr/bin"]
    assert env["CLAUDE_CONFIG_DIR"] == "/custom/config" and env["TERM"] == "xterm-256color"
    assert os.environ["CLAUDE_CODE_CHILD_SESSION"] == "host-value"


FAKE_CLAUDE = r"""
import json, os, select, subprocess, sys, time, tty
from pathlib import Path

tty.setraw(0)
case = os.environ.get("FAKE_CLAUDE_CASE", "success")
args = sys.argv[1:]
sid = args[args.index("--resume") + 1] if "--resume" in args else args[args.index("--session-id") + 1]
settings = Path(args[args.index("--settings") + 1])
Path(os.environ["FAKE_HOOK_PATH"]).write_text(str(settings))
config = json.loads(settings.read_text())
path = Path(os.environ["CLAUDE_CONFIG_DIR"]) / "projects" / "fake-project" / (sid + ".jsonl")
path.parent.mkdir(parents=True, exist_ok=True)
assert not any(k in os.environ for k in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT"))

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
    command = config["hooks"][name][0]["hooks"][0]
    result = subprocess.run([command["command"], *command["args"]], input=json.dumps({"hook_event_name": name, **fields}).encode(), capture_output=True, timeout=5)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout) if result.stdout else {}

if case == "trust":
    output("Choose the text style\r\n❯ ✔ Dark mode")
    read_until(b"\r")
    output("\x1b[2J\x1b[H❯ No, exit\r\n  Yes, I trust this folder\r\nEnter to confirm")
    assert b"\r" not in read_until(b"\x1b[B")
    output("\x1b[2J\x1b[H  No, exit\r\n❯ Yes, I trust this folder\r\nEnter to confirm")
    read_until(b"\r")
    output("\x1b[2J\x1b[H")
elif case == "resume":
    output("Resume this conversation?\r\nThis conversation has been inactive for 3d 19h 47m and is 293.4k tokens long. Resuming it will use about 5% of your 5-hour usage limit.\r\n❯ 1. Resume\r\nEsc to resume")
    read_until(b"\x1b")
    output("\x1b[2J\x1b[H")
output("❯ \x1b[6n")
read_until(b"\x1b[1;1R")
pasted = read_until(b"\x1b[201~")
assert b"\x1b[200~" in pasted
output("[Pasted text: 2 lines]")
if case == "channel_failure":
    output("\r\nDo you want to proceed?")
    if select.select([0], [], [], 3)[0]:
        Path(os.environ["FAKE_UNEXPECTED_KEY"]).write_bytes(os.read(0, 65536))
    time.sleep(12)
read_until(b"\r")
if case == "resume":
    # Simulate an Enter consumed while the paste was still being processed.
    read_until(b"\r")
with path.open("a") as f:
    f.write(json.dumps({"type": "user", "message": {"content": "work"}}) + "\n")
if case == "failure":
    hook("StopFailure", error="authentication_failed")
else:
    response = hook("PermissionRequest", tool_name="Bash", tool_input={"command": "echo example"})
    assert response["hookSpecificOutput"]["decision"]["behavior"] == "allow"
    response = hook("PreToolUse", tool_name="AskUserQuestion", tool_input={"questions": [{"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]}]})
    assert response["hookSpecificOutput"]["permissionDecision"] == "allow"
    assert response["hookSpecificOutput"]["updatedInput"]["answers"] == {"Which?": "A"}
    with path.open("a") as f:
        for row in [
            {"type": "assistant", "isSidechain": True, "message": {"content": [{"type": "text", "text": "child"}]}},
            {"type": "assistant", "message": {"model": "fake-model", "content": [{"type": "text", "text": "finished"}], "stop_reason": "end_turn"}},
            {"type": "system", "subtype": "turn_duration"},
        ]:
            f.write(json.dumps(row) + "\n")
time.sleep(12)
"""


@pytest.mark.skipif(os.name == "nt", reason="POSIX PTY runtime")
@pytest.mark.parametrize("case", ["success", "resume", "failure", "trust", "channel_failure"])
def test_fake_claude_turn_through_the_standalone_helper(
    case: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "claude"
    executable.write_text(f"#!{sys.executable}\n" + FAKE_CLAUDE)
    executable.chmod(0o755)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "config"))
    monkeypatch.setenv("FAKE_CLAUDE_CASE", case)
    hook_path = tmp_path / "hook-path"
    monkeypatch.setenv("FAKE_HOOK_PATH", str(hook_path))
    unexpected_key = tmp_path / "unexpected-key"
    monkeypatch.setenv("FAKE_UNEXPECTED_KEY", str(unexpected_key))
    for name in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT"):
        monkeypatch.setenv(name, "host-state")
    sid = None
    if case == "resume":
        sid = "existing-session"
        transcript = tmp_path / "config/projects/fake-project/existing-session.jsonl"
        transcript.parent.mkdir(parents=True)
        transcript.write_text(json.dumps({"type": "system", "subtype": "turn_duration"}) + "\n")
    adapter = ClaudeAdapter()
    monkeypatch.setattr(adapter, "executable_path", lambda: str(executable))
    command = adapter.prepare(
        InvocationRequest(
            "claude", "work", tmp_path, session_id=sid, provider_options={"interactive": True}
        )
    )
    process = subprocess.Popen(
        command.argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=tmp_path,
    )
    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    process.stdin.write(command.stdin or b"")
    process.stdin.flush()
    lines: list[bytes] = []
    try:
        for line in process.stdout:
            lines.append(line)
            event = json.loads(line)
            if event.get("type") == "agentnave_prompt":
                answer = {
                    "type": "agentnave_answer",
                    "prompt_id": event["prompt_id"],
                    "decision": "allow",
                }
                if event["kind"] == "question":
                    answer["answers"] = {"Which?": "A"}
                process.stdin.write((json.dumps(answer) + "\n").encode())
                process.stdin.flush()
        process.wait(timeout=20)
        completed = subprocess.CompletedProcess(
            command.argv, process.returncode, b"".join(lines), process.stderr.read()
        )
    finally:
        process.kill() if process.poll() is None else None
        process.wait()
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()
    assert completed.stderr == b"", completed.stderr.decode()
    capture = command.capture_line
    assert capture is not None
    retained = b"".join(capture(line) for line in completed.stdout.splitlines())
    parsed = adapter.parse(completed.returncode, retained, completed.stderr)
    assert parsed.status is (
        InvocationStatus.SUCCEEDED
        if case in ("success", "resume", "trust")
        else InvocationStatus.BLOCKED
    ), completed.stdout.decode()
    assert parsed.session_id
    if case in ("success", "resume", "trust"):
        assert parsed.output == "finished"
        assert not any("denied" in warning for warning in parsed.warnings)
        events = [
            cast(dict[str, object], json.loads(line)) for line in completed.stdout.splitlines()
        ]
        assert not any("child" in json.dumps(event) for event in events)
        assert any(adapter.event_model(event) == "fake-model" for event in events)
    if case == "resume":
        assert parsed.session_id == sid
        assert any("5%" in warning for warning in parsed.warnings)
    if case == "trust":
        assert any("now trusts" in warning for warning in parsed.warnings)
    if case == "channel_failure":
        assert "without a corresponding hook" in str(parsed.error_message)
        assert not unexpected_key.exists()
    assert not Path(hook_path.read_text()).parent.exists()


def test_turn_waits_for_background_tool_notifications() -> None:
    turn = tui.Turn("session")
    start = {
        "type": "tool_use",
        "id": "tool-1",
        "name": "Bash",
        "input": {"run_in_background": True},
    }
    end: dict[str, object] = {"type": "system", "subtype": "turn_duration"}
    turn.transcript({"type": "user", "message": {"role": "user", "content": "task"}})
    turn.transcript({"type": "assistant", "message": {"role": "assistant", "content": [start]}})
    turn.transcript(end)
    assert not turn.done
    notice = "<task-notification>\n<tool-use-id>tool-1</tool-use-id>\n</task-notification>"
    turn.transcript({"type": "user", "message": {"role": "user", "content": notice}})
    turn.transcript(end)
    assert turn.done


@pytest.mark.parametrize(
    ("event", "answer", "expected"),
    [
        (
            {"hook_event_name": "PermissionRequest", "tool_name": "Bash"},
            {"decision": "allow"},
            {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}},
        ),
        (
            {"hook_event_name": "PermissionRequest", "tool_name": "Edit"},
            {"decision": "deny", "message": "outside scope"},
            {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "deny", "message": "outside scope"},
            },
        ),
        (
            {
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": {"questions": [{"question": "Which?"}]},
            },
            {"decision": "allow", "answers": {"Which?": "A"}},
            {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {"questions": [{"question": "Which?"}], "answers": {"Which?": "A"}},
            },
        ),
    ],
)
def test_hook_waits_for_atomic_boss_answer(
    event: dict[str, object], answer: dict[str, object], expected: dict[str, object], tmp_path: Path
) -> None:
    process = subprocess.Popen(
        [sys.executable, "-I", str(Path(tui.__file__)), "hook", str(tmp_path)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdin is not None
    try:
        process.stdin.write(json.dumps(event).encode())
        process.stdin.close()
        deadline = time.monotonic() + 5
        while not list(tmp_path.glob("prompt-*.json")) and time.monotonic() < deadline:
            time.sleep(0.01)
        path = next(tmp_path.glob("prompt-*.json"))
        assert process.poll() is None
        prompt_id = path.stem.removeprefix("prompt-")
        tui.atomic_json(tmp_path / f"answer-{prompt_id}.json", answer)
        process.wait(timeout=5)
        assert process.returncode == 0
        assert process.stdout is not None
        assert json.loads(process.stdout.read()) == {"hookSpecificOutput": expected}
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()


def test_hook_timeout_settings_and_bounded_event(tmp_path: Path) -> None:
    settings = json.loads(tui.write_settings(tmp_path).read_text())
    for name in ("PermissionRequest", "PreToolUse"):
        hook = settings["hooks"][name][0]["hooks"][0]
        assert hook["timeout"] == 604800 and hook["onFailure"] == "block"
    event = tui.prompt_event(
        "f" * 32, {"tool_name": "Bash", "tool_input": {"command": "x" * 10000}}
    )
    assert len(str(event["tool_input"])) == 4000


def test_prompt_scan_closes_dead_hook_and_preserves_parallel_requests(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    turn = tui.Turn("session")
    for prompt_id in ["a" * 32, "b" * 32]:
        tui.atomic_json(
            tmp_path / f"prompt-{prompt_id}.json",
            {
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": {},
                "hook_pid": os.getpid(),
            },
        )
    tui.read_hooks(tmp_path, turn)
    events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert len(events) == 2 and all(event["type"] == "agentnave_prompt" for event in events)
    tui.read_hooks(tmp_path, turn)
    assert not capsys.readouterr().out
    (tmp_path / f"prompt-{'a' * 32}.json").unlink()
    tui.read_hooks(tmp_path, turn)
    assert json.loads(capsys.readouterr().out)["type"] == "agentnave_prompt_closed"
    assert turn.pending_prompts == {"b" * 32}
