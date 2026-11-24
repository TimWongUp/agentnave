# ADR-0114：Claude 与 agy 保持交互 TUI，boss 以用户身份驱动，结果靠宿主自己唤醒收取

- 状态：已接受
- 日期：2026-11-23
- T3 主线程的会议后台等待与 T3 标识剥离由 [ADR-0121](0121-t3-code-orchestration.md) 修订。
- 修订：
  - [ADR-0003](0003-agent-only-stdio-mcp-server.md)：`agentnave-mcp` 之外，服务包内多一个 boss 可在后台运行的只读等待模块（第 3 节）。
  - [ADR-0043](0043-service-prompt-instructions.md)、[ADR-0107](0107-specialist-scope-closing-and-fallback-note.md)、[ADR-0057](0057-boss-answers-native-prompts.md)：派发说明改用户口吻、按「本次能否答题」选措辞（第 2 节）。
  - [ADR-0062](0062-service-owned-member-names.md)：会议身份行措辞（第 2 节）。
  - [ADR-0056](0056-claude-interactive-runtime.md)、[ADR-0072](0072-antigravity-interactive-runtime.md)：后台任务判定、等待提醒、空闲受阻的适用阶段（第 4 节）。
  - [ADR-0082](0082-claude-long-wait-terminal-only.md)、[ADR-0084](0084-claude-terminal-default-long-wait.md)：长等待的判据加上「未关闭后台任务」（第 3 节）。
  - [ADR-0064](0064-dispatch-receipts-steer-to-wait.md)、[ADR-0067](0067-wait-270s-window-and-host-background-limits.md)：回执 `next` 的收件方式与 agy 的等待方式（第 3 节）。
  - [ADR-0104](0104-in-app-boss-push.md)：作废 PTY 键入投递条款（含 2026-11-21、2026-11-22 两次自身修订中的 PTY 部分）与「不改变 App 外等待行为」；保留 `inbox/`、`push/` 进程级文件合同与服务侧终态阻断单报规则（第 3 节）。
  - [ADR-0109](0109-in-app-derived-sessions.md) 第 1、3、5、6、7、8 节：派生入口的公布条件、派生说明措辞、一轮的结算与归属、续派忙的定义、`inbox/`／`push/` 新增字段。
- 沿用：[ADR-0031](0031-prompt-only-orchestration.md)（单向编排靠派发说明约束）、[ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md)（会议条目是数据）、[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0049](0049-service-initiated-termination.md)（到限只提醒，不新增主动终止）、[ADR-0086](0086-windows-claude-interactive.md)、[ADR-0102](0102-agy-native-status.md)（提问仍受阻）、[ADR-0105](0105-codex-app-server-terminal-push.md)、[ADR-0113](0113-chat-desktop-split-out.md)。

## 背景

ADR-0113 之后 AgentNave 回到以 MCP 编排为主。用户提出四个问题：App 内外功能不一致（派生会话与推送只在 App 内）；App 内 boss 靠托盘往 TUI 里键入唤醒，与 App 外挂起 `wait` 差别大且不稳定，过去的问题主要出在回合终点判断；Claude 与 agy 子代理是否改走 SDK／ACP；以及想要一个类似 herdr、能从手机经终端控制的纯终端版本。

三家会议、实测与调查得到这些事实：

- **Claude 终端交互会话**：`wait(1500)` 满约两分钟后被 Claude Code 转为后台任务，本轮结束；子代理结束时 Claude 自己开新一轮取结果（2026-11-23 实测，2.1.330）。桌面 App 等非交互入口不转后台（ADR-0082）；`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` 等变量也会关掉转后台。
- **agy 交互 TUI**：不把 MCP 调用转后台，`wait` 在前台阻塞到返回；超过 10 秒的 shell 命令自动转后台，命令结束 agy 自己开新一轮。boss 派出子代理后在后台跑一个「收件箱有结果才退出」的命令，结果到达时 agy 自己调 `wait(0)` 取回；等待期间用户发消息照常即时回复（2026-11-23 实测，1.5.14）。后台命令未结束前 OSC 7501 一直报 `working`，全部结束后才报 `done`。
- **Claude 后台任务原生信号**：2.1.330 的 `Stop` 钩子带可选字段 `background_tasks`，说明为「区分会话已结束与会话在等后台工作唤醒」，覆盖后台 Bash、超时或被新消息打断转后台的 Bash、Monitor、异步子代理、MCP 任务与 workflow；`dream`、`auto-mode scan` 等内部任务不会唤醒主回合。现有 transcript 白名单记账有 issue #504 列出及调查新发现的漏洞。
- **入口名与额度**：订阅下 `sdk-*` 入口消耗 5 小时窗口约为 `cli` 的 1.8 倍（anthropics/claude-code#97074，未获官方回应）；2.1.330 在非交互时把 `cli` 强制改回 `sdk-cli`，并新增入口名校验。
- **参考实现**：herdr（Apache-2.0）判断忙闲靠读屏，回到提示符即不计后台任务；它的推送在提交后 5 秒内须观察到 `working`／`blocked`，否则报 `agent_prompt_stalled`，投递前核对前台进程仍是目标 CLI。magpie（MIT）的 Claude 驱动按粘贴体积延长等待、回车未落 transcript 时重按。二者都没有草稿保护与多条合批。herdr 窗格设置的 `HERDR_*` 会被子进程继承，嵌套运行同种 CLI 时覆盖外层窗格的会话绑定。

## 决定

### 1. 运行形态

- 现有的 Claude 与 agy 交互路径（boss、后台子代理、派生会话）保持交互 TUI，不改走 `-p`、Agent SDK 或 ACP。现有例外不变：Windows 上的 agy、受限工具席位与隔离会议席位、未放开权限的 agy、显式 `interactive=false` 仍走 `-p`（ADR-0056、ADR-0072、ADR-0086）。
- Claude 入口名不改（ADR-0056），满足任一条件时重评：#97074／#107106 有官方答复、取得书面许可、或第三方入口名机制对外开放。agy 保持交互运行是用户的决定。
- Claude 以 API key 或云认证运行时是否改走双向 `-p stream-json`，以后单独评估。
- 派生会话先覆盖 Claude、Codex、agy 三家（现有 `terminal_providers`），其他 CLI 之后再加入。
- 驱动 TUI 的方案先参考 herdr 与 magpie（AGENTS.md），复用其中不与本 ADR 冲突的做法。

### 2. boss 就是用户

**输入**。Claude、agy 的输入经 PTY 以粘贴加回车送入，Codex 经它原生的用户输入通道（app-server），都不另开注入通道。等待时长按可靠性定（按粘贴体积延长，回车未落 transcript 时有限重按），不为显得像人而人为放慢。送达沿用各 CLI 现有的确认标准（如 agy 要求 transcript 出现与粘贴全文相同的用户行），不降低。

**可见内容**。服务追加的文字（`core.py` 的 `SUBAGENT_INSTRUCTIONS` 与只读说明、ADR-0109 第 3 节的派生说明、隔离目录说明、转发引言、派生任务包装、会议引言与身份行）改写成用户口吻，像用户在交代任务，不讲「子代理」「工位」「受管分支」这类编排术语。约束一条不少：在权限内自己做完，不要调用 AgentNave、不要转给别人；最后说明未满足的要求、未查范围、阻塞和需要的授权；只读时不改文件、不提交推送、不发消息或评论。用户口吻与约束效果冲突时以效果为准。

- 措辞按 Adapter 声明的「本次能否答题」选择（Claude 交互运行：有问题可以问我；其余：有不确定的写在最后），核心不按 Provider 名分支。
- 派生会话的说明保留 ADR-0109 第 3 节的能力差异：可以派后台任务、开会，不能再开派生终端，同样用用户口吻。
- 会议条目保留 JSON 结构与转义（ADR-0018 的数据边界依赖它），只改外层引言与身份行（ADR-0062）。

**提问**。Claude 交互运行可以像对真人一样提问，经 PreToolUse 钩子交给 boss（ADR-0057）。agy 停在提问时仍按 ADR-0102 受阻，保留会话与已有输出，回执带上问题原文；boss 用同一 `session_id` 续接、把回答作为下一条消息发送，与用户在下一条消息里回答一样。不向 agy 的提问菜单按键。

**防递归**。沿用 ADR-0031：靠派发说明里「不要调用 AgentNave、不要转给别人」约束，不另加运行时识别或工具隐藏。

**边界**。只针对模型看到的内容和交互方式。面向服务商的标识（入口名、User-Agent、请求头）保持 CLI 原样，不伪造，不以伪装方式规避服务商的计量或限制。运行时弹框沿用 ADR-0056 经钩子应答。

### 3. 收结果：各宿主靠自己的机制唤醒

| boss 所在 | 收结果的方式 |
|---|---|
| Claude 交互会话（入口 `cli`，未关闭后台任务；终端或 App 内终端） | `wait(1500)`（ADR-0084 默认值）：满两分钟转后台，子代理结束时 Claude 自己开新一轮 |
| agy 交互 TUI（可信判据确认是已验收版本的 agy CLI；终端或 App 内终端） | 派发后照回执运行等待命令，命令返回后 `wait(0)` 收取 |
| Codex（App 内终端，平台支持时） | 私有 app-server 结构化推送（ADR-0105） |
| 其他情况：Claude 非 `cli` 或关了后台、Antigravity App、判不出版本的 agy、Codex 普通终端、Claude 桌面端、Codex App 等 | 前台 `wait`；Claude `cli` 关了后台时回执明确给 `wait_seconds=270` |

- **Claude 判据**：入口为 `cli`，且环境里没有关闭转后台的变量（`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`、`CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND`，或 `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS` 大于 1500 秒）。
- **agy 判据**：服务需要一个可信来源区分 agy CLI 与 Antigravity App、并确认版本（例如 MCP `initialize` 的 `clientInfo`，实现时核实）；判不出就给前台 `wait`，不靠试错发现能力。用户的 agy 若每次都要确认 shell 命令，接受这次确认。

**等待命令**。不新增公开入口：服务包内的模块，回执 `next` 给出整条命令——当前服务自己的解释器加 `-I -m agentnave.<模块>`、数据根、服务实例标识（进程号加启动时间）与 opaque 的 boss 键，boss 原样运行。

- 服务在快照里为每个 boss 记三个数：在途工作数、事件序号（有新事件就递增）、已交付序号（`wait` 交出就绪项时推进）。在途数大于 0 时保留快照文件；`start_agent`、`start_agents`、`run_round`、`wait` 返回前同步刷新快照，不留异步窗口。
- 命令只读快照、按路径重读（发布方原子替换文件），不建 MCP 连接、不收取。退出并打印一行不含正文的原因：
  - `ready`：事件序号大于已交付序号，且 boss 没有正在阻塞的 `wait`；未收的终态优先于在途数归零。
  - `all_done`：在途数为 0 且没有未交付事件。
  - `service_gone`：服务实例不在了（进程号与启动时间对不上）。
  - `expired`：窗口到期（初值 2 小时，只防孤儿进程）。
  - `already_watching`：同一 boss 已有实例在等（按键加锁），后来的立即退出。
- boss 收到 `ready`、`expired` 就 `wait(0)`，仍有在途工作再照回执续等；`already_watching` 与 `all_done` 直接结束本轮；`service_gone` 回到当前连接核对。命令在前台 10 秒内就返回时同样处理，不等后台唤醒。

**App 内**。

- 托盘把「能开派生终端」与「接收键入推送」拆成两项能力：`terminal_available()` 只看 `terminal_providers`，不再依赖推送名单，托盘祖先、派生身份与平台检查照旧（修订 ADR-0109 第 1 节）。
- App 内 Claude 与 agy boss 不再接收键入唤醒。App 与服务同版本发布；混跑期间，新托盘配旧服务会暂时少掉 Claude／agy 的派生入口、旧托盘配新服务可能多键入一次提示，重连后恢复。
- 未收结果的角标与通知改为独立扫描：覆盖所有已绑定的 App 内终端（含派生终端作 boss 时），按 boss 键与未收事件计算，事件滞留满 30 秒且 boss 没有阻塞的 `wait` 才显示，`wait` 收走后撤销。它只提醒用户，不保证模型自己开新一轮。
- Skill 的收件方式由「二选一」改为按回执选择。

**删除键入推送**。移除 Claude 与 agy 后，键入推送只剩 Grok 与 Pi。先用真实调用测 Grok、Pi 的后台命令能否自唤醒：能的话改用同一等待命令；不能的话退回前台 `wait`，App 内由角标兜底。两种结果都删除托盘的键入投递（`deliver_pty` 及其提交、撤回逻辑）；`input_gate` 的草稿跟踪保留，派生续派的忙判断要用它。

### 4. 一轮的结束：等本轮后台任务结束再汇报

派生会话先覆盖 Claude、Codex、agy；Codex 沿用 app-server 的 `turn/completed`，本节只改 Claude 与 agy。

- **Claude（后台 helper 与派生会话共用一套判定）**：
  - 结束信号：同一子回合的 `system/turn_duration` 加 `Stop`，且 `Stop.background_tasks` 里没有会唤醒主回合的类型（shell、monitor、subagent、MCP task、workflow；不等 `session_crons`）。`StopFailure`、transcript 里的中断、进程退出与取消照旧优先结算。两个信号按到达顺序配对；先到的等另一个最多数秒，等不到就退回 transcript 记账并在 `warnings` 注明。区分 `background_tasks` 字段缺失与空列表。
  - 派生 Claude 无条件注入常驻的 `Stop` 钩子（不依赖权限兜底、不是一次性），钩子目录随派生终端存活，按派发基线忽略用户自己聊产生的事件。
  - 老版本没有 `background_tasks`：helper 用补齐后的 transcript 记账（按 #504 与调查结果）；派生会话按 `turn_duration` 结算并在 `warnings` 注明后台任务完成未确认。
  - 归属与结果：本次派发之后、用户下一条消息之前由 `<task-notification>` 引起的子回合都属于这次派发；托盘在结算回执里写出起止锚点，服务只读这一段，按顺序拼接各子回合的回复作为结果。用户中途插话的，按 ADR-0109 第 5 节注明用户参与或归属未确认。`terminal_turns.py` 不再停在第一个 `turn_duration`，`native_logs.rs` 不把任务通知行当作新的用户回合。
  - Claude 与 agy 派生都去掉 1.8 秒强行结算。
- **agy**：helper 与派生都只认 OSC 7501 `done`（它已等后台命令结束）且本轮已有最终回复；`done` 后有界等待回复落盘，读不到就照实报。helper 的 60 秒空闲受阻（`STALL_LIMIT`）不适用于「回复已写出、OSC 仍为 `working`」的阶段。
- **续派**：本会话有未结算的派发时，续派返回忙（修订 ADR-0109 第 7 节的忙定义）。
- **等待提醒**：本轮已有回复、仍有本轮后台任务时开始计时（agy 派生看不到任务列表，以「回复已写出且 OSC 仍为 `working`」为准）；只有后台任务开始或结束才重新计时，Monitor 的中间事件不算。满 30 分钟，每次派发或席位尝试最多发一次 `execution_blocked`，错误码 `background_tasks_pending`，说明只带任务数或标识、不带正文（正文仍按 `include_output` 返回），且不参与账号健康分类与冷却。任务继续运行，终态先到就直接交付。派生会话的阻断经 `push/` 回执桥接到服务的待提醒状态。
- **只剩后台任务时的 `cancel`**：不向终端按键，直接按已写出的回复结算并注明后台任务去向：helper 终止进程组，派生会话留给用户；会议席位的回答照常提交。

### 5. 环境卫生

去掉继承的 `HERDR_*`：服务的 Provider 环境（`processes.py`）、Claude／agy helper 的剥离清单（含 Windows ConPTY 路径）、托盘内嵌终端与派生终端的环境（`session.rs`）。用户自己的外层 herdr 窗格与未来终端垫片自身的环境不动。验收看两点：外层窗格绑定仍有效，内层同种 CLI 不再覆盖它。

### 6. 终端版

另立 ADR。方向按会议结论：在 herdr 窗格（或 tmux、普通 SSH）里运行单窗格 PTY 垫片，持有 CLI 的 PTY、回答 OSC 7501，派生在 herdr 下用 `pane split`。新增可执行入口、垫片写 `push/` 的合同、无 herdr 时如何派生，都由那篇 ADR 决定。第 3 节的自唤醒让终端版的 Claude 与 agy boss 不依赖键入推送。

## 取舍

- **不改 `-p`／SDK／ACP**：结构化协议的终点判断更直接，但 Claude 订阅下 `sdk-*` 入口约 1.8 倍消耗、入口名不宜伪造；agy 由用户决定保持交互。代价是继续维护 TUI 驱动与 transcript 解析，因此固定参考 herdr、magpie。
- **自唤醒取代键入推送**：宿主自带的唤醒不依赖空闲判定、草稿与按键时序，App 内外行为一致。代价是 agy boss 每次派发后多跑一条后台命令，服务要维护等待模块和快照里的在途计数；非交互宿主仍只能前台等。
- **以用户身份驱动**：子代理更接近真实使用；约束仍全部写在 prompt 里，效果优先，不为隐藏编排另建运行时机制。代价是改写说明后要按 CLI 重新验证遵从度；会议保留 JSON，换来不被伪造边界。
- **agy 提问走续接而不按键**：零误按，与运行中不按键（ADR-0072）一致；代价是提问会结束这次派发，回答要多一次续接。
- **等后台任务并提醒**：避免半截结果；常驻服务不会让 boss 永远不知情。到限只提醒、不改变任务状态，代价是 boss 要处理一次提醒，并决定继续等还是按已有回复收尾。
- **考虑过不采用**：照搬 herdr 读屏判定（与 ADR-0056 运行期不读屏冲突，且回到提示符即不计后台任务）；Claude channels 唤醒（需开发参数与启动确认框、协议降级，桌面 App 不可用）；`agentnave-mcp await` 子命令（旧版启动器会把它当普通启动，且改变唯一入口的合同）；运行时识别被派发的连接并隐藏派发工具（Codex 不透传环境变量，要另配白名单，复杂度不抵 prompt 约束）；保留并加固键入推送（只剩 Grok、Pi 两家使用）；会议改成纯散文转述（失去结构化的数据边界）。

## 需要同步的文档

AGENTS.md（入口一节）；`docs/context/architecture.md`（入口、Claude／agy 运行时、派发说明、`inbox/`／`push/` 合同与新读取方、派生会话结算、环境不继承清单）；`CONTEXT.md`（派生会话）；`docs/adr/README.md` 索引与被修订各篇的头部状态；`skills/agentnave-manager` 与 `skills/agentnave-meeting`（收件方式、`references/studio.md`、Claude／Antigravity 参考，Grok／Pi 参考按实测结果，`background_tasks_pending` 的处理）；`mcp_runtime.py` 的 MCP 说明，`wait`、`answer_prompt`、`start_agent` 的工具说明，`push_capability.py` 的回执文本。
