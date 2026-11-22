# ADR-0104：App 内 boss 由 App 提示收件

- 状态：部分修订
- 日期：2026-11-20
- 修订：[ADR-0007](0007-wait-any-ready-subset.md)、[ADR-0064](0064-dispatch-receipts-steer-to-wait.md) 的推送边界。
- 修订：Codex 暂不推送的排除由 [ADR-0105](0105-codex-app-server-terminal-push.md) 修订（Unix 改走私有 `app-server` 结构化投递，Windows 维持不推送）；自定义档中逐字匹配的旧生成等待句迁移由 [ADR-0107](0107-specialist-scope-closing-and-fallback-note.md) 修订；不新增 MCP 工具、仅推 `inbox/` 的边界、唤醒提示合并与 `wait` 收件箱模式扩展至派生事件（含已读确认同步）由 [ADR-0109](0109-in-app-derived-sessions.md) 修订。

## 背景

App 内嵌终端由 App 管理，可以在结果就绪时唤醒其中的 boss；其他宿主没有共同的推送通道。

## 决定

App 内可推送的 boss 派发后可以结束本轮。服务在数据根发布未收事件和收件箱阻塞等待状态，App 在 CLI 可接收输入时向终端键入提示；boss 仍用收件箱 `wait(wait_seconds=0)` 收取。服务依据 App 进程祖先及其声明的 provider 判断能力，回执 `next` 告知本连接的等待方式。服务和 App 分别拥有各自的进程级文件，状态变化时更新，退出时删除。

App 外 boss 保持原有等待方式，`wait=false` 默认值和收件语义不变。用户自定义规则不改写。

## 取舍

- 状态来源采用 CLI 原生 OSC 7501，不安装宿主钩子。
- `/clear` 后新聊天调用 AgentNave 前，旧聊天提示可能进入新聊天；新聊天 `wait(0)` 可能只得到 `nothing_pending`，接受这一代价。
- Codex 暂不推送，另行评估 app-server 路线；Devin、CodeBuddy 和 ZCode 继续使用等待。

## 不做

不新增 MCP 工具、心跳、定时写盘或 App 外宿主钩子；不改变 App 外等待行为。

2026-11-21 阻塞等待与未提交撤回修订：服务发布的「阻塞等待状态」同时覆盖显式 `ids` 阻塞 `wait`（含 `start_agent`／`run_round` 的 `wait=True`，仅标调用方所属工作区，不改变收件箱 `already_waiting` 互斥）；取舍是显式等待期间同工作区其余已就绪事件暂不推送，待本次等待结束后再推送。托盘向 PTY 投递时在 `begin_wakeup` 前与 300ms 持键后提交 `\r` 前均复查最新 `inbox`，若对应事件均已离场或 boss 已进入阻塞等待则退格撤回未提交提示，不发送回车。上文「决定」就此补充，其余不变。
