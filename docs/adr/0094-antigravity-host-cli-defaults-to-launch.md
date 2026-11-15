# ADR-0094：Antigravity 宿主的同类 CLI 私聊任务默认由 AgentNave 启动

- 状态：已接受
- 日期：2026-11-14
- 修订 [ADR-0090](0090-quota-group-cooldown-and-host-cli-choice.md) 中「交给宿主自带子代理（默认）」一条：默认值按宿主能力决定，Antigravity（`agy` CLI 与 Antigravity 应用）默认由 AgentNave 启动，其他宿主仍默认交给宿主自带子代理。
- 不改变：[ADR-0090](0090-quota-group-cooldown-and-host-cli-choice.md) 的三选一选项、额度组冷却、会议与生图工位边界，以及 [ADR-0074](0074-app-seat-model-source-and-claude-family-aliases.md) 的工位模型选择边界。

## 背景

ADR-0090 将私聊同类 CLI 的默认处理统一设为交给宿主自带子代理（`subagent`），且 `subagent` 作为唯一缺省值不写入 `orchestration.json`。

实测 `agy` 1.5.9 的自带子代理工具 `invoke_subagent` 的 `model` 参数只接受 `'inherit'`、`'flash_lite'`、`'flash'`、`'pro'` 四档（其余值报 `unsupported model`），不能指定具体模型 ID，也没有推理档参数。各档对应的实际模型由服务端按账号下发：本机 211 个 `agy` 会话记录一致为 `flash_lite`→`gemini-3.5-flash-lite`、`flash` 与 `pro` 同为 `gemini-3.8-flash-tiered`，`inherit` 为 boss 当前模型。因此用户在 App 工位里保存的具体模型（如 `gemini-4-carbon-low`）或推理档，Antigravity 自带子代理几乎无法满足，按 ADR-0074 每次都只能判工位不可用并停下询问用户。

## 决定

- **默认值按宿主能力决定。** 原生子代理能按工位模型执行的宿主（Claude Code、Codex 等）默认 `subagent`；Antigravity（`agy` CLI 与 Antigravity 应用）默认 `launch`，由 AgentNave 启动 `agy` 子进程并传入工位的模型与推理档。Provider 专属默认值由 Adapter 声明，核心不按 Provider 名分支；托盘维护对应声明并向前端下发，测试锁住两侧一致。
- **仅省略等于该宿主默认值的选择。** `orchestration.json` 的 `host_cli` 在选择等于该宿主默认值时省略条目，偏离默认值时写入（包括在 Antigravity 上显式选择 `subagent`）。旧文件中已写入的 `launch`、`skip` 照常读取生效。读取或校验失败时回退 `subagent`；旧连接无法确定应用行时也回退 `subagent`，避免覆盖可能存在的显式选择。

## 取舍

- **默认多启动一个 `agy` 子进程并与 boss 共用账号额度**，换来 App 工位保存的模型与推理档开箱可用，且任务进入 AgentNave 的历史、办公室、隔离目录与权限档位；只需继承当前 boss 模型的用户仍可在 App 显式切回 `subagent`。
- **不把工位模型硬套到 `flash` 或 `pro`。** 服务端档位映射不透明且可能变动，实测 `flash` 与 `pro` 甚至落到同一模型，无法代表用户在 App 选定的具体模型，硬套会违反 ADR-0074。
