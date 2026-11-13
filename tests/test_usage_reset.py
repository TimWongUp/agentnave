"""A quota failure with no reset time asks the CLI's own usage query (ADR-0081)."""

from __future__ import annotations

import asyncio
import dataclasses
import json
import os
import sys
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

from agentnave import provider_health, usage_probe
from agentnave.adapters import usage_reset as usage
from agentnave.adapters.base import PreparedCommand, UsageQuery
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codex import CodexAdapter
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest, InvocationStatus

NOW = datetime(2026, 11, 13, 4, 0, tzinfo=UTC)
AGY = (
    "Gemini Models\tWeekly Limit Remaining\t0%\t2026-11-14T03:38:56Z\n"
    "Gemini Models\tFive Hour Limit Remaining\tdisabled\t\n"
    "Claude and GPT models\tWeekly Limit Remaining\t{claude}\t2026-11-18T12:13:15Z\n"
    "Claude and GPT models\tFive Hour Limit Remaining\tdisabled\t\n"
)


def test_antigravity_reset_follows_the_models_group() -> None:
    both = AGY.format(claude="0%")
    gemini = datetime(2026, 11, 14, 3, 38, 56, tzinfo=UTC)
    claude = datetime(2026, 11, 18, 12, 13, 15, tzinfo=UTC)
    assert usage.antigravity_reset(both, "gemini-4-carbon-medium", NOW) == gemini
    assert usage.antigravity_reset(both, "claude-sonnet-5-5-high", NOW) == claude
    assert usage.antigravity_reset(both, "gpt-oss-120b-medium", NOW) == claude
    # No model to place: the CLI is blocked only once every group ran out.
    assert usage.antigravity_reset(both, None, NOW) == gemini
    left = AGY.format(claude="40%")
    assert usage.antigravity_reset(left, None, NOW) is None
    assert usage.antigravity_reset(left, "claude-opus-5-5-low", NOW) is None
    assert usage.antigravity_reset(left, "gemini-4-argon-low", NOW) == gemini
    assert usage.antigravity_reset(both, "gemini-4-carbon-medium", gemini) is None
    assert usage.antigravity_reset("not a table", None, NOW) is None


def test_codex_reset_needs_a_spent_window_with_a_time() -> None:
    at = int((NOW + timedelta(days=2)).timestamp())

    def limits(primary: int, secondary: int, reset: object = at) -> dict[str, object]:
        window = {"windowDurationMins": 300, "resetsAt": reset}
        return {
            "rateLimits": {
                "primary": {"usedPercent": primary, **window},
                "secondary": {"usedPercent": secondary, **window},
            }
        }

    assert usage.codex_reset(limits(21, 50), NOW) is None
    assert usage.codex_reset(limits(21, 100), NOW) == datetime.fromtimestamp(at, UTC)
    assert usage.codex_reset(limits(100, 100, None), NOW) is None
    assert usage.codex_reset({}, NOW) is None


def test_claude_reset_reads_local_times_and_per_model_weeks() -> None:
    text = (
        "Current session: 100% used · resets Nov 13 at 6:20pm (Asia/Taipei)\n"
        "Current week (all models): 28% used · resets Nov 15 at 9:59pm (Asia/Taipei)\n"
        "Current week (Fable): 100% used · resets Nov 15 at 10pm (Asia/Taipei)\n"
    )
    now = datetime(2026, 11, 13, 12, 0).astimezone(UTC)
    session = datetime(2026, 11, 13, 18, 20).astimezone(UTC)
    fable = datetime(2026, 11, 15, 22, 0).astimezone(UTC)
    # Both windows ran out for a Fable run: it is usable again once the later one resets.
    assert usage.claude_reset(text, "claude-fable-5", now) == fable
    assert usage.claude_reset(text, "sonnet", now) == session
    assert usage.claude_reset(text, None, now) == session
    spent = text.replace("100% used · resets Nov 13", "20% used · resets Nov 13")
    assert usage.claude_reset(spent, "sonnet", now) is None
    assert usage.claude_reset(spent, "claude-fable-5", now) == fable
    # The next occurrence of a time with no date; a date already past rolls to next year.
    assert usage.claude_reset("Current session: 100% used · resets 6:20pm", None, now) == session
    late = datetime(2026, 12, 20, 12, 0).astimezone(UTC)
    assert usage.claude_reset(
        "Current week (all models): 100% used · resets Jan 2 at 9am", None, late
    ) == datetime(2027, 1, 2, 9, 0).astimezone(UTC)
    assert usage.claude_reset("Current session: 100% used · resets soon", None, now) is None


@pytest.mark.skipif(os.name == "nt", reason="needs tzset")
def test_claude_reset_without_a_date_follows_daylight_saving(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TZ", "America/New_York")
    time.tzset()
    try:
        # 01:30 EST on the night the clocks jump: 4am the same day is already EDT (08:00 UTC).
        now = datetime(2026, 3, 8, 6, 30, tzinfo=UTC)
        got = usage.claude_reset("Current session: 100% used · resets 4am", None, now)
        assert got == datetime(2026, 3, 8, 8, 0, tzinfo=UTC)
    finally:
        monkeypatch.undo()
        time.tzset()


def test_a_reset_time_replaces_the_earlier_one_hour_guess(tmp_path: Path) -> None:
    provider_health.record(tmp_path, "claude", "quota", "m")
    soon = datetime.now(UTC) + timedelta(minutes=5)
    provider_health.record(tmp_path, "claude", "quota", "m", soon)
    until = provider_health.active(tmp_path)[("claude", "m")]["until"]
    assert until is not None and abs(datetime.fromisoformat(until) - soon) < timedelta(seconds=1)


def _script(code: str) -> tuple[str, ...]:
    return (sys.executable, "-c", code)


@pytest.mark.asyncio
async def test_codex_query_stops_once_the_answer_arrives(tmp_path: Path) -> None:
    at = int((datetime.now(UTC) + timedelta(days=2)).timestamp())
    server = (
        "import json, sys, time\n"
        "for line in sys.stdin:\n"
        "    m = json.loads(line)\n"
        "    if m.get('id') == 2:\n"
        f"        result = {{'rateLimits': {{'primary': {{'usedPercent': 100, 'resetsAt': {at}}}}}}}\n"
        "        print(json.dumps({'id': 2, 'result': result}), flush=True)\n"
        "        time.sleep(60)\n"
    )
    base = CodexAdapter().usage_query(None)
    assert base is not None
    query = dataclasses.replace(base, argv=_script(server))
    started = time.monotonic()
    reset = await usage_probe.run_usage_query(query, datetime.now(UTC))
    assert reset == datetime.fromtimestamp(at, UTC)
    assert time.monotonic() - started < 15  # it did not wait for the server to exit


@pytest.mark.asyncio
async def test_a_query_that_never_answers_is_cut_off(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(usage_probe, "QUERY_SECONDS", 0.5)
    marker = tmp_path / "pid"
    code = f"import os, time; open({str(marker)!r}, 'w').write(str(os.getpid())); time.sleep(60)"
    query = UsageQuery(_script(code), lambda _out, _now: datetime.now(UTC) + timedelta(days=1))
    assert await usage_probe.run_usage_query(query, datetime.now(UTC)) is None
    pid = int(marker.read_text())
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except OSError:
            break
        time.sleep(0.1)
    else:
        raise AssertionError("the query process was left running")


@pytest.mark.asyncio
async def test_failed_flooding_or_unreadable_queries_give_no_reset(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(usage_probe, "QUERY_SECONDS", 2.0)
    soon = datetime.now(UTC) + timedelta(hours=1)

    def reset(_out: bytes, _now: datetime) -> datetime:
        return soon

    def unreadable(_out: bytes, _now: datetime) -> datetime:
        raise TypeError("naive and aware datetimes")

    async def run(code: str, parse: object = reset) -> datetime | None:
        query = UsageQuery(_script(code), parse)  # type: ignore[arg-type]
        return await asyncio.wait_for(usage_probe.run_usage_query(query, datetime.now(UTC)), 15)

    assert await run("print('ok')") == soon
    # A command that fails is not believed, whatever it printed.
    assert await run("print('ok'); raise SystemExit(3)") is None
    # A parser that raises is a failed query, not a failed run.
    assert await run("print('ok')", unreadable) is None
    # Output past the cap is drained, so stopping the command cannot hang on a full pipe.
    assert await run("import sys\nwhile True: sys.stdout.write('x' * 65536)") is None


@pytest.mark.asyncio
@pytest.mark.parametrize("known", [True, False])
async def test_quota_failure_without_reset_asks_the_usage_query(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, known: bool
) -> None:
    reset = datetime.now(UTC) + timedelta(hours=3)
    asked: list[str | None] = []

    def query(_adapter: ClaudeAdapter, model: str | None) -> UsageQuery:
        asked.append(model)
        return UsageQuery(_script("print('usage')"), lambda _out, _now: reset if known else None)

    monkeypatch.setattr(ClaudeAdapter, "usage_query", query)
    event = {"type": "result", "is_error": True, "errors": ["weekly limit reached"]}
    manager = InvocationManager()
    manager.health_root = tmp_path
    try:
        invocation = manager.start(
            InvocationRequest("claude", "work", tmp_path, provider_options={"model": "test"}),
            prepared=PreparedCommand(_script(f"print({json.dumps(event)!r})"), tmp_path),
        )
        result = await manager.wait(invocation)
    finally:
        await manager.shutdown()
    assert result is not None and result.status is InvocationStatus.FAILED
    assert asked == ["test"]
    until = provider_health.active(tmp_path)[("claude", "test")]["until"]
    assert until is not None
    if known:
        assert abs(datetime.fromisoformat(until) - reset) < timedelta(seconds=1)
    else:
        # No reset time from the query either: the model rests for an hour.
        assert timedelta(minutes=59) < datetime.fromisoformat(until) - datetime.now(UTC)
