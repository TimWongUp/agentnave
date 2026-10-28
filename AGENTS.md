# AgentNave Agent Entry

## Scope and authority

- `README.md` 面向首次使用者；当前行为以代码、Adapter、CLI/MCP schema 与测试为真源。
- 项目特有术语见 `CONTEXT.md`，稳定边界见 `docs/context/architecture.md`，收敛理由见 `docs/adr/0002-lightweight-cli-subagent-adapter.md`。
- 仓库外的旧版 AgentNave 上下文不是当前 Authority；除非用户明确要求，否则不读取或同步。

## Implementation boundaries

- AgentNave 是只供 Agent 使用的本地 STDIO MCP Server，发行包 `agentnave` 只提供 `agentnave-mcp` 一个入口。统一核心负责 CLI 子代理、保存的并联／串联协作轮次、任务历史、可选审核、主持人发言与按需只读看板；App 负责接入、组员配置、展示与历史清理。不再提供无历史核心模式（ADR-0016）。入口先启动只用标准库的中继，由它运行真正服务，安装变化且空闲时换用新进程；保存的协作组不固定旧进程。单向编排通过统一派发指令约束：子代理完成任务后回报 Manager，不再调用 AgentNave；不检测嵌套或据此禁用工具（ADR-0031）。规划、角色、模型、轮次编排、审核判断、综合、重试与 worktree 仍由调用方 Manager 决定，不提供面向人的 CLI。
- Provider Adapter 不得静默设置模型、effort、权限、工具或 prompt 增补；除协议必需参数与调用方显式参数外，只允许套用用户在 AgentNave App 中设定、且在回执中注明的组员默认值；唯一例外是 DeepSeek 官方路由在未显式指定时恢复 DeepSeek Harness App 已存的认证（ADR-0027）。`restricted_tools=true` 是显式受限配置；普通 Invocation 不继承此配置。权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`、`approve_for_me`）只能来自服务按 CLI 套用的权限档位（缺省「自动」，用户可在 App 改为「完全放开」；CLI 没有可用的自动审核时「自动」即完全放开，见 ADR-0032）、用户在 App 中设定的默认 options，或用户本轮明确要求；boss 不得自行追加。权限档位与 App 权限默认值也会在续接时套用，并在回执中注明。
- Invocation 只在当前进程内存中存在；server 退出对仍在 Provider 进程树内的活跃进程做 best-effort 清理，不引入跨重启恢复。
- AgentNave 不是沙箱；Provider 原生权限才是安全边界，不声称能隔离主动杀 supervisor、创建新 OS session 或其他同用户恶意逃逸。
- macOS／Linux 使用专用 POSIX 进程组 supervisor；Windows 使用挂起创建并先加入
  kill-on-close Job Object 的专用 supervisor，不得用 Windows process group 冒充树级所有权。
- stdout 的 JSON 结果遵守 `InvocationResult` 稳定字段；Provider 业务终态必须尽量保全输出和原生 `session_id`。

## Setup and verification

- 开发环境：`uv sync --all-groups`。
- MCP：`uv run agentnave-mcp`。
- Python 门：`uv run ruff format --check .`、`uv run ruff check .`、`uv run pyright`、`uv run pytest`。
- 托盘门（改动 `apps/tray` 或看板前端时，在 `apps/tray` 下）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
- 托盘界面目测：`scripts/preview_tray.sh` 把当前分支编译成「AgentNave 预览」并打开（macOS），与已装正式版并存、读同一份数据；预览版不改 AgentNave 数据、宿主配置和 Skill（改动数据的请求一律拒绝，启动也不修复宿主接入），不更新、不升级服务；只有它自己的界面显示偏好单独保存。界面改动先用它给用户看，不必退出正式版。AGENTNAVE_PREVIEW 构建的 clippy 用 `AGENTNAVE_PREVIEW=1 CARGO_TARGET_DIR=target/preview cargo clippy --all-targets -- -D warnings`。
- 真实 CLI 标准套件：`uv run python scripts/real_suite.py [--duration 秒] [--providers ...]`，用各 CLI 的便宜模型覆盖服务各模式并持续运行（默认 10 分钟），会消耗配额，执行前同样需要授权；加 `--real-history` 写入真实历史，可在托盘和看板实时查看，标题用可读名称，工位角色为 `real-suite` 以便识别测试数据，事后在 App 清理；宿主自己的 CLI 不会被 `start_agent` 派发，需要覆盖它时从普通终端运行。
- 新增或接入 Provider CLI 的验收必须包含至少一次通过 AgentNave 发起的真实调用，不得以 fake executable 或代码模拟替代；真实调用会消耗配额，执行前仍须取得用户在当前会话的明确授权，未获授权时停下请求授权，不得声称接入完成。其他测试默认使用 fake executable。

## Context change gate

- 术语变化更新 `CONTEXT.md`；稳定所有权或接口边界变化更新 `docs/context/architecture.md`；难逆转且经过真实取舍的决定写 ADR。
- 精确实现、临时路线、分支、commit、测试输出和实时运行状态不写入长期上下文。

## Version releases

- 用户明确要求更新本项目版本时，按完整发布任务执行：同一任务完成版本 PR、审查、合并，并等待对应 tag 与 GitHub Release 发布成功；用户明确限制交付范围时遵从该限制。

- 版本 PR 同步更新版本（`pyproject.toml`、`src/agentnave/__init__.py`、`apps/tray/Cargo.toml`）、`uv.lock`、`apps/tray/Cargo.lock`、安装文档和 `docs/releases/v<version>.md`；安装文档只保留当前版本的升级说明，替换上一版的说明，历史升级说明留在 `docs/releases/`。
- 版本号变更合入 `origin/main` 后，由 `.github/workflows/python.yml` 在 macOS/Linux 验证通过后自动创建对应 tag 和 GitHub Release，并附上签名的 macOS 托盘安装包、更新包与 `latest.json`（需仓库 Secret `TAURI_SIGNING_PRIVATE_KEY`）；普通合并不发布，相同版本不重复发布，也不移动已有 tag。
- 发布任务须等待该工作流和 GitHub Release 发布成功后交付。瞬时或外部故障解决后重跑原工作流；若需修改代码或发布元数据，则通过新 PR 再次增加版本，同版本普通 push 不重试此前发布。不绕过 CI 手动提前发布。
