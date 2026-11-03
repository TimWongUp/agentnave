# 1.0 轻量化与架构基线

Status: Ready

## Problem

AgentNave 在 46 篇 ADR 中快速迭代，准备发布 1.0 正式版。2026-11-03 的四席会议核验出以下问题：

- 文档和界面承诺与实际行为不一致：
  - Codex「允许在 git 仓库外运行」开关不起作用；
  - App 升级服务后，宿主不会空闲热切换；
  - 服务会主动终止持续重试的任务，architecture.md 却写着「停止只靠显式 cancel」。
- 托盘越过写入清单，自行给服务库补表补列。
- 兼容层、看板残留和 mock 原型随正式包发布。
- 约 15 篇 ADR 被修订或取代后没有回标，architecture.md 有 3 处引用错误。
- 没有约束保证后续修改与现行架构和 ADR 对齐。

## Outcome

- 1.0.0 是最低兼容基线：早于它的程序和数据兼容代码全部删除，旧数据由服务一次性迁移。
- 界面、回执、文档与实际行为一致；架构边界只有一份现行描述（`docs/context/architecture.md`），每条都能追溯到包含该条款的现行 ADR。
- 服务与托盘去掉已确认无用的代码路径，模块依赖方向单向、清楚。
- 后续修改有明确的对齐约束：Skill、全局规则、评审工位、AGENTS.md 各管一层。

## Scope

本次分三个功能 PR，加上仓库外的规则改动，全部合入后另开 1.0.0 版本 PR。

### 1. 文档 PR（`docs/`、`AGENTS.md`、`CONTEXT.md`、`skills/agentnave-review`）

1. 本 Spec。
2. 旧 ADR 头部加状态和取代回链，正文不改；补齐缺失的状态行（0015、0017、0019、0029、0030、0044、0045、0046）。
3. 新增 `docs/adr/README.md` 索引：编号、标题、状态（现行／部分取代／已取代）、取代者。
4. 修正 architecture.md：
   - 3 处错误引用：2–7 席、七天保留、同事务更新快照应引 0026；
   - 依赖图改为分叉：InvocationManager 同时调用 Adapter 与 processes/supervisor；
   - 补上服务主动终止的例外和版本化安装；
   - 删除热切换承诺；
   - 写明托盘不迁移服务库。
5. 修正 ADR-0038 第 14 行与第 31 行的矛盾；ADR-0006 加修订行，上限为 7 席；MCP 新协议补进 0010，收件箱补进 0007/0024。
6. 新增 ADR 0047–0050（见 Accepted decisions）。
7. 安装文档第 220 行改为「宿主重连后生效」；删除 CONTEXT.md:125 等处对 `cli-options` 和 `skip_git_repo_check` 开关的描述。
8. AGENTS.md 只补项目事实：现行基线是 `docs/context/architecture.md`，ADR 索引是 `docs/adr/README.md`。
9. `skills/agentnave-review` 的架构评审工位要点加一条：核对 diff 是否偏离现行架构基线和现行 ADR；偏离却没有相应 ADR 修订的，报为 finding。

### 2. 服务 PR（`src/`、`tests/`）

1. Codex 固定传 `--skip-git-repo-check`；`cli-options.json` 整条链（读取、校验、合并、`defaults_applied`、`list_members.cli_options`、共享 fixture）删除，磁盘上的旧文件忽略。
2. 删除看板残留投影：视图中的 `public` 和 `/p/<id>/`、`workbench_url`、`public_url`、`storage`、`refresh_error`。
3. 删除 `provider_usage` 链路和只给测试用的 `to_dict`。
4. 架构调整：
   - `host.py` 中按 CLI 回读原生会话和模型的代码移到中性模块，解开 host 与 adapters 的双向依赖，host 只做宿主识别；
   - `zcode_protocol.py` 移进 `adapters/`；
   - `PROVIDER_OPTIONS` 说明、受限工具名单与受限时额外选项改为各 Adapter 的类属性；
   - 权限档位仍在服务入口套用。
5. SQLite 升到 `user_version = 2`。服务打开 v1 库时，在一个事务内完成迁移：
   - 补齐 `workspaces`、`former_bosses` 表和 `title`、`cwd` 列；
   - 删除旧身份会话（`manager_session` 为空或不以 `s-` 开头）及其级联数据和席位目录；
   - 写入版本 2。

   迁移完成后删除每次打开都执行的表、列探测。
6. 删除服务端兼容分支：
   - 旧身份兜底查找；
   - 「旧服务只在启动时持锁」的运行中判断；
   - `role` 兼当工位名（只认 `seat`）。

   MCP 旧握手和 `RetiredHostName` 保留。

### 3. 托盘 PR（`apps/tray`）

1. 删除 `cli-options` 的 Rust 读写和设置页开关。
2. 正式包不含 mock：
   - 去掉 `include_bytes!` 和 `mock.js` 路由；
   - `settings.js` 的 mock 数据和 `isMock` 分支并入 `mock.js`；
   - HTML 不再写死引用它，静态预览通过专门入口注入。
3. 宿主卡片：连接版本落后就直接显示「需要重连」，去掉 `updating` 状态、600 秒宽限期和 v0.31 之前 relay 的判断。
4. 托盘只认 v2 库。打开 v1 库时可以只读显示；删除、交接、改名等写操作提示「先重连宿主，让服务完成升级」。删除 `add_handover_schema`。
5. 删除兼容层：
   - live v1 快照合并；
   - `UPGRADE_MARKER`；
   - 主题从 localStorage 迁移；
   - 语音旧默认 prompt 识别；
   - uv 旧目录布局回退与旧 launcher 路径；
   - `agentnave-studio-mcp` 排除；
   - 旧工作台重定向；
   - 托盘里按「非 `s-` 身份」区分的分支。

### 4. 仓库外（由 boss 直接改，不进本仓库 PR）

1. `tim-domain-modeling` Skill（Vault 真源）新增「ADR 演进」一节：
   - 改动与现行基线或 ADR 冲突时，先修订 ADR 再改代码；
   - 修订或取代时，旧 ADR 头部加状态和回链；
   - 维护 `docs/adr/README.md` 索引；
   - 基线引用的 ADR 必须包含被引用的条款。
2. 各 Agent 的全局规则入口加一行触发句，只写共有约束，不写进 `preferences.md`：「改动与项目现行架构基线或 ADR 冲突时，先按 `tim-domain-modeling` 修订 ADR 再改代码，不默默偏离。」

## Scenarios

- **App 1.0 启动、宿主仍连着旧服务**：
  - App 装好 1.0 服务，并切换 `active.json`；
  - 旧服务在宿主重连前继续运行，宿主卡片立即提示「需要重连」；
  - 重连后新服务迁移库到 v2。
- **库已迁到 v2、还有旧服务连接在运行**：旧服务读不了 v2 库，相关调用报错，用户按卡片提示重连即可恢复。这是接受的代价。
- **服务还没迁移、托盘先读到 v1 库**：托盘只读显示，写操作提示先重连宿主。
- **Linux 无 App**：`uv tool install --force` 原地升级，relay 照旧在空闲时热切换；首次启动 1.0 服务时完成迁移。
- **用户本机有旧身份记录**：迁移时直接删除（Tim 本机 4 条，其他用户可能更多），在发布说明中写明。

## Accepted decisions

| 决定 | 理由／来源 |
| --- | --- |
| App 启动时自动把服务升级到配套版本（ADR-0047，修订 0014） | `7d1ee4cc` 有意引入；新 App 配旧服务只剩宿主重连前这段窗口 |
| App 升级服务后宿主重连才生效；relay 只在原地重装时热切换（ADR-0047，修订 0010） | 最轻；跨版本热切换要改跨进程逻辑并做实测 |
| 版本落后就直接提示重连 | App 只按版本安装，600 秒宽限期只是白等 |
| 1.0.0 是最低兼容基线（ADR-0050） | 发布后再改文件格式或 schema 成本更高 |
| 旧身份记录迁移时直接删除 | Tim 选择删除，而不是合成工作区 |
| schema 改动必须升 `user_version`，迁移只由服务执行，托盘不迁移（ADR-0048，修订 0026、0033） | 边界清楚；托盘遇到旧库只读，写操作提示重连 |
| 服务主动终止的情形只有：输出超限、连续重试满 300 秒；终态句柄内存保留 7 天（ADR-0049，补充 0002、0007） | 代码已是这样，文档补上例外 |
| Codex 固定传 `--skip-git-repo-check`，`cli-options.json` 整条链删除 | 开关本来就不起作用；行为不变 |
| 正式包彻底不含 mock | 自查改用静态预览，正式包里的 `?mock` 失效 |
| 语音全部保留 | ChatGPT 登录走 OpenAI 官方的开源本地应用通道，见 ADR-0044 |
| `architecture.md` 是唯一的现行基线，不另写基线 ADR | 避免两份真源 |
| 对齐约束分四处写：Skill 写做法、全局规则写触发句、评审工位负责核对、AGENTS.md 写路径 | 遵守「项目级 AGENTS.md 只放项目特有约束」 |
| 三个功能 PR 依次合入，中间不发版；1.0.0 单独开版本 PR，等用户另行要求再发 | 合并和发版分开 |

## Verification

- **Python 门**：`uv run ruff format --check .`、`uv run ruff check .`、`uv run pyright`、`uv run pytest`。已核实测试不会写真实的 `~/.agentnave`。
- **托盘门**（在 `apps/tray` 下）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`；预览构建另跑 `AGENTNAVE_PREVIEW=1 CARGO_TARGET_DIR=target/preview cargo clippy --all-targets -- -D warnings`。
- **迁移**：在现有 db 测试里用 v1 库验证以下几点：
  - 补列；
  - 删除旧身份会话及其级联数据；
  - 版本变为 2；
  - 迁移幂等；
  - 托盘读 v1 库只读，写操作被拒绝。
- **回执合同**：`conftest.py` 的 `check_mcp_reply_contract` 会自动校验移除 `cli_options` 后的 `list_members` schema。
- **视图**：现有托盘视图测试（`stage.rs`、`stage/show.rs`）在没有 `public` 时照常通过。
- **重连提示**：扩展 `hosts/tests.rs` 中现有的 reload 用例，版本落后时立即返回 `required`。
- **依赖方向**：`adapters/` 不导入 `agentnave.host`，`host.py` 不导入 `adapters`，用 grep 检查。
- **Codex 真实调用**：至少一次通过 AgentNave 在非 Git 目录派发 Codex 并成功（真实调用已预授权）。
- **界面**：托盘改动先用 `scripts/preview_tray.sh` 给用户目测，确认 mock 没进正式包，静态 `?mock` 预览可用。
- **删除 uv 旧目录回退前**：确认 App 1.0 启动后，宿主配置都指向版本化入口。

## Acceptance criteria

1. `rg "skip_git_repo_check|cli-options|cli_options"` 在 `src/`、`apps/tray/src`、`apps/tray/ui`、`tests/`、`docs/context`、`CONTEXT.md` 中没有生产引用；Codex 在非 Git 目录真实调用成功。
2. 视图快照中不再出现 `public`、`workbench_url`、`public_url`、`storage`、`refresh_error`。
3. 正式构建的托盘二进制里没有 `mock.js` 内容，也没有对应路由。
4. 服务打开 v1 库后，库变为 `user_version = 2`，旧身份会话为 0 条；服务和托盘代码里没有表、列探测，也没有「非 `s-`」分支。
5. 托盘的落后连接立即显示「需要重连」；代码中没有 `STALE_RUNTIME_SECONDS` 和 `updating` 状态。
6. `adapters/` 与 `host.py` 之间没有任何方向的导入；`zcode_protocol` 位于 `adapters/`；受限名单不再在 `base.py` 中按 Provider 名分支。
7. 每篇 ADR 都有状态行；`docs/adr/README.md` 与 `docs/adr/` 文件一一对应；architecture.md 引用的每条 ADR 都确实包含被引用的条款；ADR 0047–0050 已存在。
8. `tim-domain-modeling` 有「ADR 演进」一节；各 Agent 的全局规则入口都有触发句；`agentnave-review` 架构工位有核对项。
9. 三个 PR 都通过 CI 和合并前评审，评审记录留在 PR 评论里；版本仍是 0.66.x，等用户另行要求再开 1.0.0 版本 PR。

## Out of scope

- 拆分超长文件和函数、合并 Rust 限时子进程 helper：发布后按「改到哪拆到哪」处理。
- 语音功能缩减、`Groups` 并入 `Conversations`、合并设置文件、把权限档位下沉到 Adapter：会议中已否决。
- 跨版本热切换。
- 1.0.0 版本 PR、发布说明 `docs/releases/v1.0.0.md` 和发布：用户另行要求后再做。
- CI 自动检查 ADR 一致性。

## Open questions

无。
