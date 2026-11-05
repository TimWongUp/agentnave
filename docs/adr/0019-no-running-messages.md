# ADR-0019：不提供运行中消息

- 状态：部分取代
- 日期：2026-10-19
- MCP 回到十六个工具的条款由 [ADR-0024](0024-consolidated-tools-and-private-review.md) 修订
- 由 [ADR-0057](0057-boss-answers-native-prompts.md) 修订：运行中允许应答 CLI 自己发起的弹框
- 由 [ADR-0056](0056-claude-interactive-runtime.md) 修订：Claude 默认交互模式，会话出现在 Claude Code 自带的历史里

v0.28.0 曾为普通任务加入 `start_agent(enable_messages=true)` 与 `send_agent_message`，让主控在子代理运行中追加约束。此功能现已整体移除：MCP 回到十六个工具，各 CLI 只保留一次性调用（print／exec），结束后用 `session_id` 续接。

移除理由来自真实使用与代价对比：

- 真实历史中没有一次非测试使用，也没有因跑偏而取消的任务。需要补充要求时，等任务结束后续接即可保留上下文；确实跑偏时取消再续接，只损失当前一轮。
- 能力只能在启动时开启，默认关闭就等于几乎不会被用到；默认开启则要承担确定的代价：各 CLI 需要更严格的版本与原生协议门槛，CLI 升级改协议时会让本可成功的任务失败；Codex 必须改用 `app-server`，它创建的线程按交互会话记录，会出现在 Codex App 的对话列表里，审批处理也与 `exec` 不同。
- 每个 CLI 的消息通道都是一套独立的协议适配，移除后服务更简单。

旧历史中保存的 `messages`、`native_turns` 字段在读取时忽略，不需要迁移。

以后若重新考虑，先确认出现了等不到任务结束、又不值得取消重来的真实场景，再按 CLI 分别评估：原生通道不得改变会话在其自带 App 中的可见性，不得放宽或改变审批语义，并保留 `session_id` 续接。
