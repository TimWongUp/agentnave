---
name: agentnave-manager
description: 当用户要求使用 AgentNave、本地 CLI 子代理、Antigravity CLI、Claude Code CLI、CodeBuddy Code CLI、Codex CLI 或 Grok CLI 子代理执行独立任务时使用。负责由当前 Manager 决定任务、Provider、并行和审核策略，再通过 AgentNave 的 start/wait/cancel 接口运行；宿主原生子代理不触发。
---

# AgentNave Manager

AgentNave 是只供 Agent 使用的本地 MCP Server，负责适配 CLI 子代理。你仍然是 Manager，必须自己负责目标解释、任务拆解、Provider 和模型选择、并行、重试、审核、综合与工作树策略。

## 默认路由

调用 `start_agent` 时，每条 CLI 路由都同时传入 `model` 与 `effort`：

- `claude`：`{"model": "opus", "effort": "max"}`
- `codebuddy`：`{"model": "hy3", "effort": "high"}`
- `codex`：`{"model": "gpt-5.6-sol", "effort": "high"}`
- `grok`：`{"model": "grok-4.6", "effort": "high"}`
- `antigravity`：`{"model": "gemini-3.8-flash", "effort": "high"}`

用户显式指定模型或 effort 时，以用户值覆盖对应默认项；未显式指定的字段继续使用上述默认值。只传 Adapter allowlist 已支持的字段，不覆盖权限、工具或其他 Provider 设置。

## 调用

1. 把一个已经明确、可独立执行的任务写成完整 prompt，并指定绝对 `cwd`。
2. 调用 `start_agent`，保存返回的 `invocation_id`。
3. 调用 `wait_agent`。返回 `running` 时可继续做本地工作，稍后再次等待。
4. 需要停止时调用 `cancel_agent`。
5. 需要让同一 Provider 继续对话时，用上次结果里的 `session_id` 发起新的 `start_agent`。

运行中的 Invocation 不接受追加消息。需要引导时，等待本轮结束，再以返回的 `session_id` 和新 prompt 调用 `start_agent`；如果当前工作已不应继续，使用 `cancel_agent` 终止。

除上述默认模型路由外，只在确有必要时增加其他 `provider_options`；权限、工具和其他 Provider 设置继续继承原生配置。

`running` 结果中的 `snapshot` 只反映 AgentNave 生命周期、耗时和距最近 Provider 官方流事件的时间。用它确认 Invocation 是否仍在运行；最近事件时间不是任务进度或完成证据。

## 结果处理

- `succeeded`：核对输出是否真正完成任务，再纳入 Manager 的综合结果。
- `blocked`：由 Manager 判断需要补充权限、登录或用户决定，不自动绕过 Provider 门禁。
- `failed`：检查结构化错误，只有在任务仍然成立且重试理由明确时才重新发起。
- `cancelled` / `timed_out`：视为该 Invocation 已终止；需要继续时创建新 Invocation。

Invocation 句柄只在当前 MCP server 生命周期内有效。不要把 AgentNave 当任务数据库，也不要假设 server 重启后仍能等待旧任务。

AgentNave 不是安全沙箱。不要通过 Provider 权限放行你不愿让本机同用户进程执行的命令；需要对抗恶意进程时，应在 AgentNave 外使用操作系统级隔离。
