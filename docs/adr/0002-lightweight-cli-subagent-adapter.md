# ADR-0002：AgentNave 收敛为轻量 CLI 子代理适配器

- 状态：已接受
- 日期：2026-08-25
- 公开接口条款由 [ADR-0003](0003-agent-only-stdio-mcp-server.md) 取代

## 背景

AgentNave 原本同时承担任务图执行、进程监督、SQLite／JSONL 投影、恢复、worktree 生命周期和 macOS 状态 App。上层 Manager 已经拥有目标解释、策略和多代理编排，这些能力形成了重复所有权，也使一个本地 CLI 调用问题演变成独立运行时产品。

## 决定

AgentNave 只保留 Claude Code 与 Grok CLI 的透明适配、统一终态、超时／取消／进程树清理，以及当前 MCP server 生命周期内的 Invocation 句柄。Manager 继续拥有所有规划和编排策略。

公开接口收敛为前台 `run` 命令，以及 STDIO MCP 的 `start_agent`、`wait_agent`、`cancel_agent` 三个工具。Provider 对话延续使用 Provider 原生 `session_id`。

AgentNave 不持久化 Invocation，不提供跨重启恢复，也不提供 DAG、数据库、worktree、桌面状态 App 或自动策略。Adapter 不静默覆盖 Provider 的模型、effort、权限、工具和用户原生配置。

## 后果

实现、测试和安全边界显著缩小，Manager 与执行层只有一个清晰接口。代价是 AgentNave 进程退出后无法继续等待旧 Invocation；仍需跨重启的任务状态由 Manager 或 Provider Session 承担。专用 supervisor 在 Provider 正常退出后继续占有其 POSIX 进程组，完成组清理后才被回收，避免旧 PGID 复用。

AgentNave 不成为安全沙箱。同用户 Provider 一旦获得命令执行权限，就能主动杀死 supervisor、创建新 OS session 或以其他方式逃逸普通进程组；叠加同权限 guardian 不能改变这条事实。Provider 原生权限是安全边界，需要对抗恶意进程时由外层降权、容器或平台资源域承担。
