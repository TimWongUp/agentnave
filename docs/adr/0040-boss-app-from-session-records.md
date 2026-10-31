# ADR-0040：工作区按会话记录显示 boss 的入口，接入按配置文件划分

- 状态：已接受
- 日期：2026-10-31

## 背景

Claude 和 Codex 都有桌面端和单独的 CLI。用户要在工作区里分清 boss 来自哪一边，而接入页只有 `claude`、`codex` 各一行。原先只有 Claude 能区分，且只看工作区保存的对话记录路径，会话换过目录、记录被归档或工作区早于 `workspaces` 表时都识别不出来，一律显示成「Claude Code」。

## 决定

- 接入页按实际目录划分，不按产品划分：入口读同一份配置就合成一行，否则各占一行。共用只是默认目录下的结果，App 每次按解析出的目录比较（符号链接按实际位置算）。
  - Claude 桌面端读登录 shell 变量时明确丢弃 `CLAUDE_CONFIG_DIR`，所以 `claude` 行的目录不是默认的 `~/.claude` 时，已装 Claude 桌面端的机器多一行「Claude 桌面」，写默认位置的 `~/.claude.json`。这个变量如果是 App 进程本身就带的，桌面端同样带着，不分行。
  - Codex 桌面端继承登录 shell 变量，与 `codex` 命令走同一个 `CODEX_HOME`；另外的目录是多账号用的。home 下名字以 `codex`、`.codex` 开头的目录及其下一层，含 `auth.json` 或 `sessions/` 的，除主目录外各占一行（规则同 magpie）。
  - 多出来的行写的 `AGENTNAVE_HOST` 仍是 `claude`、`codex`，服务和调度看到的宿主不变。
- 工作区按会话自己的记录显示入口，`host` 仍是 `claude` 或 `codex`，「仅开会」、私聊排除与交接不变：Claude 看对话记录最新的 `entrypoint`（`claude-desktop` 开头为桌面端），Codex 看 rollout 的 `originator`（含 `desktop` 为桌面端，`codex-tui`、`codex_cli_rs`、`codex_exec`、`codex-cli` 为 CLI）。
- 找不到保存的路径时，由 App 按会话 ID 算出 boss 键，到接入页解析出的每个 Claude、Codex 目录的 `projects`、`sessions`、`archived_sessions`（含 zstd 压缩）里找；对话记录已不在时，再看 Claude 桌面端 `Claude`、`Claude-3p` 目录下的 `claude-code-sessions` 记录。
- 无法判断入口时只显示「Claude」或「Codex」，不猜。
- 全部只读，由 App 完成，服务和数据库不变。

## 取舍

- 这些字段不是 CLI 的公开合同，升级后可能变化；变化时退回不判断入口，不会影响接入和调度。判定方式参考 magpie（`yetone/magpie`），后续变化优先对照它。
- 未传原生会话 ID 的旧工作区，对话记录也已不在时，永远无法判断。
- 桌面端也能在自己的设置里改 `CLAUDE_CONFIG_DIR`，App 读不到这项设置；此时接入页以为它还在默认目录。
- 桌面端丢弃 `CLAUDE_CONFIG_DIR` 来自 macOS 版 Claude 2026-10 的代码，继承 shell 变量是在 macOS 版 Codex 上比较其他变量得出的，Windows、Linux 未实测。`codex-tui` 只在 Codex 0.161.0 实测。
- Claude 桌面端的聊天模式读 `claude_desktop_config.json`，配置不共用，按本决定应单独成行；它是新的宿主，要经真实调用验收，不在本决定内落地。
