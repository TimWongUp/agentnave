# Pi Coding Agent

`provider: "pi"`；省略模型和 effort 时沿用 Pi 原生设置，不指定默认模型。
用户选择模型时传 `model`（可用 `provider/model`）；选择思考等级时传 `effort`，映射为 `--thinking`，支持 `off`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`。

首次使用先调用 `describe_provider("pi")`。支持的其他选项为 `provider`（模型后端，须同时提供 `model`）、`tools`（原生工具允许列表）和布尔 `discussion_mode`。普通任务保留原生工具与配置；Pi 没有内置的文件或进程权限隔离。

使用结果返回的 `session_id` 在相同 cwd 续接，Adapter 映射为 `--session`，不使用自动创建会话的 `--session-id`。讨论模式显式关闭工具、扩展、MCP、Skill 与 prompt template 发现，保留原生会话；只接受模型和 effort。

安装：`npm install -g --ignore-scripts @earendil-works/pi-coding-agent`。运行 `pi` 后通过 `/login` 完成认证。真实调用沿用已有凭据，不把密钥放入 prompt 或 Provider Options。
