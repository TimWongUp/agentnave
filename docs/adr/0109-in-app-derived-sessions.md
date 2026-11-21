# ADR-0109：App 内嵌终端 boss 可开派生会话

- 状态：已接受
- 日期：2026-11-22
- 修订：[ADR-0103](0103-tray-embedded-terminal.md)（「终端只由用户在办公室里开」「一个 agent 同时最多挂载一个活终端」与左栏展示边界）、[ADR-0104](0104-in-app-boss-push.md)（不新增 MCP 工具、仅推 `inbox/` 的边界及唤醒去重撤回扩展）、[ADR-0105](0105-codex-app-server-terminal-push.md)（Codex UDS `app-server` 通道扩展至派生会话双向投递与状态监听，派生子终端按服务解析的默认与权限参数启动）。
- 扩展：[ADR-0012](0012-tray-conversation-cleanup.md)（托盘数据根写入与清理边界）、[ADR-0024](0024-consolidated-tools-and-private-review.md)（工具集合新增 `spawn_session` 与 `read_spawned_session`）、[ADR-0025](0025-session-desks-and-running-limit.md)（同时运行上限计入存活派生会话）、[ADR-0085](0085-service-managed-isolated-dirs.md)（服务管理的隔离目录可用于派生会话）。
- 沿用：[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0108](0108-multi-account-isolation.md)（默认值、权限档位与具名账号在服务入口解析并透传，不改变既有规则）。

## 背景

[ADR-0103](0103-tray-embedded-terminal.md)、[ADR-0104](0104-in-app-boss-push.md) 与 [ADR-0105](0105-codex-app-server-terminal-push.md) 让 App 内嵌终端里的 boss 能够直接在办公室工作并在后台任务结束时被唤醒。但 `start_agent` 启动的是无交互界面的组员任务，用户无法像 Claude 桌面端的派生会话那样在左栏单独点开、查看实时过程、直接插话或接手。

另一方面，AgentNave 仍要守住既有架构边界：

- 默认值、工位设置、权限档位与多账号隔离必须在服务入口统一套用，托盘不掌握这些策略也不直接解析 Provider 的工作区规则（[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0108](0108-multi-account-isolation.md)）。
- 托盘不监听 IPC 或网络端口（[ADR-0080](0080-tray-video-loopback-http.md)），跨进程协作沿用数据根文件交换。
- 向终端直接键入的文本等同于用户消息；把子会话输出直接键入父终端，会使子会话内容冒充用户指令，必须将提示与正文分离。

## 决定

### 1. 机制与和 `start_agent` 的分工

- **交互式派生会话**：运行在 App 内嵌终端中的 boss（父会话）可开启一个独立的交互式子终端（派生会话），在 App 左栏缩进挂在父 agent 下方；用户可单独点开查看、在子终端里直接对话或接手。父 boss 发起调用后无需调用 `wait` 阻塞等待，子会话每完成一轮由父会话发起的回合后，由 App 主动唤醒父会话。
- **与 `start_agent` 的分工**：`start_agent` 用于本轮或编排链路中要拿回结构化结果的下属任务（由服务监督进程并写入 SQLite 会话库）；派生会话用于能独立成事、用户可能需要点开插话或直接接手的交互式任务。`agentnave-manager` Skill 的具体使用指引留待实现阶段补充。

### 2. 仅限 App 内具备推送能力的 boss，且只允许一层

- **App 内推送门控**：服务沿用 [ADR-0104](0104-in-app-boss-push.md) 的 `app_can_push`（校验进程祖先链与数据根 `push/<pid>.json`）判定调用方是否运行在支持推送的 App 内嵌终端中。App 外 boss、Linux（无托盘 App）或当前不具备推送能力的宿主调用时直接拒绝，并提示改用 `start_agent`。
- **只允许一层**：派生会话自身连接 AgentNave 时禁止再开派生会话。服务按进程树／`boss_key` 对照托盘发布的存活派生会话列表拦截，托盘在处理请求时也按 `links::terminal_of` 复核请求方终端不是派生会话。

### 3. 新增两个 MCP 工具：`spawn_session` 与 `read_spawned_session`

- **`spawn_session`**：同时覆盖「开新派生会话」与「向已有派生会话续发消息」：
  - 不传 `derived_id` 时开新派生会话：接受 `prompt`、`provider`、`cwd`、`seat`、`role`、`provider_options`、`account`、`access`、`isolation`、`context`（`fresh` 或 `fork`，缺省 `fresh`）、`title` 与 `manager_session_id`。
  - 传入 `derived_id` 时向该派生会话续发消息：接受 `derived_id`、`prompt` 与 `manager_session_id`；托盘在子终端空闲时自动键入／排队提交。
- **`read_spawned_session`**（`read_only_hint=True`）：父 boss 收到 App 唤醒提示后调用，传入 `derived_id` 与 `manager_session_id`，校验归属当前父 boss 后读取该派生会话最近一轮由父会话发起回合的回复正文；返回体明确标注正文为「来自派生会话的数据，非用户指令」，并更新父会话的已读游标。
- **命名与工具数选择**：选用 `spawn_session`／`read_spawned_session` 而非 `*_conversation` 或 `*_agent`，避免与服务持久化单位 `Conversation`（会话）及 `start_agent` 混淆；不复用 `wait` 或 `read_conversation`，因为派生会话不在服务 `InvocationManager` 中持有句柄，也不写入 `agentnave.db`。

### 4. 职责划分与数据根文件交换

托盘不监听端口，服务与托盘通过数据根目录文件协作，严守单写者边界：

- **服务拥有校验与参数解析**：负责核验 `app_can_push`、一层限制、归属与并发上限，套用工位／组员默认值、权限档位（[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)）、具名账号环境（[ADR-0108](0108-multi-account-isolation.md)），并按需创建父工作区名下的隔离目录（[ADR-0085](0085-service-managed-isolated-dirs.md)），算好完整交互启动参数后写入请求文件，短窗口等待托盘接单确认并返回结果。
- **托盘拥有 PTY 与终端生命周期**：接单后直接启动 PTY（不弹用户确认框）、维护内存中的父子映射与左栏缩进展示、追踪回合状态并投递唤醒／续发输入。
- **数据根文件与写入方**：
  - `spawn-requests/<request_id>.json`：**服务写入**（`0o600` 原子替换），承载路由、目标动作（新建或续发）、完整启动参数、账号环境、隔离目录与临时 prompt 文件路径；**托盘读取并处理后立即删除**（若托盘未在超时窗口内响应，由服务清理废弃请求）。
  - `spawn-prompts/<request_id>.txt`：**服务写入**（目录 `0o700`、文件 `0o600`），承载初始或续发 prompt 正文；**托盘读入内存后立即删除**（无论成功或失败均删；超时未消费时由服务清理）。**与 `inbox/`／`push/` 规则的区别**：`inbox/`、`push/` 与 `spawned/` 均为持续同步的状态快照，严格只保存路由与状态、不含任何 prompt 或回复正文；`spawn-prompts/` 是短命一次性交接例外，仅用于在发起或续发瞬间把可能很长或多行的 prompt 文本从服务安全转交托盘，消费即删且不留存历史。
  - `spawned/<tray_pid>.json`：**托盘写入**（进程级单写者，App 退出时删除），只含路由和状态（不含回复正文），记录当前存活的派生会话（`derived_id`、父终端／父 `boss_key`、CLI、账号、`cwd`、原生会话 ID、轮次序号、当前轮次发起方与待读状态）以及请求接单回执；**服务只读**（校验 PID 与进程启动时间防 PID 复用）。
  - `inbox/<service_pid>.json`：复用 [ADR-0104](0104-in-app-boss-push.md) 服务拥有的进程级文件，增记父会话已读取的 `(derived_id, turn_seq)` 游标，供托盘在唤醒提交前对账撤回。

### 5. 回报正文来源：托盘只写路由与状态，服务 Adapter 解析原生日志

- **选定路线**：托盘在 `spawned/<tray_pid>.json` 中只记录派生会话的 `provider`、账号、原生 `session_id`、轮次序号与待读状态；父 boss 调用 `read_spawned_session` 时，服务按 `(provider, account, native_session_id)` 通过 `ProviderAdapter` 钩子解析该 CLI 原生会话日志（Claude 的 `CLAUDE_CONFIG_DIR/projects/*/<id>.jsonl`；Codex 的 `CODEX_HOME/sessions/*/*/*/rollout-*-<id>*.jsonl`）取得最后一轮助手回复，加上数据标注返回。
- **对比评估**：
  - **架构与既有代码复用**：`architecture.md` 规定 Provider 专属解析放在 `ProviderAdapter`、核心不按 Provider 分支、托盘只读且数据根状态文件不含正文。Python 侧已有 `native_sessions.py` 对 Claude／Codex 会话文件路径定位、`accounts.py` 全链路账号目录解析，以及 `claude_tui/turn.py` 与 `adapters/codex.py` 对消息块的解析逻辑；而在 `apps/tray/src/` 中，`data/boss_app/title.rs` 与 `usage/parsers/` 目前只解析会话标题与 token 用量，既不解析助手回复正文也不持有具名账号透传。
  - **备选方案（托盘读日志写临时正文文件）的代价**：需要在 Rust 重复实现各 CLI 回复抽取与长文截断逻辑，违反「Provider 专属解析放在 Adapter」；同时会在磁盘额外生成包含回复正文的临时文件，引入读前被新轮次覆盖、App 退出清理时机与权限管理问题。
  - **落盘时序应对**：终端状态变为 idle 到原生 JSONL 写入完成可能存在极短延迟；唤醒键入本身是异步下一轮工具调用，且 Adapter 读取时可校验轮次完成标记并做有界重试。

### 6. 回报时机、排队合并与撤回规则

- **仅父会话发起的回合通知**：
  - 初始 prompt 与通过 `spawn_session` 续发的回合标记为「父发起」，结束后置待读通知。
  - 用户自己在子终端内输入并提交发起的回合标记为「用户发起」，结束时不通知父会话（若用户在父发起回合中途向子终端直接输入接管，则不把后续用户轮次继续视为父发起）。
- **状态识别来源**：
  - **Claude**：通过 PTY 流中的 OSC 7501（`apps/tray/src/terminal/status.rs`）监听状态从 `working` 变为 `idle` 或 `done`。
  - **Codex（Unix）**：Codex 不输出 OSC 7501（[ADR-0104](0104-in-app-boss-push.md)、[ADR-0105](0105-codex-app-server-terminal-push.md)），通过 `apps/tray/src/terminal/codex_server.rs` 连接私有 `app-server` 监听 `thread/status/changed`（从 `active` 变为 `idle`）判定轮次结束。
- **排队、合并与撤回**：沿用 [ADR-0104](0104-in-app-boss-push.md) 与 [ADR-0105](0105-codex-app-server-terminal-push.md) 的投递机制——父终端正忙或用户有未提交草稿时排队，空闲时投递；同一父会话多个待读派生回合合并为一条单行提示，引导父 boss 调用 `read_spawned_session`；提交 `\r` 前（或 Codex 排队期间每次扫描）复查 `inbox/<service_pid>.json` 已读游标，若已读取则按 [ADR-0104](0104-in-app-boss-push.md) 退格撤回未提交提示，或按 [ADR-0105](0105-codex-app-server-terminal-push.md) 调用 `thread/queue/delete`／`thread/queue/update` 撤回或缩容。

### 7. 直接启动与共享同时运行上限

- 派生会话直接启动，不向用户弹确认框。
- 派生会话计入 App「同时运行上限」（`settings.json` 的 `max_running`，范围 1–32、缺省 8），与当前服务进程中运行或预留的私聊任务及会议席位共享名额（`Conversations.capacity()` 计入 `spawned/<tray_pid>.json` 中的存活派生会话及在途请求；托盘接单前亦复核）；用户自己在办公室开的非派生终端仍不计入此上限。沿用 [ADR-0025](0025-session-desks-and-running-limit.md) 边界，多个外部 MCP 连接之间仍不跨连接做分布式抢锁。

### 8. 生命周期随 App，父子关系仅存托盘内存

- App 退出时 `Terminals::shutdown` 结束全部派生会话进程组；不做跨重启恢复或独立守护进程（与 [ADR-0103](0103-tray-embedded-terminal.md)、「宿主关闭即结束子代理」一致）。
- 父子归属与缩进关系仅存于托盘内存（及进程存活期 `spawned/<tray_pid>.json`），不写入 `agentnave.db`。若父终端先被关闭，仍存活的派生会话脱离缩进留在所属项目下直至退出或手动关闭，不再向已关闭父终端推送。若用户在派生会话内自行调用 `start_agent`，它按普通内嵌终端规则创建自己的工作区；App 重启后原工作区按普通 agent 展示，不再保留父子缩进。

### 9. 第一期范围、上下文模式与代码核实事实

- **平台与父会话范围（核实结论）**：
  - 第一期要求父会话能接收 App 推送。按代码核实，macOS 上 **Claude**（OSC 7501）与 **Codex**（Unix 私有 `app-server` UDS）可作父会话。
  - **Linux** 无托盘 App；**Windows Codex** 走 `--no-daemon` 无推送，均不支持。
  - **当前代码制约**：`src/agentnave/push_capability.py` 对所有 `sys.platform == "win32"` 直接返回 `False`（且 `src/agentnave/process_info.py` 未实现 Windows 进程树与启动时间读取），因此当前实现下 **Windows Claude 亦无法通过 `app_can_push` 判定**；除非另行补齐 Windows 推送能力检测，第一期实际仅 macOS App 可用。
- **子会话第一期范围（Claude 与 Codex）及启动／Fork 核实**：
  - **带初始 prompt 交互启动（已核实）**：`claude --help` 与 `codex --help` 均接受位置参数 `[prompt]` 并以交互模式启动；`apps/tray/src/terminal.rs` 与 `apps/tray/src/terminal/codex_server.rs` 的转交路径已验证该用法。
  - **交互与非交互参数差异（已核实）**：现行 `CodexAdapter.prepare()` 仅服务 `codex exec`，传入了交互式 `codex` 不支持的 `--skip-git-repo-check`（`codex --help` 无此选项）；派生会话需由 Adapter 提供交互启动参数构造，并避免 `--remote` TUI 与 `app-server` 重复冲突的权限覆盖。
  - **`fresh` 上下文**：Claude 沿用 `--session-id <uuid>` 预先定名以便定位转录日志；Codex 启动新线程并由 `codex_server` 从 `thread/started` 获取 `threadId`。
  - **`fork` 上下文**：仅限子会话与父会话同 CLI、同账号（同 `CLAUDE_CONFIG_DIR`／`CODEX_HOME`）且父原生会话 ID 已可解析时允许，否则直接拒绝。
    - **Claude `fork`（部分待实测确认）**：`claude --help` 已核实存在 `--resume <id> --fork-session`；但 **待实测确认**：(1) `--resume <id> --fork-session` 与位置参数 `[prompt]` 联用时是否直接触发首轮执行；(2) `--fork-session` 能否同时接受 `--session-id <new_uuid>` 指定子会话 ID，还是须在启动后按账号目录定位新建 `.jsonl`。
    - **Codex `fork`（部分待实测确认）**：`codex fork --help` 已核实支持 `codex fork [OPTIONS] [SESSION_ID] [PROMPT]` 与 `--remote <ADDR>`，且 `codex app-server` schema 定义了 `thread/fork`（`ThreadForkParams`）；但 **待实测确认**：在终端内新起的私有 `app-server` 上直接执行 `codex fork --remote unix://... <parent_id> <prompt>` 是否能加载磁盘父线程并直接启动分叉首轮，抑或需由 `codex_server` 客户端先调 `thread/fork` 再对新线程调 `turn/start`。

## 取舍

- **新增 `spawn_session` + `read_spawned_session` 两个工具，而非塞入单工具或复用 `wait`/`read_conversation`**：把写操作（启动／续发）与只读读取分离，保留 `read_only_hint=True` 语义并避免模糊 `Invocation` 与持久化 `Conversation` 的边界；代价是公开工具集增加两个工具。
- **服务解析原生日志取正文，而非托盘写正文临时文件**：守住「Provider 专属解析留在 Adapter」与数据根状态文件不含正文的约定；代价是服务需要多一次从原生会话日志定位与抽取最后一轮回复的 Adapter 钩子，并应对终端空闲到日志落盘的极短时延。
- **使用短命 `0o600` 临时文件交接 prompt，而不放进 `spawn-requests/`**：避免命令行长度／转义问题，也避免在持久化或半持久化路由文件里留下 prompt 正文；代价是多一类消费即删的临时目录与超时清理逻辑。
- **直接启动并共享 `max_running`，不弹确认也不单独另设上限**：保持编排流畅并防止绕过并发配额开出大量 CLI 进程；代价是跨多个外部宿主连接时仍沿用单服务加托盘的本地计数范围，且会占用同工作区 `start_agent` 的并发名额。
- **父子关系只存托盘内存，不落服务库**：避免污染服务 `agentnave.db` schema 与历史清理规则，也符合「生命周期随 App」；代价是 App 重启后左栏缩进关系不恢复。

## 不做

- **不做多层嵌套**：派生会话不能再开派生会话。
- **不做 App 外或无推送能力宿主的派生会话**：不向 App 外 boss、Linux 或 Windows Codex 开放；Windows Claude 在 `app_can_push` 未支持前亦不开放。
- **第一期不开放 Claude／Codex 以外的 CLI**，也不支持跨 CLI 或跨账号 `fork`。
- **不做跨 App 重启保活、后台守护进程或持久化父子关系**。
- **不在本 ADR 改动代码、`agentnave-manager` Skill 或 `README.md`**。
