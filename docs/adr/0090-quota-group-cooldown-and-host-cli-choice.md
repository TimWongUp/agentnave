# ADR-0090：配额按额度组冷却，宿主同类 CLI 按 boss 选择处理

- 状态：已接受
- 日期：2026-11-14
- 修订 [ADR-0081](0081-account-cooldown-duration.md) 的「冷却仍按请求的模型记录」：配额失败优先按额度组冷却；修订 [ADR-0013](0013-members-defaults-and-host-identity.md) 的单任务排除宿主同类 Provider、[ADR-0089](0089-seat-fallback-order.md) 的同类顺位一律交还 boss 及配额冷却可用性判断。补充 [ADR-0087](0087-decision-model-recommends-seats.md) 的候选可用性和 [ADR-0074](0074-app-seat-model-source-and-claude-family-aliases.md) 的执行边界。
- 不改变：[ADR-0081](0081-account-cooldown-duration.md) 的认证、计费冷却规则；[ADR-0074](0074-app-seat-model-source-and-claude-family-aliases.md) 使用宿主原生子代理时的同系列别名规则；[ADR-0031](0031-prompt-only-orchestration.md)、[ADR-0043](0043-service-prompt-instructions.md) 的派发说明与单向编排约定；会议席位及生图工位的宿主同类边界。

## 背景

Antigravity 的用量查询实测分为「Gemini Models」和「Claude and GPT models」两组，每组各有周窗口和五小时窗口，同组模型共用额度；Codex 的查询只有一份「codex」额度，含五小时窗口和周窗口，所有模型共用。现有冷却只记失败的模型，即使用量查询能对应额度组并取得重置时间，同组其他模型仍会被派发。本次实际有三个不同的 Antigravity Gemini 模型先后报 RESOURCE_EXHAUSTED 429。

私聊派发遇到宿主同类 CLI 时，目前服务不启动，返回 `not_started` 回执，交 boss 使用宿主自带子代理。有些宿主无法为原生子代理指定精确模型版本；原生子代理也不进入 AgentNave 的历史、办公室、隔离目录和权限档位。

## 决定

- **配额按额度组冷却。** 各 Provider Adapter 声明模型到额度组的映射，配额失败能对应到组时记录组级冷却，同组模型均视为冷却；对应不到时仍按请求的单个模型冷却，未指定模型时仍按 CLI 冷却。Antigravity 使用上述两组，Codex 所有模型属于一组。映射是 Provider 专属知识，放在 Adapter，核心不按 CLI 名分支。
- **派发前按同一范围判断。** 可用性判断同样匹配额度组；与 ADR-0089 的备选顺位配合，同组备选一起跳过，不同组或其他 CLI 的备选不因该组冷却而被跳过。同组任一模型调用成功即清除该组冷却，托盘可手动解除整组；认证和计费类冷却不变。
- **组级记录是共享合同。** 冷却仍记录在数据根 `provider-health.json`，服务与托盘共同写入。旧版托盘或服务可能忽略组级记录，接受这一兼容代价。
- **私聊同类 CLI 按 boss 三选一。** 用户在 App 按 boss 选择，读同一规则文件的 boss 共用选择；App 写入数据根 `orchestration.json`，服务读取。仅影响 `start_agent`／`start_agents` 私聊派发：

  1. 交给宿主自带子代理（默认）：服务返回 `not_started` 回执，带该顺位的模型和档位。
  2. 由 AgentNave 启动：服务像派其他 CLI 一样启动宿主同类 CLI 子进程，该 boss 的单任务不再排除同类 Provider。子进程与 boss 共用同一账号的登录和额度。
  3. 跳过，换下一个：同类 CLI 顺位视为不可用，按 ADR-0089 取下一个可用顺位；「决策模型」档下只配了宿主同类顺位的工位不进候选，全部不可用时按现有方式报告。
- **单向编排与会议边界不变。** 服务启动的同类子进程与其他 CLI 一样，由服务在每次派发前统一加子代理说明约束嵌套派发，不加运行时拦截（ADR-0031、ADR-0043）；这是行为约定，不是安全隔离。正常运行的服务子进程不会被 App 当成新的 boss：App 工作区来自服务记录，原生会话文件只用于识别 boss 入口。会议本来就可加入宿主同类 CLI，不受三选一影响。生图工位仍不接收宿主同类 CLI，生图顺位中的同类 CLI 总是跳过。

## 取舍

- **不按整个 CLI 冷却配额。** Antigravity 的 Gemini 组用完时 Claude/GPT 组仍可用；聚合 CLI 更容易误挡无关模型，ADR-0081 的原有理由仍成立。
- **按真实额度组，不把模型名前缀当合同。** 实测两个 CLI 的额度组与供应商系列一致，但 Adapter 声明的真实额度组更贴近实际；前缀可作为 Adapter 内部实现手段。对应不到组时宁可按模型，避免误挡。
- **三选一替代两个独立开关。** 「允许同类独立干活」和「跳过同类」同时打开会自相矛盾，三选一直接表达结果；不设全局单一值，因为不同宿主的原生子代理能力差别大。
- **不在服务端拦截子进程调用 AgentNave。** 派发前已有服务统一追加的说明，模型遵守能力足够；拦截还需另行识别 Codex（MCP 进程收不到环境变量标记）、Pi 和 Grok（MCP 进程进入新的进程会话），耗费资源。ADR-0011 的祖先进程识别也曾误禁正常宿主，接受行为约定可能不被遵守的限制。
