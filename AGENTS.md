# AgentNave Agent Entry

## Scope and authority

- `README.md` 面向首次使用者；当前行为以代码、Adapter、CLI/MCP schema 与测试为真源。
- 项目特有术语见 `GLOSSARY.md`；现行架构基线是 `docs/context/architecture.md`，ADR 索引与各篇状态见 `docs/adr/README.md`。
- 仓库外的旧版 AgentNave 上下文不是当前 Authority；除非用户明确要求，否则不读取或同步。

## Implementation boundaries

- AgentNave 是只供 Agent 使用的本地 STDIO MCP Server，对外只提供 `agentnave-mcp` 一个入口（服务包内另有一个供 boss 在后台运行的只读等待模块，ADR-0114），不提供面向人的 CLI；服务、App、Manager 与 Provider CLI 的所有权划分见 `docs/context/architecture.md`。规划、角色、模型、轮次编排、审核判断、综合、重试与 worktree 由调用方 Manager 决定。
- Provider Adapter 不得静默设置模型、effort、权限、工具或 prompt 增补；参数只能来自协议必需、调用方显式或回执注明的 App 默认值，权限类参数另受权限档位约束，boss 不得自行追加，要点见 `docs/context/architecture.md` 的「参数与默认值」，细则见 ADR-0028、ADR-0032。Provider 专属的协议、解析与模型回读放进 Adapter 及其钩子，核心与历史层不按 Provider 名分支；默认值与权限档位映射仍在服务入口套用，不下沉到 Adapter；Adapter 不在模块级导入 `agentnave.host`。
- Invocation 只在当前进程内存中存在；server 退出对仍在 Provider 进程树内的活跃进程做 best-effort 清理，不引入跨重启恢复。`start_agent` 终端模式（派生会话）的进程归托盘，不由服务 supervisor 监督，server 退出不结束也不中断它（ADR-0109）。
- AgentNave 不是沙箱；Provider 原生权限才是安全边界，不声称能隔离主动杀 supervisor、创建新 OS session 或其他同用户恶意逃逸。
- macOS／Linux 使用专用 POSIX 进程组 supervisor；Windows 使用挂起创建并先加入
  kill-on-close Job Object 的专用 supervisor，不得用 Windows process group 冒充树级所有权。
- stdout 的 JSON 结果遵守 `InvocationResult` 稳定字段；Provider 业务终态必须尽量保全输出和原生 `session_id`。
- 交互 TUI 的驱动、回合终点判断、推送投递和终端拥有者，先查 herdr（<https://github.com/herdrdev/herdr>，Apache-2.0）和 magpie（<https://github.com/yetone/magpie>，MIT）怎么做：有成熟做法就复用或移植，移植代码保留许可证要求的署名并在 PR 注明出处；它们没覆盖或与现行 ADR 冲突时才自行设计，冲突先修订 ADR。
- 办公室的会话界面与 Git、PR 状态的显示，先参考 Claude 桌面端的交互和 t3code（<https://github.com/pingdotgg/t3code>，MIT）的做法，规则同上；托盘不做提交、推送、开 PR 等 Git 写操作（ADR-0126）。

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
- 托盘门（改动 `apps/tray` 时，在 `apps/tray` 下）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`；涉及预览版构建时另跑 `AGENTNAVE_PREVIEW=1 CARGO_TARGET_DIR=target/preview cargo clippy --all-targets -- -D warnings`。
- 托盘预览与实机界面验收（macOS）：用 `scripts/preview_tray.sh` 编译并打开「AgentNave 预览」（在 worktree 里自动带目录名、互不覆盖），与正式版并存且不写正式数据、宿主配置或 Skill（隔离细则见 `docs/context/architecture.md`）。每次交付预览版或发布／安装正式版前，发版者必须通过桌面控制或其他实机手段亲自检查运行中的托盘界面与设置，确认无报错或界面 bug 后再交给用户，不得未经验证直接丢给用户（实机尝试后仍受限时须明确说明未验证项）。本机正式版（`/Applications/AgentNave.app` 及内嵌服务）只装已评审并合入 `origin/main` 的代码，未合并改动只用预览版；PR 合并或放弃后，先退出并删掉 `~/Applications` 中对应预览版，再清理 worktree。
- 真实 CLI 验收与标准套件：新增或接入 Provider CLI 必须同时接入为宿主（boss），且至少包含一次通过 AgentNave 发起的真实调用验收，不得以 fake executable 或模拟替代（其他测试默认使用 fake executable）。`uv run python scripts/real_suite.py` 可用便宜模型持续覆盖各模式（参数见脚本头部说明；`--real-history` 会以 `real-suite` 角色写入真实历史，事后在 App 清理；宿主自身 CLI 不会被 `start_agent` 派发，需从普通终端覆盖）。任何真实 CLI 调用都会消耗配额，执行前须取得当前会话的明确授权；未获授权时停下请求授权，不得声称验收完成。

## Art assets

- 生成或修改托盘美术素材前先读 `apps/tray/ART-SOURCES.md`，新素材必须直接输出真透明底。

## Context change gate

- 术语变化更新 `GLOSSARY.md`；所有权、模块依赖方向、安全边界或跨进程／跨语言合同（中继与服务、服务与托盘、两端共用的数据根文件）变化时更新 `docs/context/architecture.md`，只写边界与合同要点，可以写数据根文件名及其写入方，不写参数、字段、路由或错误文本等能从代码、schema 和测试读到的细节；新增功能本身不是更新它的理由。难逆转且经过真实取舍的决定写 ADR。
- 精确实现、临时路线、分支、commit、测试输出和实时运行状态不写入长期上下文。

## Version releases

- 执行版本更新、核对发版与本机正式版一致性或处理发布工作流失败时，先读 `docs/agents/release-process.md`。
- 用户明确要求更新版本时，按完整发布任务执行（版本 PR、审查、合并、等待 CI tag 与 GitHub Release 全部产物发布成功；用户明确缩小范围时遵从）。
- 发布内容须与本机正式版一致（版本元数据除外），只经 `.github/workflows/python.yml` 自动发布，不手动建 tag／发布、不移动已有 tag、不绕过 CI 提前发布。

## Agent skills

### Issue tracker

GitHub Issues (`TimWongUp/agentnave`, via `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Canonical five-role triage label vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context (`GLOSSARY.md`, `docs/context/architecture.md`, and `docs/adr/`). See `docs/agents/domain.md`.
