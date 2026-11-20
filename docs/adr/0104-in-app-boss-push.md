# ADR-0104：App 内 boss 由 App 提示收件

- 状态：已接受
- 日期：2026-11-20
- 修订：[ADR-0007](0007-wait-any-ready-subset.md)、[ADR-0064](0064-dispatch-receipts-steer-to-wait.md) 的推送边界。

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
