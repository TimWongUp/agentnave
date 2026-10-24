# DeepSeek Harness

App 中配置了组员时以 `list_members` 为准，超出模式范围会被拒绝。

`provider: "deepseek"`；对应 DeepSeek Harness 的 `dsh --profile headless --json`，prompt 经 stdin 原样传入。未配置 App 默认值且省略模型和 effort 时沿用 dsh profile 的默认模型（发行版为 `deepseek-flash`），不指定默认模型。用户选择时示例：

```json
{ "model": "deepseek-v4-pro", "effort": "high" }
```

dsh 没有模型、effort 或权限的命令行参数；Adapter 把显式选项写成系统临时目录中的 `--patch` 覆盖层，调用结束即删除，不改用户的 dsh 配置。`model` 写入 `agent-default-model`，模型路由取 `provider`；省略时按 dsh headless profile 的组合配置补上：`model` 是它的默认模型时沿用默认路由，是某个已配置 provider（如网关）声明的模型时用该 provider，否则为发行版的 `deepseek-official`；`provider` 须同时提供 `model`。`effort`（`off`、`low`、`high`、`max`）可单独设置：Adapter 实时读取 headless profile 的默认模型与路由，仅为满足整行 patch 协议而重述它们，不使用 App 缓存或内置目录猜测；读取失败时要求显式指定模型。

首次使用先调用 `list_members(provider="deepseek")`。其他选项：

- `permission_mode`：`read-only`、`workspace-write`、`danger-full-access`，与 dsh 的 `DSH_PERMISSION_MODE` 等效。省略时为原生默认 `workspace-write`。headless 没有审批通道，越权请求按原生规则失败关闭；`danger-full-access` 关闭沙箱与审批，只在用户授权时传。
- `stored_credentials`（布尔）：headless 默认停用桌面 App 保存的登录与密钥，只读启动环境、cwd 下 `.env`、`$DSH_HOME/.env` 中的 `DEEPSEEK_API_KEY`。路由为 `deepseek-official` 且未传此项时，Adapter 自动恢复已保存的认证；网关路由不恢复。显式 `true`／`false` 优先，恢复时不读取或传递密钥本身。
- `restricted_tools`（布尔）：停用 dsh 发行 profile 的全部工具插件与计划模式，沙箱改为只读；只接受 `model`、`effort`、`provider`、`stored_credentials`。依据 dsh 0.2.1 的插件清单，后续版本新增的工具插件不会自动停用。

使用结果返回的 `session_id`（`session-<uuid>`）在相同 cwd 续接，Adapter 映射为 `--session-id`；dsh 拒绝不存在、属于其他 cwd 或子代理的会话。

macOS 优先使用 `/Applications/DeepSeek Harness.app`（其次 `~/Applications`）内置的 `dsh`，再查 PATH 与常见安装目录。安装与登录在 DeepSeek Harness App 中完成；不把密钥放入 prompt 或 Provider Options。
