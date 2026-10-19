# Pi Coding Agent

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "pi"`；未配置 App 默认值且省略模型和 effort 时沿用 Pi 原生设置，不指定默认模型。
用户选择模型时传 `model`（可用 `provider/model`）；选择思考等级时传 `effort`，映射为 `--thinking`，支持 `off`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`。

首次使用先调用 `describe_provider("pi")`。支持的其他选项为 `provider`（模型后端，须同时提供 `model`）、`tools`（原生工具允许列表）和布尔 `restricted_tools`。普通任务保留原生工具与配置；Pi 没有内置的文件或进程权限隔离。

使用结果返回的 `session_id` 在相同 cwd 续接，Adapter 映射为 `--session`，不使用自动创建会话的 `--session-id`。`restricted_tools` 显式关闭工具、扩展、MCP、Skill 与 prompt template 发现，保留原生会话；只接受模型、effort 与模型后端 `provider`。当前受限配置仍自动加载工作目录中的 `AGENTS.md`，不等于只接收 prompt 中的材料。

安装：`npm install -g --ignore-scripts @earendil-works/pi-coding-agent`。运行 `pi` 后通过 `/login` 完成认证。真实调用沿用已有凭据，不把密钥放入 prompt 或 Provider Options。

## 运行中消息

普通 Pi 任务需要追加约束时，显式传 `start_agent(enable_messages=true)`，要求 Pi >= 1.1.0（协议核验基线 1.1.0）；所选可执行文件版本不可识别或过旧时拒绝，不换 CLI。该参数属于 AgentNave 请求，不放进 `provider_options`。默认调用继续使用 print/JSON；启用后使用原生 RPC，模型、effort、权限和 cwd 仍来自原请求与既有默认值。

用 `send_agent_message(invocation_id, message)` 向原任务发送 steer；文本最多 12000 字符，后续仍用同一 invocation 等待。`accepted` + `queued_by_provider` 只表示入队；`handled_by_provider_extension` 表示扩展处理，不保证模型收到。`rejected` 表示原生拒绝；`not_delivered` 表示未写入；`unknown` 表示写入尝试没有确认，不自动重发；`history_saved=false` 表示回执未落盘，需要先保留回执。消息历史可由 `read_conversation` 核对。

Steering 在当前 assistant turn 与工具执行结束后、下一次模型请求前消费，不会中断正在执行的工具。`prompt` response 和 `agent_end` 都不是最终完成；AgentNave 等 `agent_settled` 后收集最终回答并清理进程。协作组席位未开放消息，完成后仍用 `start_agent(session_id=...)` 续接；发送工具不暗中创建新轮。
