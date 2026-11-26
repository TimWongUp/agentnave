# 架构与边界

AgentNave 是供 Manager（boss）通过本地 STDIO MCP 使用的薄进程边界服务，不是面向人的 CLI、多智能体运行时或工作流引擎。本文只记录所有权、模块依赖方向、安全边界与跨进程／跨语言合同；参数、字段、路由、超时常量与错误文本以代码、MCP schema 与测试为准，取舍背景见 [docs/adr/README.md](../adr/README.md)。

## 所有权

| 角色 | 拥有 |
| --- | --- |
| Manager（boss） | 目标解释、任务拆解、角色与模型选择、并行与重试决策、会议审核、结果综合、是否建隔离目录与合入、长期任务状态 |
| AgentNave 服务 | Provider 进程树启动与取消、入口默认值与权限档位套用、活动快照与终态归一化、进程内 Invocation 句柄、会议轮次、SQLite 会话历史与自动清理、隔离目录创建／登记／无改动自动释放／显式释放删除、派生会话参数解析与按锚点结算（[ADR-0109](../adr/0109-in-app-derived-sessions.md)） |
| 托盘 App | 宿主接入与配套 Skill 安装、组员／多账号／设置文件维护、托管账号目录（建目录、登录终端、仅接入同步）与外部目录关联（[ADR-0118](../adr/0118-link-existing-claude-account-dirs.md)）、会话只读展示／改名／手动清理、隔离目录手动删除与规则自动清理（[ADR-0098](../adr/0098-tray-deletes-isolated-dirs.md)、[ADR-0115](../adr/0115-tray-git-status-and-basic-ops.md)）、Git／PR 只读状态（[ADR-0126](../adr/0126-tray-git-status-read-only.md)）、宿主全局规则块写入（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)）、App 与服务按版本更新及 CLI 沿原方式更新（[ADR-0055](../adr/0055-app-updates-member-clis.md)）、内嵌终端与派生会话终端（[ADR-0103](../adr/0103-tray-embedded-terminal.md)、[ADR-0109](../adr/0109-in-app-derived-sessions.md)）、本机用量统计（[ADR-0106](../adr/0106-tray-usage-panel.md)）、语音播报 |
| Provider CLI | 自身安装、认证、原生配置、原生权限边界与原生 Session |
| MCP Host | MCP 注册格式、作用域与加载行为 |

- 统一核心始终保存历史，不再提供无历史模式（[ADR-0016](../adr/0016-single-core-and-mcp-entrypoint.md)）；AgentNave 不拥有 DAG、调度器、自动重规划或合入逻辑。
- 宿主全局规则中的 AgentNave 标记块由 App 按服务包共享定义 `orchestration_rules.json` 写入，只放授权与派发前约束，服务不据此调度（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)、[ADR-0068](../adr/0068-rules-hold-authorization-skill-holds-method.md)）；普通私聊用法放在工具描述与回执，复杂编排放在 `agentnave-manager` Skill，「只开会」档互斥安装 `agentnave-meeting`（[ADR-0091](../adr/0091-meetings-only-skill-and-auto-meeting-switch.md)、[ADR-0117](../adr/0117-private-dispatch-without-skill.md)）。

## 进程与模块

- **中继与版本化安装**：`agentnave-mcp` 先启动仅依赖标准库的中继（`relay.py`）转发 STDIO；同一环境原地重装且空闲时热切换子进程。App 将每个服务版本装入独立目录，macOS／Linux 宿主配置指向固定启动器 `service/bin/agentnave-mcp`，切换版本只改启动器指向、宿主重连后生效（[ADR-0010](../adr/0010-launcher-relay-swaps-upgraded-server.md)、[ADR-0047](../adr/0047-versioned-service-install.md)、[ADR-0061](../adr/0061-fixed-service-launcher.md)）。
- **`restart_safe` 合同**：空闲切换由服务 `restart_safe` 与中继在途请求／未读 Invocation 共同判定；持有会话锁、有未落盘回写、有未送达终态轮次，或**任何新增的进程内易失状态**都必须计入 `restart_safe`（保留期内的空闲 Provider 进程除外，丢弃后仅退回冷启动，[ADR-0120](../adr/0120-idle-keep-for-resume.md)）。
- **只读等待模块与会话隔离**：服务包内提供只读等待模块供支持后台命令自唤醒的宿主在后台运行，只读 `inbox/` 快照、不建 MCP 连接；发放资格沿父进程链探测真实宿主路径、参数与版本，不受 `AGENTNAVE_HOST` 短路（[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)）。服务运行在独立会话中，宿主挂起时中继暂停转发、已启任务继续运行，仅在 stdin 关闭时退出。
- **服务依赖方向**：`mcp_server` → `Conversations` → `Groups` → `InvocationManager` → Adapter / `processes`。Adapter 不拥有进程；`host` 与 Adapter 互不导入，按 CLI 读原生会话日志的逻辑放在两者都不依赖的中性模块。
- **交互模式运行时**：Claude（macOS／Linux 伪终端、Windows ConPTY；受限工具路径走 `-p`）与 Antigravity（macOS／Linux 且完全放开权限时）默认由服务自带 helper 驱动；启动信任与外部 `CLAUDE.md` 确认由 CLI 永久写入用户配置（[ADR-0056](../adr/0056-claude-interactive-runtime.md)、[ADR-0078](../adr/0078-startup-confirmations.md)、[ADR-0086](../adr/0086-windows-claude-interactive.md)、[ADR-0102](../adr/0102-agy-native-status.md)）。一轮须等会唤醒主回合的本轮后台任务结束才结算，已有回复但后台任务久未完成时仅发一次不计冷却的提醒（[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)）。
- **托盘进程与依赖方向**：托盘 App（`apps/tray`，Tauri 2）是独立进程，不调用 MCP 工具；协议入口只按路径分发到各业务模块，宿主层协调探测、规则与更新且反向不引用，共用工具放在无业务依赖的底层。运行记录失去活跃证据时，托盘仅在同一会话锁下确认无人持有，重读视图并在展示中标记中断，**绝不回写历史**（[ADR-0060](../adr/0060-tray-detects-orphaned-running-work.md)）。

## MCP 合同

- **严格校验与错误分级**：入参 schema 禁止未声明参数，整批拒绝而不静默丢弃（另有供中继空闲探测的未列出工具 `list_invocations`，[ADR-0024](../adr/0024-consolidated-tools-and-private-review.md)）。调用方可修正的错误返回 Tool error 及指引；Provider 终态返回结构化 `InvocationResult`，业务失败亦尽量保全输出与原生 `session_id`。
- **T3 Code 路由边界**：服务按连接区分 T3 主线程与 T3 子线程。T3 主线程在支持的编排档位且开启 T3 编排时，单独任务与评审改由选位返回 T3 候选交 T3 委派，`start_agent`/`start_agents` 拒绝，T3 任务不入 AgentNave 历史；会议、收件、历史与交接照常。T3 子线程拒绝选位、派发、开会与交接（[ADR-0121](../adr/0121-t3-code-orchestration.md)）。
- **单向编排提示与数据边界**：每次派发附加用户口吻的单向编排指令（不调 AgentNave、不转派、汇报未验证与阻塞；按需附只读或隔离目录边界），会议条目与转发内容标明为数据边界而非指令；依赖模型遵守，不构成安全边界（[ADR-0031](../adr/0031-prompt-only-orchestration.md)、[ADR-0043](../adr/0043-service-prompt-instructions.md)、[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)）。
- **生命周期与终止边界**：Invocation 仅存进程内存（不跨重启恢复），`wait` 返回就绪子集（[ADR-0007](../adr/0007-wait-any-ready-subset.md)）；无总运行时限，停止靠显式 `cancel`。服务仅在输出超限、持续重试无输出或终态后退出宽限期满时主动终止进程树（[ADR-0049](../adr/0049-service-initiated-termination.md)、[ADR-0095](../adr/0095-provider-terminal-grace.md)）；长时间无实质进展只发一次不终止、不计冷却的提醒（[ADR-0116](../adr/0116-progress-stall-reminder.md)）。
- **原生弹框代答**：支持原生弹框的 Adapter 保持运行中 stdin 通道，仅传回由工作区拥有者 boss 决定的答复；工具输入在交 boss 与落盘前打码，审计写入现有任务／席位 JSON 记录，不新增表结构（[ADR-0057](../adr/0057-boss-answers-native-prompts.md)）。
- **并发与转发边界**：原生会话锁互斥禁止并发续接及跨工作区／跨账号续接（[ADR-0099](../adr/0099-handover-links-workspaces.md)、[ADR-0108](../adr/0108-multi-account-isolation.md)）。同时运行上限仅在接纳时检查（保留期空闲进程不计入，复用时重新接纳，[ADR-0025](../adr/0025-session-desks-and-running-limit.md)、[ADR-0120](../adr/0120-idle-keep-for-resume.md)）。转发仅允许引用本工作区或已认领的直接交接来源工作区（有方向、不传递）的已成功任务与已提交条目，历史只存 boss 原文。

## 参数与默认值

- **来源优先级与入口套用**：Adapter 只附加协议必需参数，不得静默设置模型、档位、权限或提示增补；所有默认值、工位配置与权限档位在服务入口统一套用、不下沉到 Adapter（唯一例外：DeepSeek 官方路由恢复已存认证，[ADR-0027](../adr/0027-deepseek-official-route-stored-credentials.md)）。显式值优先但受模型池与范围校验，超出直接拒绝不静默替换；无档位时按 `cli-defaults.json` 兜底，并在回执 `defaults_applied` 注明来源（[ADR-0028](../adr/0028-follow-cli-default-model.md)、[ADR-0038](../adr/0038-role-applies-app-seat.md)、[ADR-0042](../adr/0042-pass-cli-default-effort.md)）。
- **工位顺位、随机选与模型池**：App 工位可在主选外配备选顺位，按 `(provider, account, model)` 判重与追踪；仅在派发新线程且启动前主选不可用时按顺位（或「决策模型」档开启同档随机选时于可用候选中随机）挑选，显式指定 CLI／模型、续接原生会话或启动后失败均不切换（[ADR-0089](../adr/0089-seat-fallback-order.md)、[ADR-0093](../adr/0093-decision-seat-random-candidates.md)）。CLI 可用模型池是任务、工位与会议的共同边界；启用编排时普通任务目录退出编排输入，停用角色不进入目录与 schema 但保留绑定（[ADR-0063](../adr/0063-cli-model-pool-and-orchestration-precedence.md)、[ADR-0065](../adr/0065-predefined-specialist-roles.md)）。
- **宿主识别与同类处理**：宿主按显式 `AGENTNAVE_HOST` → Unix 祖先进程只读匹配（Windows 只读环境变量）识别；具名账号接入行复用 `AGENTNAVE_HOST_APP=<provider>@<id>`。私聊同类宿主以「同 CLI 且同规范化账号目录」判定，按该 boss 设置交给宿主自带子代理、由服务启动或跳过，同 CLI 不同账号由服务直接启动；同类生图顺位始终跳过（[ADR-0090](../adr/0090-quota-group-cooldown-and-host-cli-choice.md)、[ADR-0094](../adr/0094-antigravity-host-cli-defaults-to-launch.md)、[ADR-0108](../adr/0108-multi-account-isolation.md)）。`AGENTNAVE_EXCLUDED_PROVIDERS` 不排除宿主自身，App 自检时自动清除遗留自排除项（[ADR-0096](../adr/0096-auto-heal-legacy-self-exclusion-and-recovered-faults.md)）。
- **权限档位**：权限选项只来自按 CLI 的权限档位（缺省完全放开；无自动审核时「自动」等同完全放开）、App 默认 options 或用户本轮明确要求，boss 不得自行追加；调用方显式传任一权限键或启用 `restricted_tools=true` 时不套用档位。Claude 两档不可用时自动以另一档重试一次（[ADR-0032](../adr/0032-cli-permission-tiers.md)、[ADR-0059](../adr/0059-claude-tier-fallback.md)、[ADR-0076](../adr/0076-full-permission-default.md)）。
- **环境变量隔离与凭据边界**：启动子进程（及托盘终端）时不继承宿主身份变量（`AGENTNAVE_HOST`、`AGENTNAVE_HOST_APP`、`AGENTNAVE_HOST_SESSION_ID`、`AGENTNAVE_EXCLUDED_PROVIDERS` 及中继会话变量）、`T3CODE_*`、`HERDR_*`、父宿主内部 HTTP 监听地址、`CLAUDE_CODE_OAUTH_TOKEN` 与宿主会话变量；WorkBuddy 派发的 CodeBuddy 不继承宿主产品认证（[ADR-0058](../adr/0058-claude-stored-login.md)、[ADR-0096](../adr/0096-auto-heal-legacy-self-exclusion-and-recovered-faults.md)、[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)、[ADR-0121](../adr/0121-t3-code-orchestration.md)）。保留 `ANTHROPIC_API_KEY`／`ANTHROPIC_AUTH_TOKEN`／自定义网关变量（共存附 warning）；默认账号不设不清账号目录变量，具名账号全链路透传对应账号环境变量且不改服务自身环境。
- **生图与决策模型边界**：生图由所选 CLI 自行调用生图工具，AgentNave 不直连生图接口（T3 生图工位独立管理）。「决策模型」档选位时服务向已配置决策 API 发送截断后的派发内容与候选描述，这是**服务唯一对外的模型调用**（[ADR-0087](../adr/0087-decision-model-recommends-seats.md)、[ADR-0123](../adr/0123-multi-decision-model-providers.md)）。

## Provider Adapter 与进程安全边界

- **Adapter 边界**：每个 Adapter 独占对应 CLI 的命令准备、选项校验、原生事件解析、流／文件模型回读、隔离席位准备、账号环境变量声明及排除继承环境变量清单；核心与历史层不按 Provider 名分支。每次准备命令时现解析可执行文件；同一 CLI 的不同安装来源不产生新的 Provider 身份（[ADR-0055](../adr/0055-app-updates-member-clis.md)）。任务原文不写服务日志，stderr 仅在失败时有界返回。
- **进程树所有权**：macOS／Linux 由专用 POSIX 进程组 supervisor 监督（声明保留能力的 Provider 可在正常回合后留在保留期供同会话续接，到期／挤出／服务退出时清理，[ADR-0120](../adr/0120-idle-keep-for-resume.md)）；Windows 必须挂起创建并先加入 kill-on-close Job Object 再恢复，任一步失败即报错不降级（[ADR-0004](../adr/0004-windows-job-object-supervision.md)）。AgentNave 不是沙箱，Provider 原生权限才是安全边界。
- **用户仓库与隔离目录唯一写入边界**：服务与托盘写用户仓库仅限主工作树根 `.agentnave-worktrees/`、`info/exclude`、`agentnave/` 受管分支及 git 公共目录下 `agentnave/` 仓库锁与租约（托盘不做提交、推送或开 PR，[ADR-0126](../adr/0126-tray-git-status-read-only.md)）。创建与续接重建遵循 `git worktree add`（执行仓库 hooks、filter 与 LFS，不初始化子模块，[ADR-0125](../adr/0125-isolated-dirs-follow-git-worktree.md)）；检查与删除绝不触发 hooks、filter 或 fsmonitor，不进入嵌套仓库。只读无改动且无其他租约的任务成功时自动释放、续接原位重建；其余由 boss 释放或托盘在同一仓库锁与租约下手动删除／规则自动清理（派生会话租约由托盘持至终端结束；双方均识别保留分支的「已收起」状态，[ADR-0085](../adr/0085-service-managed-isolated-dirs.md)、[ADR-0092](../adr/0092-auto-release-untouched-isolated-dirs.md)、[ADR-0098](../adr/0098-tray-deletes-isolated-dirs.md)、[ADR-0115](../adr/0115-tray-git-status-and-basic-ops.md)）。

## 会议

- 会议即保存的协作组（2–7 席），按并联或串联执行轮次，同组同时仅一轮；各席续接自有原生会话并按游标增量接收非己已提交条目（作数据送达，[ADR-0006](../adr/0006-core-rounds-and-studio-split.md)）。
- 可选审核在提交前决定公开或不公开，不回滚工具副作用或清除作者记忆；并联轮全员结束前可补跑失败／受阻席位（[ADR-0039](../adr/0039-retry-lost-seat-within-round.md)）。
- 会议本身不提供文件隔离：席位不能新建隔离目录，在已有隔离目录中运行时仅占用租约；隔离会议为每席建独立空目录并启用 `restricted_tools`，不等同操作系统沙箱。

## 历史、数据根与跨进程产物

数据根默认 `~/.agentnave`（`AGENTNAVE_DATA_DIR` 可覆盖）。两端所有持久化与跨进程产物按归属与隔离边界统一如下：

| 位置与产物 | 写入方 | 读取方 | 预览版行为 | 锁、生命周期与故障边界 |
| --- | --- | --- | --- | --- |
| 数据根 `agentnave.db`、会话锁、`live/`、原生会话锁 | 服务；托盘（仅限会话改名／删除与工作区清理） | 服务、托盘 | 只读，拒绝一切写操作 | 每会话一把跨语言 OS 文件锁；视图快照与写入同事务提交；schema 仅服务迁移；`live/` 仅运行期存在；原生会话锁用完即删 |
| 数据根 `worktrees.db` | 服务；托盘（手动删除与自动清理） | 服务、托盘 | 只读不写 | 独立于会话库与历史清理；仅服务迁移 schema；损坏不影响普通派发；受仓库锁与租约保护 |
| 数据根 `members.json`、`accounts.json`、`permissions.json`、`settings.json`、`cli-defaults.json` | 托盘（无 App 时前两者与 `settings.json` 可手写；`cli-defaults.json` 为探测缓存） | 服务（每次派发／清理现读）、托盘 | 只读不写 | 除 `cli-defaults.json` 与 `settings.json` 兜底外，损坏即返回 App 修复错误；默认账号不入 `accounts.json` |
| 数据根 `review.json`、`orchestration.json` | 托盘 | 服务、托盘 | 只读不写 | 损坏时不套用并由 `list_members` 报告，普通工位调用拒绝（评审工位按自身配置校验）；有效档位由 App 计算保存 |
| 数据根 `decision-seats.json`、`decision-keys.json`、`t3-seats.json` | 托盘（无 App 时配置可手写） | 服务、托盘 | 禁止写；**禁止读取正式 `decision-keys.json`** | `decision-keys.json` 仅当前用户可读；正式版仅在自定义端点与已保存地址一致时附带密钥 |
| 数据根 `provider-health.json` | **服务与托盘同锁共写** | 服务、托盘 | 只读不写 | 固定 schema v1，具名账号嵌套在 `accounts.<id>` 下；认证按账号、配额按额度组冷却与同组成功解冷（旧版按整 CLI 解冷会连带清除账号记录，[ADR-0108](../adr/0108-multi-account-isolation.md)） |
| 数据根 `inbox/`、`push/`、`spawn-requests/` | `inbox/` 服务写；`push/` 托盘写；`spawn-requests/` 服务写、托盘领取即删 | 互为只读；`inbox/` 另供包内只读等待模块 | 只读不写 | `inbox/` 与 `push/` 不含任务正文、进程退出清理；`spawn-requests/` 含任务正文、私有权限；`push/` 派生锚点保留至服务确认结算 |
| 数据根 `connections/`、`connection-faults/` | 中继写；托盘仅在刷新／确认时清理已恢复故障 | 托盘 | 只读，不删故障记录 | 仅含观测与配置行／组员标记，不含原始工具定义、提示词或凭据；托盘只读新鲜、限大小、非符号链接文件 |
| 数据根 `preferences.json`、`usage.db`、`pricing-cache.json`、`voice.json`、`voice-keys.json`、`voice-cache/`、`video-packs/` | 托盘独写 | 托盘（服务不读） | 写预览独立应用数据目录（`video-packs/` 读时先查预览目录再回退正式目录） | `voice-keys.json` 仅当前用户可读、不暴露给页面；`usage.db` 卸载随数据根开关处理 |
| `~/.agentnave-accounts/<provider>/<id>` | 托盘（建目录与同步接入／Skill） | 对应 CLI、托盘、服务 | 禁止建账号、登录、同步与删除 | 数据根外固定位置，不受 `AGENTNAVE_DATA_DIR` 或卸载删除影响；外部已登记目录永不删除 |
| 托盘自有配置目录（`projects.json`、派生会话启动配置、切换标记） | 托盘独写 | 托盘 | 写入预览独立目录 | 派生启动配置仅本人可读、不含密钥正文，随历史删除清理；`projects.json` 记目录与仓库归属，服务不感知 |
| 服务包共享定义 `orchestration_rules.json`、`orchestration_roles.json` | 随服务包发布 | 服务、托盘 | 只读 | 规则正文与角色目录的跨端单一真源 |
| 外部只读数据根（`T3CODE_HOME`、Magpie 本机网关配置） | 外部应用 | 服务／托盘按需只读 | 只读 | 绝不写外部文件、绝不读取外部凭据；读不到时降级不阻断（[ADR-0121](../adr/0121-t3-code-orchestration.md)） |

- **SQLite 锁与迁移**：每会话一把 OS 锁，持锁服务独写、其他服务只读、空闲可接手；托盘改名与删除受同一把锁保护，工作区级批量清理不取会话锁（[ADR-0012](../adr/0012-tray-conversation-cleanup.md)、[ADR-0026](../adr/0026-sqlite-conversation-store.md)）。`agentnave.db` 与 `worktrees.db` 每次 schema 变化均提升 `user_version`，**仅服务执行迁移**；托盘遇旧库只读展示并提示重连宿主（[ADR-0048](../adr/0048-sqlite-schema-evolution.md)）。
- **跨语言配置夹具**：App 与服务分别校验共享配置文件，由 `tests/fixtures/app-files/` 黄金样例在双端测试中共同断言接受与拒绝。
- **派生会话跨进程握手**：服务通过 `spawn-requests/` 交出结构化启动配置，托盘起终端、提交任务、按 CLI 判定一轮结束，并通过 `push/` 回报原生会话与**轮次起止锚点**至服务确认结算；**服务只读起止锚点之间的原生日志**（[ADR-0109](../adr/0109-in-app-derived-sessions.md)、[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)）。App 内推送唤醒仅 Codex 保留结构化通道，其余 CLI 不再接收托盘键入唤醒（[ADR-0105](../adr/0105-codex-app-server-terminal-push.md)、[ADR-0114](../adr/0114-tui-runtime-like-a-user.md)）。
- **连接回执恢复与归属**：中继在 `SIGTERM`/`SIGINT` 时与 stdin 关闭同等收尾；被强杀漏标结束的回执由托盘在确认中继已退出后补结（同名故障记录存在时除外）；配置行出现新健康连接即视为故障恢复（[ADR-0022](../adr/0022-auto-clear-host-reload.md)、[ADR-0088](../adr/0088-relay-records-service-exit-fault.md)、[ADR-0096](../adr/0096-auto-heal-legacy-self-exclusion-and-recovered-faults.md)、[ADR-0097](../adr/0097-relay-termination-signal-and-exited-tool-receipts.md)）。服务按工作区最后活动、交出与最近被接手时间自动清理过期记录，并独立回收孤儿锁、席位目录与超期空交接工作区（[ADR-0046](../adr/0046-service-history-retention.md)）。
- **身份、称呼与交接边界**：派发与收件要求确定的 boss 聊天身份（磁盘只存宿主与原始 ID 哈希，不回退全服务收件，[ADR-0017](../adr/0017-manager-chat-session-identity.md)）。成员称呼与旧视图重建由服务拥有，App 与前端只消费不重拼；账号邮箱／组织／套餐仅 App 本地展示，不进 MCP 回执、决策输入或日志（[ADR-0062](../adr/0062-service-owned-member-names.md)、[ADR-0108](../adr/0108-multi-account-isolation.md)）。交接只在工作区之间建立有方向关联，不转移原归属、在途调用或隔离租约，原 boss 下次派发收到一次核对工作树提醒（[ADR-0033](../adr/0033-boss-workspace-handover.md)、[ADR-0099](../adr/0099-handover-links-workspaces.md)）。模型优先取原生日志回报值，不从 CLI 默认配置推测。

## 托盘 App 与语音播报

- **协议与端口边界**：界面只经 `agentnave` 自定义协议提供，不开放 IPC、导航限于自身协议，写操作（含可能刷新并保存令牌的目录请求）一律要求 `X-AgentNave-Action: 1`。除 ChatGPT 订阅登录临时回环回调与 macOS 带启动令牌的只读视频回环端口外，不监听端口（[ADR-0080](../adr/0080-tray-video-loopback-http.md)）。
- **内嵌终端与派生会话所有权**：托盘拥有所有内嵌 PTY 终端进程（随 App 退出结束），不为终端写会话记录，按中继 PID 进程树与聊天键哈希关联工作区（每 agent 至多一个活终端，跨客户端占用先确认再续接）。Unix 上 Codex 经进程树内私有 `app-server` + `--remote` TUI 运行，Windows 维持 `--no-daemon`。派生会话进程归托盘而不归服务 supervisor，服务退出不终止它；空闲超时由托盘结束进程、保留会话并支持按已存启动配置随时续接，并随 boss 终端关闭（[ADR-0103](../adr/0103-tray-embedded-terminal.md)、[ADR-0105](../adr/0105-codex-app-server-terminal-push.md)、[ADR-0109](../adr/0109-in-app-derived-sessions.md)、[ADR-0124](../adr/0124-derived-session-lifecycle.md)）。
- **项目归属、Git 状态与用量边界**：项目由托盘从目录与 `projects.json` 派生，服务不感知；Git／PR 状态由托盘只读调用 `git`/`gh` 放内存（[ADR-0115](../adr/0115-tray-git-status-and-basic-ops.md)、[ADR-0126](../adr/0126-tray-git-status-read-only.md)）。本机用量由托盘独占拥有：以各 CLI 原生日志为单一数据源，不改服务与 `agentnave.db`，按服务同一哈希公式归因，低频只读拉取 `models.dev` 精简价表缓存且不上传任何本地数据（[ADR-0106](../adr/0106-tray-usage-panel.md)）。
- **宿主管理、设置、更新与卸载**：宿主配置、Skill 与规则块均定点编辑、先备份、写后校验（[ADR-0015](../adr/0015-app-host-integration.md)、[ADR-0036](../adr/0036-app-owns-service-skills.md)）。设置面向非技术用户以选择为主，仅密钥、端点、自定义摘要／人设与账号信息允许手填（[ADR-0029](../adr/0029-settings-without-free-text.md)、[ADR-0044](../adr/0044-meeting-voice-cloud-and-key-input.md)、[ADR-0108](../adr/0108-multi-account-isolation.md)、[ADR-0123](../adr/0123-multi-decision-model-providers.md)）；模型与档位只探测各 CLI 自身输出（[ADR-0034](../adr/0034-models-and-efforts-from-each-cli.md)）。App 更新经 Tauri 签名校验（视频包经归档与像素校验），启动对齐服务版本；Unix 上停止旧宿主进程前须重检归属／PID／启动时间且只发 `SIGTERM`（[ADR-0014](../adr/0014-tray-members-and-one-click-update.md)、[ADR-0101](../adr/0101-tray-stops-old-cli-copies.md)）。卸载由 App 确认与清理用户级接入／Skill／规则块，保留仓库隔离目录、信任标记与外部账号目录，托管目录仅在用户明确勾选且无活跃会话、校验真实路径后删除（Windows NSIS 经本机带令牌通道等待 App 确认，[ADR-0100](../adr/0100-app-uninstall.md)、[ADR-0108](../adr/0108-multi-account-isolation.md)）。
- **语音播报边界**：语音合成、摘要请求、密钥与 ChatGPT 令牌由托盘 Rust 端拥有，页面仅获状态、脱敏预览与合成音频；关闭或未配密钥时不发外网请求，Magpie 网关只读不改配置；限时收听状态归页面临时持有、不跨重启；语音配置不影响任何 CLI 参数。办公室主窗口内嵌设置与用量页时保持播放器不卸载。

### 预览版隔离（`AGENTNAVE_PREVIEW`）

- 预览版读取正式数据根用于界面目测，但**拒绝一切改写正式数据根、宿主配置、Skill、账号生命周期、应用／服务更新、隔离目录清理与故障记录删除的写操作**，启动亦不自动修复宿主接入。
- 预览版自己的显示偏好、`usage.db`、`pricing-cache.json`、`voice*`、`video-packs/`（读时先查预览目录再回退正式目录）与手动项目均写入其独立应用数据目录；预览版可浏览公开／本机决策模型目录，但绝不读取正式 `decision-keys.json`。
- **已知例外（[ADR-0103](../adr/0103-tray-embedded-terminal.md)）**：预览版内嵌终端里启动的 CLI 仍按宿主配置连接正式 AgentNave 服务，在其中发起的派活会写入正式会话历史。
