from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from agentnave.adapters import base


@pytest.fixture(autouse=True)
def no_common_provider_dirs(monkeypatch: pytest.MonkeyPatch) -> None:
    # Never let a test fall back to a real provider CLI installed on this machine.
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ())


@pytest.fixture
def fake_pi_rpc(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A JSONL executable exercises real pipes and supervision without model calls."""
    directory = tmp_path / "rpc-bin"
    directory.mkdir()
    script = directory / ("pi.py" if os.name == "nt" else "pi")
    script.write_text(
        f"#!{sys.executable}\n"
        + """import json, pathlib, sys
if "--version" in sys.argv:
    print("1.1.0")
    raise SystemExit
session = sys.argv[sys.argv.index("--session") + 1] if "--session" in sys.argv else "rpc-session"
messages = []
def emit(event):
    print(json.dumps(event), flush=True)
def finish():
    emit({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":json.dumps(messages)}],"stopReason":"stop"}})
    emit({"type":"agent_end", "willRetry":False})
    emit({"type":"agent_settled", "aborted":False})
for line in sys.stdin:
    command = json.loads(line)
    with pathlib.Path("rpc-inputs.jsonl").open("a") as log:
        log.write(json.dumps(command) + "\\n")
    kind = command["type"]
    response = {"id":command["id"],"type":"response","command":kind,"success":True}
    if kind == "get_state":
        response["data"] = {"sessionId":session}
        emit(response)
    elif kind == "prompt":
        if command["message"] == "reject-start":
            response["success"] = False
            emit(response)
            continue
        response["data"] = {"disposition":"started"}
        emit(response)
        emit({"type":"agent_start"})
        emit({"type":"agent_end","willRetry":True})
        emit({"type":"auto_retry_start"})
        if command["message"] == "exit-before-settled":
            raise SystemExit
        if command["message"] == "finish-now":
            finish()
    elif kind == "steer":
        message = command["message"]
        messages.append(message)
        if message == "no-ack":
            continue
        if message != "finish-race":
            response["success"] = message != "reject"
            response["data"] = {"disposition":"queued"}
            emit(response)
        if message in ("finish", "finish-race"):
            finish()
""",
        encoding="utf-8",
    )
    script.chmod(0o755)
    if os.name == "nt":
        (directory / "pi.cmd").write_text(f'@"{sys.executable}" "%~dp0pi.py" %*\n')
    monkeypatch.setenv("PATH", str(directory) + os.pathsep + os.environ["PATH"])
    return script


@pytest.fixture
def fake_codex_rpc(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    directory = tmp_path / "codex-rpc-bin"
    directory.mkdir()
    script = directory / ("codex.py" if os.name == "nt" else "codex")
    script.write_text(
        f"#!{sys.executable}\n"
        + """import json, pathlib, sys
if "--version" in sys.argv:
    print("codex-cli 0.161.0")
    raise SystemExit
session = "codex-thread"
turn = "codex-turn"
messages = []
def emit(event):
    print(json.dumps(event), flush=True)
for line in sys.stdin:
    command = json.loads(line)
    with pathlib.Path("codex-rpc-inputs.jsonl").open("a") as log:
        log.write(json.dumps(command) + "\\n")
    method = command.get("method")
    params = command.get("params", {})
    response = {"id": command.get("id"), "result":{}}
    if method == "initialized":
        continue
    if method in ("thread/start", "thread/resume"):
        session = params.get("threadId", session)
        response["result"] = {"thread":{"id":session}}
    elif method == "turn/start":
        response["result"] = {"turn":{"id":turn,"status":"inProgress"}}
        emit({"method":"turn/started","params":{"threadId":session,"turn":{"id":turn}}})
        emit({"method":"item/started","params":{"threadId":session,"turnId":turn,"item":{"id":"tool","type":"commandExecution","status":"inProgress"}}})
    elif method == "turn/steer":
        assert params["threadId"] == session and params["expectedTurnId"] == turn
        text = params["input"][0]["text"]
        messages.append(text)
        if text == "reject":
            response = {"id":command["id"],"error":{"code":-32600,"message":"turn changed"}}
        else:
            response["result"] = {"turnId":turn}
        if text == "no-ack":
            continue
        if text != "finish-race":
            emit(response)
        if text in ("finish", "finish-race"):
            emit({"method":"item/completed","params":{"threadId":session,"turnId":turn,"item":{"id":"reply","type":"agentMessage","text":json.dumps(messages)}}})
            emit({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":"completed"}}})
            emit({"method":"item/completed","params":{"threadId":"foreign-thread","turnId":"foreign-turn","item":{"type":"agentMessage","text":"FOREIGN"}}})
        continue
    emit(response)
"""
    )
    script.chmod(0o755)
    if os.name == "nt":
        (directory / "codex.cmd").write_text(f'@"{sys.executable}" "%~dp0codex.py" %*\n')
    monkeypatch.setenv("PATH", str(directory) + os.pathsep + os.environ["PATH"])
    from agentnave.adapters import codex

    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", ())
    return script


@pytest.fixture
def fake_claude_queue(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    directory = tmp_path / "claude-queue-bin"
    directory.mkdir()
    script = directory / ("claude.py" if os.name == "nt" else "claude")
    script.write_text(
        f"#!{sys.executable}\n"
        + """import json, pathlib, sys, time
if "--version" in sys.argv:
    print("2.1.295 (Claude Code)")
    raise SystemExit
initial = None
session = "claude-session"
def emit(e):
    print(json.dumps(e), flush=True)
def lifecycle(uuid, state):
    emit({"type":"command_lifecycle","command_uuid":uuid,"state":state,"session_id":session})
for line in sys.stdin:
    command = json.loads(line)
    with pathlib.Path("claude-queue-inputs.jsonl").open("a") as log:
        log.write(json.dumps(command)+"\\n")
    uuid = command["uuid"]
    text = command["message"]["content"][0]["text"]
    if initial is None:
        initial = uuid
        emit({"type":"system","subtype":"init","session_id":session})
        lifecycle(uuid,"queued")
        lifecycle(uuid,"started")
        continue
    lifecycle(uuid,"queued")
    if text == "rejected":
        lifecycle(uuid,"discarded")
        continue
    if text in ("finish", "delayed-result"):
        lifecycle(initial,"completed")
        emit({"type":"result","result":"old reply","is_error":False,"session_id":session,"user_message_uuid":initial,"queued_turn_count":1})
        lifecycle(uuid,"started")
        lifecycle(uuid,"completed")
        if text == "delayed-result":
            time.sleep(0.1)
        emit({"type":"result","result":"queue reply","is_error":False,"session_id":session,"user_message_uuid":uuid,"queued_turn_count":0})
    elif text == "merged":
        lifecycle(initial,"completed")
        lifecycle(uuid,"completed")
        emit({"type":"result","result":"merged reply","is_error":False,"session_id":session,"user_message_uuids":[initial,uuid],"queued_turn_count":0})
    elif text == "missing-count":
        emit({"type":"result","result":"not proven complete","is_error":False,"session_id":session,"user_message_uuids":[initial,uuid]})
"""
    )
    script.chmod(0o755)
    if os.name == "nt":
        (directory / "claude.cmd").write_text(f'@"{sys.executable}" "%~dp0claude.py" %*\n')
    monkeypatch.setenv("PATH", str(directory) + os.pathsep + os.environ["PATH"])
    return script


@pytest.fixture
def fake_codebuddy_stream(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    directory = tmp_path / "buddy-stream-bin"
    directory.mkdir()
    script = directory / ("codebuddy.py" if os.name == "nt" else "codebuddy")
    script.write_text(
        f"#!{sys.executable}\n"
        + """import json, pathlib, sys
if "--version" in sys.argv:
    print("2.142.0")
    raise SystemExit
session = "buddy-session"
owner = "native-buddy-request"
def emit(e):
    print(json.dumps(e), flush=True)
for line in sys.stdin:
    command = json.loads(line)
    with pathlib.Path("buddy-inputs.jsonl").open("a") as log:
        log.write(json.dumps(command)+"\\n")
    if command['type']=='user':
        text = command['message']['content'][0]['text']
        frame = {"type":"system","subtype":"init","session_id":session}
        if text != 'no-owner':
            frame['_requestId'] = owner
        emit(frame)
        continue
    request = command['request']
    assert request['expected_request_id'] == owner
    assert request['session_id'] == session
    assert request['subtype'] == 'steer'
    text = request['content_blocks'][0]['text']
    response = {"type":"control_response","response":{"request_id":command['request_id'],"subtype":"success","response":{"session_id":session,"steered":text!='stale'}}}
    emit(response)
    if text == 'finish':
        emit({"type":"result","result":"FOREIGN","is_error":False,"session_id":session,"_requestId":"foreign-request"})
        emit({"type":"result","result":"buddy steered","is_error":False,"session_id":session,"_requestId":owner})
"""
    )
    script.chmod(0o755)
    if os.name == "nt":
        (directory / "codebuddy.cmd").write_text(f'@"{sys.executable}" "%~dp0codebuddy.py" %*\n')
    monkeypatch.setenv("PATH", str(directory) + os.pathsep + os.environ["PATH"])
    return script
