# AgentNave 领域语言

## AgentNave

供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用，负责把统一的 Invocation 生命周期工具映射到不同 Provider CLI。
_Avoid_: CLI 产品、多智能体运行时、工作流引擎

## Manager

AgentNave 的唯一用户。Manager 解释目标，选择 Provider，决定角色、模型、并行、重试、审核、综合与工作树策略。

## Provider

AgentNave 可启动的本地 CLI 智能体。目前支持 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI 与 Grok CLI。

## Invocation

一次 Provider 进程调用。它由 prompt、cwd、可选 Provider Session、超时和显式 Provider Options 构成，只在当前 AgentNave 进程内拥有句柄。

## Provider Session

Provider 原生会话标识。Manager 可把完成结果返回的 `session_id` 传给新的 Invocation，以继续同一 Provider 对话；它不是 AgentNave 的持久会话。

## Invocation Result

AgentNave 对 Provider 终态的归一化结果；`output` 只承载 Provider 最终回答，不包含流式过程播报。状态只能是 `succeeded`、`failed`、`blocked`、`cancelled` 或 `timed_out`。

## Invocation Snapshot

AgentNave 对运行中 Invocation 的粗粒度观察，描述生命周期阶段、耗时和距最近 Provider 官方流事件的时间；它不是 Provider 的任务进度或终态证据。
