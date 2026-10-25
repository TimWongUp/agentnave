# Antigravity CLI

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "antigravity"`；默认 `provider_options`：

```json
{"model": "gemini-3.8-flash", "effort": "high"}
```

对应 `agy` CLI。原生 `print_timeout` 由 CLI 执行，与 AgentNave 的单次等待时长是不同参数；仅在用户明确要求时设置。

`"dangerously_skip_permissions": true` 开启原生 YOLO（`--dangerously-skip-permissions`），本次调用自动批准全部原生工具权限请求，CLI 已存配置不变；仅在用户授权绕过权限时使用。无 App 默认值时省略即继承原生设置，传 `false` 显式不加该参数。`mode` 与 `sandbox` 是独立选项，不代表 YOLO。

## 显式受限工具

`restricted_tools=true` 要求专用 `cwd` 事先含 `.agents/agents/agentnave-discussion.md`，内容须与下列定义一致；缺失或不符时 Adapter 返回错误，不代写角色文件。Studio 的隔离席位会自动准备；单任务由调用方在专用目录中准备，保持现有项目目录不变。

```markdown
---
name: agentnave-discussion
description: Dialogue-only participant for AgentNave discussion rooms.
tools: []
mcpServers: []
skills: []
plugins: []
mainAgent: true
subagent: false
commandExecutionPolicy: off
---
You are a conversation participant. Answer the current user request.
```

## 项目目录

在项目内执行任务时，将项目绝对路径传给 `cwd`，并在交接 prompt 中明确同一路径。要求 CLI 将终端工具的 `Cwd` 显式设为该绝对路径，先执行只读 `pwd`，确认一致后再进行项目操作；后续项目命令沿用该目录。目录不符或无法设置时，停止项目操作并报告实际目录。临时交接文件的位置不改变目标目录。

`cwd` 设置 CLI 进程启动目录，不限制内部 Agent 为终端另选 `Cwd`；上述交接要求是行为约束，不是目录隔离或强制保证。验收时核对实际执行证据，不能仅凭启动参数判定目录正确。

`provider_options.project` 用于选择原生项目 ID 或名称，仅在已核对对应项目且调用方明确选择时传入。它不等同于文件系统工作目录；不要把项目路径自动填入该参数作为目录修复。会话续接保持已选择的原生项目，并明确本轮目标目录；不同目录的任务不沿用旧的目录核验结论。
