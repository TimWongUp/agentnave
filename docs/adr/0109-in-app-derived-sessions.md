# ADR-0109：App 内嵌终端中的派生会话

- 状态：已接受（修订）
- 日期：2026-11-22
- 修订：2026-11-22 自身修订，对齐 Claude 桌面端 side sessions 的完整能力边界（原生 `fork`、`hand_off`、提案与待办、同家族三向消息、名册、汇聚唤醒与连续唤醒上限、受控嵌套与链配额、停止／关闭／归档与用户接管保护、重启恢复与按需续接、权限不扩权、按能力多 CLI 与平台分批覆盖），并收敛工具面、数据根文件与托盘私有持久库。
- 修订：[ADR-0103](0103-tray-embedded-terminal.md)（「终端只由用户在办公室里开」「一个 agent 同时最多挂载一个活终端」、左栏父子树跨重启展示与按需冷启续接）、[ADR-0104](0104-in-app-boss-push.md)（不新增 MCP 工具、仅推 `inbox/` 的边界、唤醒合并与 `wait` 收件箱模式扩展）、[ADR-0105](0105-codex-app-server-terminal-push.md)（Codex UDS `app-server` 扩展至 `thread/fork`、`turn/started`／`turn/completed`、`turn/interrupt`、双向投递与参数套用）、[ADR-0024](0024-consolidated-tools-and-private-review.md)（面向有能力的内嵌终端 boss 新增 3 个条件公布工具并扩展 `wait`）、[ADR-0025](0025-session-desks-and-running-limit.md)（`max_running` 计入存活派生终端并维持单连接加托盘复核边界）、[ADR-0085](0085-service-managed-isolated-dirs.md)（隔离目录创建后向托盘交接使用租约及 `fork` 基点）。
- 沿用：[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0108](0108-multi-account-isolation.md)（默认值、权限档位与具名账号在服务入口解析并透传）；[ADR-0012](0012-tray-conversation-cleanup.md)（托盘数据根写入边界）；[ADR-0080](0080-tray-video-loopback-http.md)（托盘不监听 IPC 或网络端口）；[ADR-0098](0098-tray-deletes-isolated-dirs.md)（仓库锁与租约清理协议）。

## 背景

初版 ADR-0109 只约定了「单层 `fresh` 启动 + 回合结束短提示父终端」，未覆盖原生 `fork`、任务移交 `hand_off`、用户提案与待办队列、同家族三向消息、名册维护、多子会话汇聚唤醒与防循环上限、受控嵌套、父侧停止／关闭／归档与用户接管保护、跨重启父子树恢复、权限不扩权校验，以及 Claude／Codex 之外的 CLI 与 Windows 覆盖路径。

另一方面，AgentNave 不能照搬桌面端靠单一 Electron 主进程以 `stream-json` 接管子 CLI、向用户消息流注入 `<system-reminder>` 与 `tagScrub`、或依赖宿主专属 `wrap_up` 工具的做法，必须继续守住四条既有边界：

- **薄服务与真终端边界**：服务是每个 boss 各持一个的 STDIO MCP 进程；托盘拥有真实终端（PTY）与 Codex 私有 `app-server`，不监听 IPC 或网络端口；两者只通过数据根文件协作。
- **正文永不裸敲终端**：托盘向终端只键入固定短提示；子会话回复、跨会话消息、任务 `brief` 与移交计划一律经 MCP 工具读取并显式标注为数据。
- **Adapter 纯净性**：Provider Adapter 不静默增补参数、prompt 或工具；默认值、权限档位与具名账号在服务入口统一解析。
- **按能力与最简优先**：已接入且具备底层能力的 CLI 与平台逐步纳入覆盖，无基础的照实标不支持；罕见跨进程竞态按概率取舍，不为极低概率场景引入跨语言 OS 分布式锁。

## 决定

### 1. 身份定位与不变量

- **派生会话是 App 内 boss**：派生终端拥有独立的 `agentnave-mcp` 服务连接，而不是 `start_agent` 派发的无终端组员；「组员不得调用 AgentNave 工具」的单向编排约束不适用于它。若发起人显式声明「不得再派」，整条派生链继承该约束。
- **与 `start_agent`／`handover` 分工**：`start_agent` 用于由服务监督进程并写入 `agentnave.db` 的无终端子代理任务；派生会话用于用户可随时点开、插话或接手的交互式子终端。`hand_off` 是同家族内的派生分叉移交；[ADR-0033](0033-boss-workspace-handover.md) 与 [ADR-0099](0099-handover-links-workspaces.md) 的 `handover` 仍用于跨独立工作区的交接。

### 2. 职责边界、数据根合同与托盘私有持久库

- **服务拥有**：能力矩阵与 `app_can_push` 门控、权限不扩权校验、工位／账号／默认值与结构化启动规格解析（工位、模型池、账号、默认值与权限档位的各项解析照常复用，只跳过 `start_agent` 私聊的选席分流、宿主同类排除与 `native_seat` 分流）、隔离目录创建、原生会话日志按轮正文解析与 400 字单行清洗摘要生成、`offer` 阻塞等待、以及 `wait` 收件汇总。
- **托盘拥有**：派生终端 PTY 与 Codex 私有 `app-server` 生命周期、家族拓扑与 `startChain` 配额、回合归属与汇聚唤醒状态机、提案与待办队列、同家族三向消息路由、运行期隔离目录 OS 租约、左栏父子缩进树及提案卡片交互。
- **跨进程数据根文件**：
  - `spawn-requests/<request_id>.json`（服务写，`0o700` 目录、`0o600` 原子替换；合并初稿 `spawn-prompts/`）：承载一次性控制命令、调用方 `service_pid`、结构化启动规格、短命 `prompt`／`message` 正文及已读确认提交。托盘领取并落库后删除；服务超时只查询或取消未领取请求，不与托盘争删。
  - `derived-messages/<msg_id>.json`（托盘写，`0o600`）：承载同家族跨会话（peer）消息正文；收取后仍保留供按 `event_id` 重读，所属家族归档或满 7 天后由托盘清理。
  - `spawned/<tray_pid>.json`（托盘进程级单写，App 退出即删，**绝不含正文**）：记录本实例活派生终端与路由状态、在途请求与 `offer` 决策回执、每个接收者的有界未读事件元数据、近 7 天事件索引（把 `event_id` 映射到原生会话与轮次、peer 正文文件或提案 brief，供重读）与名册、以及已确认位置。服务按 PID 与启动时间防复用校验后按需读取，不常驻轮询（`offer` 阻塞等待期间除外）。
  - `inbox/<service_pid>.json`（既有）：只管 `start_agent` 与 `run_round`；取消初稿在其上增记的派生已读游标。
- **托盘私有持久库 `derived-sessions.db`**：置于托盘自有应用数据目录（预览版写独立预览目录，不改动 `agentnave.db`），以 SQLite WAL 单写者方式保存家族树、原生会话绑定、账号、工作目录、提案与待办、链路配额、事件引用与按 **`(稳定接收者 ID, event_id / 单调 event_seq)`** 持久化的已读确认位置。该库不作为跨进程合同，服务不直接打开它，而是通过 `spawn-requests/` 提交已读确认；因不绑定 `service_pid`，中继热切换或服务重启不会重复推送，且有请求在途或 `offer` 阻塞等待时服务的 `restart_safe()` 返回假。

### 3. MCP 工具面（3 个条件公布工具 + 扩展 `wait`）

仅对运行在 App 内且满足 `app_can_push` 并在能力矩阵中可作父会话的宿主在 `tools/list` 公布以下 3 个工具（执行时逐项复核，不合规或 App 外调用直接拒绝；移除初稿的 `read_spawned_session`）：

- **`spawn_session`**（`readOnlyHint=false`，非幂等；同一已领取 `proposal_id` 返回原回执）：
  - `action="start"`：创建 `fresh` 或 `fork` 派生会话，或凭 `proposal_id` 原子认领提案；支持 `initiation="user_asked" | "offer" | "own_initiative"`（缺省 `user_asked`）。未指定 `provider`／`seat` 时默认继承父 `(provider, account)`；`fresh` 可带 `background` 且允许同 CLI 父子；可带 `forward`，规则与 `start_agent` 相同，注入内容对子会话是数据；`fork` 仅限同 CLI 同账号。
  - `action="hand_off"`：`context="fork"` 的任务移交封装，携带 `remaining_plan` 与移交模板；子终端确认收到首轮后，父节点的该项任务标为「已移交」，父停止自动续做该计划，但父终端仍可继续接收其他人类指令或管理其他子会话。
- **`derived_session`**（`readOnlyHint=false, destructiveHint=true`）：
  - `action="send"`：同家族三向投递（父→子续发、子→父求助 `target="parent"`、子→子互发）。目标空闲直投、忙碌排队、`dormant`（休眠）按原账号／目录／原生 ID 自动冷启 `--resume` 后投递；可带 `forward`（同 `start_agent` 规则）；写入与创建分离，避免重试发信误开新终端。子向父求助时标记「等父回复」并立即退出父的阻塞等待集，防止父子互锁。
  - `action="stop" | "close" | "archive"`：祖先操作后代（中断当前回合、关闭 PTY 并释放租约、归档并移出活跃名册）。若子会话最近一轮由用户亲自发起，拒绝父侧 `stop`／`close`／`archive`（父仍可 `send` 与只读查看）；父归档不递归终止子会话，也不删除原生日志与隔离目录。
  - `action="dismiss"`：撤回未启动提案（`spawn_in_flight` 或已启动时拒绝）并同步清理待办。
- **`read_derived_session`**（`readOnlyHint=true, idempotentHint=true`）：
  - 不传 `event_id` 时返回当前家族名册（`linked_sessions`）、可用动作、提案与回执；传入不可变 `event_id` 时按页重读该次回合报告、peer 消息或任务 `brief` 全文；超出 7 天保留范围时照实返回原因。**纯只读且永不推进已读位置。**
- **扩展现有 `wait`**（`readOnlyHint=true`）：
  - 不传 `ids` 的收件箱模式（含 `wait_seconds=0` 与阻塞等待）新增 `derived` 字段与精简 `linked_sessions` 名册快照，返回未读回合通知（关联 `(derived_id, turn)`，含结局 `finished`/`stopped`/`failed`/`lost`、400 字单行清洗预览、原长度与 `event_id`）、`needs_user` 提醒、peer 来信正文及提案点击结果；领取即向托盘提交已读确认。`include_output=false` 仍返回可经 `read_derived_session` 重读的 `event_id`。派生事件可唤醒阻塞的收件箱 `wait`；空闲但可继续对话的子会话不使 `all_finished` 恒为假。显式传 `ids` 的语义不变。

### 4. 启动规格、原生 Fork 与回合正文解析

- **结构化启动规格**：服务向托盘传递结构化规格（`provider`、`account{id, dir}`、`cwd`、隔离信息、`context`、fork 源、模型、档位、结构化权限、额外选项、初始 `prompt`），不传递拼接好的 CLI `argv`。托盘负责按各 CLI 形态翻译（例如 Claude 将外观主题与跳过确认合并为单一 `--settings`；Codex 参数交给私有 `app-server` 的 `thread/start`／`thread/fork`／`turn/start`，不塞给 `--remote` TUI）。
- **只走原生 `fork`**：不把复制、截断或改写 Claude 私有 `.jsonl` 作为生产方案或回退路径。
  - **分叉时机**：父在 `spawn_session` 工具调用中途即刻分叉，不等父轮结束。子会话带上父当前轮次到这次工具调用为止的内容（含用户最新请求），这次调用在子历史里标为「已中断、结果未知」；首条任务消息说明它是分叉出的子会话。父轮不受影响，照常跑完。
  - **Claude `fork`（Phase 0 已实测）**：`--resume <parent> --fork-session --session-id <子 ID>` 在 print 与交互模式下都可用，可预先指定子 ID，交互模式可同时带位置参数作首条任务且只执行一次。在目标 worktree 目录里直接执行即可找到主仓目录的父会话，子转录落在目标目录对应的 `projects/` 下、历史行 `cwd` 改写为目标目录，不需要「先在父目录 fork 再到目标目录 resume」。父转录在分叉与子续写前后不变。父处于工具调用中途时，CLI 自动补一条 `interrupted` 合成工具结果，不报错、子不重跑该工具。隐藏参数 `--resume-session-at` 截在工具调用之前会让子重跑被截掉的工具，不进入生产路径。
  - **Codex `fork`（Phase 0 已实测）**：托盘调私有 `app-server` 的 `thread/fork`（可传目标 worktree 为 `cwd`），再以 `codex resume --remote <sock> -C <目标目录> <子线程 ID>` 让子 TUI 附着该线程，首条任务经 `thread/queue/add` 下发且只执行一次；父 rollout 不变，子 rollout 只记分叉后的新轮次。中途分叉不传 `lastTurnId`，父进行中的轮次以 `interrupted` 带入子历史（传进行中轮次的 `lastTurnId` 会被拒绝）。不用 `codex fork --remote`：TUI 自建的线程外部观察连接收不到其回合通知。
  - **其他 CLI（Phase 0 已实测，中途分叉未测）**：Grok、CodeBuddy 用 `--resume <父> --fork-session --session-id <子>`，Pi 用 `--fork <父> --session-id <子>`，都能预设子 ID、父记录不变，只能从当前末尾分叉。Antigravity、Devin 只有会话内斜杠命令 `/fork`：托盘须在父会话的恢复终端里键入并从各自会话目录或数据库发现子 ID（agy 的 TUI 会切到子会话，Devin 的留在父会话、需再 `--resume` 子会话）。
  - **子记录形态**：Claude、Grok、Pi、CodeBuddy 的子记录复制父历史（CodeBuddy 复制行的 `sessionId` 仍是父的），按轮读取一律按位置锚定在子会话首条任务之后，不按 `sessionId` 过滤；Codex、Antigravity 的子记录只含分叉后的轮次。
- **新目录信任**：新建隔离 worktree 首次启动时，除 Pi 外各 CLI 都会弹目录信任确认（Devin 在信任框期间不发任何 OSC）；启动规格须预先处理（Codex 用运行时 `-c` 覆盖该路径的信任，Claude 沿用服务对匹配 `cwd` 自动确认信任的做法，其余 CLI 在拉起前写入或确认该目录的信任），子终端首屏不得停在信任框；无法预先处理时，卡在信任框按 `needs_user` 处理。
- **回合检测与按轮正文回读**：
  - **Claude**：由托盘监听 PTY 的 OSC 7501 状态切换（实测普通一轮 `idle → working → done`，需审批时 `working → blocked:kind=permission → working → done`）；服务侧复用 `claude_tui/turn` 逻辑按轮读取原生转录日志。审批按键只在转录里多一条内容为工具结果、`promptId` 与本轮相同的 user 行，不构成新的用户回合，可据此确认归属。fork 子会话的转录开头复制了父历史（含父的轮次结束标记），按轮读取必须锚定在子会话自己的首条任务之后。
  - **Codex（Unix）**：由托盘通过私有 `app-server` 监听 `turn/started`、`turn/completed`（`status` 为 `completed`／`interrupted`／`failed`，完成时附最终助手消息）与 `thread/status/changed`（审批时为 `active` 且带 `waitingOnApproval`，批准后恢复，结束回到 `idle`）。观察连接须先对线程调用 `thread/resume`，否则收不到非本连接发起的 `turn/*` 通知。`turn/started` 不带来源；托盘发起的每条 `turn/start`／`thread/queue/add` 都带可识别的 `clientUserMessageId`，用户在 TUI 亲自提交的回合 `clientId` 不在托盘已发集合内（rollout 记 `turn_trigger=user`），审批按键不产生新回合。服务侧在 Adapter 新增 rollout 按轮解析：按 `turn_id` 取 `task_complete` 的 `last_agent_message`，缺失时取该轮最后一条 `final_answer` 助手消息。
  - **落盘延迟与诚实回退**：状态结束到原生日志落盘间由 Adapter 做有界短重试；仍读不到时照实返回失败原因，绝不拿上一轮回复补位。

### 5. 唤醒状态机、回合归属、名册与受控嵌套

- **双信号回合归属**：只有子会话处于 `idle`／`done` 且用户在子终端亲自提交输入时，才将当前回合归属为「用户发起」（置用户接管标记）；在 `blocked`（权限／提问／认证）或 Codex `waitingOnApproval` 下按键批准权限不改变当前回合归属。用户驱动回合、被停止回合及用户手工 fork 的会话只记录历史与更新名册，不主动唤醒父会话。
- **汇聚唤醒与中断守卫**：托盘按父会话汇聚直接子会话事件，只要仍有直接子会话处于 `starting`／`working` 即积压等待全员停下再合并为一条单行唤醒提示（并与该父的 `start_agent`／`run_round` 待收件合并为同一条 `wait(0)` 提示）。子会话稳定停在审批／提问卡满 **10 秒**时标为 `needs_user`、退出阻塞等待集并提醒父会话；子向父求助同样立即放行唤醒。父会话被 `Ctrl+C`／中断后暂停自动唤醒直至用户再次提交；父终端 `/clear` 切换 `boss_key` 后立即停止将旧聊天的派生唤醒推向新聊天。
- **实时名册（`linked_sessions`）**：由托盘维护并在 `wait`、`spawn_session`、`derived_session` 回执中附带精简快照（`id`、清洗后的名称、父节点、启动者、状态优先级 `needs_user > working > failed > dormant > idle` 及未读数）；`read_derived_session()` 返回完整名册、提案与回执。
- **受控嵌套与 `startChain` 配额**：支持多层派生（遍历保护上限 **32 层**并校验无环），整条链在根会话共享三项硬配额：**同时工作 ≤ 5**、**每个人类发言周期内累计启动 ≤ 10**、**连续自动唤醒 ≤ 10**。连续自动唤醒达 10 次后停止主动键入唤醒提示（事件继续积压，仍可通过 `wait` 收取），待根／父终端有真实人类输入提交后归零。自动续跑回合仅可在原项目内直启（跨项目须走 `offer`）；由 peer 消息触发的回合禁止直启，仅允许提出 `own_initiative` 提案。

### 6. 提案卡片、并发上限、隔离租约、权限与重启恢复

- **一阶段 `offer` 与 `own_initiative` 提案**：
  - `offer` 在调用时一次提交完整 `brief` 并阻塞等待用户在 App 选择「新会话」或「这里做」；用户接受时按被接受版本复核账号、权限、目录与配额后立即启动。支持取消、父回合中止与 App 偏好（总在这里／总开新会话／不再询问）；非人类发起的回合禁止弹 `offer`。
  - `own_initiative` 立即返回 `proposal_id`，托盘持久化并设 **7 天**过期、单会话上限 **20 条**；每回合最多 **3 张**作为写进工具说明与 Skill 的软约束。被撤回、挤出或过期的本地提案转入待办 backlog（**30 天**过期、单目录上限 20、全局上限 100）。用户界面点击与模型 `proposal_id` 认领竞争同一原子领取；模型不得改写提案内容，用户编辑过的以用户版本为准。
  - 离线点击提案时（父终端在线则键入短提示引导父调用 `spawn_session(proposal_id)`；父不在线则由托盘先按原绑定冷启 `--resume` 父终端而不自动提交新用户任务，待就绪后再提示），由服务重新校验并按需建隔离目录，不预建无主 worktree，也不在托盘重复实现隔离创建逻辑。
- **并发上限**：每个存活的派生 PTY（含启动中、等审批与空闲）各占一个 `max_running` 名额，关闭或休眠时释放，恢复时重新占用；不另设独立的全局派生终端总数上限，也不引入跨进程 OS 锁（服务预查 + 托盘串行受理复核）。名额满时回执列出可关闭的空闲子终端。
- **隔离目录租约与 `fork` 基点**：需要隔离时由服务按 [ADR-0085](0085-service-managed-isolated-dirs.md) 创建目录并持有临时租约 `L_svc`，托盘接单后按同一仓库锁协议取得进程级租约 `L_tray` 并反馈给服务释放 `L_svc`；终端关闭、休眠或 App 退出时释放 `L_tray`。父在隔离目录中执行 `fork` 时，从父当前已提交 `HEAD` 切出新 worktree，未提交改动不带入并在回执写明。
- **权限只降不升**：同 Provider 严格遵守各 Adapter 声明的原生偏序只降不升；跨 Provider 仅自动放行已验证映射表能证明不扩权的组合；当父会话实际权限未知或跨 CLI 组合无法证明不扩权时禁止直启，要求改为通过 `offer` 由用户在 App 针对该次启动显式确认；非人类发起的回合不能弹 `offer`，此时拒绝启动，boss 可改提 `own_initiative` 卡，卡面列出目标账号、目录与实际权限，用户点击即视为对该次启动的确认。若本轮已被用户限定为更窄范围（如只读），提案同样必须满足该范围。
- **重启恢复与防伪造**：App 退出不保活子进程；重启后托盘从 `derived-sessions.db` 重建左栏父子树，将旧终端标为 `dormant`（退出前未确认启动或送达的标为「待核对」，不自动重跑），清理未完成的阻塞 `offer` 并保留未过期的 `own_initiative` 与待办；用户点击或获准 `send` 时按原账号、目录与原生 ID 按需 `--resume` 并重新取租约。数据根与消息文件采用 `0o700` 目录、`0o600` 权限、防符号链接／junction 遍历检查及 PID 启动时间防复用校验，名称按白名单清洗，所有家族归属、深度、配额与权限均由服务与托盘基于进程树与单写者状态校验，不信任文本内容。

### 7. 不移植桌面端做法及替代方案

- **不移植 `wrap_up` 专用工具**：强制子会话调用宿主专属收尾工具违反「Provider Adapter 不静默增补工具与 prompt」边界且无法跨 CLI 通用；替代为服务 Adapter 直接从各 CLI 原生会话日志解析最后一轮助手回复。
- **不移植 `side-session-notes/` 共享便签目录 + `--add-dir`**：各 CLI 对附加目录授权参数与沙箱语义不一致，且额外目录会增加跨进程生命周期清理负担；替代为同家族 peer 消息、`spawn_session`／`send` 的 `forward` 与隔离目录。
- **不移植 `cloud` / `ssh` / `preset` 目标**：AgentNave 定位为本机 STDIO MCP 服务，没有云端或远端会话运行载体，对应能力不在范围内。
- **不移植 `<system-reminder>` 注入与 `tagScrub`**：桌面端将子会话摘要与名册直接拼入用户消息文本流，才不得不维护复杂的 Unicode 同形字 XML 标签清洗；AgentNave 坚持正文不进终端，改为工具回执附带名册与短提示唤醒，从源头隔离数据通道。

### 8. 按能力覆盖矩阵（父 / 子 / fork 独立判定，均须实测开放）

| CLI / 平台 | 父（收唤醒与工具） | 子（fresh / 续发 / 回读） | 原生 fork | 依据与限制 |
| --- | --- | --- | --- | --- |
| **Claude Code（macOS）** | 第一批 | 第一批 | 第一批（已实测） | `--session-id` 绑定、首条任务只执行一次、`--fork-session` 同目录与跨 worktree、工具调用中途分叉、审批归属均已实测通过；两个 `--settings` 后者整体覆盖前者，须合并为一个。 |
| **Codex CLI（macOS）** | 第一批 | 第一批 | 第一批（已实测） | `thread/fork` 跨 worktree、子 TUI `resume --remote` 附着、中途分叉、回合通知、`clientUserMessageId` 归属与 rollout 按轮解析均已实测通过。 |
| **Grok CLI（macOS）** | 第二批 | 第二批 | 第二批 | 已实测：终端回应 7501 查询时自发 OSC（含 `blocked:permission`、`blocked:question`）；`--session-id` 预设、首条只执行一次、`chat_history.jsonl` 按 `prompt_index` 回读、`--resume` 与 fork 均可。 |
| **Pi Coding Agent（macOS）** | 第二批 | 第二批（需补托盘续接参数） | 第二批 | 已实测：`PI_PROGRAM_STATUS=1` 下发 OSC；没有审批与提问，`needs_user` 永不触发；`--session-id` 预设、`--session <id>` 恢复、`--fork` 均可；托盘续接参数尚未接入 Pi。 |
| **Antigravity CLI（macOS）** | 第二批 | 第二批（需补 ID 发现） | 第二批（键入 `/fork`） | 已实测：OSC 完整；不能预设会话 ID，须从会话目录发现；`-i` 传首条任务只执行一次（信任框期间提交的首条缺 `working/done`，须先处理信任）；`--conversation` 恢复可用。 |
| **Devin CLI（macOS）** | 第二批（需加入推送名单） | 第二批（需补 ID 发现） | 第二批（键入 `/fork`） | 已实测：自发 OSC（含 `blocked:permission`、`blocked:question`）；不能预设会话 ID；`--resume` 恢复时丢弃位置参数里的任务，续发须键入；信任框期间不发任何 OSC。 |
| **CodeBuddy Code（macOS）** | 不支持（无 OSC 7501） | 部分（无阻塞信号） | 第二批 | 已实测：不发 OSC 7501，托盘无法判定空闲与阻塞，不能当父；当子时以转录的 `turn-metrics` 判一轮结束，但没有 `needs_user` 与审批归属判定；fork 可用。上游或可靠的启动钩子补出状态信号后再升格。 |
| **ZCode** | 未确认 | 未确认 | 未确认 | 本机未安装，无实测；桌面捆绑版被托盘排除，独立 TUI 安装后另测。 |
| **DeepSeek Harness** | 不支持 | 不支持 | 不支持 | 无交互终端载体。 |
| **Windows（Claude / Grok / Agy / Pi / Devin）** | 待补检测后实测 | 同左 | 按各自 CLI 实测 | 服务侧当前对 win32 一律不公布推送能力，须先补进程祖先与启动时间校验，再在 ConPTY 上实测 OSC 透传；**Windows Codex** 走 `--no-daemon`，不沿用 Unix UDS 结论，暂不支持。尚无任何 Windows 实测。 |
| **Linux / App 外宿主** | 不支持 | 不支持 | 不支持 | Linux 无托盘 App，App 外宿主无托盘 PTY。 |

## 取舍

- **三工具收敛与分离写操作，而非保留 `read_spawned_session` 或合并创建与续发**：把创建／移交（`spawn_session`）、已有目标控制与撤回（`derived_session`）、无副作用重读（`read_derived_session`）分开，既避免重试发信误开新终端、确保重读不推进已读游标，又兼容收件箱 `wait` 统一领取。
- **坚持原生 `fork`，否决复制并改写 Claude 私有 `.jsonl`**：即使桌面端在本地采用了复制并清理 `.jsonl` 的做法，AgentNave 仍不把依赖宿主私有内部格式作为生产方案或回退，以免随 CLI 升级静默损坏会话结构；Phase 0 已实测 Claude、Codex 的原生入口都能满足要求（跨 worktree、工具调用中途、父记录不变）；代价是没有原生入口的 CLI 照实标为不支持 `fork`。
- **一阶段完整 `brief` 的 `offer`，否决两阶段补 `brief`**：一阶段提交消除「用户同意后模型仍未补完 `brief`、父会话却退出」的半成品状态机和重启清理逻辑；代价是用户最终选择「这里做」时，模型已预先花费一次生成 `brief` 的成本。
- **存活 PTY 统一计入 `max_running`，否决另设独立终端总数上限**：保持与 [ADR-0025](0025-session-desks-and-running-limit.md) 单一并发配置一致，不逼迫托盘拦截空闲子终端上用户的下次输入；代价是空闲子终端会占用 `max_running` 槽位（可通过 `close`／`archive` 释放），且仍不跨独立服务进程加全局 OS 锁。
- **托盘单写 `derived-sessions.db`，否决在 `inbox/` 记游标或整份 JSON 重写**：按 `(稳定接收者 ID, event_id / 单调 event_seq)` 在托盘 SQLite WAL 库持久化，既支持增量写入、中继热切换不丢游标与跨重启重建树，又规避 Python 只读 WAL 库风险且不触碰 `agentnave.db`。

## 分阶段计划与验收

- **Phase 0（能力实测与细节定稿，macOS 部分已完成）**：使用测试仓库实测 Claude／Codex 的 `fresh`、原生 `fork`（同目录、跨 worktree、工具调用中途）、新 ID 绑定、首轮只执行一次、权限生效、审批回车归属及 Codex `rollout` 按轮正文字段；完成第二批 CLI 与 Windows 能力分类并定稿待实测细节。**验收**：产出逐格能力矩阵与明确阻塞项，阻塞项不得用降级假装通过。macOS 上 Claude、Codex、Grok、Pi、Antigravity、Devin、CodeBuddy 已实测，结果见第 4 节与第 8 节；仍未实测的是 ZCode（未安装）、全部 Windows 组合、以及第二批 CLI 的工具调用中途分叉，移到 Phase 4 开放对应格子前补测。
- **Phase 1（持久库、结构化启动与 `fresh`/`fork` 闭环 — Claude／Codex）**：落地 `spawn-requests/`、`spawned/`、`derived-sessions.db`、结构化启动规格、`spawn_session`（`fresh`+`fork`）、租约交接、`max_running` 占位、宿主同类豁免、按轮正文回读与 `wait.derived`、`read_derived_session`、`close` 与左栏父子树。**验收**：Claude 父开 Claude 子，在子里批准一次权限后父收到正确报告；同一子连续两轮回复分别可读；关闭后释放名额与租约。
- **Phase 2（家族协作、完整唤醒与权限不扩权）**：落地 `send` 三向消息与 `derived-messages/`、子求助、用户接管保护、`stop`/`archive`、汇聚唤醒 + 10s `needs_user` + 10 次攒批 + 父中断与 `/clear` 守卫、多层嵌套与 5/10/10 链配额、同／跨 Provider 权限不扩权。**验收**：两个子乱序结束仅唤醒父一次；审批回车后仍回报原父轮；中继热切换不重推且不推给 `/clear` 后的新聊天；父→子→孙链配额触顶后转积压。
- **Phase 3（提案、移交与跨重启恢复）**：落地一阶段 `offer`、`own_initiative`（7 天）、待办 backlog（30 天）、`dismiss`、`proposal_id` 认领与离线点击提示链路、`hand_off` 与父任务「已移交」标记、重启重建父子树与按需 `--resume` 对账。**验收**：用户点击与模型认领竞争仅启动一次；已撤回提案不能再启；重启后父子树、未读、提案、配额与租约状态一致且不重复执行。
- **Phase 4（扩面覆盖与收口）**：按实测结果逐格开放第二批 CLI 与 Windows；更新 `agentnave-manager` Skill、安装／说明文档与工具名清单；完成预览版隔离验收。**验收**：逐项核对本 ADR 功能边界，未通过实测项保持未开放或显式标为不支持。
