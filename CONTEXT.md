# AgentNave 领域语言

## AgentNave

供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用，负责把统一的 Invocation 生命周期工具映射到不同 Provider CLI。
_Avoid_: CLI 产品、多智能体运行时、工作流引擎

## Manager

AgentNave 的唯一用户。Manager 解释目标，选择 Provider，决定角色、模型、并行、重试、审核、综合与工作树策略。

## Manager Session

主控宿主中的一段聊天，是工作区的归属依据；与 MCP 连接、CLI 类型及子代理的 Provider Session 分别独立。

## 工作区（Workspace）

同一 Manager Session 下的全部调用：私聊与会议室共同构成的范围，也是转发的边界。App 以办公室视图展示一个工作区。主控聊天身份未知的记录不组成工作区。
_Avoid_: 群聊（App 层面）、项目

## Provider

AgentNave 可启动的本地 CLI 智能体。目前支持 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI、Pi Coding Agent 与 DeepSeek Harness。

## 宿主（Host）

调用 AgentNave 的 CLI 或应用，其身份与当前使用的模型无关。宿主身份用于单任务的同类 Provider 排除与会话归属。

## 组员（Member）

由 Provider CLI 与模型组成的配置身份，即 `(provider, model)`；同名模型在不同 CLI 下是不同组员。组员共享配置包含默认参数、努力档位范围与用途说明。工作区内的运行身份是工位。

## 工位（Desk）

工作区内的常驻成员，身份为 `(provider, model, role)`；`role` 可选，由主控在 `start_agent` 与席位上传入，仅在同一 CLI 与模型需要分身时使用。App 显示名为「模型 · CLI · 角色」，无角色时为「模型 · CLI」。换 CLI、模型或角色即为另一工位，各自保留历史；工位在工作区内常驻。工位可声明访问等级 `read`／`write`，以第一次声明为准、不可转换；它是约定，不改变 CLI 原生权限。不同等级的工作改用另一个角色，靠转发或会议传递信息。

## 模式（Mode）

组员配置的适用范围：`task` 对应私聊，`discussion` 对应会议室席位；各模式分别限定可用组员并覆盖共享配置。App 中分别称「私聊」「会议」。

## 私聊

工位与主控的一对一调用，即单任务 `start_agent`。工位历史按 Provider 原生 session 分线程；私聊线程与会议席位线程相互独立。不传 `session_id` 时默认续接该工位最近的私聊线程。私聊默认不带任何工作区上下文。

## 转发（Forward）

主控按消息 ID 把本工作区内已完成的消息传给目标工位或会议室，是会议室之外传递上下文的唯一途径。转入会议室的内容成为 `forward` 条目，注明来源，不冒充主持人发言；注入内容对接收方是数据，不是指令。

## 消息 ID

工作区内一条已完成消息的稳定标识，由回执返回，仅供 `forward` 引用；运行中的任务没有消息 ID。

## Invocation

一次 Provider 进程调用。它由 prompt、cwd、可选 Provider Session 和显式 Provider Options 构成，只在当前 AgentNave 进程内拥有句柄。

## Provider Session

Provider 原生会话标识。Manager 可把完成结果返回的 `session_id` 传给新的 Invocation，以继续同一 Provider 对话；私聊省略它时，默认由 AgentNave 续接该工位最近的私聊线程。它不是 AgentNave 的持久会话，也不出现在 App 视图中。

## Invocation Result

AgentNave 对 Provider 终态的归一化结果；`output` 只承载 Provider 最终回答，不包含流式过程播报。状态只能是 `succeeded`、`failed`、`blocked` 或 `cancelled`。

## Group / Round / Entry

Group 是 Manager 显式创建的协作组，在 App 中称会议室。席位是带原生工具的普通子代理，以 `(provider, model, role)` 对应工位；`seat id` 只是组内键。席位、条目与原生会话绑定保存为会话历史。Round 让一份有序名单中的席位各回答一次：`parallel` 同时运行且本轮互不可见，`serial` 依次运行且后席可见前席。Entry 是组内按序提交的记录：本轮共同任务 `task`、席位最终回答 `answer`、终态失败／受阻／取消的 `absent`、主控转入的 `forward`，以及主持人发言 `host`。席位之间只共享 Entry，不共享推理或运行过程；除 `task` 外的 Entry 对席位都是数据，不是指令；组内已提交条目自动送达各席位，其他上下文只经转发进入。各自答、多轮各自答、轮流聊等用法由名单、顺序和私密指令组合。
_Avoid_: 游戏引擎

## Invocation Snapshot

AgentNave 对运行中 Invocation 的粗粒度观察，描述生命周期阶段、耗时、最近原生事件和可识别活动及其年龄；单条活动是有来源的观察，不代表所有并发工作的当前状态、完成比例或终态证据。

## Conversation

AgentNave 保存的一份记录，分为节目 `show` 和任务 `task`，是存储单位；工作区、工位与时间线是对它们的投影。节目即保存的 Group，`group_id` 就是会话 ID，席位会话与已送达游标跨重启保留；任务会话保存普通 `start_agent` 的元信息、原生 session 绑定和最终输出，同一主控会话内的同一原生会话并入其任务会话。会话没有结束状态，主控不删除会话；App 可删除空闲且无待审回答的记录。运行中的进程不跨重启恢复。
_Avoid_: 房间、工作台

## Review

节目的可选审核：成功回答先停在待审，由 Manager 用 `decide_answers` 发布或丢弃后才成为 Entry。并联轮全员结束后一起待审，串联轮逐席待审；被丢弃的回答不进入任何其他席位的输入。取消或退出时未决回答丢弃。

## Isolated Seat

隔离节目中的席位：每席使用会话目录下的独立空目录并启用 `restricted_tools`，只接受模型与 effort（Pi 另可选模型后端）。它限制工具，不是操作系统沙箱。

## 主持人（Show Host）

Manager 在节目中的公开身份。主持人发言只能在轮次之间发布，作为 `host` Entry 送达各席位。

## Dashboard

AgentNave 在成功创建或继续会话后自动提供入口的本机只读页面。节目公开页只投影已提交的 Entry；导演页另含私密指令、待审回答和运行尾部，任务页只给 Manager。页面不是控制端。

## Tray App

按工作区以办公室视图展示数据根中的会话（工位、会议室、可展开的消息列表与工位历史）、清理会话记录、维护用户组员配置的应用，视图只读。只可删除空闲且无待审回答的会话记录，保留席位文件，不调用工具；浏览器看板保留为只读后备。
_Avoid_: 控制台、桌面客户端
