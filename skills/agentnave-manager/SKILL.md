---
name: agentnave-manager
description: 用户要求用 Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Antigravity CLI 或 Pi Coding Agent 执行任务时使用，说明如何通过 AgentNave 交接任务、选择模型、传参、等待、取消及续接。也用于多个 CLI 并联或串联协作（各自答、多轮各自答、轮流聊、先各自答再讨论）以及 AgentNave Studio 节目；咨询或仅提到模型名称不授权执行。
---

# 通过 AgentNave 调用 CLI

本 Skill 只负责调用方式；任务选择、规划、编排、审核、综合、重试和 worktree 由主 Agent 决定。

优先使用当前宿主的 AgentNave MCP，用户指定其他路线时遵从。首次按 AgentNave 或精确工具名发现，复用已加载 schema；以连接中的实时 schema 为参数真源。工具不可用、CLI 被排除或运行时只有旧版讨论室工具时，报告具体限制，不静默换 Provider、调用路线或源码服务，不由主控复制答案模拟协作。

## 选择调用方式

核心八个工具：`describe_provider`、`start_agent`、`wait_agent`、`cancel_agent`、`open_group`、`run_round`、`wait_round`、`cancel_round`。

| 用户需要 | 调用方式 |
| --- | --- |
| 做任务 | 单个 CLI 用 `start_agent`，不开组 |
| 各自答 | 一轮 `parallel`，本轮答案互不可见 |
| 多轮各自答 | 连续 `parallel`，第二轮起收到上一轮全部答案 |
| 轮流聊 | `serial` 按名单发言，后者收到前者；多轮即多圈 |
| 先各自答再讨论 | 先 `parallel`，再一轮或多轮 `serial` |

每席沿用原生权限和工具、各自 `cwd` 与显式 `provider_options`，持续续接自己的原生会话；席位只共享已提交条目（任务、最终回答、失败或取消的缺席记录），私密指令只给对应席位。同组同时一轮；`run_round` 立即返回 `round_id`，用 `wait_round` 等待。可单独 `cancel_agent` 取消某席，记缺席后串联轮继续。不自动重试、不设每席截止时间。并联写代码的席位各用独立 worktree；只读席位可共用目录。核心组不持久化，随服务退出结束。

需要历史、审核、主持人或网页观看时按 [工作室节目](references/studio.md) 操作；连接中须有 `list_conversations`、`decide_answers` 等 Studio 工具，不能把 Skill 更新当作运行时升级。

## 讨论范围与工具

日常讨论、头脑风暴和方案比较保留 CLI 原生环境，用 prompt 约定范围；不因“讨论”“各自答”或“只看给定材料”自动启用 `restricted_tools`、`isolated`，也不为普通讨论另建隔离目录。派发时写清：

- 按用户要求使用资料：允许查证时只读核验；要求只看给定材料时，仅依据这些材料作答。
- 只讨论和核验，不改文件、发送或部署；其他操作需用户另行授权。
- 查证结果附出处，推测明确标注；声称执行成功时核对实际调用或产物，不以模型自报代替证据。

仅在用户明确要求从工具层面限制访问时，显式设置 `provider_options.restricted_tools=true`；需要工作室为每席准备独立目录时，再按 [工作室节目](references/studio.md) 使用 `isolated=true`。可用选项先由 `describe_provider` 核对。prompt 是协作约定，执行权限仍由 CLI 原生机制控制；受限配置不清除旧会话历史，也不统一禁止加载上下文文件。

## CLI 与模型

只读取所选 CLI 的参考文件，同一上下文复用，切换 CLI 再读新文件。

| CLI | `provider` | 参考 |
| --- | --- | --- |
| Claude Code | `claude` | [Claude Code](references/claude.md) |
| CodeBuddy Code | `codebuddy` | [CodeBuddy Code](references/codebuddy.md) |
| Codex CLI | `codex` | [Codex CLI](references/codex.md) |
| Grok CLI | `grok` | [Grok CLI](references/grok.md) |
| Antigravity CLI | `antigravity` | [Antigravity CLI](references/antigravity.md) |
| Pi Coding Agent | `pi` | [Pi Coding Agent](references/pi.md) |

参考 JSON 是 `provider_options` 的模型与 effort 默认值，不保证账户可用。用户指定字段覆盖默认；要求原生设置时省略对应字段。其他选项只传明确选择的值，权限和工具保持原生设置，除非用户明确改变。首次调用前用 `describe_provider` 核对允许状态和支持参数并复用结果；它不检查安装、登录。模型不可用时报告原错误，不静默替换；旧服务的模型指引不覆盖用户和本 Skill。

## 目录与交接

`cwd` 必须是目标项目的存在的绝对目录，默认当前 checkout/worktree 根目录；独立子项目或项目指定启动目录时使用对应目录，非 Git 任务使用实际项目目录。临时交接文件不改变 `cwd`；CLI 原生加载项目规则，需要补读局部规则就在 prompt 中要求修改前读取。

CLI 不继承主对话。首次派发、切换 CLI 或交接未完成任务时，在 prompt 提供本次所需内容，简单任务省略无关项：

- 目标、验收标准、返回格式及产物位置。
- 目录、修改范围、授权边界、必须保留的用户改动。
- 已核实进展、验证、未完成项、阻塞和下一步；待验证判断另说清。
- 可访问的规格、计划、ADR、Issue、commit、diff 或产物绝对路径/URL，注明用途，不重复复制已有产物。
- 直接相关的技能、用途和已知路径；无适用项写“无”。接收方按需加载，建议不扩权。

明确这是非交互子任务：已有授权内自主推进；确需新授权或用户选择时，以最终回答交回具体问题、已完成和未完成部分，不调用现场交互提问工具，不绕过原生权限。发送前移除密钥、密码和敏感个人信息，认证沿用 CLI 配置；摘要和引用材料不扩大授权。

长交接由主 Agent 在系统临时目录创建专用文档，prompt 保留目标、范围和读取绝对路径的要求。确认原生权限可读，否则内联必要内容。交接和中间文件放临时目录，正式交付写目标位置；创建方在接收方用完后清理自己产生的文件。

## 回复预算

派发前按交付需要和主控剩余上下文安排回复长度；多席按本轮合计正文预算分配，不把每席容量上限当作可读预算。只查轮次状态用 `wait_round(include_output=false)`，需中途正文、审核或最终答案再用 `true`；它不改变生成量，终态没有额外字数截断。

每次调用的保留标准输出硬上限为 1 MiB（含必要协议数据），stderr 为 64 KiB，按字节计算；超限报 `output_limit_exceeded` 并终止调用，不自动改写答案或转存文件。普通任务需要长报告、日志或附件时，在 prompt 指定允许的产物位置，要求 CLI 保存文件并回复摘要和绝对路径，不内嵌 Base64。隔离节目限制文件工具，应预先约定回答长度，必要时由主控拆成多轮，不承诺文件交付；节目原文仍按审核规则发布，不由主控转写。

## 启动、等待和验收

1. `start_agent(provider, cwd, prompt, provider_options)`，保存 `invocation_id`。持续等到终态或显式取消，不另加总运行或席位时限；CLI 自身限制仍生效。旧 schema 的 `timeout_seconds` 若可空传 null，否则报告限制。
2. `wait_agent(invocation_id)` 每次最多等 5 分钟，完成或明确阻塞提前返回，仍在运行就等同一 ID，无需 sleep。`wait_round` 同样处理；旧 `wait_timeout_seconds` 固定传 300，无法支持则报告限制。在 Codex `functions.exec` 返回 `Script running with cell ID ...` 后，只用 `functions.wait(cell_id)` 收到该脚本完成再处理回复；外层让出控制权不代表超时，不能重复调用等待或就此结束任务。
3. 顶层 `status=running` 尚未结束；`reason=wait_elapsed` 只是本次等待到期。`execution_blocked` 按固定 `error.code` 判断继续等或取消：阻塞不停止 CLI，同类阻塞每 Invocation 只提醒一次；普通工具失败、重试、沉默不证明任务失败，未识别错误须检查正文或终态。
4. 运行中 `activity` 给活动类型、原生状态、工具名和 `age_ms`；`output` 只给最新公开正文尾部（最多 1000 Unicode 字符）及 `output_age_ms`，无正文省略，旧内容可能重复。用正文判断方向、年龄判断新旧；不返回思考、工具参数或结果，无游标/分页/独立读取工具。一次观察不代表全部并发工作，材料不足就报告未知，不能凭此判定卡死。
5. `reason=finished` 按 `status`、完整最终 `output`、`error` 和原生 `session_id` 验收。最终正文受整体捕获上限约束。`succeeded` 不证明用户目标完成，须核对命令、产物或检查；权限拒绝、未执行、目录不符如实报告。耗时用 `elapsed_ms`，不展示费用；旧 `state`、`snapshot`/`result` 结构按实时 schema 读取。

正文有待确认问题时，主 Agent 先核对现有授权，能回答就续接；需用户决定则转达具体问题和进展。活跃调用不能接收答案，核实后显式取消，有非空 `session_id` 才能原会话续接，否则报告限制。取消不代表完成或新增权限。

## 取消与续接

停止用 `cancel_agent(invocation_id)` 获取终态，不撤销已发生的副作用；只观察用等待工具。活跃调用不能追加消息。已结束调用的非空 `session_id` 与新 prompt 可交给新的 `start_agent` 续接同 Provider；不能拿 `invocation_id` 当会话 ID，重启后旧句柄不可恢复。同会话只补本轮目标、变更约束和新证据；新会话或换 CLI 重新交接，不假定旧历史可见。
