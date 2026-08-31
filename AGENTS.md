# AgentNave Agent Entry

## Scope and authority

- `README.md` 面向首次使用者；当前行为以代码、Adapter、CLI/MCP schema 与测试为真源。
- 项目特有术语见 `CONTEXT.md`，稳定边界见 `docs/context/architecture.md`，收敛理由见 `docs/adr/0002-lightweight-cli-subagent-adapter.md`。
- 仓库外的旧版 AgentNave 上下文不是当前 Authority；除非用户明确要求，否则不读取或同步。

## Implementation boundaries

- AgentNave 是只供 Agent 使用的本地 STDIO MCP Server，只做 CLI 子代理适配，不承担规划、角色、DAG、并行策略、审核、综合、重试、worktree、持久化或 UI，也不提供面向人的 CLI。
- Provider Adapter 不得静默设置模型、effort、权限、工具或 prompt 增补；只允许协议必需参数和调用方显式提供的 allowlist options。
- Invocation 只在当前进程内存中存在；server 退出对仍在 Provider 进程组内的活跃进程做 best-effort 清理，不引入跨重启恢复。
- AgentNave 不是沙箱；Provider 原生权限才是安全边界，不声称能隔离主动杀 supervisor、创建新 OS session 或其他同用户恶意逃逸。
- 0.2 只支持 POSIX 进程监督；不得用 Windows process group 冒充 Job Object 的树级所有权。
- stdout 的 JSON 结果遵守 `InvocationResult` 稳定字段；Provider 业务终态必须尽量保全输出和原生 `session_id`。

## Setup and verification

- 开发环境：`uv sync --all-groups`。
- MCP：`uv run agentnave-mcp`。
- Python 门：`uv run ruff format --check .`、`uv run ruff check .`、`uv run pyright`、`uv run pytest`。
- 真实 Provider 调用会消耗配额，只有用户在当前会话明确授权后才运行；测试默认使用 fake executable。

## Context change gate

- 术语变化更新 `CONTEXT.md`；稳定所有权或接口边界变化更新 `docs/context/architecture.md`；难逆转且经过真实取舍的决定写 ADR。
- 精确实现、临时路线、分支、commit、测试输出和实时运行状态不写入长期上下文。
