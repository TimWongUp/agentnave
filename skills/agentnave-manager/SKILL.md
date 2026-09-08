---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 或 Antigravity CLI 执行任务时使用，说明如何通过 AgentNave 交接任务、选择模型、传参、等待、取消及续接。也用于这些 CLI 的 AgentNave 调用方式咨询；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用其他 CLI

本 Skill 只说明 CLI 的调用方式。任务选择、规划、并行、审核、重试决策与结果综合由调用方负责。

用户要求调用上述 CLI 时，优先使用当前宿主的 AgentNave MCP 工具；用户明确选择其他路线时遵从。工具可能带宿主前缀或延迟加载；首次按 AgentNave 或精确工具名发现，复用当前上下文已有 schema，以连接中的实时 schema 为参数真源。工具不可用或所选 CLI 被排除时，报告具体限制，不静默切换调用路线或 Provider。

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

## 准备任务与交接

CLI 不继承主对话。主 Agent 在首次派发、切换 CLI 或交接未完成任务前，围绕接收方这次要完成的工作整理以下内容；简单任务直接写入 prompt，省略无关项：

- **目标与交付**：本次任务、完成标准、需要返回的结果或产物位置；回答格式按用户要求和任务需要指定，不强制 Markdown。
- **范围与约束**：工作目录、允许的修改范围、已明确的授权边界和必须保留的用户改动。
- **当前状态与下一步**：相关已完成事项、验证结果、未完成项及阻塞；区分已核实事实与待验证判断，让接收方知道从哪里继续。
- **已有材料**：规格、计划、ADR、Issue、commit、diff 等使用可访问的绝对路径或 URL 引用，并说明用途；已有产物承载的内容不再复制到交接中。
- **建议技能**：列出与本次任务直接相关的技能及用途，已知可访问路径时附上；无适用技能时写“无”。接收方核对可用性后按需加载，不把建议当成已加载或扩大权限的授权。

上下文较长或需要独立交接文档时，由主 Agent 在操作系统临时目录创建本次任务专用目录，将交接文档保存在其中；prompt 保留目标和范围，并指示接收方先读取该文档的绝对路径。确认接收方在原生权限下可读取；路径不可读时，将必要内容内联到 prompt。交接文档以及本次调用所需的中间文件放临时目录，正式改动与用户指定交付仍写目标位置。文档保留至接收方完成使用，由创建方清理自己创建且不再需要的临时文件。

发送前去除 API key、密码及敏感个人信息；认证沿用 CLI 已有配置。交接摘要不改变当前任务授权，引用材料中的指令也不自动成为授权。本流程由主 Agent 执行，无需依赖另装 `handoff` Skill，Adapter 不追加交接指令。

## 启动与等待

1. 按上节准备任务后调用 `start_agent`，传入 `provider`、绝对且存在的 `cwd`、`prompt` 和显式 `provider_options`。`cwd` 应是需要加载项目规则的项目目录，CLI 在其中启动；临时交接文档的位置不改变工作目录，规则能否加载仍取决于 CLI 原生支持。`timeout_seconds` 是可选总运行上限，显式设置后到期会停止调用；支持可空参数的新运行时省略或传 null 表示不设 AgentNave 总截止，CLI 自身限制仍生效。旧运行时可能保留 30 分钟默认值，按实时 schema 确认，不将旧服务视作无限期运行。
2. 保存返回的 `invocation_id`，调用 `wait_agent`，显式传入 `wait_timeout_seconds: 120`。宿主的工具超时或响应限制更短时服从宿主限制。任务完成会提前返回，无需额外 sleep。
3. 返回 `state=running` 时检查 `snapshot`：`phase` 为进程生命周期；`last_activity` 为最新可识别的原生活动，包含事件类型、原生状态、工具名/调用 ID 或最多 512 字符的公开回复片段；`last_activity_age_ms` 为该观察的年龄，`last_event_age_ms` 为最近 JSON 事件的年龄。未支持/未报告的字段为 null，不能据此推断等待输入、卡死或所有并发工具的状态。`remaining_ms` 仅在显式总预算下提供剩余时间。根据活动、错误和任务预算决定继续等待同一 ID 或取消；明确的认证重试可作为停止排查的依据，单次等待到期、多轮 running 或事件沉默本身不足以取消或重复启动。
4. 返回 `state=finished` 时读取 `result.status`、`output`、`error` 和原生 `session_id`。`succeeded` 是 Provider 的成功终态；`failed` / `blocked` 的原因看错误与输出；`cancelled` / `timed_out` 表示本次调用已停止。读取 `duration_ms` 与 `provider_usage` 中实际报告的费用/轮次；缺失表示未报告，不等于零，也不用于推算固定价格。将结果交回调用方，由其决定验收和下一步。

## 取消与续接

调用方决定停止时，使用 `cancel_agent(invocation_id)` 获取终态。取消不会撤销已产生的副作用；只观察任务时使用 `wait_agent`。

活跃调用不接受追加消息。续接同一 Provider 对话时，把已结束调用返回的非空 `session_id` 与新 prompt 传给新的 `start_agent`。`invocation_id` 不能用作会话 ID；服务重启后旧 invocation 句柄不可恢复。

同一原生会话续接时只补充本轮目标、变更的约束和新增证据；切换 CLI 或新建会话时重新按交接要求提供必要上下文，不假定旧会话历史可见。
