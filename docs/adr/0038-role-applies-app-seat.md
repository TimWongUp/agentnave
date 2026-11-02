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
- 显式传 `provider` 时，`review.json`、`orchestration.json` 缺失或损坏仍不套用，由 `list_members` 报告；省略 `provider` 而需工位定 CLI 时必须报错，不回退。会议席位不套用。

2026-11-01 修订：`role` 给用户看，档位名对用户没有意义，于是私聊新增 `seat` 传 Explorer／Worker 档位，`role` 只写具体职责；App 不显示档位名，评审角色名显示中文。`role` 传档位名仍套用工位，兼容已写入宿主的旧规则。

2026-11-01 参数推断修订：续接时，服务在取得原生会话锁前，从本 boss 历史中该原生会话最近一次任务补齐省略的 `provider`、`cwd`、`role`、`seat`，显式值优先，工位选择也随任务保存；旧记录没有 `seat` 时按未选择处理。历史未知或跨 provider 重名时要求补传 `provider`、`cwd`，已知原生会话不允许换 provider；其他 boss 的历史不用于继承，显式传原生会话身份的既有续接与会话归属校验保留。`manager_session_id` 的要求不变。

新任务有 `seat` 或评审 `role` 而省略 `provider` 时，服务按 App 工位定 CLI；工位缺失、未启用或文件损坏报错，工位属于宿主自身 CLI 时提示改用宿主自带子代理。没有工位就必须传 `provider`，新任务仍必须传 `cwd`。

`access` 属于职责，能推断的不让 boss 填：Explorer／评审工位固定 `read`，Worker 工位固定 `write`，服务补上，声明不一致时拒绝，免得 App 显示矛盾的标签。其他任务由 boss 每次声明；续接不继承 `access`，工位按继承的 `seat` 重新得出，没有工位时留空，按可能写入对待。线程转去写文件时，继承旧声明会留下错误的「只读」并跳过并行写入提醒，留空最多多一条提醒。继承和推断都在 `defaults_applied` 标明来源；模型、档位及权限参数的套用保持原规则，访问等级仍只作声明。

规则块要求 boss 使用 `seat` 等新参数，旧版服务会以未知参数拒绝，因此本机服务落后于 App 时，App 暂不写入新规则块，已有规则块保留（旧规则的 `role` 写法两边都可用）；关闭编排仍立即移除规则块。服务升级后 App 刷新宿主并补写。

## 取舍

- `role` 不再只是显示标签：boss 随手用了某个评审角色名，也会带上那个工位的设置。名单固定且有专门含义，误用的代价是一次换模型的调用，回执可见。
- 工位设置过期（模型已从员工里移除、档位不再允许）时，调用会报错而不是退回员工默认，boss 需显式传参；这比悄悄跑错模型更容易被发现。
