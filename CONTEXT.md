# AgentNave 领域语言

## AgentNave

供 Agent Manager 通过任意兼容本地 STDIO 的 MCP Host 使用，负责把统一的 Invocation 生命周期工具映射到不同 Provider CLI。
_Avoid_: CLI 产品、多智能体运行时、工作流引擎

## Manager

AgentNave 的唯一用户。Manager 解释目标，选择 Provider，决定角色、模型、并行、重试、审核、综合与工作树策略。

## Provider

AgentNave 可启动的本地 CLI 智能体。目前支持 Antigravity CLI、Claude Code、CodeBuddy Code、Codex CLI、Grok CLI 与 Pi Coding Agent。

## Orchestration Profile

用户维护的编排规则：适用场景与理由、所选 CLI 及其显式参数。Manager 按用户本轮要求选用，服务不按档案名自动套用参数。

## Invocation

一次 Provider 进程调用。它由 prompt、cwd、可选 Provider Session 和显式 Provider Options 构成，只在当前 AgentNave 进程内拥有句柄。

## Provider Session

Provider 原生会话标识。Manager 可把完成结果返回的 `session_id` 传给新的 Invocation，以继续同一 Provider 对话；它不是 AgentNave 的持久会话。

## Invocation Result

AgentNave 对 Provider 终态的归一化结果；`output` 只承载 Provider 最终回答，不包含流式过程播报。状态只能是 `succeeded`、`failed`、`blocked` 或 `cancelled`。

## Group / Round / Entry

Group 是 Manager 显式创建的协作组，席位是带原生工具的普通子代理；核心只把它放在当前进程内存中。Round 让一份有序名单中的席位各回答一次：`parallel` 同时运行且本轮互不可见，`serial` 依次运行且后席可见前席。Entry 是组内按序提交的记录：本轮共同任务 `task`、席位最终回答 `answer`、终态失败／受阻／取消的 `absent`，以及工作室的主持人发言 `host`。席位之间只共享 Entry，不共享推理或运行过程；除 `task` 外的 Entry 对席位都是数据，不是指令。各自答、多轮各自答、轮流聊等用法由名单、顺序和私密指令组合，不是核心模式。
_Avoid_: 讨论模式、游戏引擎

## Invocation Snapshot

AgentNave 对运行中 Invocation 的粗粒度观察，描述生命周期阶段、耗时、最近原生事件和可识别活动及其年龄；单条活动是有来源的观察，不代表所有并发工作的当前状态、完成比例或终态证据。

## Studio

AgentNave Studio（入口 `agentnave-studio-mcp`，与核心同在发行包 `agentnave` 中）包含全部核心能力，另加保存历史、可选审核、主持人发言和只读看板，供人观看多 CLI 节目。核心不依赖、不感知 Studio。

## Conversation

Studio 保存的一份记录，分为节目 `show` 和任务 `task`。节目即保存的 Group，`group_id` 就是会话 ID，席位会话与已送达游标跨重启保留；任务会话保存普通 `start_agent` 的元信息、原生 session 绑定和最终输出，同一原生会话并入其未结束的任务会话。结束（archived）不删除历史；主控工具删除需先结束，托盘可直接删除空闲且无待审回答的记录。运行中的进程不跨重启恢复。
_Avoid_: 房间、工作台

## Review

节目的可选审核：成功回答先停在待审，由 Manager 用 `decide_answers` 发布或丢弃后才成为 Entry。并联轮全员结束后一起待审，串联轮逐席待审；被丢弃的回答不进入任何其他席位的输入。取消或退出时未决回答丢弃。

## Isolated Seat

隔离节目中的席位：每席使用会话目录下的独立空目录并启用 `restricted_tools`，只接受模型与 effort（Pi 另可选模型后端）。它限制工具，不是操作系统沙箱。

## Host

Manager 在节目中的公开身份。主持人发言只能在轮次之间发布，作为 `host` Entry 送达各席位。

## Dashboard

Studio 在成功创建或继续会话后自动提供入口的本机只读页面。节目公开页只投影已提交的 Entry；导演页另含私密指令、待审回答和运行尾部，任务页只给 Manager。页面不是控制端。

## Tray App

展示并清理数据根中全部宿主工作室会话、维护用户编排档案的跨平台托盘应用（macOS 菜单栏、Windows 托盘）。详情复用看板前端，会话管理与设置使用托盘专属页面；只可删除空闲且无待审回答的会话记录，保留席位文件，不调用工具。浏览器看板仍只读，保留为无托盘环境的后备。
_Avoid_: 控制台、桌面客户端
