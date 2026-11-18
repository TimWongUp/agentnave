"""收口待跟进清单的账本：只在进程内存里记 wait 已交付的坏结局和仍保留的隔离目录（ADR-0064）。"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal

FollowUpKind = Literal["task", "round", "isolation"]


@dataclass
class FollowUpLedger:
    # (种类, 句柄) -> 所属工作区；插入顺序即交付顺序。报过或处理掉的条目随清单一并移出。
    entries: dict[tuple[FollowUpKind, str], str] = field(
        default_factory=dict[tuple[FollowUpKind, str], str]
    )
    # 记过的键永不再记：按 ids 重读已交付的终态不会让同一项再报一次。
    seen: set[tuple[FollowUpKind, str]] = field(default_factory=set[tuple[FollowUpKind, str]])

    def add(self, kind: FollowUpKind, handle: str, workspace: str | None) -> None:
        key = (kind, handle)
        if workspace is None or key in self.seen:
            return
        self.seen.add(key)
        self.entries[key] = workspace
