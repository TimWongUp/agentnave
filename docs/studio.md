# AgentNave Studio：可观看的多 CLI 节目

节目、历史与看板属于工作室入口 `agentnave-studio-mcp`，与核心入口 `agentnave-mcp` 由同一个 `agentnave` 包安装。工作室包含全部核心工具，另加保存历史、可选审核、主持人发言、隔离席位和本机只读看板。只需要子代理时注册核心入口即可；取舍见 [ADR-0006](adr/0006-core-rounds-and-studio-split.md) 与 [ADR-0009](adr/0009-single-package-two-entry-points.md)。

## 工具

| 工具 | 说明 |
| --- | --- |
| `start_agent`、`wait_agent`、`wait_any`、`cancel_agent`、`list_invocations`、`describe_provider` | 与核心相同；`start_agent` 另接受 `title`、`conversation_id`，任务自动保存为私有任务会话，模型按套用 App 默认值后的参数记录；`describe_provider` 另返回按当前 discussion CLI 名单计算的 `show_permitted` |
| `list_members` | 与核心共用只读组员工具；每次现读 `members.json`，按 task／discussion 返回生效配置、参数校验及宿主允许状态 |
| `open_group` | 创建保存的节目：`title`、2–6 个席位、`cwd`，可选 `review`、`isolated`；返回的 `group_id` 就是会话 ID |
| `run_round`、`wait_round`、`cancel_round` | 与核心相同的并联／串联轮次；审核节目中 `wait_round` 返回 `review_needed`，席位状态出现 `pending_review`、`discarded` |
| `decide_answers` | `round_id` + `{seat_id: publish/discard}`，整批校验并保存后生效 |
| `post_host_message` | 轮次之间以主持人身份发布公开发言 |
| `list_conversations`、`read_conversation`、`update_conversation`、`delete_conversation` | 列表、详情、改名／结束／重开、删除已结束会话 |

旧版的 `open_discussion`、`start_discussion_turn`、`decide_discussion_turn`、`decide_discussion_round`、`mark_discussion_absent`、`continue_discussion`、`reset_discussion_seat`、`open_workbench` 已移除：点名发言由串联轮取代，盲答揭晓由并联轮加审核取代，缺席自动记录，失败后的会话不确定由游标规则自动新开，换身份请新建节目。

## 一档节目怎么跑

1. `open_group(title, seats, cwd, review=true)` 开节目，将返回的首页或导演地址给用户观看；参演 CLI 只接收公开条目。
2. 需要时 `post_host_message` 开场；`run_round(group_id, seats, order, prompt, instructions)` 出题。并联轮里所有人同时独立作答；串联轮按名单依次作答、后者看得到前者。私密指令只发给对应席位。
3. `wait_round` 返回 `review_needed` 时，用 `read_conversation` 或返回的 `output` 查看待审回答，再 `decide_answers`。并联轮全员结束后一起审，全部决定后按名单一次公开；串联轮逐席审，决定后才轮到下一席。
4. 被丢弃的回答不会进入任何其他席位的输入；该席自己的原生会话仍记得它。失败、受阻、取消自动记为缺席，不需审核。
5. 有待审回答时不能开新轮次、发主持人消息或结束会话。`cancel_round` 与服务退出时，已决定发布的回答照常公开，未决定的丢弃。

不审核（`review=false`）时，节目与核心协作轮次的行为完全一致，只是多了保存与看板。

`start_agent` 使用 task 配置，`open_group` 每席使用 discussion 配置。未传 model／effort／其他 options 时套用 App 中的组员默认值，回执 `defaults_applied` 列出补齐的键；显式参数优先，但模式模型名单与生效努力档位范围仍强制执行；模型名单非空、没有默认模型且调用方未传 model 时，拒绝并列出允许模型。续接同样现读并套用默认值。创建节目回执的各席另含 model、effort（未配置时为 null）与 `defaults_applied`。模式缺失表示不限制，空模式对象表示禁止全部席位；宿主排除集合不限制 discussion，但 discussion 的组员名单仍生效。配置损坏时任务、节目与工作室 Provider 查询都直接返回 App 修复错误，不提示修改调用参数。`show_permitted` 每次现读 discussion 是否允许该 CLI；文件缺失或 discussion 不受限时为 true。

`wait_round(include_output=false)` 可只读状态，省略回答与运行尾部；默认 true 保持返回正文，待审或已结束的答案均能再次读取。审核只控制条目公开，不回滚文件修改或清除作者原生记忆。同一服务内，节目席位与普通任务不能同时续接同一 Provider 的同一原生会话。轮次驱动或写盘失败时，轮次状态为 failed 并附 error，不等同于正常编排结束的 finished。

## 隔离席位

普通讨论默认保留原生环境，通过 prompt 约定资料范围和只读操作，具体见 [Skill 的讨论范围与工具](../skills/agentnave-manager/SKILL.md#讨论范围与工具)。仅在用户明确要求工具层面限制访问时使用 `restricted_tools`；用户同时明确要求工作室准备独立席位目录时，才使用 `isolated`。不因“各自答”或“只看给定材料”自动隔离。

`isolated=true` 时，工作室在会话目录下为每席创建独立空目录，为 Antigravity 写入受限角色定义，并给每席加上 `restricted_tools`；调用方的 `cwd` 被忽略；仅套用 model／effort 默认值（Pi 另可套用 provider），跳过其他默认 options，回执不列出被跳过的键；调用方显式非法选项仍拒绝。席位选项只接受 `model`、`effort`（Pi 另可选 `provider`）。节目（隔离与否）都可包含被 `AGENTNAVE_EXCLUDED_PROVIDERS` 排除的 CLI，包括宿主自己的 CLI；排除项只约束 `start_agent`。

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
| `read_conversation` / 数据目录 | 完整会话状态，含原生会话 ID；只给主控 |

浏览器看板不是控制端，不显示原生会话或调用 ID；托盘专属管理页另可清理会话记录。

## 看板

看板在成功创建或继续任务／节目后自动启动并返回入口；仅查看历史时 `list_conversations` 返回首页入口，随 MCP 进程退出；只监听 127.0.0.1 随机端口，用随机路径与精确 Host/Origin 检查限制访问，CSP 禁止外部加载。重启后地址会更换。

界面是左右两栏的工作台：左侧按“需要关注／其他会话／已结束”分组列出会话，可搜索并按节目、任务筛选；“需要关注”只收进行中与待审核的会话，失败、受阻或中断是已交给主控的结果，带红色状态留在其他会话中。右侧未选会话时是总览（进行中与待审核的会话），选中后显示详情，切换会话不离开页面。节目详情分“本轮现场”（私有：各席位私密指令、实时回复、审核状态）和“公开记录”（按轮分隔的公开发言），有工作进行或待审核时默认停在现场；“选手视角”打开公开地址。任务详情按时间倒序显示每次调用的状态、模型、目录、耗时、主控派发的任务原文与回复，“已回复”不等于已验收。公开地址只显示公开记录，不含会话列表。跟随系统浅色／深色，窄屏下列表与详情分屏显示。

回复按常用 Markdown（标题、列表、引用、代码、粗体、链接）以纯文本节点渲染，长内容折叠可展开；支持按角色查看、复制单条台词或回复、回到最新发言、导出完整公共 JSONL。角色筛选只影响当前浏览器画面，不改变选手收到的公共历史；导出不随筛选裁剪。头像是原创插画，不代表厂商官方形象，也不意味着尚未接入的模型已可用。

交互参考 [SillyTavern 角色管理](https://docs.sillytavern.app/usage/characters/) 和 [群聊](https://docs.sillytavern.app/usage/core-concepts/groupchats/) 的头像、角色侧栏、共享聊天历史和手动点名概念。每次只向当前选手提供自己的私有设定，不采用合并全员私有角色卡的方式。没有复制 SillyTavern 代码，也没有引入其角色卡、插件或自动轮次引擎。

## 保存与恢复

数据根默认 `~/.agentnave`，可用 `AGENTNAVE_DATA_DIR` 指定绝对路径；首次保存会话时才创建。每个会话一个目录，含原子替换的 `state.json` 与 `.lock` 文件锁。新任务与节目保存创建者的 `host`、`host_source` 和 UUID hex `manager_session`，后续接手沿用创建归属；旧记录缺失时兼容读取为 null／unknown／null。`views.json` 摘要及 live 中的会话摘要带同样字段，live 顶层另带当前服务身份、主控会话标识与开始时间。每次写入前取锁并在锁内重读磁盘状态；运行中的轮次／任务、待审回答及未成功落盘的回写持续持锁，全部回写完成且空闲后释放，并清除写入缓存。持锁期间其他服务可读，在 `list_conversations` 中显示 `writable=false`，写入或续接直接报错；空闲后其他服务可取得同一把锁继续写入。若会话的 `state.json` 已被删除，下次写入会遗忘该会话并返回未知会话错误，不会重建历史。

重启后节目按保存的条目、席位会话与游标继续；运行中的席位记为中断并在下次新开会话补全条目，被中断的轮次不续跑。崩溃时停在待审的回答仍可用 `decide_answers` 决定。运行中的任务记为中断，旧 `invocation_id` 不恢复，原生 `session_id` 保留。

`delete_conversation` 只删除已结束且无进行中工作的会话，调用方须先取得用户授权：删除 `state.json` 与已结束的调用句柄，移除隔离席位中工作室写入的角色定义和空目录，保留会话目录及 `.lock`，Provider 原生文件不删。v0.15.0 之前的旧历史（数据根下的 `catalog/` 与含 `room.json` 的目录）不迁移，只在列表中标为不可用，可手动清除。

托盘应用所需的数据由工作室写出：每个会话目录中的 `views.json` 视图快照，以及有工作运行时数据根下 `live/` 中的运行文件（结束即删除）；见 [ADR-0008](adr/0008-tray-app-reads-studio-snapshots.md)。

## 托盘应用（预览）

宿主接入后端由 App 管理用户级 MCP 与配套 Skill（[安装路径与手动方式](installation.md#3-register-the-mcp-server)）。启动/刷新自动为已有条目补宿主标识并修复缺失 launcher；新增接入、移除和入口切换走带操作请求头的 POST，GET 只读。保留最近 30 份备份，写后仅在文件仍等于本次写入字节时回滚，否则保留外部新内容并报告冲突；外部管理的 Skill 与项目级配置不改。`/app/hosts` 提供配置、Skill 与实测状态，`/app/onboarding` 按真实服务、宿主、组员和主控会话记录给出四步引导；关闭标记保存在 App 自有数据目录。宿主加载与真实调用须另行验收，配置成功不代表模型可用。见 [ADR-0015](adr/0015-app-host-integration.md)。

`apps/tray` 是可查看并清理会话记录的菜单栏应用（Tauri 2，目前先支持 macOS），不必启动 MCP 服务即可查看数据根中全部宿主的会话。菜单栏标题在有工作时显示运行中与待审核会话数。左键点击图标弹出 480×640 的舞台面板：左侧主控、中间 AgentNave、右侧按主控分组的组员。每位组员固定占一行，运行时原地更新输出，结束后显示结果；同组达到 6 位时可折叠，连线位于卡片下方。面板顶栏按组员计数，主控和组员采用粘性顺序，输出刷新不会改变其位置；没有入选主控时区分尚无数据与最近两小时无活动。

点击主控打开其群聊，点击组员还会定位其最近消息。主窗口默认页为 `/chat/`，`/chat/<group>` 显示该主控会话派出的全部任务和节目：运行与待审群置顶，组员只保留一个原地更新的 typing 气泡，长回复可展开，向上阅读时不强制滚到底。窄窗口在群列表与消息间切换。会话 chip 可进入 `/c/<id>/`，页头可打开工作台 `/`、管理会话和设置。右键菜单有“打开 AgentNave”、“工作台”、“管理会话…”、“设置…”、“检查更新”和“退出 AgentNave”；再次启动打开群聊默认页。群聊页是托盘专属只读界面，工作台与浏览器看板仍共用前端，“选手视角”在单独窗口打开，公共 JSONL 导出到“下载”。应用不调用工具，不监听端口；数据根写入仍限于下述会话清理和组员配置。App 更新、服务升级、重试及复制升级命令保留在面板通知区和页脚，沿用原生导航动作。

托盘提供只读 `GET /app/stage`、`GET /app/chats` 与 `GET /app/chats/<group>` 数据路由，面板与群聊每 1.5 秒轮询，页面隐藏时暂停：按创建者的 `manager_session` 聚合未归档会话，旧记录各自归入 `c-<会话ID>`，组员按 Provider／模型合并并取最近活动。舞台最多返回 6 个有运行／待审或近两小时活动的主控，`counts` 统计所列主控中的运行／待审组员，`empty` 区分 `no_data`、`quiet` 与有主控时的 null；群列表最多 200 个，群时间线最多 300 条。运行中组员各有一个稳定 ID 的 typing 条目并置于末尾，时间取 live 文件的 `updated_at`；失败状态只由最近一次结束的组员决定，旧失败不持续标红，取消不算失败。舞台主控按组 ID 字典序对哈希色冲突顺延，三条路由共用这份分配，不在舞台中的群使用哈希色。新鲜 live 覆盖同会话磁盘快照，未变化的 `views.json` 按修改时间与大小复用内存解析缓存。这三个路由只接受 GET，不写数据根，不调用工具或 CLI，沿用自定义协议、无 IPC、无监听端口与既有 CSP 边界；群不存在返回 404。

“管理会话…”在主窗口打开托盘专属页面，也可从面板底部进入。列表包含标题、类型、状态、最近活动时间和是否已结束；可按“已结束 / 空闲 / 失败 / 全部”及活动早于日期筛选。空闲指未结束、当前未占用且无待审回答的会话，失败筛选包含受阻与中断。单选或多选后点击“删除所选”，先在页面中核对并确认；“清理 N 天前的会话”针对**全部会话**中早于该天数且可删的记录，不受当前列表筛选限制，同样先确认。运行中、待审及被宿主占用的记录不可勾选，并显示原因；列表定期刷新，读取失败会保留画面并暂停删除，删除后刷新并逐条显示结果。缺少视图快照时回退到状态文件，最近活动时间回退到状态文件的修改时间。

托盘删除不要求先结束会话。Rust 在会话 `.lock` 上取得与 Python 工作室相同的 OS 锁（探针冲突最多重试约 1 秒），锁内重新读取状态；有待审回答拒绝删除。取得锁但磁盘仍标 running 的轮次／任务视为崩溃残留，可清理。只删除 `state.json` 和 `views.json`，保留会话目录、`.lock`、全部席位及其文件；不删除 Provider 原生会话。删除无法撤销。确认后若宿主开始工作或出现待审，仍会分别返回“正在被某个宿主使用”或“有待审回答”，也可能返回记录已不存在或具体文件错误；两个文件的删除不保证同时成功，失败可刷新后重试。浏览器看板保持只读。见 [ADR-0012](adr/0012-tray-conversation-cleanup.md)。

“设置…”在主窗口打开托盘专属设置页，与管理会话页在页头互相链接。设置页维护数据根下的组员配置 `members.json`：每个 CLI 下配置哪些模型及其允许的 effort、默认 effort、其他参数与说明，再分别为“任务”与“讨论”两种模式勾选可派出的组员、默认模型和按模式的覆盖；覆盖的字段整体替换共享值，options 也整体替换。服务只读且每次现读，不创建缺失文件。页面会列出本机每个 CLI 是否安装、所在路径和它列出的模型：Codex 读取其模型缓存，Grok、Antigravity、Pi 运行各自的列举命令，CodeBuddy 读取帮助中的支持列表，Claude 只提供别名；探测只运行列举或帮助命令、不调用模型，每个命令最多 15 秒，结果在应用运行期间缓存，可手动刷新。探测失败时仍可手填模型 ID。

保存通过带操作请求头的 POST 原子替换 `members.json`。Rust 只校验结构与引用（未知字段、引用未配置的模型、默认模型或默认 effort 不在允许范围），参数是否有效以服务端 Adapter 校验为准；文件损坏时显示错误并禁止页面保存，需先修复文件再重新读取。首次打开时若只有旧的 `profiles.json`，托盘把每条档案迁移为共享模型配置，两种模式保持不受限，但此后调用这些模型时会开始套用迁移来的默认 effort 与参数；旧文件改名为 `profiles.json.migrated`。没有模型、重复的档案以及跳过审批、沙箱等权限类参数不迁移，并逐条提示。设置页整份保存，外部编辑后请重新读取；并发编辑以最后一次保存为准。见 [ADR-0014](adr/0014-tray-members-and-one-click-update.md)。

主控未获点名时先读 `list_members`，按模式中的组员与说明选择。`permitted=false` 反映 task 的宿主排除项，discussion 恒为 true；模式允许范围由名单本身表达；受限模式中某 CLI 不限模型时，另以空配置的 `model="*"` 条目说明。`valid=true` 只表示 Adapter 参数校验通过，不证明安装、登录、模型可用或任务目录符合要求。服务不再提供 `list_profiles`，旧 `profiles.json` 不会由服务读取。


从包含托盘的版本起，每个 GitHub Release 附带 macOS 通用安装包 `AgentNave_<版本>_universal.dmg`：打开后把 AgentNave 拖进“应用程序”。应用未经 Apple 签名公证，首次打开须在访达中右键“打开”放行。

应用启动时和此后每天检查一次新版本，也可在右键菜单“检查更新”。发现新版时面板顶部和右键菜单给出“更新并重启”，点击后才下载、校验签名并替换；不会静默更新。若 uv 安装的 `agentnave` 比托盘旧，面板会提示并提供“一键升级”：托盘调用 uv 从本仓库对应 tag 安装（必要时先卸载旧的 `agentnave-studio`），失败时可重试或复制命令。“全部更新”（以及“更新并重启”）一次点击同时处理两者：有新版托盘时先安装并重启，重启后若 uv 安装的本机服务仍较旧会自动执行同一升级；只有服务较旧时直接升级。v0.18.0 起各宿主的服务经入口中继运行，升级后会在空闲时自动换用新版本，无需重连；从更早版本升级到 v0.18.0 时仍需重连一次。

也可从源码构建（需 Rust 与 Node）：

```bash
cd apps/tray && npx @tauri-apps/cli@2 build --bundles app
```

产物位于 `apps/tray/target/release/bundle/macos/AgentNave.app`。从访达启动时不继承 shell 环境，总是读取 `~/.agentnave`；使用 `AGENTNAVE_DATA_DIR` 须从终端运行应用内的可执行文件。Windows 托盘见后续版本。

看板无人浏览时停止周期性全量扫描和运行投影刷新；隐藏标签页暂停轮询，恢复可见时重新读取。历史读取失败会保留已有画面并提示，读取恢复后清除提示。进入主控默认打开导演后台，公共页始终只含已提交条目。

v0.16.0 移除独立的 `open_dashboard` 工具；旧调用改为 `list_conversations`，列表仍只含摘要，首页地址只交给主控。`post_host_message` 返回条目序号，`update_conversation` 返回标题和结束状态，`decide_answers` 返回已保存决定、剩余待审与实际已提交席位；三者不再重复原文或导演状态。审核回执不保证轮次已结束，必要时用 `wait_round(include_output=false)` 确认。
