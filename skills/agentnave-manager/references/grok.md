# Grok CLI

Grok 作宿主时按回执 `next` 选择等待方式（App 内外终端一致，不再由 App 键入唤醒）：已验收版本的交互 `grok` CLI（1.0.92 及以上）会在 `next` 里给出只读等待命令，用 `run_terminal_command` 并显式传 `block_until_ms: 0` 原样运行（立即转入后台，省略会阻塞 30 秒，不挂阻塞 `wait`）；命令结束后自动开新一轮，按输出行处理——`ready` 或 `expired` 调 `wait(wait_seconds=0)` 收取并按新回执续等，`already_watching` 或 `all_done` 直接结束本轮，`service_gone` 回到当前连接核对；非交互或未确认版本仍按 `next` 前台 `wait`。

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "grok"`；用户点名时的写法示例：

```json
{"model": "grok-4.7", "effort": "high"}
```
