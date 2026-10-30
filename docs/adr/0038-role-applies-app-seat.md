# ADR-0038：私聊的 role 指明工位时，服务套用 App 里的工位设置

- 状态：已接受
- 日期：2026-10-30
- 扩展 [ADR-0013](0013-members-defaults-and-host-identity.md) 的默认值套用，补充 [ADR-0030](0030-optional-review-gate-skill.md) 与 [ADR-0037](0037-app-writes-orchestration-rules.md)：评审工位与 Explorer／Worker 工位原本只由 `list_members` 回报。

## 背景

用户在 App 里给每个评审角色和 Explorer、Worker 的两档选 CLI、模型和档位，服务原本只在 `list_members` 里回报，由 boss 在每次派发时自己传 `model`、`effort`。boss 漏传时（上下文压缩后、续接复核时最常见），服务按员工默认模型运行，跑的不是用户选的模型，评审记录的来源也随之失真。实际出现过：用户为正确性评审选了 `gemini-4-argon-medium`，复核却跑成员工默认的 `gemini-4-argon-high`。

## 决定

- `start_agent` 的 `role` 是 agentnave-review 角色名，或 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`，并且 `provider` 是该工位的 CLI 时，服务在套用组员默认值之前补上工位的 `model`；工位设了档位且最终模型就是工位的模型时，再补上 `effort`。
- 调用方显式传的 `model`、`effort` 优先。`provider` 不是该工位的 CLI（如 boss 按替换规则换了 CLI）时不套用。
- 回执 `defaults_applied` 先列出「`<role>` seat」及补上的键。补上的值照常过组员白名单与档位校验，不合格时报错并说明来自该工位，提示显式传参覆盖，不静默换成别的模型。
- `review.json`、`orchestration.json` 缺失或损坏时不套用，由 `list_members` 报告。会议席位不套用。

## 取舍

- `role` 不再只是显示标签：boss 随手用了某个评审角色名，也会带上那个工位的设置。名单固定且有专门含义，误用的代价是一次换模型的调用，回执可见。
- 工位设置过期（模型已从员工里移除、档位不再允许）时，调用会报错而不是退回员工默认，boss 需显式传参；这比悄悄跑错模型更容易被发现。
