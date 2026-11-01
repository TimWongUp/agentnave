# AgentNave Agent Entry

## Scope and authority

- `README.md` 面向首次使用者；当前行为以代码、Adapter、CLI/MCP schema 与测试为真源。
- 项目特有术语见 `CONTEXT.md`，稳定边界见 `docs/context/architecture.md`，收敛理由见 `docs/adr/0002-lightweight-cli-subagent-adapter.md`。
- 仓库外的旧版 AgentNave 上下文不是当前 Authority；除非用户明确要求，否则不读取或同步。

## Implementation boundaries

- AgentNave 是只供 Agent 使用的本地 STDIO MCP Server，只提供 `agentnave-mcp` 一个入口，不提供面向人的 CLI；服务、App、Manager 与 Provider CLI 的所有权划分见 `docs/context/architecture.md`。规划、角色、模型、轮次编排、审核判断、综合、重试与 worktree 由调用方 Manager 决定。
- Provider Adapter 不得静默设置模型、effort、权限、工具或 prompt 增补；参数只能来自协议必需、调用方显式或回执注明的 App 默认值，权限类参数另受权限档位约束，boss 不得自行追加，要点见 `docs/context/architecture.md` 的「参数与默认值」，细则见 ADR-0028、ADR-0032。Provider 专属的协议、解析与模型回读放进 Adapter 及其钩子，核心与历史层不按 Provider 名分支；默认值与权限档位映射仍在服务入口套用，不下沉到 Adapter；Adapter 不在模块级导入 `agentnave.host`。
- Invocation 只在当前进程内存中存在；server 退出对仍在 Provider 进程树内的活跃进程做 best-effort 清理，不引入跨重启恢复。
- AgentNave 不是沙箱；Provider 原生权限才是安全边界，不声称能隔离主动杀 supervisor、创建新 OS session 或其他同用户恶意逃逸。
- macOS／Linux 使用专用 POSIX 进程组 supervisor；Windows 使用挂起创建并先加入
  kill-on-close Job Object 的专用 supervisor，不得用 Windows process group 冒充树级所有权。
- stdout 的 JSON 结果遵守 `InvocationResult` 稳定字段；Provider 业务终态必须尽量保全输出和原生 `session_id`。

## Code readability

Python、Rust、JS 的生产代码按下列规定写。行数是触发检查的信号，目的是减少读者一次要记住的内容。

- 函数超过 100 行时，按步骤拆成有名字的函数，主函数读起来像目录；多个函数共用的局部状态收进一个小结构。声明式数据表、MCP 参数定义和 JSON 投影字面量不计入行数。
- 文件超过 1000 行时，按职责放进同名子模块（如 `stage.rs` 与 `stage/show.rs`）；新内容进子模块。
- 改到已超限的函数或文件时，小修复就地改；新增功能放进新函数或子模块。存量超限不用汇报，也不用询问，除非用户要求处理。
- 同样的校验、错误响应或工具函数出现第三次时抽成一处，跨文件的相同实现合并。
- Rust 内联测试超过 100 行时移到 `<模块>/tests.rs`。

## Setup and verification

- 开发环境：`uv sync --all-groups`。
- MCP：`uv run agentnave-mcp`。
- Python 门：`uv run ruff format --check .`、`uv run ruff check .`、`uv run pyright`、`uv run pytest`。
- 托盘门（改动 `apps/tray` 或看板前端时，在 `apps/tray` 下）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
- 托盘界面目测：`scripts/preview_tray.sh` 把当前分支编译成「AgentNave 预览」并打开（macOS），与已装正式版并存、读同一份数据；预览版不改 AgentNave 数据、宿主配置和 Skill（改动数据的请求一律拒绝，启动也不修复宿主接入），不更新、不升级服务；只有它自己的界面显示偏好单独保存。界面改动先用它给用户看，不必退出正式版。AGENTNAVE_PREVIEW 构建的 clippy 用 `AGENTNAVE_PREVIEW=1 CARGO_TARGET_DIR=target/preview cargo clippy --all-targets -- -D warnings`。
- 真实 CLI 标准套件：`uv run python scripts/real_suite.py [--duration 秒] [--providers ...]`，用各 CLI 的便宜模型覆盖服务各模式并持续运行（默认 10 分钟），会消耗配额，执行前同样需要授权；加 `--real-history` 写入真实历史，可在托盘和看板实时查看，标题用可读名称，工位角色为 `real-suite` 以便识别测试数据，事后在 App 清理；宿主自己的 CLI 不会被 `start_agent` 派发，需要覆盖它时从普通终端运行。
- 接入为组员的 Provider CLI 必须同时接入为宿主（boss）；未经真实调用验收的宿主不接入。
- 新增或接入 Provider CLI 的验收必须包含至少一次通过 AgentNave 发起的真实调用，不得以 fake executable 或代码模拟替代；真实调用会消耗配额，执行前仍须取得用户在当前会话的明确授权，未获授权时停下请求授权，不得声称接入完成。其他测试默认使用 fake executable。

## Art assets

- 生成或修改托盘美术素材前先读 `apps/tray/ART-SOURCES.md`，新素材必须直接输出真透明底。

## Context change gate

- 术语变化更新 `CONTEXT.md`；所有权、模块依赖方向、安全边界或跨进程／跨语言合同（中继与服务、服务与托盘、两端共用的数据根文件）变化时更新 `docs/context/architecture.md`，只写边界与合同要点，可以写数据根文件名及其写入方，不写参数、字段、路由或错误文本等能从代码、schema 和测试读到的细节；新增功能本身不是更新它的理由。难逆转且经过真实取舍的决定写 ADR。
- 精确实现、临时路线、分支、commit、测试输出和实时运行状态不写入长期上下文。

## Version releases

- 用户明确要求更新本项目版本时，按完整发布任务执行：同一任务完成版本 PR、审查、合并，并等待对应 tag 与 GitHub Release 发布成功；用户明确限制交付范围时遵从该限制。

- 版本 PR 同步更新版本（`pyproject.toml`、`src/agentnave/__init__.py`、`apps/tray/Cargo.toml`）、`uv.lock`、`apps/tray/Cargo.lock`、安装文档、`README.md` 与 `README.en.md` 中的配套版本和 `docs/releases/v<version>.md`；安装文档只保留当前版本的升级说明，替换上一版的说明，历史升级说明留在 `docs/releases/`。
- 版本号变更合入 `origin/main` 后，由 `.github/workflows/python.yml` 在 macOS/Linux 验证通过后自动创建对应 tag 和 GitHub Release，并附上签名的 macOS 托盘安装包、更新包与 `latest.json`（需仓库 Secret `TAURI_SIGNING_PRIVATE_KEY`）；普通合并不发布，相同版本不重复发布，也不移动已有 tag。
- 发布任务须等待该工作流和 GitHub Release 发布成功后交付。瞬时或外部故障解决后重跑原工作流；若需修改代码或发布元数据，则通过新 PR 再次增加版本，同版本普通 push 不重试此前发布。不绕过 CI 手动提前发布。
