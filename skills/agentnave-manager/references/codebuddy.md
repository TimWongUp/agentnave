# CodeBuddy Code

`provider: "codebuddy"`；默认 `provider_options`：

```json
{"model": "hy4-preview", "effort": "high"}
```

## 非交互权限与验收

AgentNave 通过 `--print` 调用 CodeBuddy。需要审批的工具若未获原生权限允许，会因非交互模式无法弹出确认而被拒绝；主 Agent 的任务授权不会自动改变 CLI 权限。

沿用用户已明确选择的原生权限设置；需要改变时，说明被拒绝的具体工具，并让用户选择最小范围的允许规则或本次调用的 `permission_mode`。不要为完成任务自动开启 `bypassPermissions` 或永久放行全部 Bash。

CodeBuddy 可能把权限拒绝写入工具结果和最终回答，同时以正常对话终态结束。按主 Skill 的结果验收规则检查实际执行证据；拒绝后尚未完成的操作应报告为未完成，不能因 `succeeded` 宣称执行成功。
