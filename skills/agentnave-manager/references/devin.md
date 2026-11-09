# Devin CLI

`provider: "devin"`。首次使用先读 `list_members(provider="devin")` 的可用模型和选项；
`model` 可省略以沿用 Devin 原生设置，指定时用 Devin 模型目录给出的 ID。Devin 作为 boss 时，
同类私聊被排除，仍可参与会议。

AgentNave 通过 Devin 自带的 ACP 服务端（`devin acp`）派发，运行中能看到思考、工具和回复进度；
续接传回执里的 `session_id`。`model` 只能取本账号在 Devin 里可用的模型，不可用时任务失败并列出
可用模型，不会悄悄换成别的模型。`permission_mode` 接受 `accept-edits`、`smart`、`ask`、`plan`、
`bypass`，`dangerous` 等同 `bypass`；App「自动」档对应 `smart`，「完全放开」对应 `dangerous`。
`smart` 判断不了的操作会作为待答弹框交给 boss，按主 Skill 的代答边界允许或拒绝。

Devin 还支持显式 `sandbox`（把命令写入限在工作区内）；权限类选项只按主 Skill 的授权边界传入。
`restricted_tools` 不支持。

用户级 MCP 配置为 `~/.config/devin/mcp_config.json`（Windows 位于
`%APPDATA%\devin\mcp_config.json`），Skill 在 `~/.config/devin/skills/agentnave-manager`
（Windows 位于 `%APPDATA%\devin\skills\agentnave-manager`）。修改后重开 Devin 会话。
