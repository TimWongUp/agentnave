# ADR-0084：Windows Claude Code 使用交互模式

- 状态：已接受
- 日期：2026-11-12
- 修订 [ADR-0056](0056-claude-interactive-runtime.md) 的 Windows 运行方式。

## 背景

Windows 此前始终使用 Claude Code 的 `-p`，无法使用与 macOS、Linux 相同的交互会话。现有交互 helper 依赖 POSIX 伪终端和管道等待，不能直接在 Windows 使用。

## 决定

- Windows 使用系统 ConPTY 驱动 Claude Code；helper、Claude 和其后代仍由现有 Windows supervisor 的 kill-on-close Job Object 管理。终端输入和输出各由独立线程服务，避免同步管道互相阻塞。
- 交互 helper 的临时权限文件位于调用专属目录；正常结束或取消后，由服务在进程树停止后清理。服务异常退出时清理是尽力而为。
- Claude 的交互开关与其他平台共用，缺省打开；受限工具仍使用 `-p`。权限请求、任务投递、会话与失败语义沿用 ADR-0056、ADR-0057 和 ADR-0078。
- 交互运行会写入 Claude Code 的历史，并可能永久信任工作目录或确认环境中的 API key，与其他平台一致。交互启动或终端失败时报告阻塞，不静默切回 `-p`。

## 取舍

使用 Windows 系统 ConPTY 不增加运行时依赖，但终端行为仍受 Windows 和 Claude Code 版本影响；需要在 Windows 覆盖终端收发、权限通道与进程清理。Windows 原有的 `-p` 可通过关闭交互开关使用。
