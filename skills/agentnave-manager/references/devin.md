# Devin CLI

`provider: "devin"`。首次使用先读 `list_members(provider="devin")` 的可用模型和选项；
`model` 可省略以沿用 Devin 原生设置，指定时用 Devin 模型目录给出的 ID。Devin 作为 boss 时，
同类私聊被排除，仍可参与会议。

AgentNave 通过 Devin 的非交互 `-p --prompt-file` 派发，保留原生 ATIF 导出中的
`session_id`、回复和模型；续接传该 `session_id`。Devin 的 `--permission-mode` 中，
App「自动」档对应 `smart`，由 CLI 的快速模型判断哪些操作可自动执行；「完全放开」对应
`dangerous`。两档均可能遇到 CLI 原生工作区信任检查，未信任目录中的 print 模式无法弹出
确认框，会返回受阻；先通过 Devin 自身的信任流程处理目录，再派发。不要由 boss 自行传
`respect_workspace_trust=false` 或其它权限选项绕过检查。

Devin 还支持显式 `sandbox`、`respect_workspace_trust` 和原生的其它 `permission_mode`；
权限类选项只按主 Skill 的授权边界传入。`restricted_tools` 不支持。CLI 报工具确认拒绝时，
按受阻回执处理，核对实际产物，不以回复文字代替完成证据。

用户级 MCP 配置为 `~/.config/devin/mcp_config.json`（Windows 位于
`%APPDATA%\devin\mcp_config.json`），Skill 在 `~/.config/devin/skills/agentnave-manager`
（Windows 位于 `%APPDATA%\devin\skills\agentnave-manager`）。修改后重开 Devin 会话。
