# ADR-0023：私聊续接改为显式开启

- 状态：已被 ADR-0025 取代
- 日期：2026-10-22
- `continue_desk` 已由 [ADR-0025](0025-session-desks-and-running-limit.md) 移除，工位改为原生线程。
- 续接不再按访问等级筛选线程，见 [ADR-0024](0024-consolidated-tools-and-private-review.md)。

修订 [ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md) 的私聊默认续接；工位、会议室与转发的其余约定不变。

## 背景

ADR-0018 让 `start_agent` 在未传 `session_id` 时默认续接同工位最近的私聊线程，漏传 `new_thread=true` 就带上旧上下文，当时已列为已知代价。真实调用验证了它的实际影响：同一工位在目录 A 记下一个词后，换到目录 B 并在 prompt 中声明“无关的新任务”，Codex 与 Claude Code 仍返回 `resumed=true` 并答出该词。候选线程不比较 cwd，换 worktree 也会把旧目录的原生会话带到新目录；同工位并行派发独立任务时第二个请求被拒绝。

选择续接哪段上下文是 Manager 的决策。漏传续接的代价是重新交接、看得见也补得上；漏传 `new_thread` 的代价是悄悄串上下文，回执里只有一个 `resumed` 字段可察觉。

## 决定

- `start_agent` 默认开新线程。显式 `continue_desk=true` 时，续接同一工作区内同工位 `(provider, 请求的 model, role)`、同一 cwd、同访问等级下最近的私聊线程；没有可续接的线程就开新线程，`resumed=false`。
- 该线程仍在运行时拒绝；`continue_desk` 不能与 `session_id`、`conversation_id` 同用；boss 聊天身份未知时拒绝，而不是静默开新线程。
- 移除 `new_thread`。服务忽略未知参数，旧调用方传 `new_thread=true` 仍得到新线程，语义一致。
- 显式 `session_id` 的语义不变，不检查 cwd：调用方点名的会话由调用方负责。

## 取舍

- 未传任何续接参数的旧调用方，行为从“续接”变为“新线程”，是破坏性变更，需要新的 minor 版本并同步 Skill。反向的旧默认会静默串上下文，代价更隐蔽，因此接受这次变更。
- cwd 只用于 `continue_desk` 的候选筛选，不改变工位身份：同一工位在不同目录各有线程，App 仍按工位合并展示。
