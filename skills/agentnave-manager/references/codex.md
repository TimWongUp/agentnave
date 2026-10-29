# Codex CLI

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "codex"`；默认 `provider_options`：

```json
{"model": "gpt-6.1-sol", "effort": "high"}
```

用户在 App 打开「允许在 git 仓库外运行」时，服务已为每次 Codex 调用带上 `skip_git_repo_check`（见 `defaults_applied` 或 `list_members` 的 `cli_options`）；未打开时，在非 Git 目录执行已获授权的任务需显式加入布尔选项 `"skip_git_repo_check": true`，Git 目录无需加入。该选项只处理仓库检查，不扩大命令权限。

`sandbox`（`read-only`、`workspace-write`、`danger-full-access`）设定本次调用的原生沙箱模式，续接同样生效；它是权限类参数，只能来自 App 默认值或用户明确要求。

`"approve_for_me": true` 由 Codex 的自动审核批准工具调用，命令在 `workspace-write` 沙箱内运行（新任务传 `--approve-for-me`，续接改用等效的 `--config`）；不能与 `sandbox` 或 `dangerously_bypass_approvals_and_sandbox` 同传。它是权限档位「自动」（缺省）对应的选项，也是权限类参数。

`"dangerously_bypass_approvals_and_sandbox": true` 开启原生 YOLO（`--dangerously-bypass-approvals-and-sandbox`），本次调用及续接都跳过审批并关闭沙箱，仅在用户同时授权这两项时使用。传 `false` 或省略（且无 App 默认值）不加参数、继承原生设置，不会强制恢复沙箱或审批；CLI 已存配置不变。
