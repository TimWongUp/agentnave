# Codex CLI

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "codex"`；默认 `provider_options`：

```json
{"model": "gpt-6.1-sol", "effort": "high"}
```

在非 Git 目录执行已获授权的任务时，显式加入布尔选项 `"skip_git_repo_check": true`；Git 目录无需加入。该选项只处理仓库检查，不扩大命令权限。

普通任务需运行中纠偏时，可显式 `start_agent(enable_messages=true)`，要求所选 Codex app-server >=0.99.0，再用原 `invocation_id` 调 `send_agent_message(mode="steer")`。AgentNave 固定原 threadId / expectedTurnId，过期轮次由原生拒绝；accepted 只表示该轮接受输入，仍需等待最终结果。默认调用继续走 exec；消息模式保留宿主 Provider 排除，不与 restricted_tools 组合。模型、effort 与权限沿用原请求与既有配置，不因切换协议自动放宽；遇原生审批则上报阻塞，不自动批准或切换配置。
