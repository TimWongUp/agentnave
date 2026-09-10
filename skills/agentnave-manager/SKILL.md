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

1. 按上节准备任务后调用 `start_agent`，传入 `provider`、绝对且存在的 `cwd`、`prompt` 和显式 `provider_options`。`cwd` 应是需要加载项目规则的项目目录，CLI 在其中启动；临时交接文档的位置不改变工作目录，规则能否加载仍取决于 CLI 原生支持。AgentNave 不提供总运行时限，持续等待至终态或显式取消；CLI 自身限制仍生效，调用方不另行添加截止时间。旧运行时若仍暴露 `timeout_seconds`，在 schema 支持可空值时传 null 关闭总截止；无法关闭时报告版本限制。
2. 保存返回的 `invocation_id`，调用 `wait_agent(invocation_id)`。运行时固定每轮最多等待 5 分钟，完成或明确执行阻塞时提前返回；到期后仍在运行就继续等待同一 ID，直至终态或显式取消。无需额外 sleep。旧版 schema 若仍暴露 `wait_timeout_seconds`，固定传 `300`；无法支持时报告版本限制。
   - 在 Codex 中通过 `functions.exec` 调用时，若返回 `Script running with cell ID ...`，使用 `functions.wait` 携带该 `cell_id` 继续接收原调用，直到脚本完成，再处理 AgentNave 回复。外层交回控制权表示脚本仍在运行，不是任务超时或卡死；此时不重复发起 `wait_agent`，也不据此结束任务。宿主的接收窗口不改变 AgentNave 的 5 分钟等待。
3. 新版启动、等待和取消共用顶层 `invocation_id`、`status`、`reason`、`elapsed_ms`，其他字段仅在有值时出现。`status=running` 表示 CLI 尚未结束；`reason=wait_elapsed` 表示本轮等待到期，`reason=execution_blocked` 表示提前发现明确阻塞，读取固定类别 `error.code`。阻塞返回不停止 CLI，同类阻塞在一次 Invocation 内只主动提醒一次；根据错误决定继续等待同一 ID 或取消。普通工具失败、暂时重试和沉默不等于任务无法执行。仅 CLI 暴露的已识别阻塞可提前返回，未知错误仍需检查输出或最终结果。
4. 每轮运行中返回包含可用的 `activity`（最近活动类型、原生状态、工具名和 `age_ms`）以及最新公开回复尾部 `output`（最多 1000 个 Unicode 字符）与 `output_age_ms`。用正文判断方向、活动判断运行情况；没有新正文时可能重复同一内容，用年龄区分。未输出正文就省略该字段；不返回思考、工具参数或工具结果，无游标、分页或独立读取工具。单次观察不代表所有并发工作，也不证明卡死。若材料不足以判断是否跑偏，应报告未知。
5. `reason=finished` 时按顶层 `status`、`output`、`error` 和原生 `session_id` 验收；最终正文不受 1000 字符限制，但仍受运行时整体捕获上限约束。`succeeded` 不保证用户任务完成，结合实际命令结果、产物或检查证据验收；权限拒绝、未执行或目录不符要报告未完成部分。费用不展示；耗时使用 `elapsed_ms`。旧版仍返回 `state`、`snapshot` / `result` 嵌套结构，按当前连接 schema 读取，不能把 Skill 更新当作运行时已升级。

## 取消与续接

调用方决定停止时，使用 `cancel_agent(invocation_id)` 获取终态。取消不会撤销已产生的副作用；只观察任务时使用 `wait_agent`。

活跃调用不接受追加消息。续接同一 Provider 对话时，把已结束调用返回的非空 `session_id` 与新 prompt 传给新的 `start_agent`。`invocation_id` 不能用作会话 ID；服务重启后旧 invocation 句柄不可恢复。

同一原生会话续接时只补充本轮目标、变更的约束和新增证据；切换 CLI 或新建会话时重新按交接要求提供必要上下文，不假定旧会话历史可见。
