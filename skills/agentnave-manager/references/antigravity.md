# Antigravity CLI

`provider: "antigravity"`；默认 `provider_options`：

```json
{"model": "gemini-3.8-flash", "effort": "high"}
```

对应 `agy` CLI。原生 `print_timeout` 与 AgentNave 的总运行上限、单次等待时长是不同参数。

## 项目目录

在项目内执行任务时，同时传入项目绝对路径 `cwd` 和显式 `provider_options.project`。`cwd` 只设置 CLI 进程启动目录；未选择原生项目时，工具仍可能在 CLI 的 `scratch` 中运行。本机已验证 `project` 传项目绝对路径可使 `pwd` 返回该目录；原生帮助也支持项目 ID 或名称，已有对应项目时可使用已核对的值。

首次在该项目调用时，让 CLI 用只读 `pwd` 核对工具实际目录，再进行项目操作；若返回目录不符，先报告并解决项目选择，不继续修改。会话续接保留已核对的项目绑定，不把旧会话静默转到另一项目。
