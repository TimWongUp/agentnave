# 架构与边界

AgentNave 是供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用的 MCP Server。它在 Manager 与 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Pi Coding Agent 之间提供很薄的进程边界，不是面向人的 CLI 产品、多智能体运行时或工作流引擎。

## 所有权

Manager 拥有目标解释、任务拆解、角色和模型选择、并行策略、重试、审核循环、结果综合、工作树和长期任务状态。

AgentNave 只拥有：

- 按需返回所选 Provider 的允许状态和支持参数；
- 根据冻结请求启动一个 Provider CLI；
- 传递 prompt、cwd、可选 `session_id` 和显式 Provider Options；
- 从 Provider 原生流事件提取有界活动快照，并归一化终端结果；
- 执行显式取消和进程树清理；
- 在 MCP server 生命周期内保存 Invocation 句柄；
- 在 Manager 显式创建的协作组内，按名单并联或串联运行轮次，只在进程内存中保存各席最终回答与原生会话游标。

核心包 `agentnave` 不建数据目录、不持久化业务数据、不监听端口、不向调用方 cwd 写文件。保存历史、审核、主持人发言与看板属于同版本的另一个发行包 `agentnave-studio`（见下文“工作室”），核心不依赖它。

AgentNave 不拥有 DAG、调度器、角色决策、自动重规划、工作树、保留策略或 TUI。托盘应用只读展示工作室数据，见 [ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md)。

## 安装与激活边界

运行时由 Python Tool Manager 安装，并在 MCP Host 注册稳定入口。配套 `agentnave-manager` Skill 承担 CLI 调用的主要触发、模型选择、传参、等待、取消与续接说明，每个 CLI 的模型与 effort 默认值放在独立参考文件中，选中后才读取。MCP instructions 与 Tool 描述只保留轻量调用合同，`describe_provider` 按需返回允许状态与支持参数；没有 Skill 时仍可显式调用 MCP。Skill 由宿主的技能安装机制或已有部署管理器单独安装与更新，MCP 与 Skill 都只提供 CLI 调用能力与用法，不制定调用方的任务规划、调度、审核、重试或综合策略。

Python Tool Manager 拥有运行时的安装、升级与卸载，并提供稳定的 `agentnave-mcp` launcher。源码 checkout 只用于贡献、调试和未发布版本验证，不作为 MCP Host 的长期启动路径。

MCP Host 拥有 AgentNave 的注册、作用域、启停、移除；能使用 Host 官方管理接口时，不由 AgentNave 直接修改其配置文件。安装运行时不会自动写入 Host Skills、全局规则或权限。Provider CLI 继续拥有自身的安装、认证、配置与 Session 数据，Invocation 句柄是进程内对象。工作室另在数据根保存会话历史；卸载不自动删除历史。

## 稳定合同

请求字段为 `provider`、`prompt`、绝对 `cwd`、可选 `session_id` 和 `provider_options`。Provider Options 必须由调用方显式给出并通过对应 Adapter allowlist；AgentNave 不默认覆盖模型、effort、权限模式、工具或 Provider 原生配置。

同一 server 进程内，同一 Provider 的同一原生 session_id 不得同时续接：共享启动入口在准备命令前拒绝重复活跃调用，单任务与组席位共用此检查。工作室在写入运行历史前检查，拒绝不产生新的任务记录。这不是跨 server 或原生直接调用的全局锁；不同 Provider 的相同标识互不影响。

MCP 初始元数据仅保留简短 Provider 目录和通用调用合同。Manager 首次使用某个 Provider 前调用只读 `describe_provider(provider)`，获取该 Provider 的允许状态与支持的选项，并在当前上下文中复用；详情查询不启动 CLI、不探测安装或认证、不改变工具列表。`describe_provider` 返回 `provider`、`permitted` 与 `supported_options`，不承载模型推荐。模型与 effort 由 Skill 的所选 CLI 参考文件和用户要求决定，使用推荐值时作为显式 Provider Options 传入；默认决策不下沉到 Adapter。

每个 Host 通过 `AGENTNAVE_EXCLUDED_PROVIDERS` 显式配置逗号分隔的 Provider 排除项，以排除与宿主同类的 CLI。按宿主产品对应的 CLI 配置，不按当前模型判断，也不猜测客户端身份。排除项在 server 启动时固定并验证，未知名称使启动失败；未配置或空值不排除任何 Provider。MCP 向 Manager 公布允许项与排除项；排除项只约束单任务 `start_agent`：宿主自己能开原生子代理完成单任务，而协作讨论需要同类 CLI 入席，因此核心协作组与工作室节目（含非隔离节目）都可加入被排除的 Provider。被排除的 `start_agent` 在创建 Invocation 前返回 Tool error，不能通过单次调用参数覆盖，也不自动回退。允许项不代表 CLI 已安装或认证。

Codex 在非 Git 目录运行时，调用方可显式传入布尔选项 `skip_git_repo_check`；Adapter 默认不绕过 Provider 的仓库检查。

macOS 的 Codex Adapter 优先选择桌面应用附带的可执行 CLI：先 ChatGPT，再 Codex，每种应用先查系统 Applications，再查用户 Applications。找不到可执行文件或在其他平台时，使用 PATH 中的 `codex`。启动与续接使用同一选择规则；AgentNave 不比较版本、不负责安装升级，也不在选中的 CLI 启动失败后改用另一版本。

内部归一化结果字段为 `status`、`provider`、`output`、`session_id`、`provider_usage`、`duration_ms` 和 `error`；`provider_usage` 只保留 Provider 可用的 `num_turns` 与 `total_cost_usd`，不转发 token、cache 或 model 明细。Provider 正常返回业务失败仍是完整的 Invocation Result。Provider 缺失、无法启动或平台不受支持也会形成带 `launch_error` 的结构化失败结果，以便 Manager 读取。

STDIO MCP 是唯一控制接口，基础工具为 `describe_provider`、`start_agent`、`wait_agent`、`wait_any`、`cancel_agent` 和只读的 `list_invocations`；`agentnave-mcp` 只负责为 MCP Host 启动 server 进程。全部 Tool 都发布输入与输出 JSON Schema；可由 Manager 修正的请求错误使用 MCP Tool error 返回重试指引，Provider 执行终态使用结构化 Invocation Result。继续 Provider 对话通过新的 `start_agent(session_id=...)` 完成。

启动回复另含 provider、model、effort，不重复 prompt。model 与 effort 只回显请求中的显式值，省略时显示“继承原生设置”，不声称已核实 Provider 实际生效的设置；等待与取消回复不重复这三项。

AgentNave 不提供总运行时限；`start_agent` 不接受截止时间参数，也不根据运行时长终止调用。`wait_agent` 的单次等待到期只返回运行状态，Manager 可继续等待或使用 `cancel_agent` 显式停止任务。Provider 原生限制继续生效，AgentNave 不静默改写。

启动、等待和取消的公开回复共用顶层 `invocation_id`、`status`、`reason`、`elapsed_ms`；按需包含 `activity`、`error`、`output`、`output_age_ms` 与 `session_id`，不返回费用或空字段。内部 Invocation Result 的 Provider 用量仍可保全，但不向公开回复转发。单次等待固定为五分钟，`wait_agent` 只接受 `invocation_id`，调用方不能覆盖时长；完成或已识别的明确执行阻塞提前返回；阻塞返回不终止 CLI，由 Manager 决定继续等待还是取消，同类阻塞每次 Invocation 仅唤醒一次。普通工具失败、暂时重试和事件沉默本身不构成阻塞。未识别的原生错误不保证提前唤醒。

`wait_any(invocation_ids)` 沿用同一五分钟窗口等待多个 Invocation；名单须非空、不重复且全部有效，否则在等待前整批返回 Tool error。任一项已终态或有未读阻塞即返回 `reason=ready`，`updates` 为此刻全部就绪项，形状与 `wait_agent` 相同，未就绪项继续运行且不出现在回复中；每个就绪项只消费进入回复的那条阻塞，其余未读阻塞留给后续等待。终态可重复读取，已终态 ID 每次都就绪，由 Manager 从下次名单移除。窗口到期返回 `reason=wait_elapsed` 与每项运行快照，不消费阻塞。聚合由 InvocationManager 在唤醒后不挂起地扫描完成；同一 Invocation 的多个等待者（包括轮次对其席位的等待）仍共享一份阻塞队列、先取先得，不广播。取舍见 [ADR-0007](../adr/0007-wait-any-ready-subset.md)。

`list_invocations` 只读列出当前进程内的活跃 Invocation 与保留期内的终态 Invocation，按启动顺序返回 `invocation_id`、`provider`、`cwd`、`status`、`elapsed_ms`（终态固定）和 `events_seen`（是否收到过任一结构化事件）；活跃项另含 `phase`，终态另含非空原生 `session_id`。不返回 prompt、正文、工具参数或启动时间，不消费阻塞，也不延长保留期。

运行中回复同时提供最新原生活动与最多 1000 字符的公开回复尾部及其年龄。正文独立保留，不因后续工具事件消失；相同正文可能在不同等待中重复，无游标、分页或独立读取工具。只增加有界进程内尾部，不持久化运行中的输出尾部。活动不返回原始事件名、工具调用 ID、工具参数/结果或推理正文；公开正文可能含任务数据，不提供自动脱敏保证。终态 `output` 保全最终回答，不应用中间正文长度限制；每次调用的保留 stdout 上限为 1 MiB（含必要协议数据），stderr 为 64 KiB，按字节计；采用事件过滤的 Provider 另沿用每条 8 MiB 的解析缓冲上限，工具／图片事件过滤后不累积保存，不占回复预算。超限返回 `output_limit_exceeded` 并终止进程树，不静默截断为成功。回复软预算由 Manager 在派发任务时安排，不能由容量推断模型上下文是否足够；长产物由有文件工具的 CLI 显式落盘并返回摘要与路径，服务不自动转存或传输附件。宿主超时限制由 Manager 尊重，无等待请求时不主动推送。

Invocation 状态只存在于当前进程内。macOS／Linux 的每次 Invocation 由一个专用 supervisor 持续占有 POSIX 进程组，Provider 正常终止后也先清理该组再回收 supervisor，避免旧 PGID 被复用。Windows 的专用 supervisor 创建 kill-on-close Job Object，将 Provider 挂起创建、加入 Job 后再恢复；Provider 结束时关闭 Job 以清理后代，取消或 supervisor 丢失时也由句柄关闭终止整棵 Job 进程树。MCP server 退出时会尽力终止仍活跃的 supervisor；重启后旧 Invocation 句柄不可恢复。Provider 自己持久化的 Session 不受此限制。

AgentNave 不是沙箱或同用户恶意进程隔离边界。POSIX 上已获得命令执行权限的 Provider 或工具可以主动创建新 OS session、杀死 supervisor，或以其他方式脱离普通进程组；发生可检测的 supervisor 丢失时返回 `supervision_lost`，但不能安全地对可能已被复用的旧 PGID 继续发信号。Windows Job Object 提供进程树所有权，但不隔离同一用户账户下的其他资源；Job 分配不被当前宿主环境允许时以 `launch_error` 失败，不降级为无树级所有权的启动。是否允许 Provider 命令由其原生权限机制和 Manager 决定；需要抵抗恶意同用户进程时，应在 AgentNave 外使用降权、容器或平台级资源域。

终态 Invocation 的内存记录在调用结束七天后自动清理，包括尚未读取的结果；重复读取不延长保留期，活跃调用不清理。Manager 应在过期前保留最终回答与原生 session_id，过期句柄的等待与取消返回 Tool error。清理不删除 Provider 原生会话、协作组已接收的回答或工作室落盘历史。

## Provider Adapter

Adapter 只能添加非交互输出、prompt 传输和 cwd 等协议必需参数。Provider stderr 仅在失败结果中以有界详情返回；prompt 不写入 AgentNave 日志或持久存储。AgentNave 自身的中间文件使用系统临时目录：目前仅 Grok prompt 使用标准库临时文件，调用结束清理；其余 prompt 走 stdin，运行中尾部留在内存。Manager 创建的任务交接/中间文件同样使用任务临时目录并管理读取生命周期，不强制回答正文格式。项目正式产物与 Provider 自己的会话、缓存不属于此临时文件所有权。绝对 `cwd` 传给 Provider 进程，项目规则加载仍由 Provider 原生实现负责，不能把临时文件目录替代为 cwd。

AgentNave 在 macOS／Linux 使用 POSIX 进程组，在 Windows 使用 Job Object。Windows Provider 必须挂起创建、成功加入配置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job 后才恢复；任何创建、配置或分配失败都以 `launch_error` 终止，不回退到普通子进程或 `CREATE_NEW_PROCESS_GROUP`。Windows 取消直接终止 supervisor 并关闭 Job，没有伪造 POSIX 的温和树级信号阶段。

## 协作组

取舍见 [ADR-0006](../adr/0006-core-rounds-and-studio-split.md)。`open_group` 在内存中登记 2–6 个席位，席位与 `start_agent` 同一合同：原生工具与权限、组级或席位级 cwd、显式 Provider Options；宿主排除项不约束席位。校验只调用 Adapter 的 `prepare` 并立即清理其临时文件，不启动进程。

`run_round` 非阻塞地运行一轮：本轮共同任务先作为 `task` 条目提交；`parallel` 同步启动全部席位，在全部终态后按名单一次提交；`serial` 逐席启动、逐席提交，后席能看到前席。同组同一时间只有一个轮次。终态 succeeded 提交最终输出为 `answer`，failed、blocked、cancelled 提交 `absent`；运行中的阻塞提醒不提交。不重试、不设每席超时、不摘要或截断。

每席续接自己的原生会话，只收到游标之后、非自己的条目；新会话收到前导与全部条目。只有 succeeded 且带本次 session_id 才推进游标；未启动的 launch_error 保留原会话；其他终态清空会话与游标，下次补全。前导声明其他席位的回答是数据而非指令，私密指令只发给对应席位，但席位复述的内容会随回答传播。

`wait_round` 与 `wait_agent` 相同的五分钟窗口，轮次结束或运行席位出现新阻塞时提前返回，结束后可重读。`cancel_agent` 取消单席后串联轮继续；`cancel_round` 先禁止新启动，再取消在途席位，已成功回答照常提交，未启动的串联席位不写条目。server 退出时先结束所有轮次，再关闭 InvocationManager。

`wait_round` 的 `include_output` 默认为 true；显式 false 只省略最终正文、运行尾部及其年龄，状态、活动、错误与原生 session_id 仍可读取。省略不消耗答案、不改变条目提交或审核，之后用 true 可重读全文。核心与工作室共用此合同，待审回答同样可重读。已结束轮次的 elapsed_ms 固定；finished 只表示编排结束，不代表各席都成功。意外执行异常形成带 internal_error 的 Invocation 失败结果；轮次驱动异常先停止新的启动、收束活跃席位、跳过未启动席位再释放组，返回 failed 与 round error，不公开未经审核的成功回答。状态写盘失败的轮次也返回 failed；取消仍遵守其独立语义。

多轮并联共享前轮已提交答案是有意设计，不提供连续盲答；串联顺序产生信息差也是有意设计，轮换顺序与追加全员修正轮由 Manager 决定。信息共享规则不提供文件隔离，写代码的并联席位由 Manager 分配独立工作树。讨论后的原生会话仍记得已收到的内容，切换 parallel 不能恢复盲答，新的独立回答需要新组与新原生会话。工作室审核只决定是否公开条目，不回滚工具副作用或清除作者会话记忆。

显式 restricted_tools=true 是受限配置，普通调用不启用；它保持原生工具限制并允许传入该席位自己的 session_id，只接受 model、effort（Pi 另可选模型后端 provider）。Claude 使用 safe-mode、空 tools、strict MCP 与禁用 skills；Grok 使用空 tools、deny-all、禁用子 Agent 和网页工具，不承诺移除工具定义或阻止 MCP 连接；CodeBuddy 复用 Claude 协议但覆盖自身限制参数；Codex 限制本地读取工具并忽略用户配置；Pi 禁用工具、扩展与 MCP。Antigravity 只能从 cwd 加载角色定义，Adapter 因此不写调用方目录：受限模式要求 cwd 已含一致的定义，由调用方（如工作室的隔离席位）事先写入。受限配置不等于操作系统沙箱。

## 工作室

工作室包 `agentnave-studio`（入口 `agentnave-studio-mcp`）与核心同版本、同仓库发布，精确依赖同版本核心，包含全部核心工具，另加保存历史、可选审核、主持人发言与本机只读看板，供人观看多 CLI 节目。取舍见 [ADR-0006](../adr/0006-core-rounds-and-studio-split.md)，用法见 [工作室节目说明](../studio.md)。

工作室通过核心 `Groups` 的三个内部覆盖点接入，不改变核心行为：`_on_change` 在席位启动前、终态后、每次提交与轮次结束时原子落盘；`_approve` 是提交前的审核屏障；`_ready` 让等待在有待审回答时提前返回 `review_needed`。取消与退出在第一次等待前置位并唤醒屏障，未决回答丢弃。

数据根默认 `~/.agentnave`（`AGENTNAVE_DATA_DIR` 可覆盖），首次保存会话时才创建。每个会话一个目录、一个原子替换的 `state.json` 与 `.lock` 文件锁：首次写入时取锁、重读磁盘状态并持有到进程退出，其他服务只读并以 writable=false 列出，不抢锁、不另建会话拆散历史。写盘失败时内存回滚到上次保存的状态；轮次中写盘失败则不再启动席位并在轮次上报告 `state_write_failed`。

节目即保存的 Group，会话 ID 就是 `group_id`；重启后首次写入时按保存的条目、席位会话与游标恢复。保存席位、条目（含时间）、各席会话与游标、每轮名单／顺序／私密指令／各席状态与最终输出、审核状态；不保存工具事件或推理；运行尾部只在运行期间写入数据根的运行文件（见下文“托盘数据”），结束即删除。重启时运行中的席位标为 interrupted 并清空其会话与游标，轮次标为 interrupted，未启动的串联席位标为 not_started，不续跑；待审回答保留，此后 `decide_answers` 直接写入条目，未决期间拒绝新轮次。

审核：并联轮全员终态后成功回答一起待审，全部决定后按名单提交；串联轮逐席待审，决定后才启动下一席。`decide_answers` 整批校验后保存再生效；同一决定幂等，改变决定被拒。被丢弃的回答不写条目，其他席位看不到，席位自己的原生会话仍记得它。主持人发言只能在无进行中轮次、无待审回答时追加为 `host` 条目。

隔离节目在会话目录下为每席创建独立目录，为 Antigravity 写入受限角色定义，给每席加 `restricted_tools`，忽略调用方 cwd。节目（隔离与否）都可包含 `AGENTNAVE_EXCLUDED_PROVIDERS` 中的 Provider；`describe_provider.show_permitted` 恒为 true 以保持兼容。

任务会话保存普通 `start_agent` 的 Provider、cwd、显式模型、状态、最终输出与原生会话：先由 Adapter 准备并校验请求，再写入记录并启动同一 PreparedCommand；同一原生会话并入其未结束的任务会话，并发续接同一会话被拒。后台采集器在终态保存结果，写盘失败时进程内标记中断，`wait_agent` 仍能读取结果。删除只针对已结束且无进行中工作的会话：删除 `state.json`、遗忘已结束的调用句柄、移除隔离席位中工作室写入的角色定义与空目录，保留会话目录及 `.lock` 以维持锁身份，Provider 原生文件不删。数据根中的旧版记录（`catalog/`、`room.json`）只列为不可用，不迁移。

看板在成功创建或继续任务／节目后自动启动并返回入口；仅查看历史时 `list_conversations` 返回首页入口，随 server 退出；启动时为全部可读会话构建投影，之后由落盘与 1.5 秒的运行刷新更新。只监听 127.0.0.1 随机端口，以随机路径及精确 Host/Origin 检查限制访问，不提供写入或文件映射接口；CSP 禁止外部加载与内联脚本，页面用 textContent 渲染。节目公开页只投影已提交条目的白名单字段，导演页另含私密指令、待审回答与运行尾部，任务页只给 Manager；页面不显示原生会话或调用 ID。用于本机查看与录屏，不作网络托管。

托盘数据（[ADR-0008](../adr/0008-tray-app-reads-studio-snapshots.md)）：持锁写入会话时，在 `state.json` 旁原子写出 `views.json`（版本 1：摘要、导演视图、公开视图，链接指向托盘路由 `/`、`/c/<id>/`、`/p/<id>/`），投影只来自 `views.py`；未被持有的会话在扫描时于探测锁下补写缺失或早于 `state.json` 的快照，写入失败不回滚状态。有工作运行时，每个服务每 1.5 秒原子写出 `live/<pid>-<随机>.json`（版本、进程号、更新时间、运行中会话含尾部与耗时的快照），无运行工作或退出时删除，并在写入前清理一小时以上的残留。托盘应用只读这些文件，不调用工具、不写数据根；浏览器看板保留为无托盘环境的后备。
