# ADR-0113：多 agent 聊天桌面端另立项目，AgentNave 保持以 MCP 编排为主

- 状态：已接受；「Git 与 PR 归新项目」一条由 [ADR-0115](0115-tray-git-status-and-basic-ops.md) 修订：只读状态与基础操作回到托盘
- 日期：2026-11-23
- 取代：[ADR-0111](0111-self-drawn-conversation-view.md)（托盘自绘对话视图）、[ADR-0112](0112-mobile-remote-via-tailscale.md)（手机经 Tailscale 访问托盘）。
- 恢复：[ADR-0103](0103-tray-embedded-terminal.md)「真终端加皮肤，不做结构化对话界面」与 [ADR-0109](0109-in-app-derived-sessions.md) 的展示与驱动方式（派生会话开在 App 终端里）；[ADR-0080](0080-tray-video-loopback-http.md) 的托盘监听边界不再扩展。
- 保留：ADR-0111 对 [ADR-0056](0056-claude-interactive-runtime.md)、[ADR-0072](0072-antigravity-interactive-runtime.md) 依据的事实更正。
- 沿用：[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)（不替 CLI 设参数、不增补 prompt）。

## 背景

ADR-0111 要在托盘里自绘接近 Claude 桌面端的对话界面，按 Antigravity → 其余 ACP → Codex → Claude 分期推进。第一期做完 Antigravity（含 Git 信息栏、提交/推送/PR 按钮、PR 监听与 Auto-fix），改动约 2 万行，还没开 PR。验收时用户提出：会话要按账号分组，点开任一会话就能直接聊，不用先点「续接」，体验要与 Claude 桌面端对齐。

对照开源的 T3 Code（MIT）后，用户认为它的基础设施和各家接入应当整体照搬：会话按需拉起、续接与空闲回收，每个账号一个独立配置目录，CLI 版本由程序管理。但 T3 Code 的做法与 AgentNave 的现行边界相反：它为每家 CLI 显式下发模型与权限、注入自己的编排指令，并让 Antigravity 使用私有 `GEMINI_HOME`，把用户 `~/.gemini` 里的 MCP 与 hooks 隔离在外。AgentNave 则要求 Adapter 不替 CLI 设参数、不增补 prompt，并使用真实 HOME 与真实登录。

## 决定

- 多 agent 聊天桌面端另立一个项目（暂名 agent-desk），负责对话界面、按账号分组、Git 与 PR、Auto-fix 和手机端，基础设施按 T3 Code 来做，规则由它自己定。
- AgentNave 回到以 MCP 编排为主，负责派发、会议、工位、派生会话、托盘办公室与语音，以及各家宿主桌面端的接入。新项目对 AgentNave 来说只是又一个宿主，里面的 agent 经 `agentnave-mcp` 使用编排。
- 托盘不做自绘对话视图，也不提供手机远程入口；办公室 agent 与派生会话继续开在 App 终端里，按 ADR-0103、ADR-0109 运行。
- 第一期分支里修复已有终端与办公室界面的改动单独合入 AgentNave，对话视图、Git 信息栏、PR 工作流与 Auto-fix 留在分支上，作为新项目的素材。

## 取舍

- 不在 AgentNave 里照搬 T3 Code：照搬就要放弃 ADR-0028、ADR-0032 与真实 HOME 的约束；破例一项，后面每家 CLI 都要再破，规则就失效了。
- 不继续在托盘里按自己的边界自绘：要做到「按账号分组、点开就能聊」，就需要 T3 那套会话与账号基础设施。托盘还会因此长成第二个 T3，偏离 AgentNave 轻量、能跨宿主使用的定位。
- 拆成两个项目的代价：第一期的对话视图工作暂时不进 AgentNave 的正式版。两个项目要各自维护一份 CLI 接入知识，新项目里的 agent 能否稳定用上 `agentnave-mcp`，也要在新项目里单独验证。
