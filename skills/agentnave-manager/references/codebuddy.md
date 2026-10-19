# CodeBuddy Code

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "codebuddy"`；默认 `provider_options`：

```json
{"model": "glm-5.3-flash", "effort": "max"}
```

## 非交互权限与验收

AgentNave 通过 `--print` 调用 CodeBuddy。需要审批的工具若未获原生权限允许，会因非交互模式无法弹出确认而被拒绝；主 Agent 的任务授权不会自动改变 CLI 权限。

沿用用户已明确选择的原生权限设置；需要改变时，说明被拒绝的具体工具，并让用户选择最小范围的允许规则或本次调用的 `permission_mode`。不要为完成任务自动开启 `bypassPermissions` 或永久放行全部 Bash。

CodeBuddy 可能把权限拒绝写入工具结果和最终回答，同时以正常对话终态结束。按主 Skill 的结果验收规则检查实际执行证据；拒绝后尚未完成的操作应报告为未完成，不能因 `succeeded` 宣称执行成功。

## 运行中消息

普通 task 可显式传 `enable_messages=true`，要求所选 CLI >=2.142.0，切换为持续 stream-json 输入。通过 `send_agent_message(invocation_id, message)` 发送 `steer`，只有收到主会话原生 `_requestId` 后才允许写入，并以 `expected_request_id` 绑定当前轮。`accepted / buffered_by_active_turn` 只表示原生已缓冲，继续等待最终结果；缺少绑定时不投递，过期轮次拒绝，不改成普通排队或中断续接。保留原模型、effort 和权限设置；默认任务路径不变。协议回归通过不等于真实模型验收，详见[安装说明](../../../docs/installation.md#运行中消息能力)。
