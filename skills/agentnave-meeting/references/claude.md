# Claude Code

Claude Code 在 App 内外终端均走 `wait`，不再由 App 键入唤醒。支持 MCP Tasks 的连接（回执 `next` 写着「转为后台任务」；终端、VS Code 与桌面端本地会话）上，阻塞等待省略 `ids` 与 `wait_seconds` 即可：`wait` 立即转为后台等待任务，`/tasks` 里显示员工状态，结果到达时 Claude Code 自动开新一轮；在 `/tasks` 里取消它只停止等待，子代理继续运行，之后照回执再 `wait`。以下 1500 秒与 270 秒的做法用于回执没有这样写的连接。终端交互会话（`CLAUDE_CODE_ENTRYPOINT=cli`）且未关闭后台任务时，阻塞等待省略 `wait_seconds`：服务自动等 1500 秒，约两分钟后由 Claude Code 转为后台任务、结果到达时自动开新一轮；若关闭了后台任务（`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` 等），按回执 `next` 显式传 `wait_seconds=270`；桌面 App、VS Code 等非终端入口同样是 270 秒，按 SKILL.md 常规做法连续等待。调用转入后台后不再另发 `wait`，同一时间只挂一个；若收到 `already_waiting` 说明已有一次收件箱等待在进行，不要再调阻塞式 `wait`（等原等待返回，要查进展或先收就绪结果改用 `wait(wait_seconds=0)`）；可以结束本轮或继续做其他工作，由完成通知唤醒。通知到达后需要继续等待时，再递增 `seq` 发下一次。后台挂着等待时用户问进展，可以对相关 `ids` 调 `wait(wait_seconds=0)` 查；它收到的终态算已收取，后台那次不会再送一遍，以这次为准。如果 `wait` 报的是宿主的工具超时，而不是正常返回 `wait_elapsed`，说明这条连接的超时还没补到 1500 秒以上：比如升级后没有重连，或者项目层配置里还是旧值。这时任务仍在运行，本会话改传 `wait_seconds=270`，重连或重启 Claude Code 后再省略。超时前就绪的结果可能已经被取走，先对相关 `ids` 调 `wait(wait_seconds=0)`，或用 `read_conversation` 核对。

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "claude"`；用户点名时的写法示例：

```json
{"model": "claude-opus-5-5", "effort": "medium"}
```

权限档位「自动」传 `permission_mode=auto`，「完全放开」传 `bypassPermissions`，另附 `permission_fallback` 指向另一档；所选模式在这台电脑或账号上不可用时，服务改用另一档重跑一次，回执 `warnings` 注明实际模式，把它转告用户；两种都不可用时任务受阻。`permission_fallback` 也是权限类参数，boss 不自行传。


macOS／Linux 默认以交互模式运行（App 关掉「交互模式」或显式 `interactive=false` 时改用 `-p`），普通任务与非隔离席位可收到 `prompt_pending`；Windows 一律 `-p`，受限工具路径也用 `-p`。交互模式只是让运行中的权限框、提问和计划确认有人接手，任务不会因此卡住或被自动拒绝；它不是员工向 boss 请示的通道。派发时不要求员工使用提问工具，需要的决定仍写在最终答复里。出现 `prompt_pending` 时按主 Skill 的代答边界调用 `answer_prompt`。

准备阶段，服务在信任屏显示目录与派发 `cwd` 为同一目录时自动确认，并自动允许外部 CLAUDE.md 导入（包含未来新增导入）；Claude 会将选择永久写进 `~/.claude.json`，这是持久放行先问用户的限定例外。目录不符或未知屏受阻。

```json
{"invocation_id":"<wait 中的句柄>","prompt_id":"<弹框编号>","manager_session_id":"<本聊天原生 ID>","decision":"allow"}
```

提问框改传 `answers={"题目原文":"选项或填写的答案"}`，省略 `decision`。计划确认（`ExitPlanMode`，工具输入里是计划正文）按权限框答：允许即批准计划，Claude 按派发时的档位继续；拒绝附理由，Claude 留在计划阶段修改。权限只允许本次调用，服务不保存 Claude 的持久放行规则；答后按回执 `next` 指示的等待方式继续。画面出现弹框但 hook 通道未收到时会报受阻，由 boss 检查，不猜按键。
