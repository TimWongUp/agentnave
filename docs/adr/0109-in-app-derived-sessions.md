# ADR-0109：派生会话是开在 App 终端里的子代理

- 状态：部分修订
- 日期：2026-11-22
- 第 1、3、5、6、7、8 节由 [ADR-0114](0114-tui-runtime-like-a-user.md) 修订：派生入口只看 `terminal_providers`；说明改用户口吻；一轮等本轮后台任务结束并按起止锚点拼接结果；有未结算派发时续派返回忙；`inbox/`／`push/` 新增字段。
- 第 1、2、4、6、7、9 节与取舍「终端的关闭交给用户」由 [ADR-0124](0124-derived-session-lifecycle.md) 修订：派生会话空闲时结束进程、随时可续，随 boss 关闭、办公室单独成区；`terminal_notes` 与 `read_conversation` 按会话读取暂不做。
- 修订：[ADR-0103](0103-tray-embedded-terminal.md) 的「终端只由用户在办公室里开」；[ADR-0025](0025-session-desks-and-running-limit.md) 的同时运行上限口径（终端模式只在 boss 派发的那一轮计入）；[ADR-0085](0085-service-managed-isolated-dirs.md) 的隔离目录租约持有方与自动释放时机；[ADR-0105](0105-codex-app-server-terminal-push.md) 的托盘 `app-server` 客户端（订阅线程回合、按轮中断、带来源标记投递）。
- 扩展：[ADR-0043](0043-service-prompt-instructions.md) 的服务派发说明（终端模式改用自己的一段）；[ADR-0104](0104-in-app-boss-push.md) 的 `inbox/`、`push/` 合同。
- 沿用：[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0108](0108-multi-account-isolation.md)（默认值、权限档位与账号在服务入口解析）；[ADR-0080](0080-tray-video-loopback-http.md)（托盘不监听端口）；[ADR-0098](0098-tray-deletes-isolated-dirs.md)（仓库锁与租约协议）。
- 修订：展示与驱动方式由 [ADR-0111](0111-self-drawn-conversation-view.md) 修订：派生会话默认显示对话视图，一轮的边界改由驱动协议给出；派发、结算合同与进程归属不变。ADR-0111 已被 [ADR-0113](0113-chat-desktop-split-out.md) 取代，此修订作废，恢复本 ADR 原条款。

## 背景

`start_agent` 派出的子代理在服务后台运行，用户只能在 App 里看它的消息，不能和它对话。用户希望有一种子代理开在 App 的内嵌终端里：派发、续派、收结果都和子代理一样，区别只是用户可以点开它的终端，看它干活、插话、帮它批准权限。

Claude 桌面端的派生会话（side sessions）是参照对象，它还带有分叉父会话上下文（fork）、移交（hand_off）、提案卡、多层嵌套、子会话之间互发消息和家族名册。评估后认为这些在 AgentNave 里收益不抵复杂度：派生代理的上下文由 boss 写进 prompt 或用 `forward` 转交，之后靠原生 `session_id` 续接，已经够用。

## 决定

### 1. 定位：`start_agent` 的终端执行模式

- `start_agent` 新增可省略的 `terminal` 参数，只对能收 App 推送的内嵌终端 boss 公布。传 `terminal=true` 时，服务照常解析工位、CLI、模型、账号、权限档位和隔离目录，但不在后台起进程，而是请托盘开一个内嵌终端运行该 CLI，并把任务作为首条消息交给它。
- 一次 boss 派发对应一个 `invocation_id`；一个派生终端对应一个原生 `session_id`，boss 的多次派发和用户在终端里的交谈都在这同一段对话里。用 `session_id` 续派时沿用终端模式；若 boss 显式要求以后台方式续接一个托盘仍打开着的会话，拒绝，避免两个进程同时续写同一原生会话。
- `start_agents` 不支持终端模式：每次派生都要先经用户同意（见第 2 节），不批量开终端。
- 派生终端在 App 左栏挂在派发它的 agent 下方。它的任务记录照常写进派发方的工作区历史，标明执行模式。

### 2. 派生前先问用户

boss 想用终端模式派发时，先在对话里问用户要不要派生、派给谁（工位或 CLI），用户同意并选定后才调用。用户本轮已经明确要求「开个终端让某某做」时视为已同意。用户选定的 CLI、工位或模型由服务落实，不可用就照实回报，不静默改派。这条只写在 Manager Skill 里，不做 App 确认卡片。

### 3. 派生代理的身份

- 可以和 boss 同类：终端模式豁免宿主同类排除与 `native_seat` 转交，例如 Claude boss 可以派生 Claude 终端。显式禁用、账号、冷却、工位选择照常生效。
- 能力和 boss 一样，可以用 `start_agent` 派后台子代理、开会议室，并以自己的会话身份收它们的结果；但**不能再派生**：托盘启动派生终端时带上派生标记，该终端里的服务连接不公布、并拒绝 `terminal` 参数。
- 服务的派发说明按终端模式另写一段：不再写「不调用 AgentNave 工具」和非交互说明，改为「可以派子代理和开会，不能再开派生终端」，其余回报要求不变。

### 4. 进程与结果的归属

- 派生终端的进程归托盘所有，不由服务的进程组 supervisor 监督；服务退出不结束、也不中断派生终端，用户之后仍可在里面继续。
- Invocation 仍只在服务进程内存中，代表 boss 派的那一轮；服务退出时在途的那一轮失联，与后台任务一样不跨重启恢复。
- 子终端进入可输入状态后，托盘把用户已同意的这次任务作为一条原生用户输入提交（含 `forward` 材料，保留既有的来源与数据标注）；不在启动页、审批框或信任框里回放文本。父终端的自动唤醒仍只键入固定短提示。

### 5. 一轮的结算

- 托盘按 CLI 判断一轮的开始与结束：Claude 与 Antigravity 看 OSC 7501，Codex 看私有 `app-server` 的 `turn/started`／`turn/completed`。托盘为每次派发记下本轮锚点（Claude 的 `promptId`、Codex 的 `turn_id` 与投递时带的 `clientUserMessageId`、Antigravity 的 `USER_INPUT` 位置）。
- 结束后由服务的 Adapter 按锚点从原生会话日志读出该轮最后的助手回复，组装成普通的任务结果，经 `inbox/` 推送、`wait` 收取，和后台子代理一样唤醒 boss。读不到就照实报原因，不拿别的轮次补位。
- 审批按键属于原来那一轮。用户在这一轮进行中插话：能确认仍是同一轮，就回报该轮结果并注明用户参与过；无法确认归属时回报「归属未确认」。
- 子终端停在权限确认、提问或目录信任上时，不替用户自动确认，沿用一次 `execution_blocked` 提醒 boss，用户在子终端里处理。
- `cancel(invocation_id)` 只中断这次派发的那一轮（PTY 中断或 Codex `turn/interrupt`），终端保留；托盘先核对该轮仍是这次派发，晚到的取消不能作用于之后用户发起的回合，无法安全确认时返回未能中断。

### 6. 用户自己聊的回合

- 用户在派生终端里发起的回合不唤醒 boss。boss 下次用无 `ids` 的收件 `wait` 时，附一条 `terminal_notes`：每个有新增的会话一条，含 `session_id`、新增轮数和最近一轮回复的截断预览（`include_output=false` 时只给计数）。附注不进 `pending_ids`，不影响 `all_finished`，不进 `follow_up`；只有附注时主动收件立即返回，正在阻塞等待其他派发时不因附注提前结束；显式 `ids` 的 `wait` 不消费附注。
- 「boss 已看过哪些回合」的位置随服务的工作区历史持久化，键为工作区、CLI、账号与原生会话 ID，值为原生轮次锚点；不放在随进程号删除的文件里。续派不算 boss 已看过。
- `read_conversation` 增加按本工作区拥有的派生会话 `session_id` 读取的入口，从对应账号的原生记录读出完整对话（含用户的回合），正文标为数据。

### 7. 并发、隔离、关闭与重启

- **同时运行上限**：boss 派发从接纳到结算（含等待用户批准期间）占一个 `max_running` 名额；空闲的派生终端和用户自己发起的回合不占，不另设终端数量上限。
- **隔离目录**：服务按 [ADR-0085](0085-service-managed-isolated-dirs.md) 创建；托盘在启动终端前按同一仓库锁协议接过使用租约，持有到终端关闭。首轮结算不清理终端仍在用的目录；终端关闭后再续派，若目录已释放，走现有恢复校验或明确失败，不悄悄切回主 checkout。
- **关闭与重启**：用户关掉终端或 App 重启时，在途的那一轮有可验证终态就按它结算，否则报中断或失联。之后 boss 明确续派时，托盘按原账号、目录和 `session_id` 恢复终端再提交；终端正忙（用户在聊、有草稿或停在审批）时续派返回忙，不排队。
- **新目录信任**：新隔离目录首次启动会弹信任确认的 CLI，在提交任务前预先处理：Codex 由托盘经私有 `app-server` 的配置写入接口把该目录写成信任，写进用户的 Codex 配置并保留，目录删除后也不清理；Antigravity 用运行时 `--settings` 的 `trustedWorkspaces`（含规范化后的路径）；Claude 沿用服务的做法，只在信任框显示的路径与派发目录一致时自动确认。处理不了时按停在信任框记录阻塞。

### 8. 服务与托盘的合同

- **数据根文件**：沿用 `inbox/<service_pid>.json`（服务写，任务完成与阻塞照常进入）与 `push/<tray_pid>.json`（托盘写，增加本实例支持终端模式的 CLI，以及按 `invocation_id` 关联的在途请求回执：已接单、原生会话与轮次锚点、阻塞、终态；回执保留到服务确认结算）。新增 `spawn-requests/<request_id>.json`：服务原子写入、托盘领取后删除，私有目录与文件权限，承载 `run`（结构化启动配置、任务正文与 `forward` 材料）和 `cancel`；送达不明时按原 ID 查询，不重复提交任务。
- **启动配置**：服务传结构化配置（CLI、账号与目录、工作目录、隔离信息、模型、档位、结构化权限、任务正文），不传拼好的命令行；托盘按各 CLI 的交互方式装配，例如 Claude 的外观主题与权限设置合并为一个 `--settings`，Codex 的模型与权限交给私有 `app-server`，不塞给 `--remote` TUI。

### 9. 第一批与分阶段

- **第一批（macOS）**：Claude Code、Codex、Antigravity。Grok、Pi、Devin 后续按能力接入；CodeBuddy 不发 OSC 7501、判断不了一轮何时结束与是否阻塞，暂不支持；DeepSeek Harness 没有交互终端；Windows 需先补服务侧的推送能力检测再实测；Linux 没有 App。
- **第一阶段**：终端模式参数与派生标记、`spawn-requests/` 与 `push/` 回执、服务的托盘执行路径与结算、三家按轮读取（Claude 从子会话自己的首条任务起读，Codex 新写 rollout 解析，Antigravity 新写转录解析）、按轮取消、隔离租约交接、同时运行上限、托盘开终端与提交任务、左栏显示。先验证两件最可能改变实现的事：Antigravity 能否沿用 `agy_tui` 的每进程 `--log-file` 取得会话 ID，Codex 托盘客户端能否从另一个连接接上子终端显示的线程。**验收**：三家用真实 CLI 各自完成首派、回报、同会话续派、按轮取消与隔离目录；Claude boss 派生 Claude 终端，用户在子终端批准一次权限后 boss 收到正确结果。
- **第二阶段**：`terminal_notes`、`read_conversation` 按会话读取、阻塞提醒、用户插话归属、关闭与重启后的续派、派生代理派子代理与禁止再派生的服务端校验、Manager Skill 的「先问用户」规则，以及预览版验收。三家都通过第二阶段才算第一批交付。

## Phase 0 实测结论（macOS）

- **Claude Code 2.1.330**：`--session-id` 在交互模式下绑定成功，位置参数传首条任务只执行一次；OSC 7501 普通一轮为 `idle → working → done`，需审批时为 `working → blocked:kind=permission → working → done`；审批按键只在转录里多一条内容为工具结果、`promptId` 不变的 user 行，可据此判定不是新回合；两个 `--settings` 时后者整体覆盖前者。
- **Codex 0.171.0**：`turn/completed` 的 `status` 为 `completed`／`interrupted`／`failed`，完成时附最终助手消息；需审批时 `thread/status/changed` 为 `active` 且带 `waitingOnApproval`。托盘作为观察连接，必须先对线程调用 `thread/resume`，否则收不到别的连接发起的回合通知（现有托盘客户端没调）。`turn/started` 不带来源，托盘发起的投递带 `clientUserMessageId` 即可与用户在 TUI 里亲自提交的回合区分，审批按键不产生新回合。rollout 可按 `turn_id` 取 `task_complete` 的最后助手回复。新 worktree 首次启动 TUI 会弹目录信任框；运行时 `-c` 覆盖信任对 `--remote` TUI 无效，经 `app-server` 的 `config/value/write` 写入信任可以跳过，但会持久化进 `~/.codex/config.toml`。
- **Antigravity 1.5.14**：`AGY_CLI_OSC7501=on` 下 OSC 完整（含 `blocked:permission`、`blocked:question`）；不能预先指定会话 ID；`-i` 传首条任务只执行一次，但在信任框期间提交的首条缺 `working`／`done`；`--conversation <id>` 可恢复；审批按键不产生 `USER_INPUT`。
- 原生 fork 在 Claude、Codex、Grok、Pi、CodeBuddy 上都实测可用，但本方案不采用（见取舍）。

## 取舍

- **不与桌面端完整对齐**：fork、hand_off、提案卡与待办、多层嵌套与链配额、子会话之间互发消息、子会话向父会话求助、等全部子会话停下再唤醒、名册工具、托盘私有家族库都不做。上下文由 prompt、`forward` 和 `session_id` 续接承担；需要协作时 boss 自己收结果再转交。代价是没有「一键把整段对话分出去」的能力。
- **复用 `start_agent`，不新增派生工具**：派发、续派、取消、收结果的语义和子代理完全一致，新工具只会让 boss 多记一套。
- **派生前在对话里问，不做 App 卡片**：问「要不要、派给谁」是对话里的一句话，用卡片要多一套阻塞等待与状态恢复。
- **不另设终端数量上限**：空闲终端不占派发名额，不会卡住派发和开会；终端的关闭交给用户。
- **任务作为原生用户输入交给子终端**：否则要另做一个让子代理读取任务正文的入口；父终端仍只收固定短提示。
- **Codex 目录信任写入后不清理**：运行时覆盖无效，只能持久化写入；与 Claude、Antigravity 自动确认后由 CLI 自己保留信任的做法一致，代价是用户的 Codex 配置里会累积已删除隔离目录的信任条目。
- **已读位置随服务历史持久化**：放在随进程号删除的 `inbox/` 文件里，用户聊完关掉终端、服务换代后提示就丢了。

## 后果

- AGENTS.md 与架构基线要写明：终端模式的进程归托盘，服务退出不清理它们。
- CONTEXT.md 的「派生会话」「同时运行上限」「隔离目录」「Tray App」词条按本 ADR 更新。
- 用户在派生终端里的交谈不进入服务会话库，只在原生日志里；boss 读取它依赖各 CLI 原生日志格式，CLI 升级改格式时由 Adapter 跟进。
