"""What a wait task watches: readiness of its handles and the status line shown in /tasks.

Readiness uses the same undelivered events as the inbox snapshot and never consumes any
(ADR-0131). The status line names members only by the service's own spoken names, never by
task titles, prompts or output.
"""

from __future__ import annotations

import ctypes
import locale
import os
import sys
from dataclasses import dataclass, field
from functools import cache
from typing import Literal

from agentnave.conversations import Conversations
from agentnave.group import Round
from agentnave.inbox_events import round_events, task_events
from agentnave.models import InvocationStatus
from agentnave.naming import MemberIdentity, cli_label

type Language = Literal["zh", "en"]


def _names() -> list[str]:
    return []


@dataclass
class WaitScan:
    """Non-consuming view of a wait's handles at one moment."""

    ready: bool = False
    ready_names: list[str] = field(default_factory=_names)
    prompts: list[str] = field(default_factory=_names)
    stalled: list[str] = field(default_factory=_names)
    working: list[str] = field(default_factory=_names)
    meeting_seats: int = 0


def scan_wait(
    conversations: Conversations, ids: list[str] | None, manager_session_id: str | None
) -> WaitScan:
    """Whether a blocking wait over these handles would return now, and who is doing what.

    An empty inbox counts as ready: the wait would answer nothing_pending.
    """
    listings = {item.invocation_id: item for item in conversations.manager.list_invocations()}
    if ids is None:
        tasks, rounds = conversations.inbox(manager_session_id)
    else:
        tasks = [handle for handle in ids if handle in listings]
        rounds = [handle for handle in ids if handle not in listings]
    scan = WaitScan(ready=not tasks and not rounds)
    for handle in tasks:
        listing = listings[handle]
        _scan_task(scan, conversations, handle, listing.provider, listing.status)
    for handle in rounds:
        try:
            round_ = conversations.round(handle)
        except KeyError:
            continue
        _scan_round(scan, conversations, round_)
    return scan


def _member_name(identity: MemberIdentity | None, provider: str) -> str:
    if identity is None:
        return cli_label(provider)
    if identity["model_source"] == "unknown":
        return identity["cli"]
    return identity["spoken_name"]


def _scan_task(
    scan: WaitScan,
    conversations: Conversations,
    handle: str,
    provider: str,
    status: InvocationStatus | None,
) -> None:
    manager = conversations.manager
    name = _member_name(conversations.task_identity(handle), provider)
    if task_events(conversations, handle, None, status):
        scan.ready = True
        scan.ready_names.append(name)
    elif manager.pending_prompts(handle):
        scan.prompts.append(name)
    elif manager.progress_stalled(handle):
        scan.stalled.append(name)
    else:
        scan.working.append(name)


def _scan_round(scan: WaitScan, conversations: Conversations, round_: Round) -> None:
    if round_.finished or round_events(conversations, round_):
        scan.ready = True
        if round_.finished:
            return
    identities = conversations.round_identities(round_)
    for run in round_.runs:
        if run.status != "running":
            continue
        seat = round_.group.seats.get(run.seat_id)
        name = _member_name(identities.get(run.seat_id), seat.provider if seat else run.seat_id)
        if any(prompt.status == "pending" for prompt in run.prompts):
            scan.prompts.append(name)
        elif run.invocation_id is not None and _stalled(conversations, run.invocation_id):
            scan.stalled.append(name)
        else:
            scan.meeting_seats += 1


def _stalled(conversations: Conversations, invocation_id: str) -> bool:
    try:
        return conversations.manager.progress_stalled(invocation_id)
    except KeyError:
        return False


# Status line

_ZH = {
    "ready": "✨ {names} 交卷啦，正在递结果…",
    "ready_anon": "✨ 有人交卷啦，正在递结果…",
    "prompt": "🙋 {names} 举手等你回话～",
    "stalled": "👀 {names} 好像卡壳了，正催你看看…",
    "working": "💻 {names} 正在工位敲键盘…",
    "working_many": "💻 {names}正在赶活…",
    "meeting": "💬 {count} 位席位正在会议室讨论…",
    "busy_and_meeting": "💻 {names} 在忙，另有会议讨论中…",
    "idle": "💻 员工们正在赶活…",
}
_EN = {
    "ready": "✨ {names} turned in the work — handing it over…",
    "ready_anon": "✨ Someone turned in the work — handing it over…",
    "prompt": "🙋 {names} raised a hand — waiting for your reply~",
    "stalled": "👀 {names} {looks} stuck — needs a quick look…",
    "working": "💻 {names} {is} typing away…",
    "working_many": "💻 {names} typing away…",
    "meeting": "💬 {count} {seats} chatting in the meeting room…",
    "busy_and_meeting": "💻 {names} {is} busy, plus a meeting in progress…",
    "idle": "💻 The team is working on it…",
}


def _unique(names: list[str]) -> list[str]:
    return list(dict.fromkeys(names))


def _join(names: list[str], language: Language) -> str:
    """Up to two names in full; more become «first and N in all» / «first + N more»."""
    if language == "zh":
        if len(names) <= 2:
            return "、".join(names)
        return f"{names[0]} 等 {len(names)} 位员工"
    if len(names) <= 2:
        return " & ".join(names)
    return f"{names[0]} + {len(names) - 1} more"


def status_message(scan: WaitScan, language: Language) -> str:
    """One short line: ready first, then a pending prompt, a stall, then work in progress."""
    texts = _ZH if language == "zh" else _EN

    def say(key: str, names: list[str], **extra: object) -> str:
        unique = _unique(names)
        plural = len(unique) > 1
        joined = _join(unique, language) if unique else ""
        return texts[key].format(
            names=joined,
            looks="look" if plural else "looks",
            **{"is": "are" if plural else "is"},
            **extra,
        )

    if scan.ready:
        return say("ready", scan.ready_names) if scan.ready_names else texts["ready_anon"]
    if scan.prompts:
        return say("prompt", scan.prompts)
    if scan.stalled:
        return say("stalled", scan.stalled)
    working = _unique(scan.working)
    if working and scan.meeting_seats:
        return say("busy_and_meeting", working)
    if working:
        return say("working" if len(working) <= 2 else "working_many", working)
    if scan.meeting_seats:
        count = scan.meeting_seats
        return texts["meeting"].format(count=count, seats="seats" if count > 1 else "seat")
    return texts["idle"]


# Language


def _language_of(value: str) -> Language | None:
    value = value.strip()
    if not value or value in ("C", "POSIX") or value.startswith(("C.", "POSIX.")):
        return None
    return "zh" if value.lower().startswith("zh") else "en"


def _macos_preferred_language() -> str | None:
    """The first of the user's preferred languages, read in-process from CoreFoundation."""
    try:
        cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        cf.CFLocaleCopyPreferredLanguages.restype = ctypes.c_void_p
        cf.CFArrayGetCount.argtypes = [ctypes.c_void_p]
        cf.CFArrayGetCount.restype = ctypes.c_long
        cf.CFArrayGetValueAtIndex.argtypes = [ctypes.c_void_p, ctypes.c_long]
        cf.CFArrayGetValueAtIndex.restype = ctypes.c_void_p
        cf.CFStringGetCString.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_long,
            ctypes.c_uint32,
        ]
        cf.CFStringGetCString.restype = ctypes.c_bool
        cf.CFRelease.argtypes = [ctypes.c_void_p]
        languages = cf.CFLocaleCopyPreferredLanguages()
        if not languages:
            return None
        try:
            if cf.CFArrayGetCount(languages) < 1:
                return None
            first = cf.CFArrayGetValueAtIndex(languages, 0)
            buffer = ctypes.create_string_buffer(64)
            utf8 = 0x08000100
            if not cf.CFStringGetCString(first, buffer, len(buffer), utf8):
                return None
            return buffer.value.decode("utf-8", "replace")
        finally:
            cf.CFRelease(languages)
    except (OSError, AttributeError, ValueError):
        return None


def detect_language() -> Language:
    """zh when the process locale is Chinese, else en; LC_ALL, LC_MESSAGES, LANG, then the
    platform's own setting. Never reads the tray's private preferences."""
    for name in ("LC_ALL", "LC_MESSAGES", "LANG"):
        if (found := _language_of(os.environ.get(name, ""))) is not None:
            return found
    if sys.platform == "darwin" and (preferred := _macos_preferred_language()):
        return _language_of(preferred) or "en"
    try:
        current = locale.getlocale()[0]
    except ValueError:
        current = None
    return (_language_of(current) if current else None) or "en"


@cache
def ui_language() -> Language:
    """detect_language, read once per server process."""
    return detect_language()
