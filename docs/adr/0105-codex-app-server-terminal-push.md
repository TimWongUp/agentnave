# ADR-0105：Codex 内嵌终端改用私有 app-server 与结构化推送

- 状态：已接受
- 日期：2026-11-20
- 修订：[ADR-0103](0103-tray-embedded-terminal.md) 的 Codex `--no-daemon` 运行方式与 [ADR-0104](0104-in-app-boss-push.md) 的 Codex 推送排除。

## 背景

[ADR-0103](0103-tray-embedded-terminal.md) 为把 Codex 留在终端进程树内采用了 `--no-daemon`，[ADR-0104](0104-in-app-boss-push.md) 因 Codex 不输出 OSC 7501 而暂不对其做 App 内推送。实测 Codex `app-server` 支持私有 Unix 域套接字（WebSocket over UDS）与 `--remote` TUI 分离前端，托盘可作为第三方客户端监听线程状态并向指定线程投递，不碰 PTY 输入与草稿。

## 决定

- **Unix 内嵌终端改用进程树内私有 `app-server` + `--remote` TUI。** 在 macOS／Linux 上，托盘启动或续接 Codex 时，在伪终端内通过 PTY 子进程 wrapper 先后台起私有 `codex app-server --listen unix://<短路径，含终端 id>`，就绪后再前台起 `codex --remote unix://… -C <cwd>`（续接为 `codex resume --remote unix://… -C <cwd> <id>`），两者都传 `-c check_for_update_on_startup=false`，且 `--remote` 的 TUI 不传 `approval_policy`／`sandbox_mode` 覆盖。wrapper 作为 PTY 子进程确保 `app-server` 拉起的 MCP 中继仍在终端进程树内，托盘的进程树归属与服务端的 `app_can_push` 祖先链检测照常成立；关闭终端或 App 退出时沿用按进程组结束（POSIX `killpg`）。
- **托盘通过 WebSocket over UDS 结构化投递。** `push/<pid>.json` 的 `providers` 在 Unix 上加入 `codex`（仅当终端以 `app-server` 模式运行时实际投递）。托盘作为第三方客户端连同一 socket（先 `initialize` 声明 `capabilities.experimentalApi: true`，再发 `initialized`），结合 `thread/loaded/list`、`thread/read` 与 `thread/started`／`thread/status/changed` 维护线程集合和状态，将终端当前 `boss_key` 按 `boss_key("codex", threadId)` 对应回目标线程。存在未收事件、boss 未在 `wait` 且目标线程不处于 `waitingOnApproval`（亦非未加载）时，通过 `thread/queue/add`（`clientUserMessageId` 取事件键去重）投递与唤醒模块相同的单行提示，不经 PTY 输入门，不碰用户草稿；若实验接口报错，则在线程空闲时退回稳定接口 `turn/start`（不带权限覆盖）。60 秒未送达的未收标记沿用既有逻辑。
- **Windows 不做。** Windows 端维持 `--no-daemon` 启动与续接，不开启 `app-server` 模式与 App 内推送。

## 取舍

- **依赖 experimental 接口与降级。** `thread/queue/add` 属于 `experimentalApi`，支持在回合进行中排队并用 `clientUserMessageId` 去重，避免草稿和并发冲突；代价是接口随 Codex 版本演进可能变化或被关闭，因此实验接口报错时仅在线程空闲时降级到稳定 `turn/start`，忙时不注入以免被当成 steer 并入当前回合。
- **wrapper 与进程组。** 由托盘在终端进程树外直接拉起 `app-server` 会使 MCP 中继脱离终端进程树并导致 `links` 与 `app_can_push` 失效；用 PTY 内 wrapper 依次拉起 `app-server` 与 `--remote` TUI 维持了单一终端进程树，代价是关闭 PTY master 本身未必带走双进程，必须依赖整组清理及 wrapper 退出清理。
- **Windows 不做。** Windows 缺少本次验证过的 UDS 传输与服务端 `app_can_push` 支持，维持 `--no-daemon` 与无推送以控制跨平台风险。

2026-11-21 已读撤回与缩容修订：投递前在解析目标线程之后复查 `inbox`，确认未读且 boss 未阻塞等待再发送；`thread/queue/add` 成功返回的 `queuedSubmission.id` 随对应事件键列表保存在托盘内存中。每轮扫描在投递前按最新原始 `inbox` 对账：该条排队唤醒对应的事件全部离开 `inbox` 时调用 `thread/queue/delete` 撤回；仅部分离开且仍有未读事件时调用 `thread/queue/update` 将排队正文缩减为剩余事件。`thread/queue/delete` 与 `thread/queue/update` 同属 `experimentalApi`；若已出队、返回 `deleted: false`、实验接口报错或此前已降级走 `turn/start`，仅移除本地追踪，不发送 `turn/interrupt` 强行中断回合，也不计入失败或未送达标记。上文「托盘通过 WebSocket over UDS 结构化投递」与「依赖 experimental 接口与降级」就此补充，其余不变。
