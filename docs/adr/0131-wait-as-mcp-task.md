# ADR-0131：支持 MCP Tasks 的连接上，`wait` 以懒收取的等待任务运行

- 状态：已接受
- 日期：2026-11-28
- 修订：
  - [ADR-0082](0082-claude-long-wait-terminal-only.md)：撤回「不把 `wait` 实现为 MCP task」一条；Claude Code 桌面端与 VS Code 改由等待任务在后台等。
  - [ADR-0077](0077-claude-long-wait.md)、[ADR-0084](0084-claude-terminal-default-long-wait.md)：1500 秒长等待只用于没有任务能力的 Claude Code 终端会话。
  - [ADR-0067](0067-wait-270s-window-and-host-background-limits.md)：「上游 Python SDK 未发版 SEP-2663 服务端类型」不再妨碍服务端实现。
  - [ADR-0114](0114-tui-runtime-like-a-user.md) 第 3 节：收件方式表增加「支持 MCP Tasks 的 Claude Code」一行。
  - [ADR-0064](0064-dispatch-receipts-steer-to-wait.md)：回执 `next` 在任务连接上指向等待任务，派发与等待回执引导收取的原则不变。
- 补充：[ADR-0007](0007-wait-any-ready-subset.md)（任务结果与就绪子集等价，每项仍只交付一次、只消费写进回复的阻塞）、[ADR-0010](0010-launcher-relay-swaps-upgraded-server.md)（中继不变，`restart_safe` 计入租约有效的进行中等待任务）。
- 沿用：[ADR-0116](0116-progress-stall-reminder.md)（到限只提醒）、[ADR-0117](0117-private-dispatch-without-skill.md)（随宿主变化的用法写进回执）、[ADR-0121](0121-t3-code-orchestration.md)（T3 主线程的后台命令等收件箱）。

## 背景

ADR-0082 否决把 `wait` 做成 MCP task，理由是 Claude Code 桌面端和 VS Code 不支持 MCP Tasks、终端 CLI 本来就会在两分钟后把阻塞调用转入后台。Claude Code 2.1.331 起，终端 CLI、Agent SDK、VS Code 与桌面端本地会话都支持 2026-07-28 协议的 `io.modelcontextprotocol/tasks` 扩展（SEP-2663）。客户端按每次 `tools/call` 的 `_meta` 能力声明决定是否接受任务，之后按 `pollIntervalMs` 轮询 `tasks/get`，用户在 `/tasks` 里按 `x` 或 Claude 停止时发 `tasks/cancel`；它不用 `tasks/result`、`tasks/list` 与任务通知，也不在 VS Code、桌面端与 `claude -p` 里支持任务内的 `input_required`。

现状的代价：桌面端和 VS Code 的 boss 每次 `wait` 在前台阻塞 270 秒，用户插话要等它返回；终端 CLI 的 `wait(1500)` 先在前台阻塞约两分钟，单次调用还受条目 `timeout` 与 stdio 空闲超时约束。

`mcp==2.1.1` 的 `CreateTaskResult` 等类型仍是 2025-11-25 的旧形状，但 SDK 自带的扩展接口（SEP-2133）可以新增 `tasks/get`、`tasks/cancel` 方法、包住 `tools/call` 并在 `server/discover` 中声明扩展；2026 连接上非核心 `resultType` 原样发出。服务端用它即可实现，不必等上游。

## 决定

### 1. 只有 `wait` 返回任务，派发照旧同步

- 派发与执行类工具（`start_agent`、`start_agents`、`run_round`、`answer_prompt` 等）照旧立即返回普通回执。创建任务的结果没有 `content`，派发回执（句柄、warnings、`not_started`、隔离目录）在终态前送不到 boss；`tasks/cancel` 又会在 Claude 停止时发出，若派发即任务，它就等于终止子代理，违背「停止只靠显式 `cancel`」。
- `wait` 同时满足以下条件才返回任务：连接为 2026-07-28 协议；本次请求的能力声明含 `io.modelcontextprotocol/tasks`；`wait_seconds` 不为 `0`；不在 T3 主线程或子线程（T3 主线程沿用 ADR-0121 的后台命令）。显式 `ids` 的等待（会议、指定句柄）同样适用。
- 参数错误、未知句柄、`nothing_pending`、`already_waiting`，以及首次不阻塞扫描已有就绪项的情况，照旧同步返回。`start_agent(wait=true)` 与 `run_round` 的派发后先等仍同步等 270 秒。
- 服务在 `server/discover` 中声明扩展，设置为空；`tasks/get` 与 `tasks/cancel` 只在 2026-07-28 协议上提供，其他协议得 `-32601`。

### 2. 懒收取：后台只判就绪，`tasks/get` 时才收

- 建任务时登记为该 boss 的阻塞等待，`already_waiting` 与托盘角标的语义与普通阻塞等待相同。
- 后台协程只按收件箱快照用的同一套事件（终态、待答弹框、阻塞含 `progress_stalled`、待审、席位丢失）判断范围内是否有就绪项，不消费任何东西。
- 收到 `tasks/get` 且已就绪时，先撤掉阻塞登记，再按阻塞等待的返回形状（不附运行中正文尾部，`next`、`follow_up`、`seq` 照常）收取；收取结果与同步 `wait` 一致，缓存为 `completed`，之后重复轮询返回同一份。收取时发现其实没有就绪项，就重新登记、保持 `working`。
- 这样交付与 `tasks/get` 的应答是同一次：任务被取消、客户端不再轮询或进程退出时，结果都还留在收件箱里，下一次 `wait` 照常收到。

### 3. `tasks/cancel` 只取消等待

按 SEP-2663（2026-07-28）：

- 进行中的任务：停止后台判断、撤掉阻塞登记、置为 `cancelled`，返回空确认；不消费结果，不结束子代理。之后的 `tasks/get` 返回 `cancelled`。
- 已终态的任务保持原状态，同样返回空确认。
- 未知或过期的 `taskId` 返回 `-32602`。

### 4. 窗口、租约与清理

- 任务模式不套用 270／1500 秒窗口，`pollIntervalMs` 为 2000、`ttlMs` 为 `null`。创建满 2 小时后的下一次 `tasks/get` 无论有无就绪项都完成（无就绪项时 `reason=wait_elapsed`，boss 照 `next` 再等），只防孤儿任务。
- 轮询租约：超过 60 秒没收到 `tasks/get`，撤掉阻塞登记，防止失联的客户端让之后的 `wait` 一直得到 `already_waiting`；恢复轮询时若该收件箱没有别的阻塞等待就重新登记。租约失效且过了 2 小时的任务直接丢弃。
- 终态任务交付 10 分钟后清理；之后的 `tasks/get` 得 `-32602`，提示重新调用 `wait` 收件。宿主重连或 `--resume` 后旧 `taskId` 同样得 `-32602`；旧服务进程随 stdin 关闭退出，历史已落盘。

### 5. `restart_safe` 与中继

- 任务只存在服务进程内存。存在租约有效的 `working` 任务时，`restart_safe` 为假，中继不换进程；租约失效的任务不阻止切换，客户端恢复轮询时得到 `-32602`，再 `wait` 即可。
- 中继不改：`tasks/*` 当作普通请求透传，热切换复用的 envelope 已带客户端能力。

### 6. 回执与状态文字

- 回执 `next` 按本次请求有无任务能力选择；任务的终态回执用建任务时记下的能力。任务连接上的 `next` 写明「调用 `wait`，它转为后台任务，结果到达时 Claude Code 开新一轮」，`already_waiting` 改为「后台等待任务仍在跑，结束本轮」。
- `statusMessage` 只用状态图标与服务自有的成员称呼（ADR-0062 的 `spoken_name`，模型未记录时用 CLI 名），不带任务标题、提示词或输出。优先级：已就绪 > 待答弹框 > 疑似卡住 > 开会与干活。语言按服务进程的 locale：依次看 `LC_ALL`、`LC_MESSAGES`、`LANG`（`C`／`POSIX` 视为未设），macOS 都未设时读系统首选语言，`zh` 开头用中文，其余用英文；进程内缓存一次，不读托盘的私有偏好文件。

### 7. 不做

- 不用 `input_required` 承载 `answer_prompt`：VS Code、桌面端与 `claude -p` 遇到它会直接取消任务。
- 不提供 `tasks/list`、`tasks/result` 与任务通知；Claude Code 不用它们。
- 不删除 1500 秒长等待与托盘写入的 Claude 条目超时下限，留给不支持任务的 Claude Code 版本与关闭任务的环境。

## 取舍

- **只让 `wait` 成为任务**：派发回执和 `cancel` 的含义不变，boss 的循环（派发、等、处理就绪项、再等）也不变，只是等的那一步在后台进行；代价是每次就绪仍要再调一次 `wait`，由任务转成新一轮唤醒，不能省掉。
- **懒收取**：交付延迟最多一个轮询间隔（2 秒），换来取消与失联都不丢结果。后台直接收取虽然少一次扫描，但 `tasks/cancel` 晚于收取就会让已交付的结果永远到不了 boss。
- **租约失效不阻止换进程**：失联客户端不会无限期拦住升级；代价是它恢复轮询时要重新 `wait`。
- **按 locale 选语言**：服务不读托盘的私有偏好；代价是终端 `LANG` 与系统语言不一致时，以 `LANG` 为准。

## 考虑过的方案

- **派发工具直接返回任务**：见第 1 节，回执延迟到终态、`tasks/cancel` 变成终止子代理，`prompt_pending`、`review_needed` 仍要 boss 调 `wait`，两套收件方式混用。
- **一个任务一直等到 `all_finished`**：待答弹框、待审与卡住提醒必须中途交给 boss，不能攒到最后。
- **按宿主名或入口判断是否发任务**：客户端已在每次请求里声明能力，按声明判断不会误判版本；只排除 T3，因为那里已有验证过的后台命令方案。
