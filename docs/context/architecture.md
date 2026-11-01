# 架构与边界

AgentNave 是供 Agent Manager 通过本地 STDIO MCP 使用的服务，在 Manager 与各 Provider CLI 之间提供很薄的进程边界；不是面向人的 CLI、多智能体运行时或工作流引擎。本文只写所有权、边界与稳定合同；参数、字段与路由的精确行为以代码、MCP schema 与测试为准，取舍见 ADR。

## 所有权

| 角色 | 拥有 |
| --- | --- |
| Manager（boss） | 目标解释、任务拆解、角色与模型选择、并行、重试、审核、综合、worktree 与长期任务状态 |
| AgentNave 服务 | 启动与取消 Provider 进程树、默认值套用、活动快照与终态归一化、进程内 Invocation 句柄、会议轮次、会话历史与按需只读看板 |
| 托盘 App | 宿主接入与 Skill 安装、组员与设置文件、历史展示、会话改名与清理、编排规则块、应用与服务更新 |
| Provider CLI | 自身的安装、认证、配置、权限与原生 Session |
| MCP Host | 注册格式、作用域与加载行为 |

统一核心总是保存历史，不再提供无历史模式（[ADR-0016](../adr/0016-single-core-and-mcp-entrypoint.md)）。AgentNave 不拥有 DAG、调度器、角色决策、自动重规划、worktree、保留策略或 TUI。编排偏好属于用户的宿主全局规则，App 只在其中写入 AgentNave 标记的一段，服务不据此调度（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)）。

## 进程与模块

- `agentnave-mcp` 先启动只用标准库的中继（`relay.py`），由它运行真正的服务并转发 STDIO；安装变化且空闲时换用新子进程，宿主不断线。空闲由服务的 `restart_safe` 判定：持有会话锁、有未落盘回写或有终态未送达的轮次时不换；新增只在进程内存、换进程会丢的状态也要计入它；中继另查在途请求与运行中或结果未读的 Invocation（[ADR-0010](../adr/0010-launcher-relay-swaps-upgraded-server.md)）。
- 服务内的依赖方向：`mcp_server`（工具与回执投影）→ `Conversations`（历史、工作区、转发）→ `Groups`（轮次）→ `InvocationManager`（进程生命周期）→ Provider Adapter → supervisor。
- 托盘 App（`apps/tray`，Tauri 2）是独立进程：读取服务写下的视图快照与运行文件，经自定义协议提供界面，不调用 MCP 工具。

## MCP 合同

- 9 个工具（[ADR-0024](../adr/0024-consolidated-tools-and-private-review.md)，`handover` 见 [ADR-0033](../adr/0033-boss-workspace-handover.md)），另有供中继空闲探测、不列出的 `list_invocations`。入参 schema 禁止未列出的参数，整批拒绝而不静默丢弃。
- 可由 Manager 修正的请求错误走 Tool error 并给出重试指引；Provider 执行终态走结构化 Invocation Result，业务失败也尽量保全输出与原生 `session_id`。
- 每次派发的 prompt 都附加单向编排指令：完成任务后回报 Manager，不再调用 AgentNave。服务不检测嵌套、不据此禁用工具；这依赖模型遵守，不是安全边界（[ADR-0031](../adr/0031-prompt-only-orchestration.md)）。
- Invocation 只在进程内存中，终态保留七天；`wait` 返回就绪子集，没有总运行时限，停止只靠显式 `cancel`（[ADR-0007](../adr/0007-wait-any-ready-subset.md)）。保留的 stdout、stderr 有上限，超限即终止进程树，不静默截断为成功。
- 同一 Provider 的同一原生 session 不得同时续接：数据根下的原生会话锁让共用数据根的各服务互斥。
- 同时运行的 CLI 进程数（私聊加席位）受 App 设置的上限约束，只在接纳时检查（[ADR-0025](../adr/0025-session-desks-and-running-limit.md)）。
- 转发只注入本工作区已完成的消息，并声明为数据；历史只保存 boss 原文。

## 参数与默认值

- Adapter 只加协议必需参数。其余参数只能来自调用方显式值、用户在 App 中设定且在回执 `defaults_applied` 注明的默认值，或 `role` 指明的评审、Explorer／Worker 工位（[ADR-0038](../adr/0038-role-applies-app-seat.md)）。仍无档位时，服务按 App 探测到的 CLI 自身默认档位兜底（[ADR-0042](../adr/0042-pass-cli-default-effort.md)）。唯一例外是 DeepSeek 官方路由恢复已存认证（[ADR-0027](../adr/0027-deepseek-official-route-stored-credentials.md)）。
- 显式值优先；超出 App 允许范围时拒绝，不替换为别的模型或档位。默认值在服务入口统一套用，不下沉到 Adapter（[ADR-0013](../adr/0013-members-defaults-and-host-identity.md)、[ADR-0021](../adr/0021-keep-default-models-when-unrestricted.md)、[ADR-0028](../adr/0028-follow-cli-default-model.md)）。
- 权限类参数只来自按 CLI 的权限档位（缺省「自动」；CLI 没有可用的自动审核时「自动」即完全放开）、App 中的默认 options 或用户本轮明确要求，boss 不得自行追加；档位与 App 默认值在续接时同样套用并在回执注明。调用方给出该 CLI 任一权限键时整档不套用；受限工具运行不套用档位；只按 `cli-defaults.json` 回退时不继承权限等执行控制（[ADR-0028](../adr/0028-follow-cli-default-model.md)、[ADR-0032](../adr/0032-cli-permission-tiers.md)）。
- `restricted_tools=true` 是显式受限配置，普通调用不继承；它收紧原生工具，但不等于操作系统沙箱。

## Provider Adapter

- 每个 Adapter 负责一个 CLI 的命令准备、选项校验、原生事件解析与终态归一化。Provider 专属知识留在 Adapter 内：流事件里的模型、运行后从 CLI 自身文件回读模型、隔离席位目录的准备，都通过 `ProviderAdapter` 的钩子提供，核心与历史层不按 Provider 名分支。
- 可执行文件在每次准备命令时解析，加装或卸载 CLI 下次调用即生效；Adapter 不负责安装、升级或版本比较。
- 任务原文只存于会话历史，不写服务日志；stderr 只在失败结果中以有界详情返回。

## 进程所有权与安全边界

- macOS／Linux：每次 Invocation 由专用 supervisor 持有 POSIX 进程组，正常结束也先清理整组；服务被强杀时 supervisor 自行终止进程组。
- Windows：专用 supervisor 挂起创建 Provider，先加入 kill-on-close Job Object 再恢复；任何一步失败都以 `launch_error` 失败，不降级为无树级所有权的启动（[ADR-0004](../adr/0004-windows-job-object-supervision.md)）。
- AgentNave 不是沙箱。主动杀 supervisor、创建新 OS session 或其他同用户逃逸不在隔离范围内；Provider 原生权限才是安全边界。

## 会议

- 会议就是保存的协作组，2–7 个席位，按并联或串联运行轮次；同组同时只有一个轮次（[ADR-0006](../adr/0006-core-rounds-and-studio-split.md)）。
- 各席续接自己的原生会话，只收到上次之后、非自己的已提交条目；其他席位的回答以数据身份送达。
- 可选审核在提交前决定公开或不公开，不回滚工具副作用，也不清除作者会话的记忆。并联轮内可补跑失败席位（[ADR-0039](../adr/0039-retry-lost-seat-within-round.md)）。
- 信息共享规则不提供文件隔离；写代码的并联席位由 Manager 分配独立 worktree。隔离会议为每席建独立目录并启用受限工具。

## 历史与数据根

数据根默认 `~/.agentnave`（`AGENTNAVE_DATA_DIR` 可覆盖）。会话历史存于 SQLite（[ADR-0026](../adr/0026-sqlite-conversation-store.md)），每个会话一把 OS 文件锁：持锁的服务才能写，其他服务只读，空闲后可接手。写入时在同一事务内更新托盘读取的视图快照（[ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md)）；运行中的输出尾部只写进程级运行文件，内容不变时只刷新修改时间。中继在 `connections/` 写连接观测（宿主、连接时间、服务版本与工具定义摘要），不存原始工具定义、提示词、环境或凭据；正常结束后保留有期限的加载证据，由中继清理，App 将它与在线连接分开读取，用于确认短连接已加载配置。App 只读新鲜、大小受限、非符号链接的观测（[ADR-0022](../adr/0022-auto-clear-host-reload.md)）。

App 设置文件各自独立，以便旧版服务仍能读取组员配置：

| 文件 | 写入方 | 服务的读法 |
| --- | --- | --- |
| `members.json` | App | 每次派发现读；损坏时返回 App 修复错误 |
| `review.json`、`orchestration.json` | App | 按 `role` 读取工位；编排工位仅对当前接入行启用，分组后的有效档位由 App 计算保存，服务不重复推导规则文件分组；损坏时不套用，由 `list_members` 报告 |
| `permissions.json`、`cli-options.json` | App | 每次派发现读；损坏时返回 App 修复错误 |
| `settings.json` | App | 每次接纳时读取同时运行上限 |
| `cli-defaults.json` | App 探测结果 | CLI 自身默认模型与档位的兜底 |

App 与服务分别校验同一批文件；`tests/fixtures/app-files/` 中的样例由两侧测试共同断言接受或拒绝，保证 App 能保存的服务就能读取。

## 身份与工作区

- 宿主在服务启动时识别：显式 `AGENTNAVE_HOST` 优先，否则 macOS／Linux 沿祖先进程只读匹配，Windows 只读环境变量。宿主身份决定单任务排除的同类 Provider，不约束会议席位。
- `start_agent`、`run_round` 要求 boss 聊天身份 `manager_session_id`；磁盘只存宿主与原始 ID 的哈希，作为工作区的分组键；交接记录另存老 boss 原生会话文件的路径（[ADR-0017](../adr/0017-manager-chat-session-identity.md)、[ADR-0033](../adr/0033-boss-workspace-handover.md)）。
- 工作区可交给另一个 boss 聊天接手；交接不转移运行中的调用，老 boss 之后的写入被拒绝；没有会话的聊天交出时，`workspaces` 行自带名字和目录，由它承载只有 boss 的工作区（[ADR-0033](../adr/0033-boss-workspace-handover.md)）。
- boss 与组员的模型优先取原生会话记录中的回报值，其次取调用声明；不从 CLI 全局默认值推测。

## 托盘 App

- 界面只经 `agentnave` 自定义协议提供：不开放 IPC、不监听端口、导航限于自身协议；写操作都要求 `X-AgentNave-Action: 1`。
- 读：视图快照与运行文件只读打开，不调用工具或 CLI。写：只限会话改名与删除（同一把会话锁，[ADR-0012](../adr/0012-tray-conversation-cleanup.md)）、工作区交接标记、清空工作区时删去交接记录和删除只有 boss 的工作区（不取会话锁，[ADR-0033](../adr/0033-boss-workspace-handover.md)）、数据根的设置文件、宿主配置与 Skill（定点编辑、先备份、写后校验，[ADR-0015](../adr/0015-app-host-integration.md)、[ADR-0036](../adr/0036-app-owns-service-skills.md)）以及全局规则中的 AgentNave 块（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)）。
- 设置界面面向非技术用户，只提供开关、单选、下拉与档位条，不让手填（[ADR-0029](../adr/0029-settings-without-free-text.md)）。
- 模型与档位以各 CLI 自己给出的为准，App 只探测、不猜测（[ADR-0034](../adr/0034-models-and-efforts-from-each-cli.md)）。
- 应用内更新只访问本仓库的 GitHub Release，用户确认后先升级服务再安装 App（[ADR-0014](../adr/0014-tray-members-and-one-click-update.md)）。
