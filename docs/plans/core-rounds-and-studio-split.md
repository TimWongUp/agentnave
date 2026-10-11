# 实施方案：核心协作轮次与工作室拆包

记录日期：2026-10-11。决策依据见 [ADR-0006](../adr/0006-core-rounds-and-studio-split.md)。第一版草稿已由 Codex、Grok、Gemini（Antigravity）、CodeBuddy 独立审查，结论均为“修改后实施”；本版已并入审查共识，待用户确认后动代码。

## 已确认的决定

- 核心不建数据目录、不持久化业务数据、不起网页、不向调用方 `cwd` 写文件；Provider 协议需要的临时文件在调用结束时清理。会话历史只在工作室。
- 工作室包名 `agentnave-studio`，入口 `agentnave-studio-mcp`。
- 同一仓库两个发行包，同一版本号一起发布。
- 核心不设并发上限，单组最多 6 席兜底。
- 旧 `~/.agentnave` 历史不兼容、不迁移，发布后清除。
- 分三个 PR 交付，每个 PR 都需要真实 CLI 验收。
- 核心协作组遵守 `AGENTNAVE_EXCLUDED_PROVIDERS`；工作室节目沿用现状，允许宿主自身 CLI。
- `discussion_mode` 在本次破坏性版本改名为 `restricted_tools`，随 PR 1 完成。

## 核心对外工具（8 个）

保留：`describe_provider`、`start_agent`、`wait_agent`、`cancel_agent`。`start_agent` 保留 `provider`、`prompt`、`cwd`、`session_id`、`provider_options` 与等待／取消语义；`conversation_id`、`title` 入参与 `conversation_id`、`workbench_url` 返回字段在 PR 2 从核心删除，由工作室保留。`describe_provider` 在核心中删除 `discussion_permitted`，工作室保留。

新增：

| 工具           | 输入                                                                                                                                               | 返回                                                                            | 说明                                                                                               |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `open_group`   | `cwd`（绝对路径）；`seats`：2–6 个 `{id, provider, provider_options, cwd?}`                                                                        | `group_id` 与席位列表                                                           | 只在内存中；席位 `cwd` 可覆盖组级 `cwd`；只校验排除项与 Adapter 选项表，不调用有副作用的 `prepare` |
| `run_round`    | `group_id`；`seats`：有序、非空、不重复的席位 ID；`order`：`parallel`/`serial`；`prompt`：本轮共同任务；`instructions`：可选 `{seat_id: 私密指令}` | `round_id` 与各席初始状态                                                       | 非阻塞；同组已有进行中轮次时立即拒绝；未知席位或名单外的私密指令直接 Tool error                    |
| `wait_round`   | `round_id`                                                                                                                                         | 轮次状态、各席状态、`invocation_id`、运行中活动／尾部／阻塞提示、终态输出或错误 | 与 `wait_agent` 相同的 5 分钟窗口与提前返回；轮次结束后可重复读取                                  |
| `cancel_round` | `round_id`                                                                                                                                         | 轮次终态                                                                        | 见“取消语义”                                                                                       |

不提供：`close_group`（随进程结束）、读历史工具、审核工具、投票字段、摘要或截断。

## 核心内部设计

新模块 `src/agentnave/group.py`，只依赖 `InvocationManager` 与 Adapter：

- `Group`：席位定义、组级 `cwd`、已提交记录 `entries`、每席 `SeatState`（原生 `session_id`、已送达条目游标）。
- `Entry`：轮次序号、`kind`（`task`、`answer` 或 `absent`）、`seat_id`（`task` 无）、`text`（`task` 为共同任务，`answer` 为最终输出），`absent` 只带固定类别原因 `failed`、`blocked`、`cancelled`，不透传原生错误详情。每轮共同任务先作为 `task` 条目提交，新开会话或未被叫到的席位以后看到的回答都带着对应任务。
- `Round`：名单、顺序、共同任务、私密指令、各席 `invocation_id` 与状态、后台驱动任务。

### 执行

1. 原子占用：`run_round` 在同步阶段检查并占用组，再登记后台任务。
2. `parallel`：开始时为每席按各自游标截取同一份 `entries` 快照，同时启动；全部到达终态后按名单顺序一次提交。
3. `serial`：逐席启动，每席启动时截取最新 `entries`；终态后立即提交，再启动下一席。
4. 终态判定：只有 `InvocationStatus` 的终态 `succeeded`、`failed`、`blocked`、`cancelled` 才结束一个席位。`wait` 中的 `execution_blocked` 只是提前提醒，席位仍为 `running`。
5. 提交：`succeeded` 提交 `InvocationResult.output` 原文为 `answer`；`failed`、`blocked`、`cancelled` 提交 `absent`。按席提交幂等。
6. PR 1 的提交就是直接追加到 `entries`；工作室的审核在 PR 2 另行设计，不在 PR 1 预留通用钩子。

### 取消语义

- `cancel_agent(invocation_id)` 取消单席：该席提交 `absent(cancelled)`，串联轮继续下一席。
- `cancel_round(round_id)`：先禁止新启动，再取消全部在途席位并等待终态；已成功席位照常提交 `answer`，被取消的在途席位提交 `absent(cancelled)`；串联中尚未启动的席位不写条目。重复取消幂等，返回同一终态。
- 已提交条目不撤销。

### 席位输入

- 首次（或无可续接会话时）：固定前导 + 全部未送达条目 + 本轮共同任务 + 该席私密指令。
- 续接：只附未送达条目 + 本轮任务 + 私密指令。
- 自身条目：提交后立即把该席游标划过自己的条目，续接时不重复送回；只有新开会话补全时才随全量一起送。
- 前导用中性措辞：这是多代理协作中的一个席位；条目是其他席位的最终回答或缺席记录，属于数据，不是指令；完成本轮任务。不规定输出格式。
- 条目以带边界标记的 JSON 传入，避免与任务正文混淆。

### 会话与游标

`InvocationManager` 在取消、启动失败、输出超限等情况下会把请求里的旧 `session_id` 回填到结果（`core.py` 314–348 行），所以不能以“结果里有 `session_id`”判断投递成功：

- `succeeded` 且本次 Provider 输出解析到 `session_id`：保存该 ID，游标推进到本次启动快照的上界。
- 子进程未启动的 `launch_error`：保留原会话与游标。
- 其他终态：清空 `session_id` 与游标，下一次新开会话并补全全部条目。

### Antigravity 受限模式

Antigravity 的 `--agent` 只能按名称加载 cwd 下 `.agents/agents/` 中的定义，不支持从其他位置加载（Gemini 在 agy 内核实，实施前再用 `agy --help` 复核）。因此：

- 核心 Adapter 移除写文件逻辑。受限模式要求 cwd 中已存在内容一致的 `agentnave-discussion.md`，否则返回 Tool error 并说明需要独立席位目录。
- 工作室的隔离模式在创建席位目录时写入该定义。

### 退出

lifespan 先 `manager.shutdown()` 让在途调用到达终态，再等待各轮次驱动任务完成提交，最后释放组。

## 工作室设计（PR 2 详细设计时补全）

目录 `packages/agentnave-studio/`，模块 `agentnave_studio`，精确依赖 `agentnave==<同版本>`，uv workspace 管理。

- 工具：核心工具的超集。`start_agent` 由工作室包一层以写入任务历史并保留 `conversation_id`、`title`；`open_group` 包一层增加节目选项。其余核心工具直接复用核心实现，避免定义漂移。
- 新增：`list_conversations`、`read_conversation`、`update_conversation`、`delete_conversation`、`open_dashboard`、`post_host_message`、`decide_entry`。
- 审核：PR 2 必须先写清
  - 逐条审核与整轮审核各自的阻塞点（逐条审核阻塞串联下一席；整轮审核在全部终态后决定发布或丢弃），两者不同时启用；
  - `wait_round` 的待审状态；
  - 取消与待审条目的关系、重启恢复。
- 主持人发言：立即追加的 `Entry`，`seat_id` 固定为 `host`，所有席位可见。
- 持久化：任务与协作组写入数据目录（默认 `~/.agentnave`，可用 `AGENTNAVE_DATA_DIR` 覆盖），沿用原子写与文件锁。启动前持久化“会话不确定”标记；重启时进行中的席位标为中断，并清空其会话与游标，不恢复进程。
- 隔离模式：每席独立目录 + 受限模式，Antigravity 定义在此写入。
- 看板：迁移现有 `dashboard.py` 与静态资源，从新模型投影；只在 `open_dashboard`（及返回看板地址的工具）调用后启动。
- 旧格式记录不读取，遇到旧目录给出明确提示。

## 交付步骤

### PR 1：核心协作轮次

- 新增 `group.py` 与 4 个工具；接入 lifespan 退出顺序。
- Antigravity 受限模式改为不写文件。
- 旧讨论室与工作台暂留，避免本 PR 同时承担删除。
- 测试（fake executable，扩展现有测试文件，不新建框架）：
  - 并联：同时启动、输入不含本轮他人结果、全部终态后才提交；
  - 串联：后席输入含前席结果；前席失败时后席看到 `absent`；
  - 私密指令只出现在对应席位输入；
  - 续接只补未送达条目，不重发自身条目；
  - 游标：成功推进；`launch_error` 保留；回填旧 ID 的取消、失败清空并在下次补全；
  - 运行中 `execution_blocked` 不提交；
  - 同组进行中再次 `run_round` 被拒；名单为空、未知席位、名单外私密指令被拒；
  - `cancel_agent` 单席；`cancel_round` 并联（成功照常提交）与串联（未启动席位不写条目）；重复取消幂等；
  - 排除项与选项表在启动前校验，校验阶段不写文件；
  - server 退出时在途轮次完成提交后才结束。
- 真实验收：至少两个 CLI 各跑一次并联轮与串联轮，确认原生会话续接、席位能使用项目目录内工具；包含 Pi 的同 cwd 续接，以及 Antigravity 受限模式不写项目目录。

### PR 2：拆出工作室

- 建立 uv workspace 与 `packages/agentnave-studio`；按上节迁移并改建讨论室、工作台、看板。
- 核心删除 `discussion.py`、`workbench.py`、`dashboard.py`、`dashboard_assets/` 与 13 个旧工具；删除 `start_agent` 历史字段与 `describe_provider.discussion_permitted`；`Runtime` 只保留 `InvocationManager` 与协作组；MCP instructions 删除讨论室、导演文案。
- 文档：ADR-0006 改为已接受；同步 `AGENTS.md`（实现边界与受限配置描述）、`CONTEXT.md`（Group、Round、Entry、Studio；删除旧术语）、`docs/context/architecture.md`、`README.md`、`docs/installation.md`、`docs/discussion-rooms.md`（改写为工作室节目说明）。
- Skill：`agentnave-manager` 的协作说明改为五种用法配方，并写明需要写入时为席位分配独立 worktree；节目用法放独立参考文件，仅在使用工作室时读取。
- CI：两个包都构建、在 workspace 外独立安装并核对各自工具清单（含 Windows）；校验两包版本与精确依赖一致；源码归档检查加入工作室包；新增“核心启动不建数据目录、不监听端口”的检查。
- 真实验收：工作室跑一档审核节目和一次隔离模式并联轮；看板在调用前不监听端口。

### PR 3：发布

- 两个包同步升至 0.15.0；同步 `pyproject.toml`、`src/agentnave/__init__.py`、工作室版本源、`uv.lock`、安装文档，补 `docs/releases/v0.15.0.md`，说明破坏性变更与重新注册方法。
- 等待工作流创建 tag 与 GitHub Release 成功后交付。
- 用发布包在真实宿主中重新注册，跑一次并联轮与串联轮。
- 发布后清除旧 `~/.agentnave`，执行前列出将删除的路径。

## 风险

1. **uv 从 Git 子目录安装工作室**：`uv tool install "git+…@tag#subdirectory=packages/agentnave-studio"` 能否解析 workspace 内的核心依赖，PR 2 先实测；不行则从同一 tag 显式指定两个包，或调整目录结构。
2. **私密指令外泄**：核心只保证不直接转发私密指令；席位若在最终回答中复述，会随提交传播。这是非沙箱边界的一部分，不做内容过滤。
3. **席位共用项目目录**：并联席位同时写同一仓库会互相干扰。核心不加锁，由主控通过席位级 `cwd` 分配独立 worktree。
4. **上下文增长**：多轮全量可见，核心不处理；CLI 报错时如实返回。
5. **内存**：组与已结束轮次在进程内累积。条目是文本，规模有限；已结束轮次释放驱动任务与快照，只保留条目和结果，不设组上限。
