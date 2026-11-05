# Claude Code

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "claude"`；用户点名时的写法示例：

```json
{"model": "claude-opus-5-5", "effort": "medium"}
```


App 开启「交互模式」（或显式 `interactive=true`）时，macOS／Linux 普通任务与非隔离席位可收到 `prompt_pending`；Windows、`-p` 与受限工具路径不变。按主 Skill 的代答边界调用 `answer_prompt`，不向 TUI 发按键。

```json
{"invocation_id":"<wait 中的句柄>","prompt_id":"<弹框编号>","manager_session_id":"<本聊天原生 ID>","decision":"allow"}
```

提问框改传 `answers={"题目原文":"选项或填写的答案"}`，省略 `decision`。计划确认（`ExitPlanMode`，工具输入里是计划正文）按权限框答：允许即批准计划，Claude 按派发时的档位继续；拒绝附理由，Claude 留在计划阶段修改。权限只允许本次调用，服务不保存 Claude 的持久放行规则；答后继续 `wait`。画面出现弹框但 hook 通道未收到时会报受阻，由 boss 检查，不猜按键。
