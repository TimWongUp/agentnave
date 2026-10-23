---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Antigravity CLI、Pi Coding Agent 或 DeepSeek Harness 执行任务时使用，说明如何通过 AgentNave 交接任务、选择模型、传参、等待、取消及续接。也用于多个 CLI 并联或串联协作（各自答、多轮各自答、轮流聊、先各自答再讨论）、问意见、交叉复核、分头做、择优、分歧收敛、接力，以及保存的协作节目；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用 CLI

本 Skill 只讲调用方式；参数细节以连接中的实时 schema 和工具说明为准。任务选择、规划、分工、审核、综合、重试和 worktree 由主 Agent 决定；用户若有固定的编排偏好，写在宿主的全局规则里，按那里执行。

编排单向：boss 是宿主里的 Agent，由 boss 自己决定、或按用户指示编排其他 CLI。被 AgentNave 启动的子代理（含其自带子代理）不调用 AgentNave 的工具，服务端也对它们禁用编排工具，无例外。

优先使用当前宿主的 AgentNave MCP，用户指定其他路线时遵从。工具不可用、CLI 被排除或连接里仍是旧版工具（如 `wait_agent`、`open_group`）时，报告具体限制并要求更新服务和重连，不静默换 Provider、调用路线或源码服务，不由 boss 复制答案模拟协作；Skill 更新不等于运行时升级。

## 工具

| 工具 | 用途 |
| --- | --- |
| `start_agent` | 私聊：一个 CLI 做一项任务，结果只回给 boss |
| `run_round` | 会议室：不带 `group_id` 时用 `title`、`members` 新建；带 `seats` 跑一轮，答案自动在席位间共享；不带 `seats` 只发一条 boss 公开发言 |
| `wait` | 等任务 `invocation_id` 和轮次 `round_id`，任一就绪即返回 |
| `cancel` | 取消任务或轮次 |
| `decide_answers` | `review=true` 的会议室：逐席公开或不公开 |
| `list` | 本服务的句柄、保存的会话（最近在前）和只读看板链接 |
| `read_conversation` | 按 App 的样子读回一个会话的最终消息 |
| `list_members` | App 组员与默认值；传 `provider` 时附该 CLI 的允许状态和可用选项 |

## boss 会话归属

每次 `start_agent`、`run_round` 都传 `manager_session_id`，同一 boss 聊天始终复用一个值。优先用宿主提供的当前聊天／session ID；宿主不公开原生 ID 时，在本聊天首次调用前生成一次 UUID 并一直沿用，不按任务重新生成。新聊天用新值。它与子 CLI 返回的 `session_id` 分开，CLI 名称、项目路径和 MCP 连接 ID 都不能替代。

`manager_model` 仅在可信宿主上下文明确给出当前模型时补充，未知就省略，不从 CLI 默认配置或组员模型猜测。只有宿主明确为每段聊天独立注入时才使用 `AGENTNAVE_HOST_SESSION_ID`；共享的全局 MCP 配置不写固定聊天 ID。历史属于其他 boss 时，用子 CLI 的 `session_id` 续接并新建当前 boss 的任务记录，不传另一 boss 的 `conversation_id`。

## 工位、续接与转发

每条原生线程就是一个工位：新开的私聊任务是新工位，用 `session_id` 续接仍是原工位；会议节目的每个席位也是一个工位。`role` 是可选的显示标签，不影响身份。

- 续接：`start_agent` 默认开新线程。接着同一线程干活时，传上次结果返回的 `session_id`，不能拿 `invocation_id` 当会话 ID；上下文里找不到时，用 `read_conversation` 读回任务的 `session_id`。
- 同时运行上限：私聊任务与会议席位合计受 App 设置限制（默认 8），超出时调用被拒绝。等部分任务结束后再派，或减少并联席位；串联轮只占一个名额。
- 转发：私聊默认不带任何工作区上下文。用 `forward` 引用 `wait`、`run_round`、`decide_answers` 回执或 `read_conversation` 里的 `message_id`，接收方把它当数据。只有成功的任务和已提交的会议条目能转发。
- 同类宿主：与宿主同类的 CLI 被排除出私聊，只用于会议室；需要同类 CLI 执行任务时优先用宿主自带的子代理能力。
- 访问等级：`access` 声明 `read` 或 `write`，只记录并在 App 显示，不拒绝调用，也不改变 CLI 权限；真正的只读靠 Provider 原生权限。

## 选择调用方式

选谁执行：用户点名优先，否则 boss 按 `list_members` 自定。

| 用户需要 | 调用方式 |
| --- | --- |
| 做任务、问意见、复核、分头做、择优、接力 | 一个或多个 `start_agent` + `wait`，结果由 boss 验收、需要时用 `forward` 交给下一位 |
| 各自答 | `run_round` 一轮 `parallel`，本轮答案互不可见 |
| 多轮各自答 | 连续 `parallel`，第二轮起收到上一轮全部答案 |
| 轮流聊 | `serial` 按名单发言，后者收到前者；多轮即多圈 |
| 先各自答再讨论、分歧收敛 | 先 `parallel`，再一轮或多轮 `serial`；轮数用户未指定时由 boss 定，汇报结论与保留的异议 |

会议室每席沿用原生权限和工具、各自 `cwd` 与显式 `provider_options`，续接自己的原生会话；席位只共享已提交条目，私密指令只给对应席位。同组同时一轮；`run_round` 立即返回，用 `wait` 等。可单独 `cancel` 某席的 `invocation_id`，记缺席后串联轮继续。不自动重试，不设每席截止时间。并联写代码的席位各用独立 worktree。会议室保存为会话，可跨服务重启继续；运行中的进程和句柄不恢复。审核、主持人、隔离和看板见[会话与审核](references/studio.md)。

## 讨论范围与工具

日常讨论保留 CLI 原生环境，用 prompt 约定范围；不因“讨论”“各自答”或“只看给定材料”自动启用 `restricted_tools`、`isolated`。派发时写清：允许查证时只读核验，要求只看给定材料时仅据此作答；只讨论和核验，不改文件、发送或部署；查证附出处，推测标注，声称执行成功时核对实际产物。仅在用户明确要求从工具层面限制访问时才设 `provider_options.restricted_tools=true`；同时要求服务准备独立席位目录时才用 `isolated=true`。

## CLI 与模型

用户未点名时先 `list_members`，按该模式的组员与说明选择；未传 model/effort 时服务套用 App 默认值并在 `defaults_applied` 注明。首次调用某 CLI 前用 `list_members(provider=...)` 核对允许状态和支持参数并复用结果，它不检查安装、登录。

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

用户指定字段覆盖默认，但不得超出允许范围；要求原生设置时须同时清除对应 App 默认字段。权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`）只能来自用户在 App 中设定的默认 options 或用户本轮明确要求，boss 不得自行追加；App 默认值续接时也会套用。模型不可用时报告原错误，不静默替换。

## 目录与交接

`cwd` 必须是目标项目存在的绝对目录，默认当前 checkout/worktree 根目录。CLI 不继承主对话；首次派发、切换 CLI 或交接未完成任务时，在 prompt 提供本次所需内容，简单任务省略无关项：

- 目标、验收标准、返回格式及产物位置。
- 目录、修改范围、授权边界、必须保留的用户改动。
- 已核实进展、验证、未完成项、阻塞和下一步；已定决策及理由，试过并放弃的做法。
- 可访问的规格、计划、ADR、Issue、commit、diff 或产物绝对路径/URL，注明用途。
- 直接相关的技能和已知路径；无适用项写“无”。

明确这是非交互子任务：已有授权内自主推进；确需新授权或用户选择时，以最终回答交回具体问题和进展，不调用现场交互提问工具，不绕过原生权限。发送前移除密钥、密码和敏感个人信息。长交接写进系统临时目录的专用文档，prompt 要求读取其绝对路径；创建方在接收方用完后清理。

## 回复预算

按交付需要和 boss 剩余上下文安排回复长度；多席按本轮合计预算分配。只查状态用 `wait(include_output=false)`，回读历史优先 `read_conversation` 默认结果和 `since_round`。每次调用保留的标准输出上限 1 MiB、stderr 64 KiB，超限报 `output_limit_exceeded` 并终止；长报告、日志或附件让 CLI 保存到约定位置，回复摘要和绝对路径。

## 等待、验收与取消

1. `start_agent` 保存 `invocation_id`，`run_round` 保存 `round_id`；持续等到终态或显式取消，不另加总时限。
2. `wait(ids)` 每次最多等 5 分钟，任一就绪即返回 `reason=ready`：`updates` 是就绪的任务，`rounds` 是就绪的轮次，其余继续运行，下次再等；已终态的每次都算就绪，处理后从名单移除。`reason=wait_elapsed` 返回每项运行快照。在 Codex `functions.exec` 返回 `Script running with cell ID ...` 后，只用 `functions.wait(cell_id)` 等该脚本完成；外层让出控制权不代表超时。
3. `status=running` 尚未结束。`execution_blocked` 按 `error.code` 决定继续等或取消：阻塞不停止 CLI，同类阻塞每个任务只提醒一次；普通工具失败、重试、沉默不证明任务失败。
4. 运行中 `output` 只是最新公开正文尾部（最多 1000 字符）及其 `output_age_ms`，不含思考或工具参数；材料不足就报告未知，不凭此判定卡死。
5. 终态按 `status`、完整 `output`、`error` 和原生 `session_id` 验收；`succeeded` 不证明用户目标完成，须核对命令、产物或检查。

正文有待确认问题时，先核对现有授权，能回答就续接；需用户决定则转达具体问题和进展。`cancel` 返回终态，不撤销已发生的副作用；活跃调用不能追加消息，要补充就取消后续接。换 CLI 或开新线程时重新交接，不假定旧历史可见。
