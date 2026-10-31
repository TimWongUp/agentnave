# ADR-0038：私聊的 role 指明工位时，服务套用 App 里的工位设置

- 状态：已接受
- 日期：2026-10-30
- 扩展 [ADR-0013](0013-members-defaults-and-host-identity.md) 的默认值套用，补充 [ADR-0030](0030-optional-review-gate-skill.md) 与 [ADR-0037](0037-app-writes-orchestration-rules.md)：评审工位与 Explorer／Worker 工位原本只由 `list_members` 回报。

## 背景

用户在 App 里给每个评审角色和 Explorer、Worker 的两档选 CLI、模型和档位，服务原本只在 `list_members` 里回报，由 boss 在每次派发时自己传 `model`、`effort`。boss 漏传时（上下文压缩后、续接复核时最常见），服务按员工默认模型运行，跑的不是用户选的模型，评审记录的来源也随之失真。实际出现过：用户为正确性评审选了 `gemini-4-argon-medium`，复核却跑成员工默认的 `gemini-4-argon-high`。

## 决定

- `start_agent` 的 `role` 是 agentnave-review 角色名，或 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`，并且 `provider` 是该工位的 CLI 时，服务在套用组员默认值之前补上工位的 `model`；工位设了档位且最终模型就是工位的模型时，再补上 `effort`。
- 调用方显式传的 `model`、`effort` 优先。`provider` 不是该工位的 CLI（如 boss 按替换规则换了 CLI）时不套用。
- 回执 `defaults_applied` 先列出「`<role>` seat」及补上的键。工位补上的模型和档位是用户为这个角色单独选的，不受「单独干活」的 CLI 名单、模型名单与档位范围约束（2026-11-01 修订，原先照常过这些校验，名单不含时工位不可用）；宿主排除、Adapter 选项校验与权限档位照常。boss 显式传的值仍按名单校验。
- `review.json`、`orchestration.json` 缺失或损坏时不套用，由 `list_members` 报告。会议席位不套用。

## 取舍

- `role` 不再只是显示标签：boss 随手用了某个评审角色名，也会带上那个工位的设置。名单固定且有专门含义，误用的代价是一次换模型的调用，回执可见。
- 工位设置过期（模型已从员工里移除、档位不再允许）时，调用会报错而不是退回员工默认，boss 需显式传参；这比悄悄跑错模型更容易被发现。
