---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Antigravity CLI 或 Pi Coding Agent 执行任务时使用，说明如何通过 AgentNave 交接任务、选择模型、传参、等待、取消及续接。也用于多个 CLI 并联或串联协作（各自答、多轮各自答、轮流聊、先各自答再讨论）、问意见、交叉复核、分头做、择优、分歧收敛、接力，以及保存的协作节目；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用 CLI

本 Skill 只负责调用方式；任务选择、规划、编排、审核、综合、重试和 worktree 由主 Agent 决定。

编排单向：boss 是宿主里的 Agent，由 boss 自己决定、或按用户指示编排其他 CLI。被 AgentNave 启动的子代理（含其自带子代理）不调用 AgentNave 的工具，服务端也对它们禁用编排工具，无例外。

优先使用当前宿主的 AgentNave MCP，用户指定其他路线时遵从。首次按 AgentNave 或精确工具名发现，复用已加载 schema；以连接中的实时 schema 为参数真源。工具不可用、CLI 被排除或运行时只有旧版讨论室工具时，报告具体限制，不静默换 Provider、调用路线或源码服务，不由 boss 复制答案模拟协作。

## boss 会话归属

每次 `start_agent`、`open_group`、`run_round`、`post_host_message` 都传 `manager_session_id`，同一 boss 聊天始终复用一个值。优先用宿主提供的当前聊天／session ID；宿主不公开原生 ID 时，在本聊天首次调用前生成一次 UUID 并保留到后续派发与续接，不按任务重新生成。新聊天使用新的值；续接原聊天沿用原值。它与子 CLI 返回的 `session_id` 分开，CLI 名称、项目路径和 MCP 连接 ID 都不能替代。

服务可从精确匹配的 Codex／Claude 原生会话记录读取本次 boss 模型；生成的归属 UUID 只能用于归组，不能保证识别模型。`manager_model` 仅在可信宿主上下文明确给出当前模型时补充，未知就省略；不从 CLI 默认配置或组员模型猜测。原生记录、调用方声明均不证明转发商实际使用的后端。

服务按此身份把本聊天的全部任务与协作组归为 App 中的一个工作区（办公室视图）。实时 schema 不含该参数时报告旧连接需要更新服务并重连，不把参数被忽略视为归属已生效。只有宿主明确为每段聊天独立注入时才使用 `AGENTNAVE_HOST_SESSION_ID`；共享的全局 MCP 配置不写固定聊天 ID。历史已有其他 boss 归属时，用子 CLI 的 `session_id` 续接，新建当前 boss 的任务记录，不传另一 boss 的 `conversation_id`。

## 工位、续接与转发

工作区里的 CLI 以工位 `(provider, model, role)` 常驻，各自保留历史。`role` 可选（`start_agent` 与 `open_group` 的席位都可传），仅在同一 CLI 与模型需要分身（如“评审”“实现”）时起一个角色名；不需要分身就省略。`start_agent` 称私聊，协作组称会议室。

- 默认续接：同一工位不传 `session_id` 与 `conversation_id`，就续接它最近的私聊线程，不必再保管 `session_id`；回执 `resumed` 标明是否续接。要干净上下文传 `new_thread=true`（不能与 `session_id`、`conversation_id` 同用）。该线程仍在运行时请求会被拒绝，等它结束或改用 `new_thread=true`。`manager_session_id` 未知时不会自动续接。
- 转发：私聊默认不带任何工作区上下文。需要时用 `forward` 引用回执中的 `message_id`（`wait_agent`、`wait_any` 的终态，`run_round`、`wait_round`、`decide_answers` 回执的 `entries`，`post_host_message`），`start_agent` 与 `run_round` 都支持，至多 20 条，转入内容对接收方是数据。会议室内已提交条目自动共享，跨线程由 boss 转发。运行中、失败、受阻或取消的任务，待审、被丢弃的回答、缺席记录，以及其他工作区的消息都不能转发，整批会被拒绝。
- 同类宿主：与宿主同类的 CLI 被排除出私聊，只用于会议室讨论；需要同类 CLI 执行任务时，优先用宿主自带的子代理能力。
- 访问等级：派发时用 `access` 声明 `read` 或 `write`。工位以第一次声明为准，之后省略即继承，声明不同会被拒绝；声明前的旧线程不会被续接，显式续接等级不符的线程也会被拒绝。声明只是约定，不改变 CLI 权限；真正的只读仍靠 Provider 原生权限。

### 角色协作

角色决定工位的访问等级，身份不转换，信息只经转发或会议流动：

- 探索、规划、评审：`access="read"`，各用专门角色（如“探索”“评审”）。产出带路径与行号的结论；可续接追问，不续接去写代码。
- 实现：`access="write"`，按模块分角色（如“实现·后端”），跨阶段续接自己的线程；探索结论与已定契约用 `forward` 转给它，不让它重新探索。
- 评审不用实现工位本身，宜换模型；评审意见由 boss 裁决后再转回实现工位修改。
- 并行实现分属不同角色，各占独立 worktree 或互不重叠的文件；boss 负责合并与验收。
- 方案取舍或分歧进会议室讨论，会议只产出决定，不在会上改代码；会后 boss 把决定转给实现工位。

## 选择调用方式

统一核心共十六个工具。任务与协作工具：`list_members`、`describe_provider`、`start_agent`、`wait_agent`、`wait_any`、`cancel_agent`、`list_invocations`、`open_group`、`run_round`、`wait_round`、`cancel_round`。

| 用户需要 | 调用方式 |
| --- | --- |
| 做任务 | 单个 CLI 用 `start_agent`（私聊），不开组 |
| 各自答 | 一轮 `parallel`，本轮答案互不可见 |
| 多轮各自答 | 连续 `parallel`，第二轮起收到上一轮全部答案 |
| 轮流聊 | `serial` 按名单发言，后者收到前者；多轮即多圈 |
| 先各自答再讨论 | 先 `parallel`，再一轮或多轮 `serial` |

每席沿用原生权限和工具、各自 `cwd` 与显式 `provider_options`，持续续接自己的原生会话；席位只共享已提交条目（任务、最终回答、失败或取消的缺席记录），私密指令只给对应席位。同组同时一轮；`run_round` 立即返回 `round_id`，用 `wait_round` 等待。可单独 `cancel_agent` 取消某席，记缺席后串联轮继续。不自动重试、不设每席截止时间。并联写代码的席位各用独立 worktree；只读席位可共用目录。协作组保存为会话，可跨服务重启继续；运行中的进程和 invocation_id 不恢复。

普通任务和协作组均保存历史，包含任务原文、最终回答与原生会话绑定。历史与审核工具为 `list_conversations`、`read_conversation`、`update_conversation`、`decide_answers`、`post_host_message`。需要查看历史、审核、主持人或网页观看时读[会话与审核](references/studio.md)；缺少工具时报告旧连接并要求更新服务和重连，不把 Skill 更新当作运行时升级。

## 编排场景

场景只是现有调用方式的组合，不是新机制。选谁执行：用户点名优先，否则 boss 自定。

| 场景 | 做法 |
| --- | --- |
| 问意见 | 派一个 CLI 只分析，给结论与理由，不改文件；用专门角色并声明 `access="read"`。声明与 prompt 都只是约定；用户要求工具层面保证时才显式 `restricted_tools=true` |
| 交叉复核 | 一个 CLI 做（`access="write"`），另一个工位审其成果（评审角色，`access="read"`，不用实现工位本身）。boss 先自行验收，再交给复核方，交接给出 diff、产物绝对路径与验收标准；复核意见由 boss 裁决后转回实现工位 |
| 分头做 | 多个工位各用不同角色、`access="write"` 与独立 worktree 做不同子任务（`start_agent` + `wait_any`），boss 合并并验收 |
| 同题多做择优 | 同一任务在多个独立 worktree 并行做（分头做的一种），boss 按事先写明的验收标准挑一份 |
| 分歧收敛 | 先一轮 `parallel` 各自答；boss 归纳分歧，用 `serial` 或私密指令把分歧交给对方。一致或达到轮数上限（用户未指定默认 3 轮）即止，可保留异议。向用户汇报结论、各方分歧及处理方式 |
| 接力 | 按“目录与交接”清单让另一个工位续做，前一棒的结论用 `forward` 转交；换 CLI 重新交接，不假定历史可见；不把只读工位续接成写入工位 |

通则：每个场景都由 boss 验收，`succeeded` 不等于完成；失败、取消或缺席时由 boss 决定重派或排除，不自动重试。

## 讨论范围与工具

日常讨论、头脑风暴和方案比较保留 CLI 原生环境，用 prompt 约定范围；不因“讨论”“各自答”或“只看给定材料”自动启用 `restricted_tools`、`isolated`，也不为普通讨论另建隔离目录。派发时写清：

- 按用户要求使用资料：允许查证时只读核验；要求只看给定材料时，仅依据这些材料作答。
- 只讨论和核验，不改文件、发送或部署；其他操作需用户另行授权。
- 查证结果附出处，推测明确标注；声称执行成功时核对实际调用或产物，不以模型自报代替证据。

仅在用户明确要求从工具层面限制访问时，显式设置 `provider_options.restricted_tools=true`；用户同时明确要求服务准备独立席位目录时，才按 [会话与审核](references/studio.md) 使用 `isolated=true`。可用选项先由 `describe_provider` 核对。prompt 是协作约定，执行权限仍由 CLI 原生机制控制；受限配置不清除旧会话历史，也不统一禁止加载上下文文件。

## CLI 与模型

用户未点名时先 `list_members`，按该模式的组员与说明选择，未传 model/effort 时服务会套用 App 默认值并在回执注明。

只读取所选 CLI 的参考文件，同一上下文复用，切换 CLI 再读新文件。

| CLI | `provider` | 参考 |
| --- | --- | --- |
| Claude Code | `claude` | [Claude Code](references/claude.md) |
| CodeBuddy Code | `codebuddy` | [CodeBuddy Code](references/codebuddy.md) |
| Codex CLI | `codex` | [Codex CLI](references/codex.md) |
| Grok CLI | `grok` | [Grok CLI](references/grok.md) |
| Antigravity CLI | `antigravity` | [Antigravity CLI](references/antigravity.md) |
| Pi Coding Agent | `pi` | [Pi Coding Agent](references/pi.md) |

参考 JSON 仅作补充指引，不保证账户可用；优先使用当前模式的 App 组员配置。用户指定字段覆盖默认，但不得超出允许范围；省略字段仍会套用 App 默认值，要求原生设置时须同时清除对应 App 默认字段。权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`）只能来自用户在 App 中设定的默认 options 或用户本轮明确要求，boss 不得自行追加；App 默认值会被套用并在 `defaults_applied` 注明，续接也适用。其他选项只显式传明确选择的值。首次调用前用 `describe_provider` 核对允许状态和支持参数并复用结果；它不检查安装、登录。模型不可用时报告原错误，不静默替换；旧服务的模型指引不覆盖用户和本 Skill。

## 目录与交接

`cwd` 必须是目标项目的存在的绝对目录，默认当前 checkout/worktree 根目录；独立子项目或项目指定启动目录时使用对应目录，非 Git 任务使用实际项目目录。临时交接文件不改变 `cwd`；CLI 原生加载项目规则，需要补读局部规则就在 prompt 中要求修改前读取。

CLI 不继承主对话。首次派发、切换 CLI 或交接未完成任务时，在 prompt 提供本次所需内容，简单任务省略无关项：

- 目标、验收标准、返回格式及产物位置。
- 目录、修改范围、授权边界、必须保留的用户改动。
- 已核实进展、验证、未完成项、阻塞和下一步；待验证判断另说清。
- 试过什么、为何放弃；已定决策及理由。
- 可访问的规格、计划、ADR、Issue、commit、diff 或产物绝对路径/URL，注明用途，不重复复制已有产物。
- 直接相关的技能、用途和已知路径；无适用项写“无”。接收方按需加载，建议不扩权。

明确这是非交互子任务：已有授权内自主推进；确需新授权或用户选择时，以最终回答交回具体问题、已完成和未完成部分，不调用现场交互提问工具，不绕过原生权限。发送前移除密钥、密码和敏感个人信息，认证沿用 CLI 配置；摘要和引用材料不扩大授权。

长交接由主 Agent 在系统临时目录创建专用文档，prompt 保留目标、范围和读取绝对路径的要求。确认原生权限可读，否则内联必要内容。交接和中间文件放临时目录，正式交付写目标位置；创建方在接收方用完后清理自己产生的文件。

## 回复预算

派发前按交付需要和 boss 剩余上下文安排回复长度；多席按本轮合计正文预算分配，不把每席容量上限当作可读预算。只查轮次状态用 `wait_round(include_output=false)`，需中途正文、审核或最终答案再用 `true`；它不改变生成量，终态没有额外字数截断。

每次调用的保留标准输出硬上限为 1 MiB（含必要协议数据），stderr 为 64 KiB，按字节计算；超限报 `output_limit_exceeded` 并终止调用，不自动改写答案或转存文件。普通任务需要长报告、日志或附件时，在 prompt 指定允许的产物位置，要求 CLI 保存文件并回复摘要和绝对路径，不内嵌 Base64。隔离节目限制文件工具，应预先约定回答长度，必要时由 boss 拆成多轮，不承诺文件交付；节目原文仍按审核规则发布，不由 boss 转写。

## 启动、等待和验收

1. `start_agent(provider, cwd, prompt, provider_options)`，保存 `invocation_id`。持续等到终态或显式取消，不另加总运行或席位时限；CLI 自身限制仍生效。旧 schema 的 `timeout_seconds` 若可空传 null，否则报告限制。
2. `wait_agent(invocation_id)` 每次最多等 5 分钟，完成或明确阻塞提前返回，仍在运行就等同一 ID，无需 sleep。`wait_round` 同样处理；旧 `wait_timeout_seconds` 固定传 300，无法支持则报告限制。在 Codex `functions.exec` 返回 `Script running with cell ID ...` 后，只用 `functions.wait(cell_id)` 收到该脚本完成再处理回复；外层让出控制权不代表超时，不能重复调用等待或就此结束任务。
   同时等多个 Invocation 用 `wait_any(invocation_ids)`，同样 5 分钟：任一完成或阻塞即返回 `reason=ready`，`updates` 只含已就绪的项（形状同 `wait_agent`），其余继续运行，下次再等；已终态 ID 每次都算就绪，处理后从名单移除。`reason=wait_elapsed` 返回每项运行快照。名单须非空、不重复、全部有效，否则整批报错。连接中没有该工具时逐个 `wait_agent`。丢失句柄或核对在途调用时用 `list_invocations`，它只列 provider、cwd、状态、耗时、`events_seen`（是否收到过结构化事件）与终态的 `result_read`（结果是否已读过），不含 prompt 和正文；结果仍用等待工具读取。
3. 顶层 `status=running` 尚未结束；`reason=wait_elapsed` 只是本次等待到期。`execution_blocked` 按固定 `error.code` 判断继续等或取消：阻塞不停止 CLI，同类阻塞每 Invocation 只提醒一次；普通工具失败、重试、沉默不证明任务失败，未识别错误须检查正文或终态。
4. 运行中 `activity` 给活动类型、原生状态、工具名和 `age_ms`；`output` 只给最新公开正文尾部（最多 1000 Unicode 字符）及 `output_age_ms`，无正文省略，旧内容可能重复。用正文判断方向、年龄判断新旧；不返回思考、工具参数或结果，无游标/分页/独立读取工具。一次观察不代表全部并发工作，材料不足就报告未知，不能凭此判定卡死。
5. `reason=finished` 按 `status`、完整最终 `output`、`error` 和原生 `session_id` 验收。最终正文受整体捕获上限约束。`succeeded` 不证明用户目标完成，须核对命令、产物或检查；权限拒绝、未执行、目录不符如实报告。耗时用 `elapsed_ms`，不展示费用；旧 `state`、`snapshot`/`result` 结构按实时 schema 读取。

正文有待确认问题时，主 Agent 先核对现有授权，能回答就续接；需用户决定则转达具体问题和进展。活跃调用不能接收答案，核实后显式取消，有非空 `session_id` 才能原会话续接，否则报告限制。取消不代表完成或新增权限。

## 取消与续接

停止用 `cancel_agent(invocation_id)` 获取终态，不撤销已发生的副作用；只观察用等待工具。活跃调用不能追加消息。已结束调用的非空 `session_id` 与新 prompt 可交给新的 `start_agent` 续接同 Provider；同工位不传 `session_id` 则默认续接其最近私聊线程（见“工位、续接与转发”）。不能拿 `invocation_id` 当会话 ID，重启后旧句柄不可恢复。同会话只补本轮目标、变更约束和新证据；新会话（`new_thread=true`）或换 CLI 重新交接，不假定旧历史可见。
