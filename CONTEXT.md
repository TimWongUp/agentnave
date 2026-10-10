# AgentNave 领域语言

## AgentNave

供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用，负责把统一的 Invocation 生命周期工具映射到不同 Provider CLI。
_Avoid_: CLI 产品、多智能体运行时、工作流引擎

## Manager

AgentNave 的唯一用户。Manager 解释目标，选择 Provider，决定角色、模型、并行、重试、审核、综合与工作树策略。

## Provider

AgentNave 可启动的本地 CLI 智能体。目前支持 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 与 Pi Coding Agent。

## Invocation

一次 Provider 进程调用。它由 prompt、cwd、可选 Provider Session 和显式 Provider Options 构成，只在当前 AgentNave 进程内拥有句柄。

## Provider Session

Provider 原生会话标识。Manager 可把完成结果返回的 `session_id` 传给新的 Invocation，以继续同一 Provider 对话；它不是 AgentNave 的持久会话。

## Invocation Result

AgentNave 对 Provider 终态的归一化结果；`output` 只承载 Provider 最终回答，不包含流式过程播报。状态只能是 `succeeded`、`failed`、`blocked` 或 `cancelled`。

## Invocation Snapshot

AgentNave 对运行中 Invocation 的粗粒度观察，描述生命周期阶段、耗时、最近原生事件和可识别活动及其年龄；单条活动是有来源的观察，不代表所有并发工作的当前状态、完成比例或终态证据。

## Discussion Room

Manager 显式创建的本地讨论会话，保存席位和按序公开台词，支持技术讨论或节目对话。一个房间目录对应一档节目；跨节目不共享公开记录、轮次或选手会话。它不决定谁发言或如何裁决。

## Director

Manager 在讨论室中的职责：配置席位、下达本轮私有指令、发布或丢弃候选台词。导演权限不交给参与者。

## Seat / Turn / Candidate

Seat 是导演指定的 Provider、模型及显示名称。Turn 是该席位原生会话中的一次发言，只补充公共发言区的未读台词和本轮指令。Candidate 是最终回答中通过结构校验的 `public_text`，只有导演确认后才成为公开 Message；同房间同席位续接自己的原生会话，不转发其他席位的隐藏推理。

## Public Board / Director Desk

公共发言区是所有席位共用的公开 Message 记录，提供本机只读页面和 JSONL；不含私有指令、候选稿、调度状态或会话详情。席位适配器从同一记录读取各自尚未收到的台词。

导演工作台使用独立地址，同步同一份公开 Message，并另行显示本轮指令、候选稿和运行状态。其地址只给导演，不给参演 CLI。导演台可切换当前服务已打开的节目；节目列表与其他节目的地址不进入公共视图。两个页面都不是 MCP 控制端。

## Seat Session

每个房间的每个席位独占一个原生会话、固定 cwd 和已读消息 ID 集合。正常轮次只增量补充公开发言；重启恢复这些绑定。换身份或失败后原生历史不确定时，导演显式重置指定席位；不自动新开会话，也不删除公开记录或其他席位的历史。

### Host（主持人）

导演在公共区的发言身份，不占选手席位、不创建额外模型会话。公开 Message 的稳定 speaker 键为 `director`，页面展示为“主持人 · 导演扮演”。后台指令和主持人公开台词使用不同接口；主持人台词与选手台词共享同一公开记录。

## Discussion Provider Permission

节目模式支持全部已注册 CLI，包括宿主自己的 CLI；普通 start_agent 的排除设置保持原样。describe_provider 的 permitted 与 discussion_permitted 分别表达这两个作用域。选手使用专属原生会话，不继承主控聊天。

- **Blind Round（独立作答轮次）**：节目 `mode=blind` 时，CLI 有效回复由服务自动封存，不经过主控转述。全员提交后由导演一次性揭晓；封存答案只在导演投影中可见，收集期间固定公共记录。不同题目沿用本席原生会话，但已接触他人答案的旧会话不能作为该题的新盲测。

## Workbench / Task Conversation

工作台是主控的统一私有会话列表，界面用途名为“一起聊”“各自答”“做任务”。Task Conversation 保存普通 Invocation 的任务元信息、原生 session 绑定和最终输出；相同 conversation_id 只负责归组展示，不向其他 CLI 共享内容。TaskRun 的运行中尾部在内存刷新，旧 invocation_id 在重启后清空，保存的运行状态标为中断。已回复不等于任务已验收。普通任务保留原生工具权限及宿主排除项，不沿用讨论限制。

会话结束是 archived 状态，不删除历史；运行任务和未解决轮次不能直接结束。工作台记录索引及新会话默认位于 ~/.agentnave（可由 AGENTNAVE_DATA_DIR 指定），不会自动接管另一个服务已锁定的会话。Web 页面只读。

独立回答允许主控显式标记本轮未作答席位，必须先解决该席位的在途调用；部分揭晓明确标注结果不完整。揭晓后可单向转入一起聊，沿用原会话和共同历史，不提供反向恢复盲测语义。
