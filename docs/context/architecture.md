# 架构与边界

AgentNave 是供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用的 MCP Server。它在 Manager 与 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Pi Coding Agent、DeepSeek Harness 之间提供很薄的进程边界，不是面向人的 CLI 产品、多智能体运行时或工作流引擎。

## 所有权

Manager 拥有目标解释、任务拆解、角色和模型选择、并行策略、重试、审核循环、结果综合、工作树和长期任务状态。

AgentNave 只拥有：

- 按需返回所选 Provider 的允许状态和支持参数，以及用户组员配置的校验结果；
- 根据冻结请求启动一个 Provider CLI；
- 传递 prompt、cwd、可选 `session_id` 和经过默认值套用的 Provider Options；
- 从 Provider 原生流事件提取有界活动快照，并归一化终端结果；
- 执行显式取消和进程树清理；
- 在 MCP server 生命周期内保存 Invocation 句柄；
- 在 Manager 显式创建的协作组内，按名单并联或串联运行轮次，只在进程内存中保存各席最终回答与原生会话游标。

发行包只保留 `agentnave`，唯一入口 `agentnave-mcp` 提供 8 个工具（[ADR-0024](../adr/0024-consolidated-tools-and-private-review.md)）。普通任务与协作组均保存历史，包含可选审核、主持人发言、隔离席位和按需只读看板；App 负责安装接入、配置、展示与清理。不再提供无历史模式，数据根和既有会话格式沿用，见 [ADR-0016](../adr/0016-single-core-and-mcp-entrypoint.md)。

AgentNave 不拥有 DAG、调度器、角色决策、自动重规划、工作树、保留策略或 TUI；编排偏好属于用户的宿主全局规则，AgentNave 只在 App 中给出模板与文件入口，不写入。托盘应用展示会话数据，并可在原生锁保护下改名与清理会话记录，见 [ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md) 与 [ADR-0012](../adr/0012-tray-conversation-cleanup.md)。

## 单向编排

单向编排由派发指令约束。统一核心在每次实际派发、Adapter 编码之前，为私聊和会议席位的 prompt 添加固定指令：只完成分配任务、回报 Manager、不再调用 AgentNave，遇阻交回 Manager；原生会话续接同样带此指令。转发和会议上下文先组装，再统一添加派发指令；历史仍保存调用方的原始任务文本。

服务不根据环境标记或祖先进程判断嵌套，也不因身份无法读取而禁用工具。此约定依赖模型遵守指令，不是工具权限或安全隔离；Provider 原生权限仍是安全边界。取舍见 [ADR-0031](../adr/0031-prompt-only-orchestration.md)。

## 安装与激活边界

运行时由 Python Tool Manager 安装，并在 MCP Host 注册稳定入口。配套 `agentnave-manager` Skill 承担 CLI 调用的主要触发、模型选择、传参、等待、取消与续接说明，App 维护按模式生效的组员默认值，Skill 按说明选择组员；CLI 参考文件仅提供补充指引。MCP instructions 与 Tool 描述只保留轻量调用合同，`list_members(provider)` 按需返回允许状态与支持参数；没有 Skill 时仍可显式调用 MCP。Skill 可由 App 安装内嵌副本，或由宿主的技能安装机制或已有部署管理器单独安装与更新，MCP 与 Skill 都只提供 CLI 调用能力与用法，不制定调用方的任务规划、调度、审核、重试或综合策略。可选的 `agentnave-review` Skill 是独立配套：为调用方提供 PR／合并的评审门控，由调用方 Agent 自行执行并经 AgentNave 派只读评审工位，不进入运行时、MCP 或 `agentnave-manager`（[ADR-0030](../adr/0030-optional-review-gate-skill.md)）。

Python Tool Manager 拥有运行时的安装、升级与卸载，并提供稳定的 `agentnave-mcp` launcher。launcher 先启动只用标准库的中继，由它把服务作为子进程运行并转发 STDIO；安装指纹变化后，中继在两侧无在途请求、无运行中或未读结果的 Invocation，且会话没有持锁或未落盘回写时，重放宿主握手换用新子进程，宿主不断线。2026-07-28 起的宿主没有握手，中继改用其请求信封探测与启动新子进程；`subscriptions/listen` 订阅流不算在途请求，切换时转交新子进程（[ADR-0010](../adr/0010-launcher-relay-swaps-upgraded-server.md)）。源码 checkout 只用于贡献、调试和未发布版本验证，不作为 MCP Host 的长期启动路径。

MCP Host 拥有注册格式、作用域和加载行为；托盘 App 可管理用户级 `agentnave` 注册及自身安装的 Skill（[ADR-0015](../adr/0015-app-host-integration.md)），由 App 定点编辑并校验，不调用宿主 CLI 写配置。安装 Python 运行时本身不会写入 Host Skills、全局规则或权限。Provider CLI 继续拥有自身的安装、认证、配置与 Session 数据，Invocation 句柄是进程内对象。核心在数据根保存会话历史；卸载不自动删除历史。

## 稳定合同

请求字段为 `provider`、`prompt`、绝对 `cwd`、可选 `session_id` 和 `provider_options`。Provider Options 可由调用方显式给出，或来自用户在 AgentNave App 中配置的组员默认值；均须通过对应 Adapter allowlist。显式参数优先，超出配置范围时拒绝，不替换为其他模型或 effort；未配置的参数继承 Provider 原生设置。默认值在服务入口套用，不下沉到 Adapter。权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`）只能来自用户在 App 中设置的默认 options 或用户本轮明确要求，boss 不得自行追加；App 中的权限默认值在调用明确模型（含 App 默认模型）时套用并在回执注明；仅按 `cli-defaults.json` 缓存回退时不继承权限、仓库检查绕过、工具、agent、mode 或 project 等执行控制，避免 CLI 默认变化后将另一模型的授权带入调用。权限另来自按 CLI 的权限档位（数据根 `permissions.json`，版本 1，`clis.<provider>` 为 `auto`／`full`，文件或条目缺失即 `auto`；没有可用自动审核的 CLI 的 `auto` 即完全放开；Pi 无档位）：服务把档位映射为该 CLI 的原生选项，与模型无关地在任务与会议席位入口套用并在回执注明；调用方显式给出该 CLI 任一权限键时整档不套用、也不补入模型级权限键，否则档位替换模型级默认 options 中的权限键，受限工具运行不套用；会议节目在开节目时定下档位（[ADR-0032](../adr/0032-cli-permission-tiers.md)）。Claude 与 CodeBuddy 报成功但有工具调用被拒（Claude 的 `permission_denials`，CodeBuddy 只在工具结果中写明）时，终态回执带 `warnings`，状态不变。

同一 server 进程内，同一 Provider 的同一原生 session_id 不得同时续接：共享启动入口在准备命令前拒绝重复活跃调用，单任务与组席位共用此检查。在写入运行历史前检查，拒绝不产生新的任务记录。这不是跨 server 或原生直接调用的全局锁；不同 Provider 的相同标识互不影响。

私聊续接（[ADR-0025](../adr/0025-session-desks-and-running-limit.md)）：`start_agent` 默认开新线程，续接只能显式传 `session_id`；旧参数 `continue_desk` 已移除，与其他未列出的参数一样会被拒绝，续接改传 `session_id`。`role` 是 1–40 个可打印字符的可选标签，空白视为无角色；它只显示在工位上，不参与身份，不进入 prompt 或 Provider 参数。可选 `access`（`read`／`write`）只是本次调用的访问等级标签：保存在任务与席位记录、回显在回执，工位按最近一次声明显示；不拒绝调用，不改变 Provider 参数，也不由服务推断。

同时运行上限（ADR-0025）：每次 `start_agent` 与带席位的 `run_round` 现读数据根 `settings.json`（版本 1，`max_running` 为 1–32 的整数，缺失时默认 8，忽略未知键；损坏或越界时返回 App 修复错误）。本服务仍在运行的 Invocation（私聊任务与席位合计）加上这次将启动的数量超过上限时拒绝：私聊计 1，并联轮计席位数，串联轮计 1。检查只在接纳时进行：串联轮内后续席位不再检查，切换席位的瞬间其他任务可能占用名额，造成短暂超出一个。上限按服务进程计算，多个宿主连接各自计数。

转发：`start_agent` 与 `run_round` 的 `forward` 按序引用消息 ID，只用于本工作区、已完成的消息：成功的私聊任务终态，以及节目中已提交的 `answer`、`host`、`task`、`forward` 条目。运行中、失败或受阻的任务，待审、不公开的回答，`absent` 条目，另一工作区或身份未知的引用，重复引用与超过 20 条均整批拒绝，不部分注入。消息 ID 为不透明的稳定标识，来源于会话 ID 与任务或条目序号，由 `wait` 的任务终态与轮次 `entries`、`run_round` 与 `decide_answers` 的回执以及 `read_conversation` 返回，跨重启不变。私聊中被转发内容前置于 prompt 并声明为数据，会话历史仍只存 boss 原文；会议室中在本轮任务之前提交为独立的 `forward` 条目，注明来源，随条目规则送达席位，不冒充主持人发言。声明只降低误导概率，不是安全边界。

MCP 初始元数据仅保留简短 Provider 目录和通用调用合同。Manager 首次使用某个 Provider 前调用只读 `list_members(provider=...)`，在组员名单之外另取 `provider_details`：`provider`、`permitted`（单任务，含宿主排除）、`show_permitted`（席位，按 discussion 组员限制）与 `supported_options`，并在当前上下文中复用；详情查询不启动 CLI、不探测安装或认证、不改变工具列表，不承载模型推荐。模型选择由 Manager 按用户要求与 `list_members` 返回的模式组员及说明决定。

用户组员配置由 App 维护，位于数据根（`AGENTNAVE_DATA_DIR`，空值或未设置时为 `~/.agentnave`）的 `members.json`，版本为 1，顶层 `version`、`clis`、`modes` 三项必填，可选 `defaults`。`clis.<provider>.models.<model>` 保存共享的 `efforts`、`effort`、`options`、`notes`；`options` 不得含 `model` 或 `effort`。`modes.task` 与 `modes.discussion` 分别配置允许的 CLI、模型列表、默认模型与按模型的 overrides。模式键缺失表示不限制；存在的空对象表示禁止全部 CLI；CLI 的空模型列表表示不限模型。`defaults.<mode>.<provider>` 只为不受限模式保存默认模型，同一模式不得同时出现在 `modes` 中，模型须已在共享配置中（[ADR-0021](../adr/0021-keep-default-models-when-unrestricted.md)）。模型列表、默认模型与 override 必须引用共享模型；默认模型必须在非空允许列表中。override 按出现字段整体替换共享配置，包含 `options` 整体替换；生效的默认 effort 必须在非空 `efforts` 中。未知字段、Provider、模式与结构错误均拒绝。

服务在每次 `start_agent` 或新开节目的 `run_round` 时现读配置，不缓存、不写入、不创建目录。先检查模式允许的 CLI，再对 task 检查宿主排除集合，然后按默认模型（不受限模式取 `defaults`）、默认 effort、其他 options 的顺序补齐未传字段，最后保留 Adapter 原有校验；显式传入的 model／effort 超出范围时拒绝；模式模型列表非空、未设 default 且调用方未传 model 时，也拒绝并列出允许模型，禁止退回原生模型绕过白名单。`session_id` 非空的续接同样现读配置并套用默认值。组席位在创建时冻结套用后的参数，后续轮次沿用，不自动更换模型。文件缺失时保留原生行为；读取或格式错误用专用异常保留 App 修复原文，直接返回 Tool error，不引导修改调用参数；被排除错误保留完整允许／排除清单。只有 Adapter 选项校验异常才追加支持参数提示，不根据错误文本猜测。只读 `list_members` 替代 `list_profiles`，分别列出两个模式的生效配置、默认标志、`permitted` 与 Adapter `option_args` 的 `valid`／`error`；不准备命令、不探测安装、认证或模型可用性。无配置时两个模式不受限且名单为空；受限模式中 CLI 的空模型列表除枚举已知组员外，另列 `model="*"`、default=false、effort=null、efforts=[]、options={}、notes=“不限模型”的通配条目。见 [ADR-0013](../adr/0013-members-defaults-and-host-identity.md)。

宿主在服务启动时识别：显式 `AGENTNAVE_HOST` 优先，未知值使启动失败；否则 macOS／Linux 通过只读进程信息接口，沿祖先进程最多检查 32 层，匹配可执行文件名、应用包路径或原生 Claude 的 `claude/versions/<版本号>` 路径；已知解释器／包装层（node、bun、deno、python／python3／python3.x、uv、uvx、npx、env、sh、bash、zsh、fish、dash）还按第一个非选项参数识别脚本，去掉 .js／.mjs／.cjs／.py 后缀后匹配。AgentNave 自身的 Python 中继脚本跳过；某层 argv 不可读时继续读父进程，父进程不可读或遇到未匹配宿主的非包装进程则停止，避免误认更外层 GUI。Windows 只读取环境变量。无法识别则 `host=null`、`host_source=unknown`；来源另可为 `env` 或 `parent`。WorkBuddy 映射 codebuddy，opencode／hermes 无对应 Provider，其余同名。排除集合为宿主 Provider 与 `AGENTNAVE_EXCLUDED_PROVIDERS` 的并集；旧环境变量的校验与行为保留。MCP instructions 按此并集公开允许项与排除项，只约束单任务 task，不作用于 discussion 的协作组；组员配置的 discussion 限制仍生效。允许项不代表已安装或认证。与宿主同类的 CLI 进入会议只为方便参与讨论；需要同类 CLI 执行任务时，boss 优先使用宿主自带的子代理能力，而不是通过 AgentNave 私聊，App 对这类工位标为「仅会议」。

Codex 在非 Git 目录运行时，调用方可显式传入布尔选项 `skip_git_repo_check`；Adapter 默认不绕过 Provider 的仓库检查。Codex 的 `sandbox`（`read-only`／`workspace-write`／`danger-full-access`）以 `--config sandbox_mode=…` 传入，因为 `exec resume` 不接受 `--sandbox`；它是权限类参数。布尔 `approve_for_me` 新任务传 `--approve-for-me`，续接改传它等效的 `approval_policy`、`approvals_reviewer`、`sandbox_mode` 三项 `--config`，同为权限类参数。

Adapter 每次准备命令时解析 Provider 可执行文件，加装或卸载 CLI 下次调用即生效：先查当前 PATH，再依次查 `~/.local/bin`、`~/.grok/bin`、`/opt/homebrew/bin`、`/usr/local/bin`、`~/.bun/bin`、`~/.npm-global/bin`、`~/.volta/bin`、`~/.cargo/bin`，argv[0] 使用解析出的绝对路径；都找不到时保留裸命令名，启动失败仍以 `launch_error` 报告未找到。macOS 的 Codex 在此之前优先选择桌面应用附带的可执行 CLI：先 ChatGPT，再 Codex，每种应用先查系统 Applications，再查用户 Applications。启动与续接使用同一选择规则；AgentNave 不比较版本、不负责安装升级，也不在选中的 CLI 启动失败后改用另一版本。Windows 保持由 supervisor 在 PATH 中解析裸命令名。

macOS／Linux 启动 Provider 时，只在传给 supervisor 与 Provider 的环境中补齐 PATH：依次为可执行文件所在目录、继承的 PATH、上述存在的常见目录（去重），不覆盖宿主选择的工具链，使 `#!/usr/bin/env node` 类 CLI 在宿主给的精简 PATH 下也能运行；服务自身环境不变。

内部归一化结果字段为 `status`、`provider`、`output`、`session_id`、`provider_usage`、`duration_ms` 和 `error`；`provider_usage` 只保留 Provider 可用的 `num_turns` 与 `total_cost_usd`，不转发 token、cache 或 model 明细。Provider 正常返回业务失败仍是完整的 Invocation Result；取消、失败或超限而没有最终回答时，`output` 保留运行中最后的公开回复尾部（最多 1000 字符），不代表完成。CLI 报告成功的运行保持 `succeeded`：Claude／CodeBuddy 途中被拒的工具调用只在运行中作为阻塞提醒，不改写成功终态。Provider 缺失、无法启动或平台不受支持也会形成带 `launch_error` 的结构化失败结果，以便 Manager 读取。

STDIO MCP 是任务与协作的唯一控制接口，工具为 `start_agent`、`wait`、`cancel`、`list`、`list_members`、`run_round`、`decide_answers` 与 `read_conversation`；`agentnave-mcp` 只负责为 MCP Host 启动 server 进程。`wait` 与 `cancel` 同时接受任务 `invocation_id` 与轮次 `round_id`。中继空闲探测使用的 `list_invocations` 仍可调用，但不出现在工具列表中。全部 Tool 都发布输入与输出 JSON Schema，输入 Schema 声明 `additionalProperties: false`，未列出的参数整批拒绝、不静默丢弃；结构化结果另以去掉空值的紧凑 JSON 文本返回，不重复缩进与 null 字段；可由 Manager 修正的请求错误使用 MCP Tool error 返回重试指引，Provider 执行终态使用结构化 Invocation Result。继续 Provider 对话通过新的 `start_agent` 并显式传 `session_id` 完成。

启动回复另含 provider、model、effort 与 `defaults_applied`，不重复 prompt。model 与 effort 回显套用默认值后的请求值，仍省略时显示“继承原生设置”；`defaults_applied` 列出实际补齐的键名，无则空列表。新开节目的 `run_round` 每席另返回 model、effort（未传且无默认时省略）与 `defaults_applied`。这些字段不证明 Provider 实际生效的设置；等待与取消回复不重复启动参数。

AgentNave 不提供总运行时限；`start_agent` 不接受截止时间参数，也不根据运行时长终止调用。`wait` 的单次等待到期只返回运行状态，Manager 可继续等待或使用 `cancel` 显式停止任务。Provider 原生限制继续生效，AgentNave 不静默改写。

启动、等待和取消的公开回复共用顶层 `invocation_id`、`status`、`reason`、`elapsed_ms`；按需包含 `activity`、`error`、`output`、`output_age_ms` 与 `session_id`，不返回费用或空字段。内部 Invocation Result 的 Provider 用量仍可保全，但不向公开回复转发。单次等待默认且最长五分钟，`wait_seconds` 可缩短；完成或已识别的明确执行阻塞提前返回；阻塞返回不终止 CLI，由 Manager 决定继续等待还是取消，同类阻塞每次 Invocation 仅唤醒一次。普通工具失败、暂时重试和事件沉默本身不构成阻塞。未识别的原生错误不保证提前唤醒。

`wait(ids, wait_seconds?)` 在同一窗口（默认且最长五分钟，`0` 即刻返回）内等待任意组合的任务与轮次；名单须非空、不重复且全部有效，否则在等待前整批返回 Tool error。任一项已终态、有未读阻塞或轮次待审即返回 `reason=ready`，`updates` 为此刻全部就绪的任务，`rounds` 为就绪的轮次，未就绪项继续运行且不出现在回复中；每个就绪项只消费进入回复的那条阻塞，其余未读阻塞留给后续等待。终态可重复读取，已终态 ID 每次都就绪，由 Manager 从下次名单移除。窗口到期返回 `reason=wait_elapsed` 与每项运行快照，不消费阻塞。省略 `ids` 时等待收件箱：本服务由 `start_agent` 启动、终态尚未被任何 `wait` 或 `cancel` 返回的任务，以及终态尚未送达的轮次，按开始时间排列；给出 `manager_session_id`（或注入的 `AGENTNAVE_HOST_SESSION_ID`）时只含该 boss 的会话。每个终态只送达一次，待审轮次在决定前持续就绪；没有待等项时立即返回 `reason=nothing_pending`。收件箱只在进程内存中，不跨服务重启；七天内结束而终态未送达的轮次会让 `restart_safe` 为 false，中继据此推迟换进程；只用 `read_conversation` 回读而不 `wait` 的轮次也会推迟升级，直到七天期满。聚合由 InvocationManager 在唤醒后不挂起地扫描完成；同一 Invocation 的多个等待者（包括轮次对其席位的等待）仍共享一份阻塞队列、先取先得，不广播。取舍见 [ADR-0007](../adr/0007-wait-any-ready-subset.md)。

`list` 列出本进程的 Invocation 句柄与已保存会话（按最近活动倒序，可按 kind 过滤、限条数并给出 `total`），并启动看板返回入口；其中句柄部分与隐藏的 `list_invocations` 相同。`list_invocations` 只读列出当前进程内的活跃 Invocation 与保留期内的终态 Invocation，按启动顺序返回 `invocation_id`、`provider`、`cwd`、`status`、`elapsed_ms`（终态固定）和 `events_seen`（是否收到过任一结构化事件）；活跃项另含 `phase`，终态另含 `result_read`（等待或取消是否已返回过最终结果）与非空原生 `session_id`。不返回 prompt、正文、工具参数或启动时间，不消费阻塞，也不延长保留期。顶层 `restart_safe` 表示会话是否允许服务重启：本进程仍持有会话锁、存在未落盘回写，或七天内结束的轮次尚未由等待或取消返回终态时为 false；中继还须确认没有运行中或未读结果的 Invocation。探测缺少该字段时不自动换进程。

运行中回复同时提供最新原生活动与最多 1000 字符的公开回复尾部及其年龄。正文独立保留，不因后续工具事件消失；相同正文可能在不同等待中重复，无游标、分页或独立读取工具。只增加有界进程内尾部，不长期保存运行中的输出尾部。活动不返回原始事件名、工具调用 ID、工具参数/结果或推理正文；公开正文可能含任务数据，不提供自动脱敏保证。终态 `output` 保全最终回答，不应用中间正文长度限制；每次调用的保留 stdout 上限为 1 MiB（含必要协议数据），stderr 为 64 KiB，按字节计；采用事件过滤的 Provider 另沿用每条 8 MiB 的解析缓冲上限，工具／图片事件过滤后不累积保存，不占回复预算。超限返回 `output_limit_exceeded` 并终止进程树，不静默截断为成功。回复软预算由 Manager 在派发任务时安排，不能由容量推断模型上下文是否足够；长产物由有文件工具的 CLI 显式落盘并返回摘要与路径，服务不自动转存或传输附件。宿主超时限制由 Manager 尊重，无等待请求时不主动推送。

Invocation 状态只存在于当前进程内。macOS／Linux 的每次 Invocation 由一个专用 supervisor 持续占有 POSIX 进程组，Provider 正常终止后也先清理该组再回收 supervisor，避免旧 PGID 被复用。Windows 的专用 supervisor 创建 kill-on-close Job Object，将 Provider 挂起创建、加入 Job 后再恢复；Provider 结束时关闭 Job 以清理后代，取消或 supervisor 丢失时也由句柄关闭终止整棵 Job 进程树。MCP server 退出时会尽力终止仍活跃的 supervisor；POSIX supervisor 另每秒检查父进程，服务被强杀而父进程改变时自行终止整个进程组，不留无人回收的 Provider；重启后旧 Invocation 句柄不可恢复。Provider 自己持久化的 Session 不受此限制。

AgentNave 不是沙箱或同用户恶意进程隔离边界。POSIX 上已获得命令执行权限的 Provider 或工具可以主动创建新 OS session、杀死 supervisor，或以其他方式脱离普通进程组；发生可检测的 supervisor 丢失时返回 `supervision_lost`，但不能安全地对可能已被复用的旧 PGID 继续发信号。Windows Job Object 提供进程树所有权，但不隔离同一用户账户下的其他资源；Job 分配不被当前宿主环境允许时以 `launch_error` 失败，不降级为无树级所有权的启动。是否允许 Provider 命令由其原生权限机制和 Manager 决定；需要抵抗恶意同用户进程时，应在 AgentNave 外使用降权、容器或平台级资源域。

终态 Invocation 的内存记录在调用结束七天后自动清理，包括尚未读取的结果；重复读取不延长保留期，活跃调用不清理。Manager 应在过期前保留最终回答与原生 session_id，过期句柄的等待与取消返回 Tool error。清理不删除 Provider 原生会话、协作组已接收的回答或会话历史。

## Provider Adapter

Adapter 只能添加非交互输出、prompt 传输和 cwd 等协议必需参数；Grok 只在最终事件里报告原生 session，新对话因此由 Adapter 以 `--session-id` 预先指定一个 UUID（续接不指定），使取消或提前失败的调用仍返回可续接的 `session_id`，不改变模型、权限、工具或 prompt。DeepSeek 的模型覆盖必须重述路由，Adapter 按组合后的 headless 配置为模型补上路由：默认模型沿用其路由，配置的 provider 声明的模型用该 provider，其余为官方路由 `deepseek-official`；官方路由且调用方未显式给出 `stored_credentials` 时恢复 DeepSeek Harness App 已存的认证（[ADR-0027](../adr/0027-deepseek-official-route-stored-credentials.md)）。Provider stderr 仅在失败结果中以有界详情返回；任务原文只保存于会话历史，不写入服务日志（[ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md)）。AgentNave 自身的中间文件使用系统临时目录：目前仅 Grok prompt 使用标准库临时文件，调用结束清理；其余 prompt 走 stdin，运行中尾部留在内存。Manager 创建的任务交接/中间文件同样使用任务临时目录并管理读取生命周期，不强制回答正文格式。项目正式产物与 Provider 自己的会话、缓存不属于此临时文件所有权。绝对 `cwd` 传给 Provider 进程，项目规则加载仍由 Provider 原生实现负责，不能把临时文件目录替代为 cwd。

AgentNave 在 macOS／Linux 使用 POSIX 进程组，在 Windows 使用 Job Object。Windows Provider 必须挂起创建、成功加入配置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job 后才恢复；任何创建、配置或分配失败都以 `launch_error` 终止，不回退到普通子进程或 `CREATE_NEW_PROCESS_GROUP`。Windows 取消直接终止 supervisor 并关闭 Job，没有伪造 POSIX 的温和树级信号阶段。

## 协作组

取舍见 [ADR-0006](../adr/0006-core-rounds-and-studio-split.md)。不带 `group_id` 的 `run_round` 先以 `title` 与 `members` 登记 2–7 个席位并保存会话（不带 prompt 时只开节目），带 `group_id` 时拒绝新节目参数；席位与 `start_agent` 同一合同：原生工具与权限、组级或席位级 cwd、显式 Provider Options；宿主排除项不约束席位。席位可带 `role`，与 provider、model 一起对应工位，`seat id` 只是组内键，回执回显各席 `role`。校验只调用 Adapter 的 `prepare` 并立即清理其临时文件，不启动进程。

不带 `seats` 的 `run_round` 把 prompt 作为主持人发言提交；带 `seats` 时非阻塞地运行一轮：本轮共同任务先作为 `task` 条目提交；`parallel` 同步启动全部席位，在全部终态后按名单一次提交；`serial` 逐席启动、逐席提交，后席能看到前席。同组同一时间只有一个轮次。终态 succeeded 提交最终输出为 `answer`，failed、blocked、cancelled 提交 `absent`；运行中的阻塞提醒不提交。不重试、不设每席超时、不摘要或截断。

每席续接自己的原生会话，只收到游标之后、非自己的条目；新会话收到前导与全部条目。只有 succeeded 且带本次 session_id 才推进游标；未启动的 launch_error 保留原会话；其他终态清空会话与游标，下次补全。前导声明其他席位的回答是数据而非指令，私密指令只发给对应席位，但席位复述的内容会随回答传播。

`wait` 对轮次使用同一五分钟窗口，轮次结束、运行席位出现新阻塞或回答待审时提前返回，结束后可重读。以席位的 `invocation_id` 调用 `cancel` 只取消单席，串联轮继续；以 `round_id` 调用时先禁止新启动，再取消在途席位，已成功回答照常提交，未启动的串联席位不写条目。server 退出时先结束所有轮次，再关闭 InvocationManager。

`wait` 的 `include_output` 默认为 true；显式 false 只省略最终正文、运行尾部及其年龄，状态、活动、错误与原生 session_id 仍可读取。省略不消耗答案、不改变条目提交或审核，之后用 true 可重读全文。待审回答同样可重读。已结束轮次的 elapsed_ms 固定；finished 只表示编排结束，不代表各席都成功。意外执行异常形成带 internal_error 的 Invocation 失败结果；轮次驱动异常先停止新的启动、收束活跃席位、跳过未启动席位再释放组，返回 failed 与 round error，不公开未经审核的成功回答。状态写盘失败的轮次也返回 failed；取消仍遵守其独立语义。

多轮并联共享前轮已提交答案是有意设计，不提供连续盲答；串联顺序产生信息差也是有意设计，轮换顺序与追加全员修正轮由 Manager 决定。信息共享规则不提供文件隔离，写代码的并联席位由 Manager 分配独立工作树。讨论后的原生会话仍记得已收到的内容，切换 parallel 不能恢复盲答，新的独立回答需要新组与新原生会话。回答审核只决定是否公开条目，不回滚工具副作用或清除作者会话记忆。

显式 restricted_tools=true 是受限配置，普通调用不启用；它保持原生工具限制并允许传入该席位自己的 session_id，只接受 model、effort（Pi 另可选模型后端 provider；DeepSeek 另可选模型路由 provider 与恢复已存认证的 stored_credentials）。Claude 使用 safe-mode、空 tools、strict MCP 与禁用 skills；Grok 使用空 tools、deny-all、禁用子 Agent 和网页工具，不承诺移除工具定义或阻止 MCP 连接；CodeBuddy 复用 Claude 协议但覆盖自身限制参数；Codex 限制本地读取工具并忽略用户配置；Pi 禁用工具、扩展与 MCP；DeepSeek 经临时 `--patch` 覆盖层停用发行 profile 的工具插件与计划模式并改为只读沙箱，不覆盖后续版本新增的插件。Antigravity 只能从 cwd 加载角色定义，Adapter 因此不写调用方目录：受限模式要求 cwd 已含一致的定义，由调用方（如隔离席位）事先写入。受限配置不等于操作系统沙箱。

## 会话历史与审核

`Conversations` 在统一核心中管理任务与协作历史；`Groups` 是轮次执行的内部组件，不是另一个 MCP 服务。`_on_change` 在席位启动前、终态后、提交与轮次结束时原子落盘；`_approve` 是提交前审核屏障；`_ready` 使待审时返回 `review_needed`。取消与退出唤醒屏障，未决回答按不公开处理。用法见[会话、审核与看板](../studio.md)。

数据根默认 `~/.agentnave`（`AGENTNAVE_DATA_DIR` 可覆盖），首次保存会话时才创建；托盘首次保存组员配置也可创建数据根。会话保存在数据根的 `agentnave.db`（SQLite，[ADR-0026](../adr/0026-sqlite-conversation-store.md)），只写与上次保存相比变化的行；每个会话一把从不删除的 `locks/<会话ID>.lock` 文件锁：每次写入前取锁并在锁内按 `rev` 重读、校验数据库状态；运行中的轮次／任务、待审回答及未成功落盘的回写持续持锁，全部回写完成且会话空闲后释放，同时清除组、轮次与审核的写入缓存。已完成轮次的进程内回复仅供重读，不再参与写入。其他服务在持锁期间只读并以 writable=false 列出，空闲后可取得同一把锁继续写入，不另建会话拆散历史；若锁内发现会话已删除，则遗忘该会话并拒绝写入，不从缓存重建。写盘失败时内存回滚到上次保存的状态；轮次中写盘失败则不再启动席位并在轮次上报告 `state_write_failed`。

节目即保存的 Group，会话 ID 就是 `group_id`；重启后首次写入时按保存的条目、席位会话与游标恢复。保存席位、条目（含时间；回答与缺席条目取该席完成时间，而非并联轮统一提交的时间，条目顺序仍按名单）、各席会话与游标、每轮名单／顺序／私密指令／各席状态、最终输出与完成时间（旧记录没有）、审核状态；不保存工具事件或推理；运行尾部只在运行期间写入数据根的运行文件（见下文“托盘数据”），结束即删除。重启时运行中的席位标为 interrupted 并清空其会话与游标，轮次标为 interrupted，未启动的串联席位标为 not_started，不续跑；待审回答保留，此后 `decide_answers` 直接写入条目，未决期间拒绝新轮次。

审核：并联轮全员终态后成功回答一起待审，全部决定后按名单提交；串联轮逐席待审，决定后才启动下一席。`decide_answers` 整批校验后保存再生效；同一决定幂等，改变决定被拒。回执返回轮次 `status` 和可选 `error`；全轮已无待执行／运行席位及待审回答时，等待本地提交收尾再返回终态，`finished` 可直接进入下一轮。部分审核和仍有串联席位时保持非阻塞，由 `wait` 继续观察。决定只有 `publish` 与 `private`。不公开的回答不写条目，其他席位看不到，席位自己的原生会话仍记得它；该席下一次续接时，若它上一轮成功回答未公开，prompt 会附一句说明，避免它误以为别人已看到。私有视图另投影各轮已保存的不公开正文，App 以 boss 私聊回复「🔒 未公开」展示；公开视图与席位输入仍只使用共享条目。旧记录缺少回答时间时按该轮任务时间定位，不推断实际完成时间。主持人发言只能在无进行中轮次、无待审回答时追加为 `host` 条目。

`read_conversation` 按 App 的方式返回最终消息时间线，而非原始状态：任务给出每次 prompt、最终回复、状态与原生 session_id；节目给出席位、最新一轮与按序的 `task`、`answer`、`absent`、`forward`、`host` 及待审的 `pending_answer`，可用 `since_round` 只读后续轮次。`limit`（1–500）只保留最近若干条消息或任务，被略去的条数记在 `omitted`。`include_private=true` 另列私密指令与不公开回答，结果只给 Manager。投影来自 `views.py`，空字段省略。

隔离节目在会话目录下为每席创建独立目录，为 Antigravity 写入受限角色定义，给每席加 `restricted_tools`，忽略调用方 cwd。节目（隔离与否）都可包含 `AGENTNAVE_EXCLUDED_PROVIDERS` 中的 Provider；`provider_details.show_permitted` 现读 `members.json`，按 discussion 是否允许该 Provider 计算；文件缺失或 discussion 未受限时为 true，损坏时返回 App 修复错误。隔离节目只套用 model／effort 默认值（Pi 另可套用 provider），不套用其他默认 options，也不在回执列出被跳过的键；调用方显式非法选项仍拒绝。

新任务与节目保存创建者的 `host`、`host_source` 与 boss 聊天身份 `manager_session`（[ADR-0017](../adr/0017-manager-chat-session-identity.md)）。`start_agent`、`run_round` 接受 `manager_session_id`：同一 boss 聊天始终传同一宿主 session ID，不同聊天分开；宿主未公开原生 ID 时，boss 在该聊天首次调用前生成一次 UUID 并沿用。省略时只读取明确按聊天注入的 `AGENTNAVE_HOST_SESSION_ID`，仍未知则拒绝调用；旧记录的 null 归属照常读取。服务把宿主与原始 ID 的 SHA-256 前 32 位 hex 加 `s-` 前缀作为分组键，不保存原始 ID；CLI 名称和 MCP 连接 UUID 均不充当聊天身份。视图快照与 live 会话摘要保留创建归属；live 顶层的 `connection_id` 仅标识当前服务连接，`manager_started_at` 表示服务开始时间。后续接手不改写创建归属；已知 boss 身份时，任务原生续接只自动关联本 boss 工作区的记录，显式指向另一工作区的任务记录会拒绝。工作区交接（[ADR-0032](../adr/0032-boss-workspace-handover.md)）不改写会话行：`workspaces` 表（`CREATE TABLE IF NOT EXISTS`，`user_version` 仍为 1）记录每个工作区现在的写入 boss 键 `owner`、待接手标记、交接说明与 boss 会话文件路径；服务先把调用方的 boss 键解析为它拥有的工作区，再按上述规则比较；它原来的工作区已被他人认领、自己又没有新工作区时，`start_agent`、`run_round` 与 `handover` 拒绝。`handover` 不带参数列出待接手的工作区，`release=true` 交出本聊天的工作区，`workspace_id` 认领：要求待接手且认领方名下没有其他有会话的工作区，回执给出交接说明、会话列表（`running`、`writable`）与仍存在的老 boss 会话文件路径。交接不等待、不转移运行中的调用：它们在原服务里结束并保存，会话锁释放后新 boss 才能续写。旧记录仍可读取；节目续轮与主持人发言按实际身份（参数或注入）匹配创建归属，无归属旧节目不能续轮，私聊可用原生 `session_id` 续接并新建当前 boss 的记录；历史内容不迁移。

任务会话保存普通 `start_agent` 的 Provider、cwd、套用默认值后的模型、状态、最终输出与原生会话：先由 Adapter 准备并校验请求，再写入记录并启动同一 PreparedCommand；同一 boss 会话内的同一原生会话并入其任务会话；同一归属内存在多个相同 Provider／原生 session 的任务会话时，未显式指定会话的续接选择最近一次绑定该 session 的记录，保留其创建者归属。并发续接同一会话被拒。任务续接先取得 `native-session-locks/<Provider与原生session的SHA-256>.lock`，在启动锁内刷新全部 boss 的活跃绑定并落盘运行记录，再释放启动锁；运行记录继续持有会话锁。锁文件保留以避免 inode 替换竞态；`AGENTNAVE_HOST_SESSION_ID` 从子 CLI 环境移除。后台采集器在终态保存结果，写盘失败时进程内标记中断，`wait` 仍能读取结果。会话没有结束状态，MCP 不提供改名与删除；标题由 Manager 创建时给出，改名与删除只在托盘 App 中进行。数据根中的旧版记录（`catalog/`、`room.json`）只列为不可用，不迁移。

看板只在 Manager 调用 `list` 时启动并返回首页入口，此后 `read_conversation` 附带该会话的页面入口，随 server 退出；启动时为全部可读会话构建投影，之后由落盘与 1.5 秒的运行刷新更新。只监听 127.0.0.1 随机端口，以随机路径及精确 Host/Origin 检查限制访问，不提供写入或文件映射接口；CSP 禁止外部加载与内联脚本，页面用 textContent 渲染。节目公开页只投影已提交条目的白名单字段，导演页另含私密指令、待审回答与运行尾部，任务页只给 Manager；页面不显示原生会话或调用 ID。用于本机查看与录屏，不作网络托管。

托盘数据（[ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md)）：持锁写入会话时，在同一事务内把视图快照写入 `views` 表（版本 1：含最近活动时间的摘要、导演视图、公开视图，链接指向浏览器看板路由 `/`、`/c/<id>/`、`/p/<id>/`；内容变化才重写并递增该行 `rev`），投影只来自 `views.py`；未被持有的会话在扫描时于探测锁下补写缺失或落后的快照，单独写视图失败不回滚状态。有工作运行时，每个服务每 1.5 秒检查一次 `live/<pid>-<随机>.json`（版本 2：进程号、宿主、连接标识与各运行中会话的增量，即运行中任务与最新一轮席位的回复尾部、回报模型、当前动作 `doing`（`thinking`、`tool:<工具名>`、`writing`、`retrying` 或 null，只含工具名）与回复最后增长的时刻 `output_at`，不含已在视图快照中的内容；两者都只随工作本身变化，不让内容每次刷新）：内容变化时原子重写，未变化时只刷新修改时间作为存活信号；无运行工作或退出时删除，并在写入前清理一小时以上的残留。托盘应用以只读连接读取 `views` 表与 live 文件，不调用工具；数据根的写入限于下述会话改名、清理与组员配置 `members.json`；浏览器看板保留为无托盘环境的后备。托盘应用位于 `apps/tray`（Tauri 2），经 `agentnave` 自定义协议在原生 webview 中只提供托盘专属界面（`apps/tray/ui`）：480×640 面板（每个 boss 一行，连到它的办公室缩略版）、办公室视图默认页与设置，各页共用同一组主导航；办公室视图由工作区列表、办公室（boss 办公室与待办、工位与会议室总览）、可展开的消息列表、工位历史组成，旧群聊页与「任」「节」分段已移除；不再内嵌浏览器看板，`/` 跳转到默认页。macOS 上是带程序坞图标的普通 App，菜单栏图标保留，忽略超过约 10 秒未更新的运行文件；不开放 IPC 命令、不监听端口，导航限于自身协议。应用内更新只访问本仓库 GitHub Release，用户确认后才安装；本机服务较旧时，用户点击“一键升级”后托盘才执行 uv 安装命令（[ADR-0010](../adr/0010-launcher-relay-swaps-upgraded-server.md)）。更新经同一个弹窗一次完成（[ADR-0014](../adr/0014-tray-members-and-one-click-update.md) 修订）：先重新检查，并列出当前版本之后各版的更新说明（来自 `latest.json` 的 `notes`），用户确认后依次下载 App、在服务确由 uv 安装且较旧时升级到目标版本、安装 App 并重启；服务升级失败就不装 App，进度见 `update.step`。重启后在 App 自己的数据目录（不是数据根）读出一次性的“已更新到”记录并删除；旧版 App 写下的“重启后升级服务”标记仍会被处理。只有服务较旧时直接升级；App 安装与服务升级在同一把锁下判断并占用、互不重叠，检查、安装或升级进行中时不重复执行；更新检查 30 秒超时。`GET /app/status` 返回 App 版本、更新状态（`checking|available|none|installing|failed`，有新版时带 `notes`，安装中带 `step`）、刚更新到的版本 `updated`、服务状态（`outdated|upgrading|done|failed`）与最近一次成功检查时间 `last_checked`；`POST /app/update/check` 立即检查并返回结果（带 `quiet=1` 时与自动检查一样，失败不报错），`POST /app/update/apply` 执行更新，二者与其他写操作一样要求 `X-AgentNave-Action: 1`。面板原有的导航动作保持可用。

托盘提供只读 `GET /app/stage`、`GET /app/chats`、`GET /app/chats/<group>` 与 `GET /app/chats/<group>/focus/<会话ID>` 数据路由，面板与办公室视图每 1.5 秒轮询，页面隐藏时暂停：按创建者带 `s-` 前缀的 boss 聊天 `manager_session` 聚合为工作区，旧版连接 UUID 或未知归属的记录各自归入 `c-<会话ID>`，工位即原生线程（[ADR-0025](../adr/0025-session-desks-and-running-limit.md)）：有原生 session 的私聊任务以 `thread:<线程键>` 为键并合并其续接，尚无 session 的任务以 `task:<会话ID>:<任务ID>` 为键，会议席位以 `seat:<会话ID>:<席位ID>` 为键；线程工位以 `aliases` 列出其任务取得 session 前的键，前端据此迁移选中项；角色、模型与 `access` 取最近一次，同一「模型 · CLI · 角色」下有多个工位时按首次出现加 `#1`、`#2` 区分。`GET /app/chats/<group>` 在原有字段上新增 `project`（项目名）、`omitted`（时间线省略条数）、`threads`、`desks`（工位键与 `kind`（`task`／`seat`）、席位所在会议 `room`、`display_model`／`model_source`／`handle`、状态、私聊任务数与运行数 `private: {tasks, running}`、进行中活动列表 `activities` 与 `private_allowed`）、`optimized`（未画出的工位：`count` 与最近的 200 个工位，另并入本次返回的会议席位与消息引用的工位）、`rooms`（会议室，含最新一轮 `round` 与席位 `handle`）以及随消息的 `visibility`、`thread`、`desk`、`message_id`、`forward`、`round`、`to`（工位键或保留字 `@boss`／`@all`）、`answering`、`reply_to`、`kept_private`，私密指令以 `kind: "instruction"` 列出，消息可见性只区分私聊与会议公开；任务线程的 `thread` 是由 Provider 与原生 session 派生的不可逆键，视图不暴露原生 session 或调用 ID。`private_allowed` 为 false 表示该工位与宿主同类，只可进会议。每个工作区最多画出 N 个工位，N 为数据根 `settings.json` 中的同时运行上限（缺失或损坏时为 8）：运行中与待审的工位总是画出，其余名额按最近活动填满，其余进入「已优化」，只以文字列出；画出的工位按首次出现倒序排列（最新在前），编号仍按首次出现正序；舞台只给其数量。舞台最多返回 5 个 boss：先是有运行／待审的，再是最近失败的，再按最近更新补足，不按时间窗筛除，每个 boss 附带不含消息的 `desks` 与 `rooms` 供面板缩略版使用，`counts` 统计所列 boss 中运行的私聊任务与会议席位数（按调用计）和待审回答数，`empty` 区分 `no_data`、`quiet` 与有 boss 时的 null；群列表最多 200 个，普通群时间线最多 300 条，超出部分以 `omitted` 计数；定位会话时额外保留该群内目标会话的完整消息，不受其他会话的近期消息挤占。失败、受阻等终态的非空正文与状态提示同时保留。运行中的每条线程（工位与会话）各有一个稳定 ID 的 typing 条目并置于末尾，时间取 live 文件的 `updated_at`；失败状态只由最近一次结束的组员决定，旧失败不持续标红，取消不算失败。舞台 boss 按组 ID 字典序对哈希色冲突顺延，各条路由共用这份分配，不在舞台中的群使用哈希色。新鲜 live 叠加到同会话磁盘快照上：只给快照中仍为运行中的任务和席位换上尾部、回报模型、`doing` 与 `output_at`（运行中气泡在尾部超过一分钟未变时写明当前动作与上次输出距今多久），任务耗时按 live 文件修改时间推算，更新时间也取修改时间；没有磁盘快照的会话不显示增量。旧服务的版本 1 整份快照仍直接覆盖。视图按 `views.rev`、live 文件按修改时间与大小复用内存解析缓存；全部快照与 30 秒时间窗都未变时复用上次的路由响应，群列表预览只为快照有变化的群重新投影。手绘美术资源在编译期打包进应用，经只读 `GET /art/<路径>` 提供。这些路由只接受 GET，不写数据根，不调用工具或 CLI，沿用自定义协议、无 IPC、无监听端口与既有 CSP 边界；群不存在、目标会话不存在或不属于该群时返回 404。

托盘会话管理（[ADR-0012](../adr/0012-tray-conversation-cleanup.md)）位于办公室：工作区和会议室右键提供改名与删除，有 boss 聊天身份的工作区另有「交给其他 boss 接手」（只确认、无输入，`POST /app/workspaces/handover`，在 `workspaces` 表标记待接手，不取会话锁；ADR-0032），工作区列表与办公室标题显示「待接手」或「boss 已更换」，后者的 boss 宿主取认领方；工作区支持多选，清理对话按 7／30／90／180 天筛选。旧 `/manage/` 地址跳转到办公室。完整管理列表从数据库读取，不受办公室最多 200 个工作区的展示限制；缺少视图、无法归组或时间不明的记录在清理对话的“其他记录”中默认不选，用户可逐条选定并再次确认删除。

删除由 Rust 原生完成，不依赖 Python 安装：对 `locks/<id>.lock` 使用 `File::try_lock`，与服务共用同一把 OS 锁；POSIX 对应 `flock`，Windows 对应 `LockFileEx`，预期与 Python 的 `msvcrt.locking` 第 0 字节区间互斥，仍需 Windows 实机验证。取锁失败时最多重试约 1 秒以避开扫描探针，然后逐项返回 `in_use`。取得锁后在数据库中重读：任一轮席位 `review=pending` 拒绝为 `pending_review`；空闲记录可直接删除，仍标 running 的记录视为崩溃残留允许删除。删除会话行，级联清除其条目、轮次、任务与视图，保留 `.lock` 的 inode、席位目录及其他文件，不修改 live 文件或进程内调用句柄；其他宿主后续写入按缺失状态拒绝，不重建历史。

托盘会话清理路由为自定义协议的 `POST /app/conversations/delete`，要求 `X-AgentNave-Action: 1` 与 JSON `{"ids": [...]}`，整批先校验 ID，逐项返回 `deleted / in_use / pending_review / missing / error`。按天数自动清理另带毫秒时间 `updated_before`，每次提交一个工作区的确认 ID 集合；后端按固定顺序取得全组会话锁，再开始 SQLite immediate 事务，重新查询成员与最大活动时间（创建时间和视图更新时间取较新者）。新成员、近期活动、未知时间、占用或待审使整组保留，返回 `changed` 或对应原因；通过后在同一事务中删除整组。手动选定 ID 的删除保留逐项语义，允许缺失视图。GET 导航不能触发删除，不开放 Tauri IPC、不放宽 CSP、不新增 HTTP 服务。

会话改名使用同一把锁与同样的 Header 要求：`POST /app/conversations/rename` 接受 `{"id", "title"}`，标题去掉首尾空白后非空、不含控制字符，显示宽度最多 48 列；MCP 创建标题与 App 改名共用 Unicode 15.0 宽／全宽字符区间表，汉字等宽字符算 2 列，其他字符算 1 列。取锁后在一个事务内改写会话标题并递增 `rev`，同步改写摘要、导演视图与公开视图的标题，返回 `renamed`，持锁中返回 `in_use`。运行中的服务下次在锁内重读时采用新标题。MCP 不提供改名工具。

编排偏好入口（[ADR-0024](../adr/0024-consolidated-tools-and-private-review.md)）在设置页展示推荐模板，可复制；`GET /app/rules` 列出各宿主常见的用户级全局规则文件位置与是否存在，`POST /app/rules/open` 用系统默认文本编辑器打开已存在的文件。App 不创建、不写入这些文件，位置清单只是常见位置，以宿主文档为准。

托盘是 `settings.json` 的唯一写入方：`GET /app/settings` 返回文档或默认值、是否已保存与错误原因，不写文件；`POST /app/settings` 要求 `X-AgentNave-Action: 1`，要求 `version` 为整数 1、`max_running` 为 1–32 的整数（缺失取 8），忽略未知键、只写入这两个键，整份校验后以同目录临时文件、sync 与 rename 原子写入；文件损坏时可直接保存合法值覆盖。服务端只读该文件，读取规则与托盘一致。

托盘设置使用独立 `/settings/` 页面，与办公室 `/chat/` 在主窗口互相可达，入口为托盘右键“设置…”。托盘是数据根下 `members.json`（版本 1，结构见服务端组员合同）、评审工位 `review.json` 与权限档位 `permissions.json` 的唯一写入方（[ADR-0014](../adr/0014-tray-members-and-one-click-update.md)）。`GET /settings/members` 返回文档、错误、评审工位、权限档位（未选择过为 null，即全部「自动」）与各 CLI 探测结果；`POST /app/permissions` 校验后原子写入 `permissions.json`，设置页每个 CLI 卡片在「自动」「完全放开」间点选即存；`members.json` 缺失时返回空文档且不创建该文件，损坏时返回空文档与原因，页面禁止保存。`POST /app/members` 要求 `X-AgentNave-Action: 1`，只做与服务端一致的结构与引用校验（顶层 `version`、`clis`、`modes` 以及各 CLI 与模式条目的 `models` 必填，`defaults` 只能用于不受限模式且引用已配置模型，未知键或字段、引用未配置的模型、默认模型不在允许列表、默认 effort 不在允许档位；覆盖的模型只需已配置；另拒绝空白模型 ID），不做 Provider 语义校验（由服务的 `list_members` 负责，服务无配置写入工具）；通过同目录唯一临时文件写入、sync 与 rename 原子替换，失败清理临时文件并保留原目标；不会触碰会话文件。整份文件保存，并发编辑最后写入者生效。设置页、头像设置与接入页只提供开关、单选、下拉与档位条，不提供手填输入；options、notes 与按模式覆盖不再在 App 中编辑，保存时原样保留，宿主配置根目录只能“恢复自动”，接口保持不变（[ADR-0029](../adr/0029-settings-without-free-text.md)）。

探测覆盖七个 Provider：可执行文件查找与 Python Adapter 相同；Codex 读取 `models_cache.json` 的可见模型及各模型的努力档位，Grok、Antigravity、Pi 运行各自的列举命令，CodeBuddy 解析 `--help` 中 `--model` 的支持列表，Claude 只给出别名，DeepSeek（dsh 无列举命令）读取组合后的 headless 配置，依次给出默认模型、发行版内置的官方模型 `deepseek-flash`、`deepseek-v4-pro` 与配置中各 provider 声明的模型；读取失败时只给出官方模型。命令以空 stdin、系统临时目录为 cwd、补齐 PATH（同上顺序）并行运行，各在独立进程组中运行、15 秒超时后整组终止，不发起模型调用；结果缓存在内存，`?refresh=1` 重新探测；设置页打开及回到前台（间隔 30 秒以上）时自动重新探测。保持自定义协议、IPC 与 CSP 的既有边界，共享浏览器看板不增加设置或写入接口。每个 CLI 另给出自身默认值 `native`：模型取 CLI 设置所写的模型（Codex `config.toml` 的 `model`、Claude `settings.json` 的 `model`、Pi 的 `defaultModel`、Antigravity CLI 的 `model` 标签、CodeBuddy `settings.json` 的 `model`、dsh 的 `agent-default-model`），没有时取列举中自标为默认的模型（Grok 的 `(default)`、CodeBuddy 支持列表的第一项）；档位取 Codex 的 `model_reasoning_effort`、Claude 的 `effortLevel`（先看该模型的 `modelSettings`）、Pi 的 `defaultThinkingLevel`、dsh 的 `reasoningEffort`，以及 Grok `models_cache.json` 为该模型标注的默认档位；读不到即未知，不猜测。App 不替用户写默认模型或档位，没有 `members.json` 时不写出首份配置；设置页逐项标明 boss 不指定时用的模型和档位是“你设的默认”还是“CLI 默认”；探测结果同时写到数据根 `cli-defaults.json`（内容变化才写），服务在调用未指定模型且无 App 默认模型时，按其中的模型套用该模型的档位与参数、不传 `model`；办公室里没有请求模型、尚未回报的工位按 CLI 默认模型显示，来源标为 `cli_default`（[ADR-0028](../adr/0028-follow-cli-default-model.md)）。关闭某场景的自选时，设置页把各 CLI 的默认模型移入 `defaults`，清除名单与本场景覆盖；重新开启时沿用它们，没有的继续跟随 CLI（[ADR-0021](../adr/0021-keep-default-models-when-unrestricted.md)）。

## 托盘宿主接入

设置各分区（执行者、接入、头像、编排）与办公室、会话并列在窗口顶部同一排导航，右上角在有更新时显示一键更新按钮；“接入”分区在同页展示版本操作与宿主列表，保留旧 `#updates` 链接。重启判断只比较实际用户级 MCP 接入条目与连接观测，不比较 App／服务版本或 Skill 的任何部分（Agent 使用时从磁盘读取 Skill，更新后无需重开）。App 偏好记录带 `v:4` 的内容观察、变更与手动确认；更早格式的记录（含 Skill 摘要）一律忽略。已知接入配置变化且有变化前建立的活跃连接时才提示重新加载；一百五十秒内都没有刷新的观测时为 idle，不提示（下次连接即在变化之后；切换服务时心跳最长暂停约 105 秒，不算断连，暂停中的连接也不让提醒自动消除）；旧中继缺少有效观测时为未知。卡片只显示需要重新加载的状态，同步、空闲、等待切换与未知都不显示说明。手动确认不能盖过仍使用旧接口的活跃连接。

中继只在宿主成功获取非空完整 `tools/list` 后，于有效数据根的 `connections/` 原子写入私有观测：宿主、连接起始时间、运行服务版本、运行工具定义摘要与已向宿主成功转发的工具定义摘要。每五秒刷新，连接关闭删除；中继首次写入时清除一小时以上未刷新的观测，这些通常是已退出中继的遗留；仍在运行但长时间暂停的中继若被误删，下次刷新会重建；不保存原始工具定义、提示词、环境或凭据。空或不完整的工具列表不创建连接观测。切换后内部探测新工具列表，仅版本变化且接口一致时不要求重启；接口不同且协商旧版有状态协议时，中继声明并发送工具变更通知。2026-07-28 及后续协议需要订阅，当前中继不合成订阅或向未订阅客户端主动通知。

App 只读有效数据根中新鲜、大小受限、非符号链接的观测，按宿主聚合全部可见连接。新接口已向所有可见连接提供且运行版本已是安装版本时为 current；接口一致但运行版本仍旧时等待空闲更新；安装超过十分钟仍未切换（一直有任务、结果未读或无法换新）时提示重连；中继从初始化、`server/discover` 及任何带 `serverInfo` 元数据的 2026-07-28 协议结果读取运行版本。观测仍缺运行版本的多为旧中继，App 只作启发式判断：连接起始时间可解析且早于本次安装、安装超过十分钟仍无版本时，视为未能切换（通常是 v0.31.0 之前的中继认不出该协议的宿主），同样提示重连；其余缺版本的连接显示未知；先判断滞留连接，再判断未知连接；接口不同时提示重连。旧中继连接不在观测范围内，需重连一次纳入检测，不能据此声称所有实际进程均已同步。全部可见连接都在变更之后建立时，视为宿主会话已重新加载（连接时间来自中继启动，属推断而非加载证据；只重连 MCP 的旧聊天仍是旧 Skill 简介），自动消除接入配置的变更提醒，无需手动确认。结果仅证明工具列表已成功转发，不证明模型上下文已采用。页面可见时只读轮询状态，编辑或确认操作中保留控件状态。

App 内置九个宿主 ID：`codex|claude|codebuddy|workbuddy|grok|antigravity|pi|opencode|hermes`。检测查可执行文件与已知应用包，保留的配置目录不单独证明仍安装。返回 `absent|available|connected|needs_attention|orphaned`，隐藏 absent；完好且命令可解析的已有条目即为 connected，用 has_host_env 表示是否已接管。命令按条目 env.PATH 解析，无法确定为 command_state=unknown，不自动替换。tested 按 App 接管及宿主真实调用验收结果标记，具体支持情况见安装文档；connected 不证明已加载工具、登录或模型可用。

根目录按 App 用户覆盖、GUI 环境、登录 shell、默认值解析，返回 root.path/source。hosts.json 保存宿主根目录覆盖、first_seen 及按宿主记录的重启确认。shell 使用固定白名单脚本、三秒超时，结果缓存至下一次刷新；失败不自动接管并提示 root_unconfirmed，显式操作仍可选默认或用户目录。MCP 配置读写前检查全部祖先符号链接（家目录本身除外）；遇到链接返回 symlink_path，不写。外部 Skill（包括符号链接）仅标 external，跳过其写入，不影响宿主状态或 MCP 操作；仅实际写入 App 管理的 Skill 时检查该路径。服务定位、安装、升级与版本检查共用缓存的有效 UV_TOOL_DIR / XDG_DATA_HOME；同一变量优先 GUI 环境、再取 shell 结果。命令链接目录按 UV_TOOL_BIN_DIR、XDG_BIN_HOME、XDG_DATA_HOME 的同级 bin、默认目录解析，并与 UV_TOOL_DIR 一起显式传给 uv。首次目录探测完成前不安装；重启后的自动升级仅等待三秒超时的 shell 目录探测，目录快照使用独立缓存，完整宿主刷新与接管在后台运行，不阻塞升级。

启动、POST /app/hosts/refresh 和服务安装成功后接管已有用户级条目。先解析全部用户层，复用备份、并发重读、原子替换及写后校验；确认安装元数据只声明 `agentnave-mcp = agentnave.relay:main` 且 launcher 可执行后，才修复已缺失的 `agentnave-mcp` 路径。服务尚未就绪时不改命令，自定义命令不自动替换；保留参数、环境、启停、工具过滤、未知字段与注释，补齐宿主标识。失败抑制签名包含宿主、各用户层路径及目标条目；无关字段变化不重试。项目层只提示、不改写。

九个宿主均由 App 定点编辑：JSON/JSONC 保留非目标区间，TOML 使用 toml_edit，Hermes 保留未知 YAML 块原文。读取字节和哈希，在内存编辑并同目录暂存；替换前重读比对，变化则基于最新内容重做，最多三次。替换前备份至 App 自有数据目录 backups/，宿主配置文件与 Skill 文件分开计数，各保留最近 30 份（拆分前的旧备份按配置计）。写后重新解析、校验非目标语义不变，仅当正式文件仍等于本次写入字节才恢复原快照，否则保留新内容并报 conflict。替换前比对与原子 rename 仍有极短竞态窗口，不声称跨应用完整事务。layers 返回各用户、覆盖与项目层；移除遍历全部用户层并逐个备份，项目层不写，多层独立替换不构成原子整体。

构建时嵌入完整同版本 agentnave-manager 与 agentnave-review。可选 Skill 在 App「技能」页列出说明与完整写法，按宿主开关安装，默认关闭，开关存于 `hosts.json` 的 `skills`；开启时已接入则立即安装，否则下次接入时安装，关闭只移除 App 副本（共用同一目录的宿主仍有一方开启时保留），移除宿主条目时两者一并移除。评审工位存于独立的 `review.json`，经 `POST /app/review` 保存，不进入 `members.json`。Skill 为 missing/app/external/manual，manual 的 path 为 null；只有版本标记和内容清单匹配的 App 副本才可更新或删除。接入及刷新更新旧版 App 副本；更新/删除前备份，替换前重查所有权。外部内容保持不变。Hermes 不能安全识别的 YAML 结构拒绝写入并提示手动处理。

GET /app/hosts、GET /app/onboarding 只读，不创建目录、不探测 shell。宿主操作 POST /app/hosts/connect|remove|root|confirm-reload 返回 {host, ok, message}，刷新返回完整列表。所有写操作要求 X-AgentNave-Action: 1，包括安装本机服务、打开备份目录、关闭和重开引导。POST /app/service/install 立即返回，复用 uv 安装并以 GET /app/status 的 service.state=installing|done|failed 展示进度，不发起 Provider 会话。引导按完整 launcher、缓存中的 connected 宿主、已保存的组员设置、带 manager_session 的保存会话计算；关闭标记位于 App 自有数据目录。MCP 服务端不导入宿主写入能力，不新增监听端口、IPC 或 Provider 调用。

### boss 模型与头像

`start_agent`、`run_round` 的可选 `manager_model` 保存调用方声明的本次 boss 模型；`manager_reported_model` 独立保存从精确原生 session 读取的模型，展示时优先使用后者。Codex 取对应 rollout 的 `turn_context.payload.model`，Claude 取对应 transcript 的主会话 `assistant.message.model`。只在调用时读取匹配 UUID 的会话文件，按时间选择最近记录，不读取全局模型默认值；无原生 ID、文件不可读或不支持的宿主保持未知。原始 boss session ID 仅保留在当前服务内存，磁盘只存归属哈希，以及为交接记录的该 boss 原生会话文件路径（按同样的宿主规则找到时，ADR-0032）；重启后由调用方沿用 `manager_session_id` 才能继续原生识别。每条消息独立保存，不把后来模型回填到旧消息。原生记录和调用方声明均不证明转发商实际后端，也不控制 Provider 参数。

组员的调用指定 `model` 与原生消息回报 `reported_model` 分别保存，展示优先回报；Claude／CodeBuddy 仅采纳主会话消息模型，Pi 优先采纳完整 assistant 消息的 `responseModel`，Codex 可按返回的原生 session ID 读取本次调用启动后新增的对应 rollout 轮次。Grok 只在结束事件的 `modelUsage` 恰好列出一个模型时采纳它；Antigravity 的输出不含模型，按返回的 conversation ID 读取本次调用期间的 agy 运行日志，日志里只出现一个所选模型标签（如「Gemini 3.8 Flash (High)」）时换算为模型 ID（`gemini-3.8-flash-high`）。不从初始化默认值、CLI 配置、多模型的累计 usage 或子代理消息推测主模型。未知时显示 CLI 名称并注明模型信息不可用，旧“原生设置”占位不作为模型名。boss 和组员消息按各自模型显示头像；boss 消息按本轮已启动席位显示收件头像，不显示 @ 或对象文字，悬停显示模型／CLI 与席位，点击按会话与席位定位消息。完整 JSON 与 fenced JSON 使用相同代码样式，所有代码块在气泡内换行。宿主 CLI 的列表、标题与接入页只展示文字身份及状态。模型头像固定为按家族选取的 LobeHub 官方图标，不可覆盖；浏览器看板的 `avatar-settings` 恒为空，不读取数据根。
