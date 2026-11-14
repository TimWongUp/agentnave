# ZCode

`provider: "zcode"`，首次使用先调用 `list_members(provider="zcode")`。ZCode 同时支持 boss 和工位；作为 boss 时同类工位从私聊排除，仍可参与会议。

工位通过 ZCode 原生 `app-server --stdio` 运行，保留原生认证与会话。macOS 未安装独立 CLI 时，可使用 ZCode App 附带的官方 Node bundle，需要可执行的 Node.js。Windows／Linux 使用 PATH 中的 `zcode`。

`model` 使用 ZCode 模型目录中的 `providerId/modelId`，例如 `my-route/my-model`；裸模型名仅在目录中唯一匹配时可用。`effort` 映射为原生 `thoughtLevel`，可用值取决于所选模型。某些路由要求指定模型时同时提供 effort；原生拒绝时返回原错误，不替换模型。省略字段保留原生设置；会话模型设置不写成工作区默认值。

`permission_mode` 支持 `auto`、`build`、`edit`、`plan`、`yolo`、`guarded`。由服务套用 App 权限档位，ZCode 的 `auto` 为未实现的预留模式，故自动与完全放开均对应 `yolo`，续接同样生效。Manager 不自行追加权限选项。`restricted_tools` 暂不支持，不能用 prompt 声称工具已禁用。

使用结果的 `sess_...` 原生 `session_id` 在相同 cwd 续接，不使用 Invocation ID。普通任务保留原生配置和工具。boss 原生 ID 未由宿主提供时，按主 Skill 生成并复用当前聊天的归属 UUID，不把共享 MCP 配置写成固定聊天 ID。

宿主用户级 MCP：`~/.zcode/cli/config.json` 的 `mcp.servers.agentnave`；Skill：`~/.zcode/skills/agentnave-manager`。修改后新建会话或重启 ZCode。
