# Claude Code

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "claude"`；用户点名时的写法示例：

```json
{"model": "claude-opus-5-5", "effort": "medium"}
```

Claude Code 作宿主时，`native=true` 的 Claude 工位交给自带子代理：先核对 Agent 工具的实时 schema，`model` 优先使用工具支持的工位模型；宿主只支持系列别名时，可将能明确归类的 Claude 模型映射为对应的 `opus`、`sonnet`、`haiku`、`fable`，仅在规则块或用户明确授权该映射时使用；未获授权时按工位不可用处理。派发前说明实际可选模型的版本或能力与工位设置的差异，回报中注明实际所用模型及差异；无法确认的版本如实标为未知，不宣称等效。工位 `effort` 有值且工具支持时才传 `effort`，空值省略，不自行补档位。

`[1m]` 的长上下文、`opusplan` 的计划／执行模型切换等语义须单独核对，不能仅剥去后缀或按名称归类就宣称可无条件替代；已授权的系列别名映射若有版本或能力差异，须按上文事前说明并在回报中记录。无法归类的复合模型、无法满足的必要能力或已设档位，以及没有可用替代的情况，按主 Skill 的工位不可用与同难度兜底规则处理；超出现有授权的特定替代须用户明确授权后才采用。

权限档位「自动」传 `permission_mode=auto`，「完全放开」传 `bypassPermissions`，另附 `permission_fallback` 指向另一档；所选模式在这台电脑或账号上不可用时，服务改用另一档重跑一次，回执 `warnings` 注明实际模式，把它转告用户；两种都不可用时任务受阻。`permission_fallback` 也是权限类参数，boss 不自行传。


macOS／Linux 默认以交互模式运行（App 关掉「交互模式」或显式 `interactive=false` 时改用 `-p`），普通任务与非隔离席位可收到 `prompt_pending`；Windows 一律 `-p`，受限工具路径也用 `-p`。按主 Skill 的代答边界调用 `answer_prompt`，不向 TUI 发按键。

```json
{"invocation_id":"<wait 中的句柄>","prompt_id":"<弹框编号>","manager_session_id":"<本聊天原生 ID>","decision":"allow"}
```

提问框改传 `answers={"题目原文":"选项或填写的答案"}`，省略 `decision`。计划确认（`ExitPlanMode`，工具输入里是计划正文）按权限框答：允许即批准计划，Claude 按派发时的档位继续；拒绝附理由，Claude 留在计划阶段修改。权限只允许本次调用，服务不保存 Claude 的持久放行规则；答后继续 `wait`。画面出现弹框但 hook 通道未收到时会报受阻，由 boss 检查，不猜按键。
