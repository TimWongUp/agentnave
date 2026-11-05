# 架构与边界

AgentNave 是供 Agent Manager 通过本地 STDIO MCP 使用的服务，在 Manager 与各 Provider CLI 之间提供很薄的进程边界；不是面向人的 CLI、多智能体运行时或工作流引擎。本文只写所有权、边界与稳定合同；参数、字段与路由的精确行为以代码、MCP schema 与测试为准，取舍见 ADR。

## 所有权

| 角色 | 拥有 |
| --- | --- |
| Manager（boss） | 目标解释、任务拆解、角色与模型选择、并行、重试、审核、综合、worktree 与长期任务状态 |
| AgentNave 服务 | 启动与取消 Provider 进程树、默认值套用、活动快照与终态归一化、进程内 Invocation 句柄、会议轮次、会话历史与按用户设定天数的自动清理 |
| 托盘 App | 宿主接入与 Skill 安装、组员与设置文件、历史展示、会话改名与手动清理、App 偏好（`preferences.json`）、编排规则块、应用更新与按版本安装服务、按原安装方式更新已装的 CLI（[ADR-0055](../adr/0055-app-updates-member-clis.md)） |
| Provider CLI | 自身的安装、认证、配置、权限与原生 Session |
| MCP Host | 注册格式、作用域与加载行为 |

统一核心总是保存历史，不再提供无历史模式（[ADR-0016](../adr/0016-single-core-and-mcp-entrypoint.md)）。AgentNave 不拥有 DAG、调度器、角色决策、自动重规划、worktree 或 TUI；历史保留天数由用户在 App 设置（不装 App 时手写）、写入数据根 `settings.json`，服务按它清理过期工作区，并独立回收孤儿资源与过期待接手空工作区（[ADR-0046](../adr/0046-service-history-retention.md)）。编排偏好属于用户的宿主全局规则，App 只在其中写入 AgentNave 标记的一段，服务不据此调度（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)）。

## 进程与模块

- `agentnave-mcp` 先启动只用标准库的中继（`relay.py`），由它运行真正的服务并转发 STDIO；中继所在环境原地重装且空闲时换用新子进程，宿主不断线。App 把每个版本装进独立目录，宿主重连后才用新版本（[ADR-0047](../adr/0047-versioned-service-install.md)）。空闲由服务的 `restart_safe` 判定：持有会话锁、有未落盘回写或有终态未送达的轮次时不换；新增只在进程内存、换进程会丢的状态也要计入它；中继另查在途请求与运行中或结果未读的 Invocation（[ADR-0010](../adr/0010-launcher-relay-swaps-upgraded-server.md)）。
- 服务内的依赖方向：`mcp_server`（工具与回执投影）→ `Conversations`（历史、工作区、转发）→ `Groups`（轮次）→ `InvocationManager`（进程生命周期）；`InvocationManager` 一边调用 Provider Adapter 准备命令与解析结果，一边经 `processes` 启动、取消 supervisor，Adapter 不拥有进程。宿主识别（`host`）与 Adapter 互不导入，按 CLI 读取原生会话记录的代码放在两者都不依赖的中性模块。
- Claude 在 macOS／Linux 默认以交互模式运行：Provider 进程是服务自带的 helper，它在同一 supervisor 进程组里用伪终端驱动 `claude`，把 transcript 与 hooks 归一成事件；Windows 与受限工具路径用 `-p`。交互运行会自动信任目录并确认环境里的 API key，这些记录由 Claude Code 永久写进用户配置（[ADR-0056](../adr/0056-claude-interactive-runtime.md)）。
- 托盘 App（`apps/tray`，Tauri 2）是独立进程：读取服务写下的视图快照与运行文件，经自定义协议提供界面，不调用 MCP 工具。

## MCP 合同

- 10 个工具（[ADR-0024](../adr/0024-consolidated-tools-and-private-review.md)、[ADR-0057](../adr/0057-boss-answers-native-prompts.md)，`handover` 见 [ADR-0033](../adr/0033-boss-workspace-handover.md)），另有供中继空闲探测、不列出的 `list_invocations`。入参 schema 禁止未列出的参数，整批拒绝而不静默丢弃。
- 可由 Manager 修正的请求错误走 Tool error 并给出重试指引；Provider 执行终态走结构化 Invocation Result，业务失败也尽量保全输出与原生 `session_id`。
- 每次派发的 prompt 都附加单向编排指令：完成任务后回报 Manager，不再调用 AgentNave，需要决定时写进最终回答；声明 `read` 时再加只读说明（[ADR-0043](../adr/0043-service-prompt-instructions.md)）。服务不检测嵌套、不据此禁用工具；这依赖模型遵守，不是安全边界（[ADR-0031](../adr/0031-prompt-only-orchestration.md)）。
- Invocation 只在进程内存中，终态保留七天；`wait` 返回就绪子集（[ADR-0007](../adr/0007-wait-any-ready-subset.md)）。没有总运行时限，停止靠显式 `cancel`；服务只在两种故障下主动终止进程树：保留的 stdout、stderr 超限（不静默截断为成功），或 Provider 持续重试满五分钟仍无输出（[ADR-0049](../adr/0049-service-initiated-termination.md)）。
- 声明支持原生弹框的 Adapter 可保持运行中 stdin 通道，只传回 CLI 自己发起的弹框答复；核心不识别 Provider 协议，决策归当前拥有工作区的 boss。待答弹框唤醒任务、会议与收件箱等待，等待期间不计持续重试，取消先拒绝再终止（[ADR-0057](../adr/0057-boss-answers-native-prompts.md)）。
- 服务把弹框与答复审计写入任务或席位已有的 JSON 记录，属于服务与托盘共用的历史合同，不引入新表结构；工具输入在交给 boss 和保存前打码。
- 同一 Provider 的同一原生 session 不得同时续接：数据根下的原生会话锁让共用数据根的各服务互斥。
- 同时运行的 CLI 进程数（私聊加席位）受 App 设置的上限约束，只在接纳时检查（[ADR-0025](../adr/0025-session-desks-and-running-limit.md)）。
- 转发只注入本工作区已完成的消息，并声明为数据；历史只保存 boss 原文。

## 参数与默认值

- Adapter 只加协议必需参数。其余参数只能来自调用方显式值、用户在 App 中设定的默认值、`role`／`seat` 指明的评审、Explorer／Worker 工位，或续接原生会话时本 boss 的最近一次任务；服务在回执 `defaults_applied` 注明继承与推断来源（[ADR-0038](../adr/0038-role-applies-app-seat.md)）。仍无档位时，服务按 App 探测到的 CLI 自身默认档位兜底（[ADR-0042](../adr/0042-pass-cli-default-effort.md)）。唯一例外是 DeepSeek 官方路由恢复已存认证（[ADR-0027](../adr/0027-deepseek-official-route-stored-credentials.md)）。
- 显式值优先；超出 App 允许范围时拒绝，不替换为别的模型或档位。默认值在服务入口统一套用，不下沉到 Adapter（[ADR-0013](../adr/0013-members-defaults-and-host-identity.md)、[ADR-0021](../adr/0021-keep-default-models-when-unrestricted.md)、[ADR-0028](../adr/0028-follow-cli-default-model.md)）。
- 私聊可由工位确定 CLI；需要它确定 CLI 时必须读到可用工位，宿主原生工位由宿主自带子代理执行。新任务的执行目录由调用方明确提供，续接只继承本 boss 的任务元数据并保持原生会话的 Provider 身份。工位决定访问等级，与之矛盾的声明被拒绝；访问等级仍是声明，不改变原生权限。
- 权限类参数只来自按 CLI 的权限档位（缺省「自动」；CLI 没有可用的自动审核时「自动」即完全放开）、App 中的默认 options 或用户本轮明确要求，boss 不得自行追加；档位与 App 默认值在续接时同样套用并在回执注明。调用方给出该 CLI 任一权限键时整档不套用；受限工具运行不套用档位；只按 `cli-defaults.json` 回退时不继承权限等执行控制（[ADR-0028](../adr/0028-follow-cli-default-model.md)、[ADR-0032](../adr/0032-cli-permission-tiers.md)）。Claude 两档所选的模式不可用时，改用另一档的模式重跑一次，两种都不可用时任务受阻（[ADR-0059](../adr/0059-claude-tier-fallback.md)）。
- `restricted_tools=true` 是显式受限配置，普通调用不继承；它收紧原生工具，但不等于操作系统沙箱。
- Provider 进程不继承 `CLAUDE_CODE_OAUTH_TOKEN`，Claude 子代理一律用 Claude Code 自己存的登录，不随 boss 宿主变化；AgentNave 不保存、不读取 Claude 凭据（[ADR-0058](../adr/0058-claude-stored-login.md)）。

## Provider Adapter

- 每个 Adapter 负责一个 CLI 的命令准备、选项校验、原生事件解析与终态归一化。Provider 专属知识留在 Adapter 内：流事件里的模型、运行后从 CLI 自身文件回读模型、隔离席位目录的准备，都通过 `ProviderAdapter` 的钩子提供；是否支持受限工具运行、受限时额外允许的选项和选项说明也由各 Adapter 声明。核心与历史层不按 Provider 名分支。
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

数据根默认 `~/.agentnave`（`AGENTNAVE_DATA_DIR` 可覆盖）。会话历史存于 SQLite（[ADR-0026](../adr/0026-sqlite-conversation-store.md)），每个会话一把 OS 文件锁：持锁的服务才能写，其他服务只读，空闲后可接手。服务回收孤儿会话锁与席位目录，删除会话后回收其席位目录；原生会话锁由持有者用完即删（[ADR-0046](../adr/0046-service-history-retention.md)）。schema 每次变化都提升 `user_version`，迁移只由服务执行（[ADR-0048](../adr/0048-sqlite-schema-evolution.md)）。写入时在同一事务内更新托盘读取的视图快照（[ADR-0026](../adr/0026-sqlite-conversation-store.md)）；运行中的输出尾部只写进程级运行文件，内容不变时只刷新修改时间。中继在 `connections/` 写连接观测（宿主、连接时间、服务版本与工具定义摘要），不存原始工具定义、提示词、环境或凭据；正常结束后保留有期限的加载证据，由中继清理，App 将它与在线连接分开读取，用于确认短连接已加载配置。App 只读新鲜、大小受限、非符号链接的观测（[ADR-0022](../adr/0022-auto-clear-host-reload.md)）。

App 设置文件按写入方和损坏时的影响范围分开：

| 文件 | 写入方 | 服务的读法 |
| --- | --- | --- |
| `members.json` | App；不装 App 时由用户手写 | 每次派发现读；损坏时返回 App 修复错误 |
| `review.json`、`orchestration.json` | App | 按职责或工位选择读取；编排工位仅对当前接入行启用，分组后的有效档位由 App 计算保存，服务不重复推导规则文件分组；损坏时不套用，由 `list_members` 报告，需要工位确定 CLI 时拒绝调用 |
| `permissions.json` | App | 每次派发现读；损坏时返回 App 修复错误 |
| `settings.json` | App；不装 App 时由用户手写 | 每次接纳时读取同时运行上限；清理时读取保留天数；派发 Claude 时读取交互模式开关，缺省打开 |
| `cli-defaults.json` | App 探测结果 | CLI 自身默认模型与档位的兜底 |

App 与服务分别校验同一批文件；`tests/fixtures/app-files/` 中的样例由两侧测试共同断言接受或拒绝，保证 App 能保存的服务就能读取。

## 身份与工作区

- 宿主在服务启动时识别：显式 `AGENTNAVE_HOST` 优先，否则 macOS／Linux 沿祖先进程只读匹配，Windows 只读环境变量。宿主身份决定单任务排除的同类 Provider，不约束会议席位。
- `start_agent`、`run_round` 要求 boss 聊天身份 `manager_session_id`；磁盘只存宿主与原始 ID 的哈希，作为工作区的分组键；交接记录另存老 boss 原生会话文件的路径（[ADR-0017](../adr/0017-manager-chat-session-identity.md)、[ADR-0033](../adr/0033-boss-workspace-handover.md)）。
- 工作区可交给另一个 boss 聊天接手；交接不转移运行中的调用，老 boss 之后的写入被拒绝；没有会话的聊天交出时，`workspaces` 行自带名字和目录，由它承载只有 boss 的工作区（[ADR-0033](../adr/0033-boss-workspace-handover.md)）。
- boss 与组员的模型优先取原生会话记录中的回报值，其次取调用声明；不从 CLI 全局默认值推测。

## 托盘 App

- 界面只经 `agentnave` 自定义协议提供：不开放 IPC、导航限于自身协议；除 ChatGPT 订阅登录期间仅在回环地址临时监听登录回调端口外，不监听端口。界面写操作都要求 `X-AgentNave-Action: 1`，可能刷新并保存令牌的目录请求也属于写操作。
- 读：视图快照与运行文件只读打开，不调用工具或 CLI。写：只限会话改名与删除（同一把会话锁，[ADR-0012](../adr/0012-tray-conversation-cleanup.md)）、工作区交接标记、清空工作区时删去交接记录和删除只有 boss 的工作区（不取会话锁，[ADR-0033](../adr/0033-boss-workspace-handover.md)）、数据根的设置文件、语音设置 `voice.json`、语音密钥与登录令牌 `voice-keys.json`、语音缓存 `voice-cache/`、宿主配置与 Skill（定点编辑、先备份、写后校验，[ADR-0015](../adr/0015-app-host-integration.md)、[ADR-0036](../adr/0036-app-owns-service-skills.md)）以及全局规则中的 AgentNave 块（[ADR-0037](../adr/0037-app-writes-orchestration-rules.md)）。托盘不迁移会话库的 schema：遇到旧版本的库只读展示，写操作提示先重连宿主（[ADR-0048](../adr/0048-sqlite-schema-evolution.md)）。
- 设置界面面向非技术用户，只提供开关、单选、下拉与档位条，不让手填（[ADR-0029](../adr/0029-settings-without-free-text.md)）；语音 API 密钥、自定义口语摘要连接信息、摘要要求及语音人设为例外（[ADR-0044](../adr/0044-meeting-voice-cloud-and-key-input.md)）。
- 模型与档位以各 CLI 自己给出的为准，App 只探测、不猜测（[ADR-0034](../adr/0034-models-and-efforts-from-each-cli.md)）。
- 员工页的 CLI 更新只沿该 CLI 原来的安装方式执行，不提权、不替用户安装；没装的 CLI 只给出官方安装命令（[ADR-0055](../adr/0055-app-updates-member-clis.md)）。
- 应用内更新只访问本仓库的 GitHub Release，用户确认后安装 App（[ADR-0014](../adr/0014-tray-members-and-one-click-update.md)）。App 每次启动都把服务对齐到配套版本：新版本装进独立目录，旧目录在不再使用并过宽限期后回收；连接的版本落后时，宿主卡片立即提示重连（[ADR-0047](../adr/0047-versioned-service-install.md)）。

## 语音播报

- 语音合成、可选的口语摘要请求与密钥由托盘 Rust 端拥有，页面只能取得密钥是否已保存和合成音频，拿不到密钥本身。合成只使用 ElevenLabs 与 MiniMax，默认选 ElevenLabs 且功能关闭；关闭时或所选引擎未配置密钥时不合成或联系语音引擎。开启并配置密钥后，发言文本会发送给所选云端引擎。
- 口语摘要通过 ChatGPT 订阅登录、各平台 API Key、Magpie 本机网关或用户配置的 OpenAI 兼容接口生成。ChatGPT 登录与令牌由托盘 Rust 端拥有，直接请求 OpenAI API；各平台分别保存 API 密钥。Magpie 拥有供应商认证和模型路由，托盘只读其本机端口配置、核验网关身份并获取模型目录，不修改 Magpie 配置，也不把摘要模型接入为 Provider CLI。语音关闭时不联网读取摘要模型目录；启用语音和摘要后，长发言先发送给摘要模型，合成引擎只收到短口语；摘要失败时停止该条播报并报错。
- 语音直接复用右侧消息列表中已有的 boss 发言与员工回答正文，按消息时间共用摘要与播放队列。自动播报与语音总开关独立；模型 prompt 定义消息缩写规则，按小人家族及 boss 保存的人设定义性格、措辞与口吻，输出保留发言者本人的第一人称视角；两者仅供语音使用，不修改 CLI 任务或 Provider 参数。
- 托盘 App 独占写入数据根的 `voice.json`、`voice-keys.json`（含摘要平台密钥与 ChatGPT 登录令牌）和 `voice-cache/`；服务不读取这些文件。预览版的语音设置、密钥和缓存写入其独立的应用数据目录，不写正式版数据根。
