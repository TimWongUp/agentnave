# AgentNave：会话、审核与只读看板

任务、协作、历史与审核统一由 `agentnave-mcp` 提供，不再区分核心与工作室入口。既有数据根、会话 ID 和历史格式保持不变；见 [ADR-0016](adr/0016-single-core-and-mcp-entrypoint.md)。

## 工具

| 工具 | 说明 |
| --- | --- |
| `start_agent` | 私聊任务，结果只回给 boss；另接受 `title`、`conversation_id`、`role`、`access` 与 `forward`，自动保存为私有任务会话，默认开新线程，传 `session_id` 续接原线程；`access` 只记录 |
| `run_round` | 不带 `group_id` 时用 `title`、2–7 个 `members`、`cwd`（可选 `review`、`isolated`）开节目，返回的 `group_id` 就是会话 ID；带 `seats` 跑一轮，不带 `seats` 发主持人发言；`forward` 把已完成消息转入会议室 |
| `wait`、`cancel` | 同时接受任务 `invocation_id` 与轮次 `round_id`；审核节目中轮次返回 `review_needed`，席位状态出现 `pending_review`、`private` |
| `decide_answers` | `round_id` + `{seat_id: publish/private}`，整批校验并保存后生效 |
| `list`、`read_conversation` | 列出句柄与会话并启动看板；按 App 的方式读回一个会话的最终消息 |
| `list_members` | 只读组员工具；每次现读 `members.json`，按 task／discussion 返回生效配置、参数校验及宿主允许状态；传 `provider` 时另返回该 CLI 的 `permitted`、`show_permitted` 与支持参数 |

标题在创建时给出，之后只在 App 中改名；MCP 不提供改名工具。中继空闲探测使用的 `list_invocations` 仍可调用，但不在工具列表中（[ADR-0024](adr/0024-consolidated-tools-and-private-review.md)）。

旧版的 `open_discussion`、`start_discussion_turn`、`decide_discussion_turn`、`decide_discussion_round`、`mark_discussion_absent`、`continue_discussion`、`reset_discussion_seat`、`open_workbench` 已移除：点名发言由串联轮取代，盲答揭晓由并联轮加审核取代，缺席自动记录，失败后的会话不确定由游标规则自动新开，换身份请新建节目。

## 一档节目怎么跑

1. `run_round(title, members, cwd, review=true)` 开节目；用户在 App 中观看，参演 CLI 只接收公开条目。
2. 需要时 `run_round(group_id, prompt)` 以主持人身份开场；`run_round(group_id, seats, order, prompt, instructions)` 出题。并联轮里所有人同时独立作答；串联轮按名单依次作答、后者看得到前者。私密指令只发给对应席位。
3. `wait` 对轮次返回 `review_needed` 时，用 `read_conversation` 或返回的 `output` 查看待审回答，再 `decide_answers`。并联轮全员结束后一起审，全部决定后按名单一次公开；串联轮逐席审，决定后才轮到下一席。
4. 选择 `private` 的回答不进入会议室，也不会进入任何其他席位的输入；它留在 boss 与该席之间，App 显示为该工位给 boss 的「🔒 未公开」回复。该席自己的原生会话仍记得它，下一次轮到它时会被告知这一回答未公开。失败、受阻、取消自动记为缺席，不需审核。
5. 有待审回答时不能开新轮次或发主持人消息。`cancel(round_id)` 与服务退出时，已决定公开的回答照常公开，未决定的按不公开处理。

不审核（`review=false`）时，成功回答按并联／串联规则自动提交，历史照常保存。

`start_agent` 使用 task 配置，节目每席使用 discussion 配置。未传 model／effort／其他 options 时套用 App 中的组员默认值，回执 `defaults_applied` 列出补齐的键；显式参数优先，但模式模型名单与生效努力档位范围仍强制执行；模型名单非空、没有默认模型且调用方未传 model 时，拒绝并列出允许模型。续接同样现读并套用默认值。创建节目回执的各席另含 model、effort（未配置时省略）与 `defaults_applied`。模式缺失表示不限制，空模式对象表示禁止全部席位；宿主排除集合不限制 discussion，但 discussion 的组员名单仍生效。配置损坏时任务、节目与AgentNave Provider 查询都直接返回 App 修复错误，不提示修改调用参数。`show_permitted` 每次现读 discussion 是否允许该 CLI；文件缺失或 discussion 不受限时为 true。

`wait(include_output=false)` 可只读状态，省略回答与运行尾部；默认 true 保持返回正文，待审或已结束的答案均能再次读取。审核只控制条目公开，不回滚文件修改或清除作者原生记忆。同一服务内，节目席位与普通任务不能同时续接同一 Provider 的同一原生会话。轮次驱动或写盘失败时，轮次状态为 failed 并附 error，不等同于正常编排结束的 finished。

## 隔离席位

普通讨论默认保留原生环境，通过 prompt 约定资料范围和只读操作，具体见 [Skill 的讨论范围与工具](../skills/agentnave-manager/SKILL.md#讨论范围与工具)。仅在用户明确要求工具层面限制访问时使用 `restricted_tools`；用户同时明确要求AgentNave准备独立席位目录时，才使用 `isolated`。不因“各自答”或“只看给定材料”自动隔离。

`isolated=true` 时，AgentNave在会话目录下为每席创建独立空目录，为 Antigravity 写入受限角色定义，并给每席加上 `restricted_tools`；调用方的 `cwd` 被忽略；仅套用 model／effort 默认值（Pi 另可套用 provider），跳过其他默认 options，回执不列出被跳过的键；调用方显式非法选项仍拒绝。席位选项只接受 `model`、`effort`（Pi 另可选 `provider`）。节目（隔离与否）都可包含被 `AGENTNAVE_EXCLUDED_PROVIDERS` 排除的 CLI，包括宿主自己的 CLI；排除项只约束 `start_agent`。

各 CLI 的原生限制（下列版本已核对本机帮助与受限调用；精确参数以 Adapter 为准，版本或配置变化后复核受影响项）：

| CLI | 原生限制 | 边界 |
| --- | --- | --- |
| Claude Code 2.1.295 | safe-mode、空 tools、strict-mcp-config、disable-slash-commands | 保留会话持久化；管理员策略仍可能生效 |
| CodeBuddy Code 2.142.0 | 空 tools、strict-mcp-config、显式空 MCP 配置、拒绝 mcp__* | 通过自己的 Claude-compatible resume 续接；不使用 Claude 专属 safe-mode 参数。空 tools 与仅启用 Read 的初始化清单都列出全部工具；实际读取以调用记录和文件内容核对 |
| Codex CLI（本机桌面版 0.162.0-alpha.17.2） | ignore-user-config、关闭 shell/unified_exec/view_image/apps/multi_agent、禁用 web search、只读 sandbox 与 never approval | 保留认证与会话；不加载用户 MCP 配置。受限初次调用与续接均成功，本地读取和终端工具不可用，其他框架工具仍存在 |
| Antigravity CLI 1.3.4 | 每席工作目录中创建 tools=[]、mcpServers=[] 的专用 agent，关闭命令执行策略与 slash commands、开启 sandbox | --conversation 按本席 ID 续接；不同内容的同名 profile 会拒绝覆盖。单任务也可使用 restricted_tools，但 cwd 必须事先含匹配的角色定义，Adapter 不代写；定义见 [CLI 参考](../skills/agentnave-manager/references/antigravity.md#显式受限工具) |
| Grok CLI 1.0.46 | 空 tools、deny `*`、no-subagents、disable-web-search、permission-mode dontAsk | 工具定义仍包含 read_file 等工具；本次真实读取被 deny 规则拒绝，不能把限制执行说成移除定义。当前受限参数不禁用 MCP 连接 |
| Pi Coding Agent 1.1.0 | no-tools、no-extensions、no-mcp、no-skills、no-prompt-templates | 当前配置仍自动加载工作目录中的 AGENTS.md；同一 session_id 跨 cwd 续接失败，需回原目录续接 |

独立目录和工具限制不清除旧会话历史，也不统一禁止加载原生规则或上下文文件，不等于操作系统沙箱。切换受限配置不能把已接收外部材料的会话变成新的独立作答；需要排除旧历史时使用新会话。私密指令只保证不被直接转发，席位若在回答中复述，会随公开回答传播，审核时应留意。

## 可见性

| 区域 | 可见内容 |
| --- | --- |
| 节目公开页 / `transcript.jsonl` | 席位资料与已提交条目：任务、主持人发言、公开回答、缺席 |
| 参演 CLI | 自己的原生会话、尚未收到的条目、本轮任务与自己的私密指令 |
| 导演页 | 公开页内容，另加本轮各席状态、私密指令、待审回答与运行中的回复尾部 |
| `read_conversation` | 与 App 相同的最终消息时间线（任务含原生会话 ID），`include_private=true` 另含私密指令与不公开回答；只给 boss |
| 数据目录 | 完整会话状态；只给 boss |

浏览器看板不是控制端，不显示原生会话或调用 ID；改名与清理会话记录在托盘 App 的办公室里进行。

## 看板

看板只在 boss 调用 `list` 时启动并返回首页入口，之后 `read_conversation` 附带该会话的页面地址，随 MCP 进程退出；只监听 127.0.0.1 随机端口，用随机路径与精确 Host/Origin 检查限制访问，CSP 禁止外部加载。重启后地址会更换。

界面是左右两栏的工作台：左侧按“需要关注／其他会话”分组列出会话，可搜索并按节目、任务筛选；“需要关注”只收进行中与待审核的会话，失败、受阻或中断是已交给 boss 的结果，带红色状态留在其他会话中。右侧未选会话时是总览（进行中与待审核的会话），选中后显示详情，切换会话不离开页面。节目详情分“本轮现场”（私有：各席位私密指令、实时回复、审核状态）和“公开记录”（按轮分隔的公开发言），有工作进行或待审核时默认停在现场；“选手视角”打开公开地址。任务详情按时间倒序显示每次调用的状态、模型、目录、耗时、boss 派发的任务原文与回复，“已回复”不等于已验收。公开地址只显示公开记录，不含会话列表。跟随系统浅色／深色，窄屏下列表与详情分屏显示。

回复按常用 Markdown（标题、列表、引用、代码、粗体、链接）以纯文本节点渲染，长内容折叠可展开；支持按角色查看、复制单条台词或回复、回到最新发言、导出完整公共 JSONL。角色筛选只影响当前浏览器画面，不改变选手收到的公共历史；导出不随筛选裁剪。头像按模型家族使用官方图标，不意味着尚未接入的模型已可用。

交互参考 [SillyTavern 角色管理](https://docs.sillytavern.app/usage/characters/) 和 [群聊](https://docs.sillytavern.app/usage/core-concepts/groupchats/) 的头像、角色侧栏、共享聊天历史和手动点名概念。每次只向当前选手提供自己的私有设定，不采用合并全员私有角色卡的方式。没有复制 SillyTavern 代码，也没有引入其角色卡、插件或自动轮次引擎。

## 保存与恢复

数据根默认 `~/.agentnave`，可用 `AGENTNAVE_DATA_DIR` 指定绝对路径；首次保存会话时才创建。会话保存在数据根的 `agentnave.db`（SQLite），每个会话一把 `locks/<会话ID>.lock` 文件锁；不读取旧版本的会话目录，见 [ADR-0026](adr/0026-sqlite-conversation-store.md)。新任务与节目保存创建者的 `host`、`host_source` 与 boss 聊天身份 `manager_session`，后续接手不改写创建归属；旧记录缺失时兼容读取为 null／unknown／null。`start_agent`、`run_round` 的 `manager_session_id` 用宿主当前聊天 ID，同一聊天全部调用沿用，新聊天换值；宿主不公开原生 ID 时，boss 为该聊天生成一次 UUID 并复用。省略时读取仅限逐聊天注入的 `AGENTNAVE_HOST_SESSION_ID`，仍未知则拒绝调用。服务把宿主与原始 ID 确定性散列为带 `s-` 前缀的分组键，不保存原始 ID；见 [ADR-0017](adr/0017-manager-chat-session-identity.md)。视图快照与 live 会话摘要带同样归属，live 顶层另带当前服务的 `connection_id` 与开始时间，连接 ID 不用于聊天归并。

每次写入前取锁并在锁内重读数据库状态；运行中的轮次／任务、待审回答及未成功落盘的回写持续持锁，全部回写完成且空闲后释放，并清除写入缓存。持锁期间其他服务可读，在 `list` 中显示 `writable=false`，写入或续接直接报错；空闲后其他服务可取得同一把锁继续写入。若会话已被删除，下次写入会遗忘该会话并返回未知会话错误，不会重建历史。已知 boss 身份时，子 CLI 原生续接只自动关联本 boss 的任务记录，不能通过 `conversation_id` 并入另一 boss 的记录；无归属的旧记录仍可读取，用原生 `session_id` 续接时创建当前 boss 的新记录。

重启后节目按保存的条目、席位会话与游标继续；续轮与主持人发言须与创建归属一致（身份来自参数或逐聊天注入），无归属的旧节目只读，需新建节目；运行中的席位记为中断并在下次新开会话补全条目，被中断的轮次不续跑。崩溃时停在待审的回答仍可用 `decide_answers` 决定。运行中的任务记为中断，旧 `invocation_id` 不恢复，原生 `session_id` 保留。

会话没有“结束”状态，boss 也不删除会话；清理由用户在托盘 App 的办公室里右键进行。旧版本写入的 `archived` 字段读取时忽略。

托盘应用所需的数据由AgentNave写出：数据库 `views` 表中的视图快照，以及有工作运行时数据根下 `live/` 中的运行文件（结束即删除）；见 [ADR-0008](adr/0008-tray-app-reads-studio-snapshots.md)。

## 托盘应用（预览）

宿主接入后端由 App 管理用户级 MCP 与配套 Skill（[安装路径与手动方式](installation.md#3-register-the-mcp-server)）。启动/刷新自动为已有条目补宿主标识并修复缺失 launcher；新增接入、移除和入口切换走带操作请求头的 POST，GET 只读。宿主配置与 Skill 文件的备份分开计数，各保留最近 30 份，写后仅在文件仍等于本次写入字节时回滚，否则保留外部新内容并报告冲突；外部管理的 Skill 与项目级配置不改。`/app/hosts` 提供配置、Skill 与实测状态，`/app/onboarding` 按真实服务、宿主、组员和 boss 会话记录给出四步引导；关闭标记保存在 App 自有数据目录。宿主加载与真实调用须另行验收，配置成功不代表模型可用。见 [ADR-0015](adr/0015-app-host-integration.md)。

`apps/tray` 是可查看并清理会话记录的桌面应用（带菜单栏图标）（Tauri 2，目前先支持 macOS），不必启动 MCP 服务即可查看数据根中全部宿主的会话。菜单栏图标旁以「=N」显示正在运行的子代理数，空闲时不显示；悬停提示写明运行数。左键点击图标弹出 480×640 的面板，最多列出 5 个 boss：先是有工作在进行的，再是最近失败的，然后按最近更新补足，只要还有记录就不会空着。每个 boss 一张卡片：卡头是 boss（宿主图标、项目名、工位与会议室数、状态和更新时间），下面是它的工位格（macOS 图标式圆角头像、模型名旁的 CLI 文字标签、角色与状态原文）和会议室行（轮次与在座者）。有工作在进行或失败的 boss 显示为卡片，其余折成底部「其他办公室」的一行；都没有动静时最近的一个仍显示为卡片。工位先列忙碌（工作中、开会、失败）的，再列空闲的；超过 30 分钟没有工作的工位只计入「休息中」的数量；工位多时折叠。会议室只列进行中的和最近结束的一间。待审回答只是短暂过程，面板、菜单栏和主窗口的办公室都不单独显示待审数（会话消息里仍标「待 boss 审核」）；主窗口工位同样按 30 分钟进入休息；主窗口工位卡左上角是访问声明与当前状态，下方依次是模型名、CLI 与角色，说话气泡放在卡片右上角、boss 头顶，不压住头部。模型名默认去掉供应商路径（如 opencode-go/deepseek-v4.1-flash 写作 deepseek-v4.1-flash），可在设置「头像」页改为完整名称，各窗口共用这一选择。作答的工位写「会议里已作答」。点工位或会议室直接打开主窗口对应位置。设置、检查更新和退出收在页脚「⋯」菜单里。没有 boss 时区分尚无数据与记录已清理。

主窗口默认页为 `/chat/`，左侧是工作区（boss 会话）列表，中间是该工作区的办公室（[ADR-0018](adr/0018-office-workspace-desks-and-meeting-rooms.md)），右侧是对话栏。工作区标题取本聊天第一个会话的标题，之后不随新会话改变；第一个标题是未起名的「<CLI> 任务」时顺延到下一个，都没有时用项目名（取会话工作目录的文件夹名，Agent worktree 归到所在仓库）。项目名不是标题时显示在副标题里。三栏之间的分隔线可拖动调宽：工作区列表占窗口宽度 14%–28%，对话栏 22%–42%，双击恢复默认，宽度会记住；办公室右边缘的按钮收起或展开对话栏，点工位或会议室时自动展开。办公室是手绘场景：boss 办公室里 boss 坐在电脑前，桌上放待审回答；每个工位（「模型 · CLI · 角色」）常驻，人物和笔记本盖子上的标志都按模型家族选取，没有对应形象的模型用默认形象和终端标志；请求时没指定模型、CLI 也没回报的工位显示「默认模型」。工位按最新在前排列，用人物神态和一行文字标出私聊处理中、去开会、待审、失败、空闲或打盹；空闲的工位保持清醒，直到办公室在它之后又开出新工位（同一会议室的席位不算）才打盹，与宿主同类的工位标「仅开会」。boss 办公室占满一行，boss 居中，左侧「最近对话」按进行中优先列出工位并标出处理中、失败等状态，右侧「会议」按进行中优先列出会议室并标出待审，点一条切到对应工位或会议室。进行中的会议室在其下方通栏画出会议桌，席位人物与工位人物同样大小，人物会从工位走到席位；结束的会议室只留在「会议」列表里。任务刚完成的工位短暂显示完成神态；角色画了可选的沮丧、走路帧时，失败显示沮丧神态，去开会时迈步走过去。点工位、会议室或 boss 办公室，右侧对话栏切换到对应范围：消息头写明「谁 对 @谁 做什么」，`@boss`、`@all` 是仅有的保留称呼，私密指令带锁、只给一席，待审回答写「等 boss 审核」，部分席位作答时注明本轮回答者，被丢弃的回答折叠，转发显示「↪ 转发自 …」并可跳到来源，更早的消息超出上限时注明省略条数。底部动态条显示最近一件事。主窗口与面板支持跟随系统、浅色和深色三种外观。人物动画只用变换和透明度，窗口隐藏时暂停，系统要求减少动态效果时只切换状态。办公室和设置共用顶部导航。菜单栏图标的右键菜单有“打开 AgentNave”、“设置…”、“检查更新”和“退出 AgentNave”；首次启动打开办公室默认页，已有窗口时再次启动或选“打开 AgentNave”只恢复该窗口。App 在 macOS 上有程序坞图标，可固定在程序坞；关闭窗口只是隐藏、不退出，点程序坞图标按原页面恢复，最小化同样如此；菜单栏图标保留。托盘不再内嵌浏览器看板（工作台），浏览器看板仍由 AgentNave 按需提供。应用不调用工具，不监听端口；数据根写入仍限于下述会话清理和组员配置。App 更新、服务升级、重试及复制升级命令保留在面板通知区和页脚，沿用原生导航动作。

托盘提供只读 `GET /app/stage`、`GET /app/chats`、`GET /app/chats/<group>` 与 `GET /app/chats/<group>/focus/<会话ID>` 数据路由，面板与办公室每 1.5 秒轮询，页面隐藏时暂停；字段与边界见[架构与边界](context/architecture.md)。

会话管理在办公室里进行，没有单独页面，旧的 `/manage/` 地址转到办公室。右键工作区可打开、改名或删除整个工作区；按住 ⌘（或 Ctrl）点选可逐个多选，按住 Shift 点选可选中一段，多选后右键可一起删除，Esc 取消选择。右键会议室（办公室里的会议桌或「会议」列表中的一条）可查看对话、改名或删除这场会议。给工作区改名就是改它第一个会话的标题；标题最多 48 列（汉字算 2 列）。工作区列表顶部的「清理…」按 7、30、90 或 180 天删除超过该时间没有动静的工作区，进行中和待审核的不在其列。删除前在对话框里列出将删除的内容并确认；运行中、待审核或被宿主占用的会话会保留并注明，删除后提示结果。

模型图标固定为 LobeHub 官方图标。内置小人为 AI 生成；GPT 白龙娘与 DeepSeek 蓝色鲸鱼女仆的形象设定参考社区二创（上善无形、ZipZipPipe）。设置页「头像」可为每个模型家族换一套办公室小人：选择含 `s`、`m`、`h` 子文件夹的文件夹，按动作命名的 27 张 WebP（s 266、m 320、h 96 像素正方形，单张不超过 1 MB）须齐全且尺寸相符，托盘校验后整套替换保存到数据根 `characters/<家族>/`，恢复默认即删除；办公室与菜单栏面板共用。早期版本保存在 `avatars.json` 的家族、指定模型图片不再显示，CLI 标志的覆盖仍读取。

设置页「编排偏好」给出可复制的推荐写法，并列出各宿主常见的全局规则文件位置，可用默认编辑器打开已存在的文件；App 不创建也不写入这些文件，是否采用由用户决定。

会话没有结束状态；托盘清理仍受会话锁与待审状态保护。Rust 在会话的 `locks/<会话ID>.lock` 上取得与 Python AgentNave相同的 OS 锁（探针冲突最多重试约 1 秒），锁内重新读取状态；有待审回答拒绝删除。取得锁但磁盘仍标 running 的轮次／任务视为崩溃残留，可清理。删除数据库中的会话及其消息、轮次、任务与视图，保留锁文件与席位目录；不删除 Provider 原生会话。删除无法撤销。确认后若宿主开始工作或出现待审，仍会分别返回“正在被某个宿主使用”或“有待审回答”，也可能返回记录已不存在或数据库错误；删除在一个事务内完成，失败可刷新后重试。浏览器看板保持只读。见 [ADR-0012](adr/0012-tray-conversation-cleanup.md)。

“设置…”在主窗口打开托盘专属设置页，与办公室在页头互相链接。设置页维护数据根下的组员配置 `members.json`：每个模型行用档位条设置允许的 effort 范围和默认 effort，再分别为“任务”与“讨论”两种模式勾选可派出的组员和默认模型。设置页面向普通用户，只提供选择与拖动，不提供手填输入：模型只能从 CLI 列出的模型中选，其他参数、说明和按模式覆盖不再在 App 中编辑，已有的值继续生效（[ADR-0029](adr/0029-settings-without-free-text.md)）。服务只读且每次现读，不创建缺失文件。App 不替用户写默认模型或档位：没有设置时使用 CLI 自身的默认值，设置页显示“跟随 CLI 默认”及探测到的 CLI 默认模型与档位（[ADR-0028](adr/0028-follow-cli-default-model.md)）。关闭某个模式的自选会保留已设置的默认模型，只清除名单和该模式的覆盖；不受限时也可设置默认模型（[ADR-0021](adr/0021-keep-default-models-when-unrestricted.md)）。页面会列出本机每个 CLI 是否安装、所在路径和它列出的模型：Codex 读取其模型缓存，Grok、Antigravity、Pi 运行各自的列举命令，CodeBuddy 读取帮助中的支持列表，Claude 只提供别名；探测只运行列举或帮助命令、不调用模型，每个命令最多 15 秒，结果在应用运行期间缓存，可手动刷新。探测失败时只显示已配置的模型。

保存通过带操作请求头的 POST 原子替换 `members.json`。Rust 只校验结构与引用（未知字段、引用未配置的模型、默认模型或默认 effort 不在允许范围），参数是否有效以服务端 Adapter 校验为准；文件损坏时显示错误并禁止页面保存，需先修复文件再重新读取。设置页整份保存，外部编辑后请重新读取；并发编辑以最后一次保存为准。见 [ADR-0014](adr/0014-tray-members-and-one-click-update.md)。

boss 未获点名时先读 `list_members`，按模式中的组员与说明选择。`permitted=false` 反映 task 的宿主排除项，discussion 恒为 true；模式允许范围由名单本身表达；受限模式中某 CLI 不限模型时，另以空配置的 `model="*"` 条目说明。`valid=true` 只表示 Adapter 参数校验通过，不证明安装、登录、模型可用或任务目录符合要求。

从包含托盘的版本起，每个 GitHub Release 附带 macOS 通用安装包 `AgentNave_<版本>_universal.dmg`：打开后把 AgentNave 拖进“应用程序”。应用未经 Apple 签名公证，首次打开须在访达中右键“打开”放行。

应用启动时和此后每天检查一次新版本，也可在右键菜单“检查更新”。发现新版时面板右上角显示小的升级按钮（`↑ v<版本>`），右键菜单给出“更新到 v<版本> 并重启”，点击后才下载、校验签名并替换；不会静默更新。若 uv 安装的 `agentnave` 比托盘旧，面板右上角显示“↑ 升级服务”：托盘调用 uv 从本仓库对应 tag 安装，失败时可重试或复制命令。“全部更新”（以及“更新并重启”）一次点击同时处理两者：有新版托盘时先安装并重启，重启后若 uv 安装的本机服务仍较旧会自动执行同一升级；只有服务较旧时直接升级。各宿主的服务经入口中继运行，升级后会在空闲时自动换用新版本，通常无需重连；工具接口变化时须重连以刷新工具清单。

也可从源码构建（需 Rust 与 Node）：

```bash
cd apps/tray && npx @tauri-apps/cli@2 build --bundles app
```

产物位于 `apps/tray/target/release/bundle/macos/AgentNave.app`。从访达启动时不继承 shell 环境，总是读取 `~/.agentnave`；使用 `AGENTNAVE_DATA_DIR` 须从终端运行应用内的可执行文件。Windows 托盘见后续版本。

看板无人浏览时停止周期性全量扫描和运行投影刷新；隐藏标签页暂停轮询，恢复可见时重新读取。历史读取失败会保留已有画面并提示，读取恢复后清除提示。进入 boss 默认打开导演后台，公共页始终只含已提交条目。
