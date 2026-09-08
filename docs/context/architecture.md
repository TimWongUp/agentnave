# 架构与边界

AgentNave 是供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用的 MCP Server。它在 Manager 与 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 之间提供很薄的进程边界，不是面向人的 CLI 产品、多智能体运行时或工作流引擎。

## 所有权

Manager 拥有目标解释、任务拆解、角色和模型选择、并行策略、重试、审核循环、结果综合、工作树和长期任务状态。

AgentNave 只拥有：

- 按需返回所选 Provider 的允许状态和支持参数；
- 根据冻结请求启动一个 Provider CLI；
- 传递 prompt、cwd、可选 `session_id` 和显式 Provider Options；
- 从 Provider 原生流事件提取有界活动快照，并归一化终端结果；
- 执行超时、取消和进程树清理；
- 在 MCP server 生命周期内保存 Invocation 句柄。

AgentNave 不拥有 DAG、调度器、角色系统、自动重规划、工作树、SQLite／JSONL、恢复、保留策略、桌面 App 或 TUI。

## 安装与激活边界

运行时由 Python Tool Manager 安装，并在 MCP Host 注册稳定入口。配套 `agentnave-manager` Skill 承担 CLI 调用的主要触发、模型选择、传参、等待、取消与续接说明，每个 CLI 的模型与 effort 默认值放在独立参考文件中，选中后才读取。MCP instructions 与 Tool 描述只保留轻量调用合同，`describe_provider` 按需返回允许状态与支持参数；没有 Skill 时仍可显式调用 MCP。Skill 由宿主的技能安装机制或已有部署管理器单独安装与更新，MCP 与 Skill 都只提供 CLI 调用能力与用法，不制定调用方的任务规划、调度、审核、重试或综合策略。

Python Tool Manager 拥有运行时的安装、升级与卸载，并提供稳定的 `agentnave-mcp` launcher。源码 checkout 只用于贡献、调试和未发布版本验证，不作为 MCP Host 的长期启动路径。

MCP Host 拥有 AgentNave 的注册、作用域、启停、移除；能使用 Host 官方管理接口时，不由 AgentNave 直接修改其配置文件。安装运行时不会自动写入 Host Skills、全局规则或权限。Provider CLI 继续拥有自身的安装、认证、配置与 Session 数据，AgentNave 不创建需要随卸载处理的持久用户数据。

## 稳定合同

请求字段为 `provider`、`prompt`、绝对 `cwd`、可选 `session_id`、可选 `timeout_seconds` 和 `provider_options`。Provider Options 必须由调用方显式给出并通过对应 Adapter allowlist；AgentNave 不默认覆盖模型、effort、权限模式、工具或 Provider 原生配置。

MCP 初始元数据仅保留简短 Provider 目录和通用调用合同。Manager 首次使用某个 Provider 前调用只读 `describe_provider(provider)`，获取该 Provider 的允许状态与支持的选项，并在当前上下文中复用；详情查询不启动 CLI、不探测安装或认证、不改变工具列表。`describe_provider` 返回 `provider`、`permitted` 与 `supported_options`，不承载模型推荐。模型与 effort 由 Skill 的所选 CLI 参考文件和用户要求决定，使用推荐值时作为显式 Provider Options 传入；默认决策不下沉到 Adapter。

每个 Host 通过 `AGENTNAVE_EXCLUDED_PROVIDERS` 显式配置逗号分隔的 Provider 排除项，以排除与宿主同类的 CLI。按宿主产品对应的 CLI 配置，不按当前模型判断，也不猜测客户端身份。排除项在 server 启动时固定并验证，未知名称使启动失败；未配置或空值不排除任何 Provider。MCP 向 Manager 公布允许项与排除项；被排除的调用在创建 Invocation 前返回 Tool error，不能通过单次调用参数覆盖，也不自动回退。允许项不代表 CLI 已安装或认证。

Codex 在非 Git 目录运行时，调用方可显式传入布尔选项 `skip_git_repo_check`；Adapter 默认不绕过 Provider 的仓库检查。

结果字段为 `status`、`provider`、`output`、`session_id`、`provider_usage`、`duration_ms` 和 `error`；`provider_usage` 只保留 Provider 可用的 `num_turns` 与 `total_cost_usd`，不转发 token、cache 或 model 明细。Provider 正常返回业务失败仍是完整的 Invocation Result。Provider 缺失、无法启动或平台不受支持也会形成带 `launch_error` 的结构化失败结果，以便 Manager 读取。

STDIO MCP 是唯一公开接口，暴露 `describe_provider`、`start_agent`、`wait_agent` 和 `cancel_agent`；`agentnave-mcp` 只负责为 MCP Host 启动 server 进程。四个 Tool 都发布输入与输出 JSON Schema；可由 Manager 修正的请求错误使用 MCP Tool error 返回重试指引，Provider 执行终态使用结构化 Invocation Result。继续 Provider 对话通过新的 `start_agent(session_id=...)` 完成。

`start_agent.timeout_seconds` 省略或为 null 时不设 AgentNave 总截止；显式正数预算到期会停止调用。Provider 原生限制继续生效，AgentNave 不静默改写。`wait_agent` 的单次等待到期只返回运行状态，不终止任务。

`wait_agent` 在 Invocation 仍运行时返回 `snapshot`，包含 `phase`、`elapsed_ms`、`remaining_ms`、`last_event_age_ms`、`last_activity` 和 `last_activity_age_ms`。无显式预算时余量为 null。Antigravity、Claude、CodeBuddy、Codex 与 Grok Adapter 分别消费 Provider 官方的 `stream-json`、`stream-json`、`stream-json`、JSONL 与 `streaming-json` 事件流；只提取已识别的事件类型、原生状态、工具名/调用 ID 和有界公开回复片段。单条活动不是所有并发工作的完整状态，事件沉默不证明卡死；未知字段为 null，重试或交互等待只按原生报告呈现，继续/终止由 Manager 决定。快照不转发工具参数、工具输出或推理正文；公开回复仍可能含任务数据，不提供自动脱敏保证。终态 `output` 只保留 Provider 最终回答，其中 Codex 取最后一个完成的 `agent_message`，Grok 取结束前最近一段连续 `text` 事件。

Invocation 状态只存在于当前进程内。每次 Invocation 由一个专用 supervisor 持续占有 POSIX 进程组，Provider 正常终止后也先清理该组再回收 supervisor，避免旧 PGID 被复用。MCP server 退出时会尽力终止仍留在该组内的活跃进程；重启后旧 Invocation 句柄不可恢复。Provider 自己持久化的 Session 不受此限制。

AgentNave 不是沙箱或同用户恶意进程隔离边界。已获得命令执行权限的 Provider 或工具可以主动创建新 OS session、杀死 supervisor，或以其他方式脱离普通 POSIX 进程组；发生可检测的 supervisor 丢失时返回 `supervision_lost`，但不能安全地对可能已被复用的旧 PGID 继续发信号。是否允许这些命令由 Provider 原生权限机制和 Manager 决定；需要抵抗恶意同用户进程时，应在 AgentNave 外使用降权、容器或平台级资源域。

## Provider Adapter

Adapter 只能添加非交互输出、prompt 传输和 cwd 等协议必需参数。Provider stderr 仅在失败结果中以有界详情返回；prompt 不写入 AgentNave 日志或持久存储。AgentNave 自身的中间文件使用系统临时目录：目前仅 Grok prompt 使用标准库临时文件，调用结束清理；其余 prompt 走 stdin，结果留在内存。Manager 创建的任务交接/中间文件同样使用任务临时目录并管理读取生命周期，不强制回答正文格式。项目正式产物与 Provider 自己的会话、缓存不属于此临时文件所有权。绝对 `cwd` 传给 Provider 进程，项目规则加载仍由 Provider 原生实现负责，不能把临时文件目录替代为 cwd。

AgentNave 的 best-effort 进程监督只支持 POSIX（macOS／Linux）。Windows 没有用 `CREATE_NEW_PROCESS_GROUP` 冒充进程树清理；如需原生 Windows，必须先引入 Job Object 的 kill-on-close 所有权。
