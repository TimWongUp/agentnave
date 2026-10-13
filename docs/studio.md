# AgentNave Studio：可观看的多 CLI 节目

自 v0.15.0 起，节目、历史与看板从核心拆为独立发行包 `agentnave-studio`（入口 `agentnave-studio-mcp`）。它与核心 `agentnave` 同版本发布，包含全部核心工具，另加保存历史、可选审核、主持人发言、隔离席位和本机只读看板。只需要子代理时安装核心即可；取舍见 [ADR-0006](adr/0006-core-rounds-and-studio-split.md)。

## 工具

| 工具 | 说明 |
| --- | --- |
| `start_agent`、`wait_agent`、`cancel_agent`、`describe_provider` | 与核心相同；`start_agent` 另接受 `title`、`conversation_id`，任务自动保存为私有任务会话；`describe_provider` 另返回 `show_permitted` |
| `open_group` | 创建保存的节目：`title`、2–6 个席位、`cwd`，可选 `review`、`isolated`；返回的 `group_id` 就是会话 ID |
| `run_round`、`wait_round`、`cancel_round` | 与核心相同的并联／串联轮次；审核节目中 `wait_round` 返回 `review_needed`，席位状态出现 `pending_review`、`discarded` |
| `decide_answers` | `round_id` + `{seat_id: publish/discard}`，整批校验并保存后生效 |
| `post_host_message` | 轮次之间以主持人身份发布公开发言 |
| `list_conversations`、`read_conversation`、`update_conversation`、`delete_conversation` | 列表、详情、改名／结束／重开、删除已结束会话 |
| `open_dashboard` | 首次调用时启动本机只读看板并返回首页地址 |

旧版的 `open_discussion`、`start_discussion_turn`、`decide_discussion_turn`、`decide_discussion_round`、`mark_discussion_absent`、`continue_discussion`、`reset_discussion_seat`、`open_workbench` 已移除：点名发言由串联轮取代，盲答揭晓由并联轮加审核取代，缺席自动记录，失败后的会话不确定由游标规则自动新开，换身份请新建节目。

## 一档节目怎么跑

1. `open_group(title, seats, cwd, review=true)` 开节目，将返回的首页或导演地址给用户观看；参演 CLI 只接收公开条目。
2. 需要时 `post_host_message` 开场；`run_round(group_id, seats, order, prompt, instructions)` 出题。并联轮里所有人同时独立作答；串联轮按名单依次作答、后者看得到前者。私密指令只发给对应席位。
3. `wait_round` 返回 `review_needed` 时，用 `read_conversation` 或返回的 `output` 查看待审回答，再 `decide_answers`。并联轮全员结束后一起审，全部决定后按名单一次公开；串联轮逐席审，决定后才轮到下一席。
4. 被丢弃的回答不会进入任何其他席位的输入；该席自己的原生会话仍记得它。失败、受阻、取消自动记为缺席，不需审核。
5. 有待审回答时不能开新轮次、发主持人消息或结束会话。`cancel_round` 与服务退出时，已决定发布的回答照常公开，未决定的丢弃。

不审核（`review=false`）时，节目与核心协作轮次的行为完全一致，只是多了保存与看板。

`wait_round(include_output=false)` 可只读状态，省略回答与运行尾部；默认 true 保持返回正文，待审或已结束的答案均能再次读取。审核只控制条目公开，不回滚文件修改或清除作者原生记忆。同一服务内，节目席位与普通任务不能同时续接同一 Provider 的同一原生会话。轮次驱动或写盘失败时，轮次状态为 failed 并附 error，不等同于正常编排结束的 finished。

## 隔离席位

`isolated=true` 时，工作室在会话目录下为每席创建独立空目录，为 Antigravity 写入受限角色定义，并给每席加上 `restricted_tools`；调用方的 `cwd` 被忽略，席位选项只接受 `model`、`effort`（Pi 另可选 `provider`）。隔离节目可包含被 `AGENTNAVE_EXCLUDED_PROVIDERS` 排除的 CLI，包括宿主自己的 CLI；非隔离节目与 `start_agent` 一样遵守排除项。

各 CLI 的原生限制（基于本机 CLI 帮助核验）：

| CLI | 原生限制 | 边界 |
| --- | --- | --- |
| Claude Code 2.1.295 | safe-mode、空 tools、strict-mcp-config、disable-slash-commands | 保留会话持久化；管理员策略仍可能生效 |
| CodeBuddy Code 2.142.0 | 空 tools、strict-mcp-config、显式空 MCP 配置、拒绝 mcp__* | 通过自己的 Claude-compatible resume 续接；不使用 Claude 专属 safe-mode 参数 |
| Codex CLI（本机桌面版 0.162.0-alpha.17.2） | ignore-user-config、关闭 shell/unified_exec/view_image/apps/multi_agent、禁用 web search、只读 sandbox 与 never approval | 保留认证与会话；不加载用户 MCP 配置。仍可能有基础工具定义或受管理配置，不声称完整无工具 |
| Antigravity CLI 1.3.2 | 每席工作目录中创建 tools=[]、mcpServers=[] 的专用 agent，关闭命令执行策略与 slash commands、开启 sandbox | --conversation 按本席 ID 续接；不同内容的同名 profile 会拒绝覆盖，不改全局设置 |
| Grok CLI 1.0.46 | 空 tools、deny `*`、no-subagents、disable-web-search、permission-mode dontAsk | deny 规则阻止工具执行；工具定义、原生通用规则及 MCP 连接仍可能存在 |

隔离是防作弊边界，不是操作系统沙箱：选手使用条目、自己的会话和自己的私密指令，不读取对手私密指令或导演内容；原生通用规则仍可能加载。私密指令只保证不被直接转发，席位若在回答中复述，会随公开回答传播，审核时应留意。

## 可见性

| 区域 | 可见内容 |
| --- | --- |
| 节目公开页 / `transcript.jsonl` | 席位资料与已提交条目：任务、主持人发言、公开回答、缺席 |
| 参演 CLI | 自己的原生会话、尚未收到的条目、本轮任务与自己的私密指令 |
| 导演页 | 公开页内容，另加本轮各席状态、私密指令、待审回答与运行中的回复尾部 |
| `read_conversation` / 数据目录 | 完整会话状态，含原生会话 ID；只给主控 |

网页都不是控制端，不显示原生会话或调用 ID。

## 看板

看板在成功创建或继续任务／节目后自动启动并返回入口；仅查看历史时可用 `open_dashboard`，随 MCP 进程退出；只监听 127.0.0.1 随机端口，用随机路径与精确 Host/Origin 检查限制访问，CSP 禁止外部加载。首页按“节目／做任务”筛选会话；节目页可在“公开对话”和“导演后台”之间切换，“选手看到的页面”打开公开地址；任务页显示各次调用的状态、模型、目录、耗时与回复，“已回复”不等于已验收。重启后地址会更换。

界面采用横向角色筛选、原创角色头像和群聊气泡；支持按角色查看、复制单条台词、回到最新发言、导出完整公共 JSONL。角色筛选只影响当前浏览器画面，不改变选手收到的公共历史；导出不随筛选裁剪。头像是原创插画，不代表厂商官方形象，也不意味着尚未接入的模型已可用。

交互参考 [SillyTavern 角色管理](https://docs.sillytavern.app/usage/characters/) 和 [群聊](https://docs.sillytavern.app/usage/core-concepts/groupchats/) 的头像、角色侧栏、共享聊天历史和手动点名概念。每次只向当前选手提供自己的私有设定，不采用合并全员私有角色卡的方式。没有复制 SillyTavern 代码，也没有引入其角色卡、插件或自动轮次引擎。

## 保存与恢复

数据根默认 `~/.agentnave`，可用 `AGENTNAVE_DATA_DIR` 指定绝对路径；首次保存会话时才创建。每个会话一个目录，含原子替换的 `state.json` 与 `.lock` 文件锁。首次写入时取锁并持有到进程退出；其他服务可读，在 `list_conversations` 中显示 `writable=false`，写入或续接直接报错，不抢锁。

重启后节目按保存的条目、席位会话与游标继续；运行中的席位记为中断并在下次新开会话补全条目，被中断的轮次不续跑。崩溃时停在待审的回答仍可用 `decide_answers` 决定。运行中的任务记为中断，旧 `invocation_id` 不恢复，原生 `session_id` 保留。

`delete_conversation` 只删除已结束且无进行中工作的会话，调用方须先取得用户授权：删除 `state.json` 与已结束的调用句柄，移除隔离席位中工作室写入的角色定义和空目录，保留会话目录及 `.lock`，Provider 原生文件不删。v0.15.0 之前的旧历史（数据根下的 `catalog/` 与含 `room.json` 的目录）不迁移，只在列表中标为不可用，可手动清除。

看板无人浏览时停止周期性全量扫描和运行投影刷新；隐藏标签页暂停轮询，恢复可见时重新读取。历史读取失败会保留已有画面并提示，读取恢复后清除提示。进入主控默认打开导演后台，公共页始终只含已提交条目。
