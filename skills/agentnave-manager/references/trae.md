# TRAE CLI

`provider: "trae"`，首次使用先调用 `list_members(provider="trae")`。组员使用独立的官方 `traecli`，TRAE CN 桌面应用的启动脚本不能代替它。CLI 的账号可用性与桌面版分别核对；登录或模型不可用时报告原错误。

普通任务经原生 `exec --json` 运行，结果的原生 `session_id` 用于续接。沿用主 Skill 的模型、权限与续接规则；支持的 options 以当前 `list_members` 回执为准。模型目录没有列出可用模型时，先处理 CLI 登录和目录刷新，不根据内置包中的模型名猜测账号权限。

支持 `model`、`permission_mode`，新会话还支持 `sandbox`；续接不支持 `sandbox`。不支持 `effort` 和 `restricted_tools`。权限参数只能来自用户明确要求或 App 配置，不能为绕过失败自行追加。当前 CLI 的 `exec` 不接受 `--no-daemon`，不要把交互会话的启动参数套到员工调用上。

TRAE CLI 和 TRAE CN 桌面版都能当 boss，两者的 MCP 配置、Skill 和规则目录分开管理。作为 TRAE boss 时，单独任务不派给同类 TRAE CLI；会议仍按成员配置决定。boss 聊天没有公开原生 ID 时，按主 Skill 为当前聊天生成并复用一个 UUID，不把固定聊天 ID 写入共享 MCP 配置。
