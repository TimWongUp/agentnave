# ADR-0082：长等待档只给 Claude Code 终端交互会话

- 状态：已接受
- 日期：2026-11-11
- 修订 [ADR-0077](0077-claude-long-wait.md) 的适用范围。

ADR-0077 依据终端 CLI 的行为，把 1500 秒窗口给了所有 Claude Code 宿主。但 Claude Code 只在交互会话里把满两分钟的 MCP 调用转入后台。桌面 App 以 stream-json 启动，被当作非交互会话，默认不转后台，官方也不为它和 VS Code 提供 MCP tasks。结果是桌面 App 里的 `wait(1500)` 会在前台静默阻塞最长 25 分钟。

因此服务按继承到的 `CLAUDE_CODE_ENTRYPOINT` 判断：宿主是 Claude Code 且入口为 `cli` 时，才在 `wait_seconds` 的可选值里公布 1500 并接受它；桌面 App、VS Code、`-p`/SDK 以及读不到入口的情况，与其他宿主一样只有 0 和 270。boss 只看得到当前可用的值，不必先传 1500 再被拒。

考虑过的做法：
- App 在 Claude Code 用户设置里写入 `CLAUDE_AUTO_BACKGROUND_TASKS=1`，让桌面 App 也转后台。没有采用，因为用户设置同时作用于 `claude -p` 和 SDK；这类一次性运行在 MCP 调用转后台后很快退出，结果会丢失。桌面 App 自己的本地环境变量能把范围限定在桌面，但它加密存储，App 无法写入。
- 服务把 `wait` 实现为 MCP task。没有采用，因为桌面 App 和 VS Code 不支持 MCP tasks，终端 CLI 本来就会转后台，收益只剩省去前两分钟。

App 为 Claude Code 条目补的 `timeout` 下限保持不变：终端和桌面 App 共用这个条目，桌面 App 只用 270 秒窗口，较长的超时不会造成影响。
