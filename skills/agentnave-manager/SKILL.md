---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Antigravity CLI、Pi Coding Agent 或 DeepSeek Harness 执行任务时使用，说明如何通过 AgentNave 交接任务、选择模型、传参、等待、取消及续接。也用于多个 CLI 并联或串联协作（各自答、多轮各自答、轮流聊、先各自答再讨论）、问意见、交叉复核、分头做、择优、分歧收敛、接力，以及保存的协作节目；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用 CLI

本 Skill 只讲调用方式；参数细节以连接中的实时 schema 和工具说明为准。任务选择、规划、分工、审核、综合、重试和 worktree 由主 Agent 决定；用户若有固定的编排偏好，写在宿主的全局规则里，按那里执行。

编排单向：boss 是宿主里的 Agent，由 boss 自己决定、或按用户指示编排其他 CLI。被 AgentNave 启动的子代理（含其自带子代理）不调用 AgentNave 的工具，只靠派发指令约束，服务端不禁用工具。

优先使用当前宿主的 AgentNave MCP，用户指定其他路线时遵从。工具不可用、CLI 被排除或连接里仍是旧版工具（如 `wait_agent`、`open_group`）时，报告具体限制并要求更新服务和重连，不静默换 Provider、调用路线或源码服务，不由 boss 复制答案模拟协作；Skill 更新不等于运行时升级。

## 工具

| 工具 | 用途 |
| --- | --- |
| `start_agent` | 私聊：一个 CLI 做一项任务，结果只回给 boss |
| `run_round` | 会议室：不带 `group_id` 时用 `title`、`members` 新建；带 `seats` 跑一轮，答案自动在席位间共享；不带 `seats` 只发一条 boss 公开发言 |
| `wait` | 等任务 `invocation_id` 和轮次 `round_id`，任一就绪即返回；省略 `ids` 等收件箱 |
| `cancel` | 取消任务或轮次 |
| `decide_answers` | `review=true` 的会议室：逐席公开或不公开 |
| `list` | 本服务的句柄、保存的会话（最近在前）和只读看板链接 |
| `read_conversation` | 按 App 的样子读回一个会话的最终消息 |
| `list_members` | App 组员与默认值；传 `provider` 时附该 CLI 的允许状态和可用选项 |

## boss 会话归属

每次 `start_agent`、`run_round` 都传 `manager_session_id`，同一 boss 聊天始终复用一个值，App 靠它读出 boss 模型。本聊天首次调用前在 shell 里取一次宿主会话 ID：Claude Code `$CLAUDE_CODE_SESSION_ID`、Codex `$CODEX_THREAD_ID`、Antigravity `$ANTIGRAVITY_CONVERSATION_ID`、CodeBuddy 与 WorkBuddy `$CODEBUDDY_SESSION_ID`、Grok `$GROK_SESSION_ID`、Pi `$PI_SESSION_ID`、Hermes `$HERMES_SESSION_ID`；其他宿主用上下文明确给出的会话 ID，都没有才生成一次 UUID 并沿用。新聊天用新值。它与子 CLI 返回的 `session_id` 分开，CLI 名称、项目路径和 MCP 连接 ID 都不能替代。缺少它时服务先用按聊天注入的 `AGENTNAVE_HOST_SESSION_ID`，再用 Claude Code 随 MCP 进程给出的 `CLAUDE_CODE_SESSION_ID`；该值在 `/clear` 等不重启 MCP 的切换后可能是旧聊天的，所以仍要每次显式传入；都没有时服务拒绝调用。

本聊天第一次 `start_agent` 或 `run_round` 的 `title` 也是 App 里这个工作区的名字，之后不再变：用它概括本聊天要做的事，写短（最多 48 列，中文算 2 列，约 24 个汉字或 48 个英文字符）。之后的私聊可不传 `title`；会议室照常各起一个短标题。

可信宿主上下文（如系统提示）写明当前模型 ID 时作为 `manager_model` 传入，未知才省略，不从 CLI 默认配置或组员模型猜测。只有宿主明确为每段聊天独立注入时才使用 `AGENTNAVE_HOST_SESSION_ID`；共享的全局 MCP 配置不写固定聊天 ID。历史属于其他 boss 时，用子 CLI 的 `session_id` 续接并新建当前 boss 的任务记录，不传另一 boss 的 `conversation_id`。

## 工位、续接与转发

每条原生线程就是一个工位：新开的私聊任务是新工位，用 `session_id` 续接仍是原工位；会议节目的每个席位也是一个工位。`role` 是可选的显示标签，不影响身份。

- 续接：`start_agent` 默认开新线程。接着同一线程干活时，传上次结果返回的 `session_id`，不能拿 `invocation_id` 当会话 ID；上下文里找不到时，用 `read_conversation` 读回任务的 `session_id`。
- 同时运行上限：私聊任务与会议席位合计受 App 设置限制（默认 8），超出时调用被拒绝。等部分任务结束后再派，或减少并联席位；串联轮只占一个名额。
- 转发：私聊默认不带任何工作区上下文。用 `forward` 引用 `wait`、`run_round`、`decide_answers` 回执或 `read_conversation` 里的 `message_id`，接收方把它当数据。只有成功的任务和已提交的会议条目能转发。
- 同类宿主：与宿主同类的 CLI 被排除出私聊，只用于会议室；需要同类 CLI 执行任务时优先用宿主自带的子代理能力。
- 访问等级：`access` 声明 `read` 或 `write`，只记录并在 App 显示，不拒绝调用，也不改变 CLI 权限；真正的只读靠 Provider 原生权限。未声明 `read` 而同一 `cwd` 已有其他运行中的 CLI 时，回执带 `warnings`：并行写入要换独立 worktree，只读就补声明 `read`。

## 选择调用方式

选谁执行：用户点名优先，否则 boss 按 `list_members` 自定。

| 用户需要 | 调用方式 |
| --- | --- |
| 做任务、问意见、复核、分头做、择优、接力 | 一个或多个 `start_agent` + `wait`，结果由 boss 验收、需要时用 `forward` 交给下一位 |
| 各自答 | `run_round` 一轮 `parallel`，本轮答案互不可见 |
| 多轮各自答 | 连续 `parallel`，第二轮起收到上一轮全部答案 |
| 轮流聊 | `serial` 按名单发言，后者收到前者；多轮即多圈 |
| 先各自答再讨论、分歧收敛 | 先 `parallel`，再一轮或多轮 `serial`；轮数用户未指定时由 boss 定，汇报结论与保留的异议 |

常用组合只说明怎么调用；选谁、几轮、何时收手按用户的全局规则或由 boss 定：

- **交接**：`start_agent` 的 prompt 按下文「目录与交接」写成自足简报，保留原任务语义（只调查就写明不改文件）；写代码的交接先由 boss 建好 worktree 作 `cwd`。
- **顾问**：`access=read`，prompt 写明只分析、不改文件、给出建议与理由；追问用同一 `session_id` 续接。
- **先计划后实现**：先 `access=read`，prompt 写明只出计划（改动范围、步骤、风险与验证方式）、不改文件；boss 按已有授权核对计划，需用户决定的转达用户，确认后用同一 `session_id` 续接、`access=write`，写明按已确认计划实现及调整内容。
- **委员会**：`run_round` 先 `parallel` 各自给根因与方案，再 `serial` 互相回应；汇报共识、分歧及如何收敛。
- **实现＋复核**：实现者 `write` 用独立 worktree，复核者 `read` 看 diff 只报可执行问题；boss 用 `forward` 把复核结果交给实现者的 `session_id` 续接修复，再请复核者续接复查。

会议室每席沿用原生权限和工具、各自 `cwd` 与显式 `provider_options`，续接自己的原生会话；席位只共享已提交条目，私密指令只给对应席位。同组同时一轮；`run_round` 立即返回，用 `wait` 等。可单独 `cancel` 某席的 `invocation_id`，记缺席后串联轮继续。不自动重试，不设每席截止时间。并联写代码的席位各用独立 worktree。会议室保存为会话，可跨服务重启继续；运行中的进程和句柄不恢复。审核、主持人、隔离和看板见[会话与审核](references/studio.md)。

## 讨论范围与工具

日常讨论保留 CLI 原生环境，用 prompt 约定范围；不因“讨论”“各自答”或“只看给定材料”自动启用 `restricted_tools`、`isolated`。派发时写清：允许查证时只读核验，要求只看给定材料时仅据此作答；只讨论和核验，不改文件、发送或部署；查证附出处，推测标注，声称执行成功时核对实际产物。仅在用户明确要求从工具层面限制访问时才设 `provider_options.restricted_tools=true`；同时要求服务准备独立席位目录时才用 `isolated=true`。

## CLI 与模型

用户未点名时先 `list_members`，按该模式的组员与说明选择；`model`、`effort` 等 CLI 设置放在 `provider_options` 里，写在顶层会被拒绝；未传时服务套用 App 默认值并在 `defaults_applied` 注明。首次调用某 CLI 前用 `list_members(provider=...)` 核对允许状态和支持参数并复用结果，它不检查安装、登录。

只读取所选 CLI 的参考文件，同一上下文复用，切换 CLI 再读新文件。

| CLI | `provider` | 参考 |
| --- | --- | --- |
| Claude Code | `claude` | [Claude Code](references/claude.md) |
| CodeBuddy Code | `codebuddy` | [CodeBuddy Code](references/codebuddy.md) |
| Codex CLI | `codex` | [Codex CLI](references/codex.md) |
| Grok CLI | `grok` | [Grok CLI](references/grok.md) |
| Antigravity CLI | `antigravity` | [Antigravity CLI](references/antigravity.md) |
| Pi Coding Agent | `pi` | [Pi Coding Agent](references/pi.md) |
| DeepSeek Harness | `deepseek` | [DeepSeek Harness](references/deepseek.md) |

用户指定字段覆盖默认，但不得超出允许范围；要求原生设置时须同时清除对应 App 默认字段。权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`、`approve_for_me`）只能来自用户在 App 中设定的默认 options、服务按 CLI 套用的权限档位（缺省「自动」，用户可在 App 改为「完全放开」；见 `list_members` 的 `permissions`），或用户本轮明确要求，boss 不得自行追加；显式传任一权限参数会让该 CLI 的档位整档不套用。App 默认值续接时也会套用。终态回执的 `warnings` 报告 CLI 报成功但有工具调用被拒，按未完成处理并告知用户可在 App 调整档位。模型不可用时报告原错误，不静默替换。

## 目录与交接

`cwd` 必须是目标项目存在的绝对目录，默认当前 checkout/worktree 根目录。CLI 不继承主对话；首次派发、切换 CLI 或交接未完成任务时，在 prompt 提供本次所需内容，简单任务省略无关项：

- 目标、验收标准、返回格式及产物位置。
- 目录、修改范围、授权边界、必须保留的用户改动。
- 已核实进展、验证、未完成项、阻塞和下一步；已定决策及理由，试过并放弃的做法。
- 可访问的规格、计划、ADR、Issue、commit、diff 或产物绝对路径/URL，注明用途。
- 直接相关的技能和已知路径；无适用项写“无”。

明确这是非交互子任务：已有授权内自主推进；确需新授权或用户选择时，以最终回答交回具体问题和进展，不调用现场交互提问工具，不绕过原生权限。发送前移除密钥、密码和敏感个人信息。长交接写进系统临时目录的专用文档，prompt 要求读取其绝对路径；创建方在接收方用完后清理。

## 回复预算

按交付需要和 boss 剩余上下文安排回复长度；多席按本轮合计预算分配。只查状态用 `wait(include_output=false)`，回读历史优先 `read_conversation` 默认结果和 `since_round`，长会话加 `limit` 只取最近若干条。每次调用保留的标准输出上限 1 MiB、stderr 64 KiB，超限报 `output_limit_exceeded` 并终止；长报告、日志或附件让 CLI 保存到约定位置，回复摘要和绝对路径。

## 等待、验收与取消

1. `start_agent` 保存 `invocation_id`，`run_round` 保存 `round_id`；持续等到终态或显式取消，不另加总时限。
   终态和阻塞不会主动推送：派发后立刻调用阻塞的 `wait`（单任务带 `ids`，多个用收件箱），转去做别的事时也始终挂着一个；`wait_elapsed` 后立即再挂。宿主把长调用转入后台的（如 Claude Code 超过 2 分钟），结束时会提醒 boss。`start_agent`／`run_round` 回执里的 `unreceived` 列出已结束、遇到阻塞或待审、但还没被 `wait`／`cancel` 取走的任务和轮次，见到就先用 `wait` 收取处理。向用户汇报进行中的任务前，先用 `wait(wait_seconds=0)` 核实状态。
2. `wait(ids)` 每次最多等 5 分钟（`wait_seconds` 可缩短，`0` 只看一眼不阻塞），任一就绪即返回 `reason=ready`：`updates` 是就绪的任务，`rounds` 是就绪的轮次，其余继续运行，下次再等；已终态的每次都算就绪，处理后从名单移除。`reason=wait_elapsed` 返回每项运行快照。同时派出多个时可用收件箱：省略 `ids`、带上本聊天的 `manager_session_id`，等本 boss 在本服务上所有尚未收到终态的任务和轮次，每个终态只送达一次（待审轮次在决定前一直算就绪）；无待等项时立即返回 `reason=nothing_pending`。在 Codex `functions.exec` 返回 `Script running with cell ID ...` 后，只用 `functions.wait(cell_id)` 等该脚本完成；外层让出控制权不代表超时。
3. `status=running` 尚未结束。`execution_blocked` 按 `error.code` 决定继续等或取消：阻塞不停止 CLI，同类阻塞每个任务只提醒一次；非交互模式下单个工具调用被拒绝不算阻塞，CLI 常会换工具继续，终态再按结果核对；普通工具失败、重试、沉默不证明任务失败。
4. 运行中 `output` 只是最新公开正文尾部（最多 1000 字符）及其 `output_age_ms`，不含思考或工具参数；材料不足就报告未知，不凭此判定卡死。
5. 终态按 `status`、完整 `output`、`error` 和原生 `session_id` 验收；会议每席全文每轮只给一次，再看用 `read_conversation`。`succeeded` 不证明目标完成，须核对命令、产物或检查。

正文有待确认问题时，先核对现有授权，能回答就续接；需用户决定则转达具体问题和进展。`cancel` 返回终态，不撤销已发生的副作用；活跃调用不能追加消息，要纠偏或补充就 `cancel`，回执带 `session_id` 时用它续接（没有就重新交接），prompt 写明：上一轮已被打断；已执行的命令和改动不要重跑，先核对现状；先交出已有结论和进展，再按新要求继续。换 CLI 或开新线程时重新交接，不假定旧历史可见。
