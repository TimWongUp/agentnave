# ADR-0129：用量统计扩至 90 天预聚合小时桶、本机全项目归因与常用本机 Agent 自动发现

- 状态：已接受
- 日期：2026-11-26
- 修订：
  - [ADR-0106](0106-tray-usage-panel.md)：
    - **覆盖范围与多账号发现**：在原有 8 家基础上，将 WorkBuddy 从 CodeBuddy 拆出独立统计，新增常用本机原生日志 Agent（Hermes、OpenCode、Claude Desktop / Cowork、Kimi Code、MiniMax Code；若本机存在 Devin CLI 日志库亦可只读扫描，不再默认将其列为不支持的组员），并自动发现 T3 Code 自定义账号目录（只读 `~/.t3/userdata/settings.json` 中的 `homePath`）与 `$HOME` 下多套 Claude / Codex 目录；不接入 Cursor 等依赖远端凭据拉取的来源。
    - **90 天窗口与「过去不重算」**：扫描与展示窗口扩至 90 天（写入端留 92 天跨日缓冲），新增 `近 24 小时`、`近 90 天` 与对齐到「日期 + 整点小时」的自定义范围；统计库采用「UTC 小时预聚合桶（92 天）+ 近期明细（7 天）+ 16 字节定长去重键（92 天）」三层结构，已封存的历史小时桶永不重算，不因后续价表刷新或仓库目录变更回溯改写历史折算金额。
    - **本机全项目归因与删除语义**：项目视角从「仅 AgentNave 视图快照归因」扩为「本机所有会话按原生日志 `cwd` 识别并持久化项目维度，各项目合计对齐主机总体消耗」；仍复用 AgentNave 只读快照在同一项目内标出「Boss / 员工 / 原生」拆解。删除工作区或临时 worktree 后，历史项目花费保留在统计库中，不再随工作区删除退出项目归因。
- 沿用：ADR-0106 的托盘 Rust 端独占拥有、原生日志单一数据源、零 Python 服务与 `agentnave.db` 改动、不接入 Magpie 网关、5 互斥 Token 归一化与 `cache_unknown` 排除命中率、内置价表 + `models.dev` 低频只读缓存与「折算花费」语义、`AGENTNAVE_PREVIEW` 预览版隔离与 [ADR-0100](0100-app-uninstall.md) 卸载开关；[ADR-0119](0119-remove-devin.md) 对 Devin 组员体系的撤除；[ADR-0121](0121-t3-code-orchestration.md) 对外部 `T3CODE_HOME` 只读且不碰凭据的边界。

## 背景

ADR-0106 落地后，用户在日常重度多 Agent 使用（含 T3 Code 隔离账号、原生终端会话及多日对比）中提出四项核心需求：

1. **报表与对比能力**：需要 `近 24 小时`、`今天`、`近 7 天`、`近 30 天`、`近 90 天` 与自定义时间范围，用更直观的多系列平滑趋势图同时支持按工具 / 按模型系列 / 按基础模型三维度比对、图例多选固定高亮、Token 去缓存净耗与单位均价对比。
2. **本机全量覆盖**：T3 Code 中新建的 Claude 自定义账号目录、Hermes、WorkBuddy、OpenCode 等常用本机 Agent 应一并自动统计，无需在 AgentNave 账号表里重复登记。
3. **本机全项目花费**：项目页不仅统计经过 AgentNave 派发的 Boss 与员工会话，还应把本机所有项目上的原生 Agent 花费统一归并进来，并正确折叠同一 Git 仓库的 linked worktree 与子目录。
4. **90 天规模下的效率与「不重复统计过去」**：实测单机 36 天即产生约 11.8 万条调用（44 MiB）；若沿用旧版每次查询将窗口内全量事件读入内存再逐条计价和线性归因的做法，90 天约 30 万条事件将带来 ~120 MB 瞬时堆分配与秒级延迟，且每次打开页面都在重复计算前 89 天已经定格的历史。

同时调研了开源生态（`tokscale`、`ccusage`、`agtrace-sdk`、`agent-session`、`tokenusage` 等）与本地 MIT 参考项目（`yetone/magpie`、`pingdotgg/t3code`）：crates.io 上并无覆盖齐全且适配字节偏移增量游标的纯 Rust 轻量库，因此继续在托盘 Rust 端维护轻量解析器，参考 MIT 开源项目的目录发现与字段口径并保留署名。

## 决定

### 1. 常用本机 Agent 全自动发现与只读解析

- **覆盖清单**：保留并增强原有 8 家（Claude Code、Codex、Antigravity CLI / T3 `antigravity-acp`、Pi、CodeBuddy、DeepSeek Harness、Grok、ZCode），将 WorkBuddy（`~/.workbuddy`、`~/.workbuddy-ai`）从 CodeBuddy 拆分为独立供应商，并一次性补齐常用本机原生日志来源：
  - **Hermes**：只读查询 `~/.hermes/state.db`（及活动 profile）的 `sessions` / `session_model_usage`；
  - **OpenCode**：只读查询 `$OPENCODE_DB` / `$XDG_DATA_HOME/opencode/opencode.db`（与 ZCode 复用同一套只读 SQLite 解析）；
  - **Claude Desktop / Cowork**：扫描桌面端 `local-agent-mode-sessions` 下的 Claude 会话日志，复用 Claude 解析器；
  - **Kimi Code**（`$KIMI_CODE_HOME` / `~/.kimi-code/sessions`）与 **MiniMax Code**（`$MINIMAX_DATA_DIR` / `~/.minimax/v2/sessions`）；
  - **Devin CLI**：仅当本机存在 `~/.devin/sessions.db` 时只读读取，不纳入组员或宿主列表。
- **多账号目录自动发现**：
  - Claude 除默认目录与 AgentNave `accounts.json` 外，自动扫描 `$HOME` 下名称以 `claude` 或 `.claude` 开头且含 `projects/` 的目录（至多下探 1 层），并只读解析 `T3CODE_HOME` 下 `userdata/settings.json` 中 `claudeAgent*` / `codex*` 实例配置的 `homePath`（绝不读取任何密钥、令牌或会话内容）；
  - Codex 同理复用已有 `$HOME` 多目录发现及 T3 `homePath`；
  - 所有目录按规范化物理路径去重，目录不存在时零扫描开销。
- **排除边界**：不接入需要远端 API 凭据或无本地逐调用日志的来源（如 Cursor 远端拉取）；不重读未变文件，JSONL 解析前按字节快速预筛跳过不含用量字段的超大行。

### 2. 三层存储结构：90 天预聚合小时桶 + 7 天明细 + 90 天去重键

在 `usage.db` 中提升 `PRAGMA user_version`，由托盘自动迁移并建立三层结构：

- **`usage_hourly_buckets`（保留 92 天）**：按 `(hour_ts_utc, provider, model, project_key, role_kind)` 聚合，存 `calls`、5 项 Token、`cache_known_prompt`、`cache_known_read`、`cost_usd`、`unpriced_tokens`、`unpriced_calls`。
  - 新事件写入时（含 7 天内活跃会话修订产生的新旧差值），在同一事务中增量累加到对应 UTC 小时桶；`cost_usd` 按单次请求实时结算后累加，保留 >200K 上下文阶梯价语义。
  - 所有查询（`24h`、`today`、`7d`、`30d`、`90d` 与整点自定义范围）一律只读 `usage_hourly_buckets`，按用户本地时区将 UTC 小时桶折叠为小时 / 天 / 周序列并计算等长上期环比，已封存的历史小时永不重算。
- **`usage_events`（仅保留近 7 天）**：仅用于承接活跃会话的流式修订差值与 `GET /app/usage/workspace` 的精细工作区视图；超过 7 天的明细在低频清理中裁剪，将 `usage.db` 体积稳定控制在 ~20–25 MiB。
- **`usage_dedupe`（保留 92 天）**：仅存 16 字节定长 `event_key` 哈希与 `ts_ms`，防止超过 7 天的旧会话被 fork / resume 复制到新文件时重复累加历史小时桶。
- **`usage_projects`**：持久化 `project_key -> (label, path)`，首次识别项目时写入，目录删除后不丢失历史展示名与路径。
- **升级迁移与冷补扫**：旧库升级时一次性从已有 `usage_events` 回填小时桶、去重键与项目表，随后后台增量补扫 31–92 天内尚未记录游标的旧文件一次；已记录游标且 `size`/`mtime` 未变的文件全程零读取。

### 3. 本机全项目归因与目录识别

- **全量 `cwd` 提取**：各解析器在解析用量事件的同时零或近零额外 I/O 提取会话 `cwd`（行内 `cwd`、文件首行头存入 `parser_state`、Grok `summary.json`、SQLite `session.directory`/`cwd`、Antigravity `conversation_summaries.db` 批量缓存），避免二次读盘。
- **项目归并规则**：复用托盘纯文件系统 `projects::classify`（不调 `git` 进程，沿祖先查找 `.git` 并读 `gitdir`/`commondir` 折叠 linked worktree 与仓库子目录；规范化符号链接；排除 `$HOME` / Desktop / Downloads、系统临时目录、宿主自动目录及 T3 隔离数据目录，无 `cwd` 或被排除路径归入 `"other"`）。
- **角色拆分**：每条事件在写入时同时检查 AgentNave 只读快照（`thread` 窗口与 `boss_key`），标记 `role_kind` 为 `boss`、`worker` 或 `native`（未经过 AgentNave 的原生 / T3 / 外部会话）。项目视图展示每个项目的本机合计（与总体消耗完全一致）以及 `Boss / 员工 / 原生` 拆解。

### 4. 自定义时间精度与报表交互边界

- **自定义时间对齐整点小时**：支持预设 `24h`、`today`、`7d`、`30d`、`90d` 及自定义范围（日期 + 整点小时 `00:00–24:00` 与 `近 N 小时`），与 UTC 小时预聚合桶完全契合；不做 90 天内任意分钟级切分。
- **分桶与下钻**：单日或 ≤48 小时自动小时桶，2–60 天自动天桶，>60 天自动周桶，在合理跨度下允许用户手动切换 `时 / 天 / 周`；按天视图支持点击某一天直接下钻到该日小时图并返回。
- **对比交互**：支持按工具（客户端拆分）/ 按模型系列（跨工具归并）/ 按基础模型（跨工具且合并 effort 变体）三维度切换、花费 / Token（含缓存与去缓存净耗）切换、折线 / 堆叠柱切换、图例与排行**多选固定高亮对比**（重缩放纵轴聚焦对比曲线），以及按列排序与 `$/1M` 均价。

## 取舍

- **历史小时桶定格不回溯重算 vs 保留 90 天全量明细随时重定价**：选择写时定格并只保留 7 天明细，换取 2–5 ms 查询延迟、~20–25 MiB 常驻库体积与「过去时间绝不重算」的清晰保证；代价是 7 天前已封存的历史不会因后续 `models.dev` 补价或事后 `git init` 自动改写，且 7 天前的历史归因随写入时点冻结。
- **自定义时间对齐整点小时而非分钟**：避免为了极低频的任意分钟查询而被迫保留和扫描 90 天逐条明细；对 90 天回顾与日常时段分析，整点小时精度已足够且能 100% 吃满预聚合红利。
- **删除工作区后保留历史项目统计**：推翻 ADR-0106「删工作区即退出项目归因且不落盘路径」的约定，使「本机所有项目合计 = 总体消耗」且删掉临时 worktree 不丢账；代价是统计库 `usage_projects` 会保留曾产生过用量的项目名与路径，用户卸载时仍随数据根开关一并清理。
- **不引入外部扫描 crate**：自维护轻量 Rust 解析器避免引入重型异步运行时或 UI 依赖，并与现有 `source_cursors` 字节偏移增量、SQLite 事务和 `cwd` 归因严丝合缝；代价是上游 CLI 变更日志格式时需在托盘侧跟进适配。
