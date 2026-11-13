# ADR-0077：Claude Code 宿主增加长等待档

- 状态：部分修订
- 日期：2026-11-10
- 适用范围由 [ADR-0082](0082-claude-long-wait-terminal-only.md) 收窄为 Claude Code 终端交互会话。
- 省略 `wait_seconds` 时的窗口由 [ADR-0084](0084-claude-terminal-default-long-wait.md) 修订：长等待会话里省略即 1500 秒。
- 修订 [ADR-0067](0067-wait-270s-window-and-host-background-limits.md) 的 Claude Code 等待窗口与条目级超时下限。

Claude Code 在约两分钟后将 MCP 调用转入后台，完成通知可唤醒模型；270 秒窗口使长任务反复唤醒并重新经过后台阈值。因此保留默认 270 秒与即时收取 0 秒，仅对识别为 Claude Code 的宿主增加显式 1500 秒窗口，其他宿主拒绝该值，保留其插话和后台边界。

Claude Code 的单次调用硬上限依次取服务条目 `timeout`、`MCP_TOOL_TIMEOUT` 环境变量或默认值；stdio 空闲超时为 1800000 毫秒，两者取较小值，转后台后继续计时。`wait` 不发送进度通知，因此 1500 秒窗口留在空闲超时以内。App 接入及刷新 Claude Code 条目时将 `timeout` 补至至少 1620000 毫秒，给回复留两分钟；更长值和 `-1` 保留。其他宿主继续沿用 ADR-0067 的窗口和超时下限。
