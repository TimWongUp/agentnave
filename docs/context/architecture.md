# 架构与边界

AgentNave 是供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用的 MCP Server。它在 Manager 与 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 之间提供很薄的进程边界，不是面向人的 CLI 产品、多智能体运行时或工作流引擎。

## 所有权

Manager 拥有目标解释、任务拆解、角色和模型选择、并行策略、重试、审核循环、结果综合、工作树和长期任务状态。

AgentNave 只拥有：

- 按需返回所选 Provider 的允许状态和支持参数；
- 根据冻结请求启动一个 Provider CLI；
- 传递 prompt、cwd、可选 `session_id` 和显式 Provider Options；
- 从 Provider 原生流事件提取有界活动快照，并归一化终端结果；
- 执行显式取消和进程树清理；
- 在 MCP server 生命周期内保存 Invocation 句柄；
- 在 Manager 显式创建讨论室后，保存房间和每席会话绑定、增量投影公共发言、收集候选台词并执行发布决定；
- 按需提供独立的公共发言区、导演工作台与公开台词 JSONL 导出。

AgentNave 不拥有 DAG、调度器、角色决策、自动重规划、工作树、保留策略、桌面 App 或 TUI。讨论室恢复只恢复房间数据，不恢复 Invocation 进程。

## 安装与激活边界

运行时由 Python Tool Manager 安装，并在 MCP Host 注册稳定入口。配套 `agentnave-manager` Skill 承担 CLI 调用的主要触发、模型选择、传参、等待、取消与续接说明，每个 CLI 的模型与 effort 默认值放在独立参考文件中，选中后才读取。MCP instructions 与 Tool 描述只保留轻量调用合同，`describe_provider` 按需返回允许状态与支持参数；没有 Skill 时仍可显式调用 MCP。Skill 由宿主的技能安装机制或已有部署管理器单独安装与更新，MCP 与 Skill 都只提供 CLI 调用能力与用法，不制定调用方的任务规划、调度、审核、重试或综合策略。

Python Tool Manager 拥有运行时的安装、升级与卸载，并提供稳定的 `agentnave-mcp` launcher。源码 checkout 只用于贡献、调试和未发布版本验证，不作为 MCP Host 的长期启动路径。

MCP Host 拥有 AgentNave 的注册、作用域、启停、移除；能使用 Host 官方管理接口时，不由 AgentNave 直接修改其配置文件。安装运行时不会自动写入 Host Skills、全局规则或权限。Provider CLI 继续拥有自身的安装、认证、配置与 Session 数据，普通 Invocation 的句柄仍是进程内对象；工作台保存其私有任务元信息、会话绑定和最终输出，不保存 prompt 或原始事件。讨论室在 Manager 指定的目录保存私有 `room.json`；卸载不自动删除房间。

## 稳定合同

请求字段为 `provider`、`prompt`、绝对 `cwd`、可选 `session_id` 和 `provider_options`。Provider Options 必须由调用方显式给出并通过对应 Adapter allowlist；AgentNave 不默认覆盖模型、effort、权限模式、工具或 Provider 原生配置。

MCP 初始元数据仅保留简短 Provider 目录和通用调用合同。Manager 首次使用某个 Provider 前调用只读 `describe_provider(provider)`，获取该 Provider 的允许状态与支持的选项，并在当前上下文中复用；详情查询不启动 CLI、不探测安装或认证、不改变工具列表。`describe_provider` 返回 `provider`、`permitted` 与 `supported_options`，不承载模型推荐。模型与 effort 由 Skill 的所选 CLI 参考文件和用户要求决定，使用推荐值时作为显式 Provider Options 传入；默认决策不下沉到 Adapter。

每个 Host 通过 `AGENTNAVE_EXCLUDED_PROVIDERS` 显式配置逗号分隔的 Provider 排除项，以排除与宿主同类的 CLI。按宿主产品对应的 CLI 配置，不按当前模型判断，也不猜测客户端身份。排除项在 server 启动时固定并验证，未知名称使启动失败；未配置或空值不排除任何 Provider。MCP 向 Manager 公布允许项与排除项；被排除的调用在创建 Invocation 前返回 Tool error，不能通过单次调用参数覆盖，也不自动回退。允许项不代表 CLI 已安装或认证。

Codex 在非 Git 目录运行时，调用方可显式传入布尔选项 `skip_git_repo_check`；Adapter 默认不绕过 Provider 的仓库检查。

macOS 的 Codex Adapter 优先选择桌面应用附带的可执行 CLI：先 ChatGPT，再 Codex，每种应用先查系统 Applications，再查用户 Applications。找不到可执行文件或在其他平台时，使用 PATH 中的 `codex`。启动与续接使用同一选择规则；AgentNave 不比较版本、不负责安装升级，也不在选中的 CLI 启动失败后改用另一版本。

内部归一化结果字段为 `status`、`provider`、`output`、`session_id`、`provider_usage`、`duration_ms` 和 `error`；`provider_usage` 只保留 Provider 可用的 `num_turns` 与 `total_cost_usd`，不转发 token、cache 或 model 明细。Provider 正常返回业务失败仍是完整的 Invocation Result。Provider 缺失、无法启动或平台不受支持也会形成带 `launch_error` 的结构化失败结果，以便 Manager 读取。

STDIO MCP 是唯一控制接口，基础工具为 `describe_provider`、`start_agent`、`wait_agent` 和 `cancel_agent`；`agentnave-mcp` 只负责为 MCP Host 启动 server 进程。全部 Tool 都发布输入与输出 JSON Schema；可由 Manager 修正的请求错误使用 MCP Tool error 返回重试指引，Provider 执行终态使用结构化 Invocation Result。继续 Provider 对话通过新的 `start_agent(session_id=...)` 完成。

AgentNave 不提供总运行时限；`start_agent` 不接受截止时间参数，也不根据运行时长终止调用。`wait_agent` 的单次等待到期只返回运行状态，Manager 可继续等待或使用 `cancel_agent` 显式停止任务。Provider 原生限制继续生效，AgentNave 不静默改写。

启动、等待和取消的公开回复共用顶层 `invocation_id`、`status`、`reason`、`elapsed_ms`；按需包含 `activity`、`error`、`output`、`output_age_ms` 与 `session_id`，不返回费用或空字段。内部 Invocation Result 的 Provider 用量仍可保全，但不向公开回复转发。单次等待固定为五分钟，`wait_agent` 只接受 `invocation_id`，调用方不能覆盖时长；完成或已识别的明确执行阻塞提前返回；阻塞返回不终止 CLI，由 Manager 决定继续等待还是取消，同类阻塞每次 Invocation 仅唤醒一次。普通工具失败、暂时重试和事件沉默本身不构成阻塞。未识别的原生错误不保证提前唤醒。

运行中回复同时提供最新原生活动与最多 1000 字符的公开回复尾部及其年龄。正文独立保留，不因后续工具事件消失；相同正文可能在不同等待中重复，无游标、分页或独立读取工具。只增加有界进程内尾部，不持久化运行中的输出尾部；终态输出由工作台另行保存。活动不返回原始事件名、工具调用 ID、工具参数/结果或推理正文；公开正文可能含任务数据，不提供自动脱敏保证。终态 `output` 保全最终回答，不应用中间正文长度限制；仍受整体捕获上限约束。宿主超时限制由 Manager 尊重，无等待请求时不主动推送。

Invocation 状态只存在于当前进程内。macOS／Linux 的每次 Invocation 由一个专用 supervisor 持续占有 POSIX 进程组，Provider 正常终止后也先清理该组再回收 supervisor，避免旧 PGID 被复用。Windows 的专用 supervisor 创建 kill-on-close Job Object，将 Provider 挂起创建、加入 Job 后再恢复；Provider 结束时关闭 Job 以清理后代，取消或 supervisor 丢失时也由句柄关闭终止整棵 Job 进程树。MCP server 退出时会尽力终止仍活跃的 supervisor；重启后旧 Invocation 句柄不可恢复。Provider 自己持久化的 Session 不受此限制。

AgentNave 不是沙箱或同用户恶意进程隔离边界。POSIX 上已获得命令执行权限的 Provider 或工具可以主动创建新 OS session、杀死 supervisor，或以其他方式脱离普通进程组；发生可检测的 supervisor 丢失时返回 `supervision_lost`，但不能安全地对可能已被复用的旧 PGID 继续发信号。Windows Job Object 提供进程树所有权，但不隔离同一用户账户下的其他资源；Job 分配不被当前宿主环境允许时以 `launch_error` 失败，不降级为无树级所有权的启动。是否允许 Provider 命令由其原生权限机制和 Manager 决定；需要抵抗恶意同用户进程时，应在 AgentNave 外使用降权、容器或平台级资源域。

## Provider Adapter

Adapter 只能添加非交互输出、prompt 传输和 cwd 等协议必需参数。Provider stderr 仅在失败结果中以有界详情返回；prompt 不写入 AgentNave 日志或持久存储。AgentNave 自身的中间文件使用系统临时目录：目前仅 Grok prompt 使用标准库临时文件，调用结束清理；其余 prompt 走 stdin，终态结果由私有工作台保存，运行中尾部留在内存。Manager 创建的任务交接/中间文件同样使用任务临时目录并管理读取生命周期，不强制回答正文格式。项目正式产物与 Provider 自己的会话、缓存不属于此临时文件所有权。绝对 `cwd` 传给 Provider 进程，项目规则加载仍由 Provider 原生实现负责，不能把临时文件目录替代为 cwd。

AgentNave 在 macOS／Linux 使用 POSIX 进程组，在 Windows 使用 Job Object。Windows Provider 必须挂起创建、成功加入配置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job 后才恢复；任何创建、配置或分配失败都以 `launch_error` 终止，不回退到普通子进程或 `CREATE_NEW_PROCESS_GROUP`。Windows 取消直接终止 supervisor 并关闭 Job，没有伪造 POSIX 的温和树级信号阶段。

## 可选讨论室

讨论室合同见 [使用指南](../discussion-rooms.md)，取舍见 [ADR 0005](../adr/0005-director-controlled-discussion-rooms.md)。主控 Agent 仍拥有全部导演判断，讨论室没有自动轮次或多 Agent 调度。

房间存储采用一个原子替换的私有 JSON 文件和操作系统文件锁，同一目录只允许一个 MCP server 写入。公共发言区的 Message 在产品接口中只追加，HTTP 的 JSONL 导出、公共页面、导演台的公共记录和席位增量输入都从这份列表投影，避免双写真源。候选稿、私有指令和运行状态只进入导演视图；两个视图使用不同随机地址，公开响应不包含导演地址。

同房间同席位保持原生 session_id、固定 cwd、已读消息 ID 与上次回答的发布状态；不同席位不共享原生会话。首次发言注入规则与完整公共历史，后续只发本轮指令、新公开 Message 和上次回答是否已发布，已知的自身发言也不重复发送。私有指令与候选稿不跨席位转发。原生会话绑定保存在私有房间文件，不进入网页。席位 cwd 跨 MCP 进程退出保留；显式重置仅删除空 cwd，不递归删除原生 CLI 文件或会话。

启动前先持久化原生历史不确定标记，正常终态确认 session_id 后保存已读位置并清除标记；失败、取消、运行中崩溃或持久化失败不静默重试续接。导演显式 reset_discussion_seat 后下一次才新开会话。正常重启恢复完成席位的会话和已读状态，旧 Invocation 句柄不恢复。采集结束但写盘失败时，进程内保留失败/中断终态作为后续修改的回滚基线；再次写盘失败不会把已结束的调用复活为 running。磁盘状态仍按原来的重启恢复规则处理。重置不删除公共记录，不改变其他席位。

显式 discussion_mode=true 保持原生工具限制并允许传入该席位自己的 session_id；只允许 model、effort 附加选项。Claude 使用 safe-mode、空 tools、strict MCP 与禁用 skills，保留原生会话持久化；Grok 使用空 tools、deny-all、禁用子 Agent 和网页工具。防作弊约束禁止读取其他席位私有内容和导演后台；原生通用规则可以存在。Grok 的 deny-all 限制工具执行，不承诺移除工具定义或阻止 MCP 连接。普通 start_agent 不启用这些参数。房间投影不等于操作系统沙箱。

公共发言区可通过公开地址的 state 或 transcript.jsonl 读取；现有 CLI 适配器直接从相同记录补发未读台词，不依赖每个 CLI 配置网页或 MCP 工具。导演工作台额外显示本轮私有内容，CLI 输入中只有公共地址。公开 JSONL 也可供后续只读集成使用；不给参演 CLI 注册导演控制工具。

两个页面随第一个 open_discussion 按需启动，随 MCP server 退出；只监听 127.0.0.1 随机端口，以随机路径及精确 Host/Origin 检查限制访问，不提供文件目录映射或写入接口。页面使用 textContent 渲染台词，脚本、样式与原创头像来自本地，CSP 禁止外部加载、嵌入及内联脚本。用于本机查看和录屏，不作网络托管，也不隔离拥有同用户文件访问权的恶意进程。

主持人是导演的公开身份，通过 post_discussion_message 发布，沿用 director speaker 键；不新增席位或模型会话。静态界面的角色筛选只作用于浏览器展示，不能改变公共记录或任何席位的可见历史。UI 参考来源与权限实测边界见 docs/discussion-rooms.md。

多节目共享 HTTP 服务与 InvocationManager，但各目录的 RoomState、轮次锁、席位 cwd/session 和已读位置独立。list_conversations 及导演网页导航仅向主控列出当前打开的节目；公共投影不含节目列表或跨节目 URL，工作台的私有目录索引记录显式打开的会话路径，重启时读取这些已登记路径。打不开或由其他服务占用的记录在首页标记不可用，不抢锁或自动接管。

节目模式允许全部五种 Provider，包括宿主自身 CLI；普通 start_agent 仍检查 AGENTNAVE_EXCLUDED_PROVIDERS。describe_provider 用 discussion_permitted 单独表达节目许可，不通过修改全局排除设置接入选手。CodeBuddy 复用 Claude 的协议与 prepare，但覆盖自身原生限制参数；Codex 限制本地读取工具并忽略用户配置；Antigravity 在每席独立 cwd 创建空工具角色并显式选择，避免改写全局角色或权限。具体原生能力与实测边界见讨论室说明。

独立作答由房间的显式 blind 模式控制：启动首席建立 blind_round_id，CLI 原生 JSON 回复经现有采集器校验后自动进入私有 pending_answers，不需主控搬运正文或逐条批准收存。普通讨论仍保留 ready 候选审批。盲答收集期间禁止单条公开、修改公共题目及已提交席位重答；整轮决定按 round ID 校验，全员收齐才能原子发布，丢弃可结束未完成轮次。模式及未揭晓答案随 room.json 恢复，公共投影和后续选手输入仅读取已公开 messages。导演只能选择发布或丢弃原文，不可代写答案。

## 统一私有工作台

Workbench 复用 Room 的原子保存、文件锁和 Dashboard，新增普通任务类型及私有会话索引。start_agent 先通过既有 Adapter 准备并校验请求，再创建 TaskRun 并启动同一 PreparedCommand，避免无效参数产生历史或重复准备。后台采集器直接读取 InvocationManager 的正文与终态：展示有界运行中回复，持久化最终正文、状态、cwd、显式模型及原生会话绑定；主控不承担正文搬运。conversation_id 只归组，不加入 prompt。模型返回 succeeded 展示为“已回复 · 待验收”。

普通任务不注册公共 token，也无台词 JSONL 导出；会话列表不含任务正文，只有私有任务页显示输出，网页不显示原生会话或 invocation IDs。MCP 私有状态保留当前进程的调用句柄供取消和等待；重启清空旧句柄，running 记录改为 interrupted，不重新启动任务。结束会话需先处理活跃任务、候选及未完盲答，历史保留。

显式 delete_conversation 仅删除已归档且无待处理工作的会话，调用方负责取得用户授权。AgentNave 清理自身历史、索引、网页投影与该会话的已结束 Invocation 记录；删除后旧句柄不可再等待或取消。Provider 原生文件及会话不属于删除范围，席位目录只移除空目录，会话根目录及其 `.lock` 保留稳定锁身份，避免不同进程锁住同路径的不同文件；用户指定的外部根目录保留。归档仍可重开，删除没有回收站；不引入自动保留策略。

默认数据根是 ~/.agentnave，可通过 AGENTNAVE_DATA_DIR 选择固定绝对路径。新讨论席位 cwd 位于会话目录内；旧记录中的席位路径继续按已有绑定恢复，不擅自搬动原生 CLI 历史。显式打开外部目录会登记其路径，不复制其数据。索引按会话分别原子更新，目录锁仍确保单写者；跨服务不会拼接运行中 Invocation 或强制接管锁。首页只汇总本服务能打开的记录，手动终端进程不自动纳入。

缺席是导演决定，不由超时自动触发。先取消并等待在途调用、处理候选，再标记未提交席位；本轮禁止其再提交。揭晓等待所有未缺席者提交，部分结果附未作答名单。收集结束后可单向从 blind 转 discussion，保留席位会话与已读游标，不撤销历史信息。
