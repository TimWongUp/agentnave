# ADR-0111：托盘为 CLI 会话自绘对话视图，按 CLI 选驱动协议

- 状态：已被 [ADR-0113](0113-chat-desktop-split-out.md) 取代（对话界面移到另立的聊天桌面端项目；对 ADR-0056、ADR-0072 依据的事实更正保留）
- 日期：2026-11-22
- 修订：[ADR-0103](0103-tray-embedded-terminal.md) 的「真终端加皮肤，不做结构化对话界面」；[ADR-0109](0109-in-app-derived-sessions.md) 的展示与驱动方式（派生会话不再只开在终端里，一轮的边界改由各驱动协议给出）。
- 更正依据：[ADR-0056](0056-claude-interactive-runtime.md) 关于第三方入口名的判断；[ADR-0072](0072-antigravity-interactive-runtime.md) 的省额度前提。两篇的运行方式本 ADR 不改。
- 沿用：[ADR-0105](0105-codex-app-server-terminal-push.md) 的私有 `app-server`；[ADR-0109](0109-in-app-derived-sessions.md) 的派发、结算合同与进程归属（托盘拥有）；[ADR-0080](0080-tray-video-loopback-http.md)（托盘不监听端口）。

## 背景

App 里的 agent 和派生会话都是把 CLI 自己的 TUI 嵌进 xterm.js。用户希望得到接近 Claude 桌面端的体验：消息、工具调用、diff、权限确认、计划都由 App 画成统一的界面。嵌终端做不到这一点，托盘也只能靠 OSC 7501、读屏、读原生转录和模拟按键来猜回合边界、阻塞与取消结果；ADR-0109 验收中「后台任务拖住 Claude 回合结算」就是这类问题。

Claude 桌面端与开源的 T3 Code（MIT）都是自绘界面、底下用结构化协议驱动 CLI：T3 Code 的 Claude 走 Agent SDK，Codex 走 `app-server`，Antigravity 走 ACP，Pi 走 RPC，另有一批通用 ACP 代理。

额度方面（2026-11-22 核查）：

- Claude 订阅下，`claude -p`／Agent SDK（入口 `sdk-cli`／`sdk-ts`）消耗 5 小时窗口约为交互模式（入口 `cli`）的 1.8 倍，多人独立复现，官方未回应（anthropics/claude-code#97074、#107106）。单变量对照显示差别只跟 `CLAUDE_CODE_ENTRYPOINT` 有关；T3 Code 把它设成 `t3-code`，作者称获得 Anthropic 非正式许可。ADR-0056 里「第三方入口名改走额外用量」与这些证据不符。
- Antigravity 官方答复（google-antigravity/antigravity-cli#1411）：交互模式、`agy -p`、`agy acp` 共用同一额度，每个 token 的权重相同；`-p` 显得更费是因为每次都是新会话、缓存是冷的。ADR-0072 的省额度前提不成立。

## 决定

### 1. 托盘为它拥有的 CLI 会话提供自绘对话视图

- 范围：办公室里的 agent 与派生会话，即所有由托盘启动并拥有进程的 CLI 会话。
- 右栏默认显示对话视图；同一原生会话可以切到终端视图，作为高级功能和协议没覆盖到的情况的兜底。
- 没有可用驱动协议的 CLI 继续只有终端视图。

### 2. 托盘把各协议归一为同一组对话事件

界面只认这组事件，不按 CLI 分支：用户消息、助手正文与思考、工具调用及其结果、diff、计划、权限请求与提问、用量、回合开始与结束（含结束原因）。派生会话一轮的结算（ADR-0109 第 5 节）改用这组事件里的回合边界与结束原因。

一轮以协议给出的回合结束为界（Claude 为本轮 `promptId` 的 `turn_duration`，ACP 为 `session/prompt` 的 `stopReason`，Codex 为 `turn/completed`）。之后同一会话里继续到达的更新（后台命令补报、CLI 自己发起的后续回合）照常显示，但不归入已结算的那一轮。

### 3. 按 CLI 选驱动方式

| CLI | 驱动方式 | 对话视图与终端视图的关系 |
| --- | --- | --- |
| Claude Code | 交互 TUI 跑在托盘的伪终端里（入口仍为 `cli`），对话事件从原生转录读出，权限框与提问由 OSC 7501 加读屏识别，界面上的回答转成按键送回 TUI。做法参考 magpie 的 `claude-tui-helper` 与服务现有的 `claude_tui`。 | 同一个进程的两种视图，随时切换 |
| Codex | 私有 `app-server`，托盘作为客户端直接收发 | 对话视图与 `--remote` TUI 是同一线程的两个客户端 |
| Antigravity、CodeBuddy、Devin、Grok、DeepSeek Harness | 一套通用 ACP 客户端，每家一层薄适配：启动参数、认证、续接、模型与模式、权限选项、私有扩展字段 | ACP 的会话 ID 就是原生会话 ID；切到终端视图时结束 ACP 进程，在 TUI 里按同一 ID 续接（Antigravity、CodeBuddy 已实测双向互通）。DeepSeek Harness 没有交互终端，只有对话视图 |
| Pi | 自有 RPC（`--mode rpc`），单独适配 | 同一会话可用 TUI 续接 |
| ZCode | 自有协议（`app-server --stdio`，NDJSON），单独适配 | 待实现时确认 |

- Claude 不用 `-p`、stream-json 输入或 Agent SDK，也不设置 `CLAUDE_CODE_ENTRYPOINT`：前者多耗订阅额度，后者没进官方文档、许可只给了 T3 Code。
- Antigravity 的托盘会话（含派生会话）改走 ACP，不再用 TUI：额度相同，ACP 有结构化的取消与续接，新目录也不弹信任框。
- 能走 ACP 的 CLI 一并获得派生会话支持的前提：ADR-0109 因 CodeBuddy 不发 OSC 7501 而暂不支持它，ACP 给出了回合边界与运行阶段，这条限制随之解除。

### 4. 协议客户端放在 Rust 托盘里

- 不引入 Node 侧车，进程归属与 Windows Job Object 清理保持在一个运行时里。
- ACP 用官方 Rust 库 `agent-client-protocol`（agentclientprotocol 组织维护，Apache-2.0，含客户端实现）。
- T3 Code 作为参考：照搬或改写它的协议处理、各家约束和测试样本时，保留其 MIT 版权与许可声明。

### 5. boss 唤醒随驱动方式走

App 推送唤醒 boss（ADR-0104、ADR-0105）时，ACP、Codex、Pi、ZCode 会话经协议提交固定短提示，Claude 仍键入 TUI。

## 实测结论（2026-11-22，macOS）

- **Antigravity 1.5.14（`agy acp`）**：`loadSession`、`session/resume`、`list`、`close` 均可用；`session/load` 重放历史，`session/resume` 不重放。`session/cancel` 让回合以 `cancelled` 结束，并结束前台正在跑的命令。长命令超过等待阈值会转后台，`session/prompt` 先以 `end_turn` 返回，命令结束后 agy 再补推更新。用户设置为 `toolPermission: always-proceed` 时不发权限请求；`--settings` 对 `acp` 无效，权限请求的实际形态未抓到。`session/new` 偶发失败，重试可恢复。不发 `fs/*`、`terminal/*` 委托。
- **CodeBuddy 2.185.0（`codebuddy --acp`）**：stdio 可用；默认模式下发 `session/request_permission`（`allow_always`／`allow_once`／`reject_once`），拒绝后回合以 `cancelled` 结束；取消会结束前台命令；`session_info_update` 带运行阶段（含等待权限）。`session/load` 对不存在的 ID 不报错、静默新建空会话；客户端声明 `fs`、`terminal` 能力时，它会把读文件和 shell 委托给客户端。同一 `toolCallId` 会多次收到 `tool_call`。
- **Grok 1.0.92（`grok agent stdio`）**：官方 ACP，续接、列出、取消实测可用；权限请求要在 `initialize` 的 `_meta` 声明 `clientType: "extension"`，本机配置下未触发。
- **DeepSeek Harness 0.2.2-rc.1（`dsh --profile acp`）**：官方 ACP，只提供 `session/resume`（不重放历史）、`list`、`close`，不支持 `session/load`；取消可用；权限只有允许一次与拒绝一次，本机未触发。
- **Pi 1.6.0（`pi --mode rpc`）**：自有 RPC，`abort` 可取消，`--session` 续接；没有逐次权限审批，只有扩展发起的对话框。
- **ZCode 0.17.1**：自有协议，不是 ACP，也不是严格的 JSON-RPC 2.0（带 `jsonrpc` 字段会被拒绝）；有 `interaction/requestPermission`、`session/stop`、`session/resume`。
- **共性**：Antigravity 与 CodeBuddy 的 `session/load` 都不保留模型、推理档与模式，续接后要重新设置。

ACP 适配层据此统一处理：声明 `fs`、`terminal` 能力为 false；续接前先确认原生会话存在；续接后重设模型与模式；按 `toolCallId` 合并工具调用；`session/new` 失败时重试一次；回合结束后的更新按第 2 节处理。

仍未核实：Antigravity、Grok、DeepSeek Harness、ZCode 权限请求的实际形态（本机配置都是自动放行）；同一会话被 TUI 与协议客户端同时打开时的行为。

## 取舍

- **自绘，而不是继续只做终端皮肤**：换来统一界面和精确状态；代价是每家功能要自己实现，CLI 升级后界面要跟进。终端视图保留，覆盖没实现的功能（如 Antigravity TUI 独有的大部分斜杠命令）。
- **Claude 读转录加读屏，而不是 `-p`／SDK**：读屏比协议脆弱，Claude Code 升级可能要跟着调；但 `-p`／SDK 的额度代价是持续的。改入口名能省额度，但没有书面许可，不采用；官方给出书面规则后重新评估。
- **Antigravity 走 ACP，而不是继续 TUI 再自绘**：TUI 方案要读屏和读内部转录，权限框在 ADR-0072 的边界下还接不进来；额度上两者一样，会话又能在 TUI 里续接。
- **一套 ACP 客户端加薄适配，而不是每家一套驱动**：ACP 是公开协议，五家已支持；Pi 与 ZCode 各自的协议单独适配。
- **Rust，而不是 Node 侧车直接用 T3 Code 的包**：T3 Code 基于 Effect，包是 monorepo 内部的，本来也要改写；侧车会把进程归属拆到两个运行时，还要多带一个运行时；ACP 已有官方 Rust 库。

## 后果

- 实现时更新 `docs/context/architecture.md`：托盘经协议驱动 CLI 会话、对话事件是托盘内部的归一层；进程仍归托盘。
- CONTEXT.md 的「派生会话」「agent」「Tray App」按对话视图与终端视图两种视图更新。
- 服务后台运行 Antigravity 是否改回 stream-json 或 ACP 另行决定；本 ADR 只更正 ADR-0072 的前提。
- 各 CLI 的用户权限设置照常生效：设为自动放行时不会有权限请求进入对话视图。
