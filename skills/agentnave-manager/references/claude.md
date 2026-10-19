# Claude Code

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "claude"`；默认 `provider_options`：

```json
{"model": "claude-opus-5-5", "effort": "medium"}
```

普通任务需运行中追加输入时，可显式 `start_agent(enable_messages=true)`，要求 Claude >=2.1.295；发送时必须显式 `send_agent_message(mode="queue")`。queue 允许原生把消息并入当前或后续 turn，不保证同轮；默认 steer 明确拒绝，不自动转成 queue 或 interrupt。accepted 只表示接收，不是任务完成。AgentNave 追踪全部输入与队列结果，等最后一轮 result 后返回最终回答；字段不满足协议时失败。模型、effort、预算、权限等 CLI 参数沿用原请求。完成后仍用 session_id 续接，组席位暂不开放。
