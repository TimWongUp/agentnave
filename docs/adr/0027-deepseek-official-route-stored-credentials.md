# ADR-0027：DeepSeek 官方路由自动恢复已存认证

- 状态：已接受
- 日期：2026-10-24
- 修订 Adapter 边界中“除协议必需参数与调用方显式参数外不静默设置参数”的规则，增加一处例外；其他 Provider 不受影响。

## 背景

dsh 的 headless profile 默认停用 `credentials`、`authorization` 与 `deepseek-account` 三个插件，只从启动环境、cwd 下 `.env` 和 `$DSH_HOME/.env` 读取 `DEEPSEEK_API_KEY`。用户通常在 DeepSeek Harness App 的模型页填写官方 key，它存在被停用的凭据服务里。于是 App 默认模型选官方模型（如 `deepseek-flash`）时，调用以 `MISSING_CREDENTIAL` 失败，而同一台机器上的 App 可以正常使用。此前只有调用方显式传 `stored_credentials=true` 才能恢复，用户和 boss 都不知道要传。

## 决定

- 解析出的模型路由是 `deepseek-official`，且调用方没有显式给出 `stored_credentials` 时，Adapter 在临时 `--patch` 覆盖层里恢复这三行，等同于 `stored_credentials=true`。
- 路由按 dsh 组合后的 headless 配置判定：未指定模型时看 `agent-default-model`；指定的模型等于该默认模型时沿用其路由；配置中某个 provider 声明了该模型时用该 provider 的路由；都不是则为官方路由。调用方显式给出 `provider` 时以它为准。
- 显式 `stored_credentials=false` 仍然停用，网关等其他路由不自动恢复。

## 取舍

- 子进程会读取 DeepSeek Harness App 保存的登录与密钥；AgentNave 不读取、不传递密钥本身。用户在 App 里保存 key 本身即表明希望官方模型可用，因此不再要求额外开关。
- 回执不单独注明此项；它不改变模型、权限、工具或 prompt。
- 判定依赖 `dsh --profile headless --dump-config`（约 0.1 秒）；读取失败时，指定的模型按官方路由处理，未指定模型时不自动恢复。
