# Codex CLI

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "codex"`；默认 `provider_options`：

```json
{"model": "gpt-6.1-sol", "effort": "high"}
```

在非 Git 目录执行已获授权的任务时，显式加入布尔选项 `"skip_git_repo_check": true`；Git 目录无需加入。该选项只处理仓库检查，不扩大命令权限。
