# 架构与边界

AgentNave 是供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用的 MCP Server。它在 Manager 与 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 之间提供很薄的进程边界，不是面向人的 CLI 产品、多智能体运行时或工作流引擎。

## 所有权

Manager 拥有目标解释、任务拆解、角色和模型选择、并行策略、重试、审核循环、结果综合、工作树和长期任务状态。

AgentNave 只拥有：

- 根据冻结请求启动一个 Provider CLI；
- 传递 prompt、cwd、可选 `session_id` 和显式 Provider Options；
- 归一化 Provider 的终端结果；
- 执行超时、取消和进程树清理；
- 在 MCP server 生命周期内保存 Invocation 句柄。

AgentNave 不拥有 DAG、调度器、角色系统、自动重规划、工作树、SQLite／JSONL、恢复、保留策略、桌面 App 或 TUI。

## 安装与激活边界

完整安装只包含两个 AgentNave 自有部分：由 Python Tool Manager 安装的隔离运行时，以及安装到各 Host 的 `agentnave-manager` Skill。两者必须来自同一个已发布版本；MCP-only 可以调用 Tool，但缺少 Manager 路由和生命周期说明，Skill-only 则不能形成可工作的 AgentNave 安装。

Python Tool Manager 拥有运行时的安装、升级与卸载，并提供稳定的 `agentnave-mcp` launcher。源码 checkout 只用于贡献、调试和未发布版本验证，不作为 MCP Host 的长期启动路径。

MCP Host 拥有 AgentNave 的注册、作用域、启停、移除和 Skill 发现位置；能使用 Host 官方管理接口时，不由 AgentNave 直接修改其配置文件。安装运行时不会自动写入 Host Skills、全局规则或权限。Provider CLI 继续拥有自身的安装、认证、配置与 Session 数据，AgentNave 不创建需要随卸载处理的持久用户数据。

## 稳定合同

请求字段为 `provider`、`prompt`、绝对 `cwd`、可选 `session_id`、`timeout_seconds` 和 `provider_options`。Provider Options 必须由调用方显式给出并通过对应 Adapter allowlist；AgentNave 不默认覆盖模型、effort、权限模式、工具或 Provider 原生配置。

随包提供的 Manager Skill 可以定义默认模型路由，但调用时仍须把模型与 effort 作为显式 Provider Options 传入；默认决策不下沉到 Adapter。
Codex 在非 Git 目录运行时，调用方可显式传入布尔选项 `skip_git_repo_check`；Adapter 默认不绕过 Provider 的仓库检查。

结果字段为 `status`、`provider`、`output`、`session_id`、`provider_usage`、`duration_ms` 和 `error`；`provider_usage` 只保留 Provider 可用的 `num_turns` 与 `total_cost_usd`，不转发 token、cache 或 model 明细。Provider 正常返回业务失败仍是完整的 Invocation Result。Provider 缺失、无法启动或平台不受支持也会形成带 `launch_error` 的结构化失败结果，以便 Manager 读取。

STDIO MCP 是唯一公开接口，只暴露 `start_agent`、`wait_agent` 和 `cancel_agent`；`agentnave-mcp` 只负责为 MCP Host 启动 server 进程。三个 Tool 都发布输入与输出 JSON Schema；可由 Manager 修正的请求错误使用 MCP Tool error 返回重试指引，Provider 执行终态使用结构化 Invocation Result。继续 Provider 对话通过新的 `start_agent(session_id=...)` 完成。

`wait_agent` 超时但 Invocation 仍在运行时返回 `snapshot`，只包含 `phase`、`elapsed_ms` 和 `last_event_age_ms`。Antigravity、Claude、CodeBuddy、Codex 与 Grok Adapter 分别消费 Provider 官方的 `stream-json`、`stream-json`、`stream-json`、JSONL 与 `streaming-json` 事件流；Snapshot 只记录最近事件时间，不解析或声称 Provider 的语义进度。终态 `output` 只保留 Provider 最终回答，其中 Codex 取最后一个完成的 `agent_message`，Grok 取结束前最近一段连续 `text` 事件。

Invocation 状态只存在于当前进程内。每次 Invocation 由一个专用 supervisor 持续占有 POSIX 进程组，Provider 正常终止后也先清理该组再回收 supervisor，避免旧 PGID 被复用。MCP server 退出时会尽力终止仍留在该组内的活跃进程；重启后旧 Invocation 句柄不可恢复。Provider 自己持久化的 Session 不受此限制。

AgentNave 不是沙箱或同用户恶意进程隔离边界。已获得命令执行权限的 Provider 或工具可以主动创建新 OS session、杀死 supervisor，或以其他方式脱离普通 POSIX 进程组；发生可检测的 supervisor 丢失时返回 `supervision_lost`，但不能安全地对可能已被复用的旧 PGID 继续发信号。是否允许这些命令由 Provider 原生权限机制和 Manager 决定；需要抵抗恶意同用户进程时，应在 AgentNave 外使用降权、容器或平台级资源域。

## Provider Adapter

Adapter 只能添加非交互输出、prompt 传输和 cwd 等协议必需参数。Provider stderr 仅在失败结果中以有界详情返回；prompt 不写入 AgentNave 日志或持久存储。

AgentNave 0.2 的 best-effort 进程监督只支持 POSIX（macOS／Linux）。Windows 没有用 `CREATE_NEW_PROCESS_GROUP` 冒充进程树清理；如需原生 Windows，必须先引入 Job Object 的 kill-on-close 所有权。
