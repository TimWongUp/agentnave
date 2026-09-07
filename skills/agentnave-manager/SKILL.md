---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 或 Antigravity CLI 执行任务时使用，说明如何通过 AgentNave 选择模型、传参、等待、取消及续接。也用于这些 CLI 的 AgentNave 调用方式咨询；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用其他 CLI

本 Skill 只说明 CLI 的调用方式。任务选择、规划、并行、审核、重试决策与结果综合由调用方负责。

用户要求调用上述 CLI 时，优先使用当前宿主的 AgentNave MCP 工具；用户明确选择其他路线时遵从。工具可能带宿主前缀，以实时 schema 为参数真源。工具不可用或所选 CLI 被排除时，报告具体限制，不静默切换调用路线或 Provider。

## 选择 CLI 与模型

只读取所选 CLI 对应的一份参考文件，同一上下文复用；切换 CLI 时再读取新文件。

| CLI | `provider` | 按需读取 |
| --- | --- | --- |
| Claude Code | `claude` | [Claude Code](references/claude.md) |
| CodeBuddy Code | `codebuddy` | [CodeBuddy Code](references/codebuddy.md) |
| Codex CLI | `codex` | [Codex CLI](references/codex.md) |
| Grok CLI | `grok` | [Grok CLI](references/grok.md) |
| Antigravity CLI | `antigravity` | [Antigravity CLI](references/antigravity.md) |

参考文件的 JSON 是 `provider_options` 的模型与 effort 默认值，不保证账户可用性。用户指定的字段覆盖对应默认值，未指定字段沿用参考文件；用户要求原生设置时省略对应选项。其他选项只传调用方明确选择的值；权限与工具保持原生设置，除非用户明确要求改变。

首次调用所选 CLI 前，使用 `describe_provider` 核对允许状态和支持参数，当前上下文复用结果。它不检查安装或登录。旧服务若仍返回模型指引，模型选择以用户要求和本 Skill 为准；模型不可用时返回具体错误，不静默替换。

## 启动与等待

1. 调用 `start_agent`，传入 `provider`、绝对且存在的 `cwd`、完整 `prompt` 和显式 `provider_options`。CLI 不继承主对话，prompt 应包含任务、必要上下文、允许的修改范围及预期输出。`timeout_seconds` 是总运行上限，到期会停止调用；按调用方的时间预算设置。
2. 保存返回的 `invocation_id`，调用 `wait_agent`，显式传入 `wait_timeout_seconds: 120`。宿主的工具超时或响应限制更短时服从宿主限制。任务完成会提前返回，无需额外 sleep。
3. 返回 `state=running` 时继续等待同一 ID。单次等待到期不代表失败；`phase`、`elapsed_ms` 与 `last_event_age_ms` 只描述生命周期和事件观察，不表示任务进度。仅凭多轮 running 或事件沉默，不足以判定卡死、取消或重复启动。
4. 返回 `state=finished` 时读取 `result.status`、`output`、`error` 和原生 `session_id`。`succeeded` 是 Provider 的成功终态；`failed` / `blocked` 的原因看错误与输出；`cancelled` / `timed_out` 表示本次调用已停止。将结果交回调用方，由其决定验收和下一步。

## 取消与续接

调用方决定停止时，使用 `cancel_agent(invocation_id)` 获取终态。取消不会撤销已产生的副作用；只观察任务时使用 `wait_agent`。

活跃调用不接受追加消息。续接同一 Provider 对话时，把已结束调用返回的非空 `session_id` 与新 prompt 传给新的 `start_agent`。`invocation_id` 不能用作会话 ID；服务重启后旧 invocation 句柄不可恢复。
