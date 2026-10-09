# 公共发言区与导演工作台

自 v0.11.0 提供。

所有选手共用一个公共发言区，只有已发布台词进入这份记录。导演工作台使用独立地址，同步公共台词，并单独显示本轮私有指令、候选稿和调度状态。两者不是互相复制的两份真源，而是同一房间数据的不同投影。

每个席位保持自己的原生 CLI 会话，下一轮补发未读公开发言和本轮指令。不会每轮新开对话，也不把其他席位的推理或私有指令发过去。

## 讨论与会话工具

1. `open_discussion(directory, title, seats, mode="discussion"|"blind")` 创建或重新打开房间，返回 `room.id`、`public_url` 和 `director_url`。省略目录时使用工作台固定保存位置；显式目录须为绝对路径；2–6 个席位，每席有 `id`、`label`、`provider`、`model`、`effort`。支持 `antigravity`、`claude`、`codebuddy`、`codex`、`grok`。显式节目模式允许宿主自己的 CLI 作为独立选手，普通 `start_agent` 的排除项不影响节目席位。重开提供原配置。`discussion` 逐条公开；独立答题必须选 `blind`，CLI 的有效答案自动封存，仅导演可见。
2. `list_conversations()` 列出本服务可打开的讨论及任务会话，仅供主控使用；普通任务没有公共地址。新节目用新的空目录；继续节目用原目录与原配置。
3. `post_discussion_message(room_id, text)` 发布面向全员、署名“主持人”的公开发言（由导演扮演，存储键仍为 `director`）。导演内部工作内容不要用此工具发送。
4. `start_discussion_turn(room_id, seat_id, instruction)` 点名发言。适配器自动从公共板选取该席位未读的台词，续接它自己的 session_id；`instruction` 只发给该席位，同时出现在导演台。
5. 用返回的 `room.turn.invocation_id` 调用原有 `wait_agent` 或 `cancel_agent`；随后 `read_conversation(room_id)` 查看导演状态与候选稿。
6. `decide_discussion_turn(room_id, turn_id, action="publish"|"discard")` 在公开讨论模式发布候选原文或丢弃；盲答有效答案由采集器自动封存，不调用此工具逐条公开。重复相同决定不会重复发言；改写应丢弃后让原席位重说。
7. `decide_discussion_round(room_id, round_id, action="publish"|"discard")` 统一揭晓或丢弃盲答轮次；`round_id` 来自导演状态的 `blind_round_id`。全员提交前不能揭晓，未完成轮次可整体丢弃。
8. `reset_discussion_seat(room_id, seat_id)` 显式解除单席位的会话绑定与已读记录。适用于换身份或失败后原生历史不确定；不影响其他席位，不删公共台词或原生 CLI 历史。先解决该席位正在运行或待发布的轮次。

每个房间同一时间只启动一个席位。公开讨论中，未处理候选稿前不启动下一轮或插入导演公告；独立作答中，有效回复自动封存后即可启动下一席。失败不自动重试或换模型。选手须输出仅含 `public_text` 的 JSON，额外字段、空白或非 JSON 都会使候选校验失败。

示例席位（模型为调用方显式指定，不是服务端默认值）：

```json
[
  {"id":"claude","label":"Claude","provider":"claude","model":"claude-opus-5-5","effort":"medium"},
  {"id":"grok","label":"Grok","provider":"grok","model":"grok-4.7","effort":"high"}
]
```

`read_conversation` 和 `director_url` 只给导演，不能转交参演 CLI。公开板的 `state`、`transcript.jsonl` 仅提供公开内容，可供其他只读集成使用。现有 CLI 由适配器直接从这份公共记录读取增量，无需开放文件、网页或导演 MCP 工具。

## 不同节目与宿主 CLI

一个房间目录就是一档独立节目。每档节目有自己的 room ID、公开记录、导演轮次、席位会话和已读位置；相同模型或席位名在不同节目中也不复用原生 session。工作台首页集中列出已打开的会话，可按用途、名称和结束状态筛选；各主控页面用“所有会话”返回首页。公共页面、公共 JSONL 和选手输入不包含其他节目名称、地址、台词或导演内容。新开节目会刷新所有导演台的列表，但不会改变正在进行的节目的轮次。

工作台在私有目录索引中保存显式打开的会话路径，重启后自动重开可取得锁的记录；网页地址会更换，台词和原生会话绑定不重置。被其他服务占用或无法读取的记录标记不可用，不自动接管。

`describe_provider.permitted` 仍表示普通 `start_agent` 的宿主许可，`discussion_permitted` 表示节目工具可用。节目允许五种已注册 CLI，包括主控同厂商 CLI；原生选手会话由节目创建，不继承主控当前任务。普通调用的宿主排除规则及全局配置均未修改。

## 独立作答与统一揭晓

同题测试用 `mode="blind"` 创建节目。主持人先公开题目，首次点名自动开始一轮盲答；收集期间固定公共记录，不能插入主持人消息或单独公开某人的答案。每位 CLI 直接输出自己的 `public_text`，采集器在有效终态自动把原文封存到 `pending_answers`，无需主控复制、转述或逐条收存。当前席位状态成为 `held` 后，可继续点名下一席；已提交者不能重复答题或重置会话。导演页面显示完整封存答案，公共 state / JSONL / 其他选手 prompt 均不包含它们。

全员收齐后，主控仅调用一次 `decide_discussion_round(..., action="publish")`，所有答案在同一次持久化和公共投影刷新中揭晓；不是逐人发送正文。失败、取消或无有效会话的席位不会算作已提交，仍须由导演处理。`discard` 可以结束未收齐的一轮，封存答案不会公开；它不清除选手自己的原生记忆。每轮使用独立 ID，过期决定不能操作下一轮。

同一节目后续题目仍续用各自原生会话，上一轮已揭晓内容属于共同历史。已经看过本题其他人答案的会话不能变回干净盲测；重测该题应新建节目或使用未接触该题答案的会话，而不是只把网页答案隐藏。公开讨论模式继续按原来的候选审核和逐条发布工作。

## 会话与已读位置

首次发言会收到基础规则和当前公共历史；续接只发送新公开消息、本轮指令及上次候选是否已发布。已确认发布的自身发言不再重复发送。丢弃候选不意味着该选手忘记自己说过什么，因此下次会告诉它上次候选已被丢弃。

成功终态保存原生 session_id 与已读消息 ID。每次启动前标记历史可能变更；若执行失败、取消、中断或保存失败，下一次拒绝静默续接，提示导演显式重置。正常重启恢复各席位原生会话和已读记录，不恢复过期 Invocation。若 CLI 未返回可续接的 session_id 或已删除原生历史，须由导演处理，不能悄悄开新会话重发全部内容。

重置后下一次创建新会话，并重新读到完整公共历史。需要全新节目场次时创建新房间。保持会话与减少应用层重复投递不等于所有历史 token 都免费；计费和原生缓存由供应商决定。

## 可见性与限制

| 区域 | 可见内容 |
| --- | --- |
| 公共发言区 / JSONL | 席位资料、已公开台词与主持人明确发布的公开发言 |
| 参演 CLI | 自己的原生会话、本轮指令、尚未收到的公开台词、公共板地址 |
| 导演工作台 | 同步公共台词，另加本轮私有指令、候选稿、状态 |
| 导演 MCP / 私有房间文件 | 完整房间状态及每席原生会话绑定 |

公开响应没有候选稿、内部指令、调度状态、session_id、Invocation ID 或导演地址。两个网页都不提供控制 API。字幕导出始终只含公开台词。

显式 `discussion_mode=true` 保留工具限制，只允许额外指定 model、effort；可以续接原生 session_id。基于本机 CLI 帮助核验的配置：

| CLI | 原生限制 | 边界 |
| --- | --- | --- |
| Claude Code 2.1.295 | safe-mode、空 tools、strict-mcp-config、disable-slash-commands | 保留会话持久化；管理员策略仍可能生效 |
| CodeBuddy Code 2.142.0 | 空 tools、strict-mcp-config、显式空 MCP 配置、拒绝 mcp__* | 通过自己的 Claude-compatible resume 续接；不使用 Claude 专属 safe-mode 参数 |
| Codex CLI（本机桌面版 0.162.0-alpha.17.2） | ignore-user-config、关闭 shell/unified_exec/view_image/apps/multi_agent、禁用 web search、只读 sandbox 与 never approval | 保留认证与会话；不加载用户 MCP 配置。仍可能有基础工具定义或受管理配置，不声称完整无工具 |
| Antigravity CLI 1.3.2 | 每席工作目录中创建 tools=[]、mcpServers=[] 的专用 agent，关闭命令执行策略与 slash commands、开启 sandbox | --conversation 按本席 ID 续接；不同内容的同名 profile 会拒绝覆盖，不改全局设置 |
| Grok CLI 1.0.46 | 空 tools、deny `*`、no-subagents、disable-web-search、permission-mode dontAsk | deny 规则阻止工具执行；工具定义、原生通用规则及 MCP 连接仍可能存在 |

防作弊的边界是：选手只使用公共发言、自己的会话和自己的私有指令，不读取对手私有指令、未发布草稿，以及导演的私有计划或工作台。原生 CLI 通用规则不视为作弊。每席使用独立固定 cwd；公共投影和原生工具限制不等于操作系统沙箱。导演也应检查候选台词是否夹带不应公开的本轮指令；结构校验不能替代语义判断。

## 酒馆式聊天界面

主持人是导演的公开身份，有独立头像与发言标识，不额外占用选手席位，也不启动另一个模型。`post_discussion_message` 的公开文本和选手台词进入同一记录，下一轮发给各席位；导演私有指令仍走 `start_discussion_turn`。

界面采用横向角色筛选、原创角色头像和群聊气泡；支持按角色查看、复制单条台词、回到最新发言、导出完整公共 JSONL。角色筛选只影响当前浏览器画面，不改变选手收到的公共历史；导出不随筛选裁剪。头像是原创插画，不代表厂商官方形象，也不意味着尚未接入的模型已可用。

交互参考 [SillyTavern 角色管理](https://docs.sillytavern.app/usage/characters/) 和 [群聊](https://docs.sillytavern.app/usage/core-concepts/groupchats/) 的头像、角色侧栏、共享聊天历史和手动点名概念。每次只向当前选手提供自己的私有设定，不采用合并全员私有角色卡的方式。没有复制 SillyTavern 代码，也没有引入其角色卡、插件或自动轮次引擎。

## 保存与运行

私有 `room.json` 保存公开记录、当前轮指令/候选稿和各席位会话/已读状态，以操作系统文件锁防止多宿主同时写入，用原子替换保存。不要公开整个房间目录。每席 cwd 跨 MCP 重启保留；重置只清理空目录，原生 CLI 的历史和文件由 CLI 自己管理。

网页只监听本机回环地址，按需启动，随 MCP 进程退出。重新打开房间会返回两个新地址。卸载不会删除用户房间或 CLI 会话。

## 验证

自动化验证覆盖共享公共历史、候选稿与指令不进入公开响应、导演台同步、每席固定 cwd 与 session_id、只投递增量、自身台词去重、丢弃回执、重启续接与显式重置，以及原有发布失败回滚、文件锁、原生工具限制、HTTP Host/Origin 和路由白名单。

本机真实验证：通过源码版 STDIO MCP，Claude Code 2.1.295 与 Grok CLI 1.0.46 各完成两次发言；第二次保留各自原生 session_id，公共区与导演台同步显示五条公开台词。浏览器确认公共区没有本轮私有指令，导演台有独立指令区。平台级对抗性文件隔离和其他操作系统实机验证不在已验证范围内。

补充实测：以全新的合成文件为目标，通过 AgentNave 的 `discussion_mode` 明确要求两种 CLI 读取文件，不在测试 prompt 中加入禁用工具的劝阻。Claude 会话没有文件/终端工具；Grok 实际请求 `read_file`，原生工具结果为 `Denied by permission policy: deny rule on any tool matching "*"`。两者输出均未含测试文件随机标记。这验证了本次原生权限行为，不代表覆盖所有插件、未来版本或同用户恶意进程。规则依据为 [Grok 官方权限文档](https://docs.x.ai/build/features/permissions)。

新增 CLI 的限制依据：[CodeBuddy CLI 参数](https://www.codebuddy.ai/docs/cli/cli-reference)、[Codex 配置参考](https://developers.openai.com/codex/config-reference/)、[Antigravity 角色工具列表](https://antigravity.google/docs/subagents/)。三种 CLI 均通过 AgentNave 对合成文件做读取尝试，输出未含随机测试标记；Codex 明确无文件/终端工具，Antigravity 专用空工具角色返回 BLOCKED，CodeBuddy 输出了未执行的 Read 调用文本。后者不能当作有效节目发言，节目仍须通过 public_text JSON 结构校验。

全员与分节目实测：五种 CLI 各完成两轮真实发言（GPT、Claude、Gemini、Grok、DeepSeek），第二轮均复用各自第一轮的原生 session_id，共产生主持人开场与十条选手台词。验证时普通调用排除了 codex，节目中的 Codex 席位仍正常完成两轮。旧节目原目录重新打开，新节目使用独立空目录；浏览器切换确认各自台词、席位和导演轮次分开，公共页面不显示节目列表或导演地址。Ruff、Pyright 与 127 项测试通过；这些结果来自本地源码版，尚未发布。

独立答题验证：五种 CLI 用全新的节目会话回答同一题，采集器逐席自动封存，整个收集期间公共 HTTP state 始终只有一条主持人题目。导演台显示 5 / 5 封存答案后，一次整轮发布使公共记录变为六条；答案正文没有经过主控复制或重写。回归覆盖封存答案不进入后答者输入、提前单发/整轮揭晓被拒绝、冻结题目、重启恢复、发布失败回滚、完整揭晓、过期决定和下一题原生会话续接。128 项测试及 Ruff、Pyright、JS 语法检查通过。

## 整合工作台

入口名为“工作台”，统一会话列表按“一起聊 / 各自答 / 做任务”区分，支持用途筛选、名称搜索和已结束历史筛选。节目详情以“公开对话 / 导演后台”切换，顶部“选手看到的页面”进入独立公共地址；公共页不提供返回私有工作台的入口。普通任务直接进入私有结果页。新建方式用可展开的指引和可复制示例说明，网页不伪装成调度入口。open_workbench 返回私有首页和实际保存位置；默认 ~/.agentnave，可由 AGENTNAVE_DATA_DIR 指定绝对目录。网页保持只读，主控通过 MCP 操作。

普通 start_agent 自动创建私有任务会话，title 命名新会话，conversation_id 将相关任务归组；程序直接保存 CLI 原文和终态。它不共享各任务上下文，不修改 CLI 原有权限。任务页展示状态、模型、工作目录、耗时与回复，明确“已回复”不是“已验收”。服务重启保存的运行记录改为中断，旧 invocation_id 不恢复；原生 session_id 保留供主控判断是否继续。未完成轮次或运行任务不能直接结束会话，update_conversation 可改名、结束或重开，均不删历史。

独立作答未收齐时，mark_discussion_absent 可明确记录某席未作答；先取消并等待它的在途调用完成，不能把活跃输出当缺席。部分揭晓自动声明结果不完整和未作答名单，迟到者不能补交到同一轮。揭晓后 continue_discussion 单向继续一起聊，不换原生会话，不支持把已曝光答案的历史恢复成盲测。

统一工作台验收：普通 Claude 与 CodeBuddy/DeepSeek 两次真实调用的原文自动进入同一私有任务会话，Claude 第三次调用复用原生 session 并正确回忆上一轮口令；主控没有转写正文。四个已有演示记录保留到固定数据根，原临时记录未覆盖或删除。浏览器核对三种用途统一列表、任务卡片和讨论页切换；公共页无工作台入口、任务正文或保存路径。129 项测试、Ruff、Pyright、JS 语法和差异检查通过；Skill 部署审计通过，317 个目标入口有效。新 Skill 使用运行时能力检查，正式宿主旧运行时尚未升级，当前为源码开发版验收。

工作台入口交互验收：用途计数、名称搜索、已结束筛选与空结果提示、复制新会话示例、返回首页、普通任务入口、公开对话/导演后台切换均经浏览器操作检查。合成的 1/2 封存轮次仅导演后台显示答案，公共页面保持空记录；正文中的 HTML 字符串按文本显示。窄窗口保留公共页面入口，已揭晓轮次显示“暂无待揭晓答案”；未重复发起真实 CLI。发行包包含 HTML、JS、CSS 和原创头像，CI 的安装后工具清单同步到 16 个工具。

对抗复核发现并修复连续写盘失败后的状态卡死：采集终态保存失败后，重置或改名若再次保存失败，仍保留可恢复的失败/中断终态，不回退成已结束调用的 running 状态。讨论轮次与普通任务均有组合故障回归，磁盘恢复后可继续操作，无需重启其他会话。
