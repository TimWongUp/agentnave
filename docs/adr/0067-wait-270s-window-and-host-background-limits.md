# ADR-0067：按宿主后台与插话边界将等待窗口定为 270 秒

- 状态：部分修订
- 日期：2026-11-08
- Claude Code 的等待窗口和条目级超时下限由 [ADR-0076](0076-claude-long-wait.md) 修订。
- 修订 [ADR-0066](0066-unified-wait-window-and-call-sequence.md) 的十分钟等待窗口与十二分钟宿主工具超时下限；补充 [ADR-0007](0007-wait-any-ready-subset.md)、[ADR-0015](0015-app-host-integration.md)。

核实各宿主对长耗时 MCP `wait` 的后台与中途插话行为后，能力边界并不对称：

- **Claude Code**：CLI 宿主层内置 `callMcpToolWithAutoBackground`（满两分钟或用户发消息时立即将阻塞的 MCP 调用转入后台，完成后注入 `<task-notification>` 跨回合唤醒模型），并原生支持 MCP `2026-07-28` 的 `io.modelcontextprotocol/tasks`（SEP-2663）客户端扩展；但上游 Python `mcp` SDK 尚未发版 SEP-2663 服务端类型，且单进程 STDIO `agentnave-mcp` 的 `invocation_id` 仅存于当前连接进程内存，不能靠另起子进程或原生子代理跨连接代等。
- **Codex**：仅在启用 Code Mode（`ChatGPT.app` 默认开启 `features.code_mode_host=true` 且使用声明支持 Code Mode 的官方模型，切到第三方模型即退回同步工具调用）时提供回合内 `functions.exec` 后台句柄（`cell_id`）。其 `yield_time_ms` 默认仅 10 秒、上限为 `300000` 毫秒（五分钟），不会跨回合自动唤醒，且在 `yield_time_ms` 等待期间用户直接打字回车（`steer`）不会提前打断等待。
- **其余宿主（ZCode、CodeBuddy、Grok、Antigravity、DeepSeek Harness、Pi）**：后台机制仅覆盖终端命令或宿主原生子代理，MCP `callTool` 均为同步阻塞；用户不按 `Esc`／停止而直接打字插话时，消息需等当前 `wait` 返回后才会被读取。

将 `wait` 单次等待窗口从 600 秒改为 **270 秒（4 分 30 秒）**，`wait_seconds` 仅接受 `0`、`270`（省略为 `270`），移除 `600`：

1. 270 秒为 Codex `functions.exec` 的五分钟 `yield_time_ms: 300000` 上限留出 30 秒余量，单次 `functions.exec` 即可直接拿到 `wait` 返回，不再在五分钟边界常态化溢出为 `cell_id` 二次收取。
2. 在同步阻塞宿主中，用户不按 `Esc` 直接打字插话的最长等待从十分钟缩短为四分半，同时仍能一次覆盖绝大多数常规任务，避免频繁 `wait_elapsed` 空转。
3. App 在接入与刷新用户级 AgentNave 条目时，将支持条目级工具超时的宿主下限设为 **360 秒（六分钟）**，已有更长值（含旧的 720 秒）及 `-1` 无上限保留；`seq` 连续等待序号语义不变。
