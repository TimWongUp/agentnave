# ADR-0091：「只开会」档只用会议室，按档位互斥安装 Skill，主动开会改为默认关的开关

- 状态：已接受
- 日期：2026-11-14
- 修订 [ADR-0083](0083-meetings-only-preset.md) 的「规则块授权 boss 主动开会」「用户点名 CLI 时的派发与『关闭』档相同」「这一档不提供『boss 可主动开会』开关」；修订 [ADR-0068](0068-rules-hold-authorization-skill-holds-method.md) 「归属」中编排方法只在 `agentnave-manager` 一份 Skill 的条款；扩展 [ADR-0036](0036-app-owns-service-skills.md) 中 App 管理的 Skill 集合。
- 不改变：服务对「只开会」按未开启任务编排处理（[ADR-0083](0083-meetings-only-preset.md)）；服务不拦截派发调用，单向编排只写在派发指令与 Skill 里（[ADR-0031](0031-prompt-only-orchestration.md)）；会议席位与开会默认人数设置；`agentnave-handover` 与可选 Skill 的安装规则。

## 背景

「只开会」原本把两件事绑在一起：选了它，规则块就授权 boss 在有取舍时主动开会；而它不写任务由谁做，用户点名 CLI 时照常私聊派发。用户对这一档的理解是「boss 只用 AgentNave 的会议室这一个功能」，主动开会应由用户另外决定。

这一档的 boss 加载的仍是完整的 `agentnave-manager`，篇幅大半讲选工位、私聊派发和交接；派活不是违规调用，服务也不拦截，只开会时 boss 照着 Skill 派活无从防止。在同一份 Skill 里写「只开会时跳过这些段落」，又要求 boss 在长文档里跳读，执行不稳定。

「决策模型」档也有用不上的段落（固定工位的定档、选组、升档），但它同样要派活，Skill 其余内容都适用，且服务在这一档拒绝固定工位名，误读有兜底。

## 决定

- **只开会只用会议室**：规则块写明 boss 不通过 AgentNave 私聊派活；用户要派活时，boss 告诉用户去 App 切换档位。服务仍按未开启任务编排处理，不拦截 `start_agent`。
- **主动开会是开关**：只开会档增加「boss 可主动开会」（`orchestration.json` 的 `meetings_options.meetings`），缺省为关，关闭时规则块写「只在用户要求时开会」。已在用这一档的 boss 升级后不再主动开会，用户打开开关即可恢复。
- **新增 `agentnave-meeting` Skill，与 `agentnave-manager` 互斥安装**：只开会档的 boss 装 `agentnave-meeting`，其余档位（含关闭）装 `agentnave-manager`；一个 Skill 目录只放其中一份，保存编排时 App 删掉另一份。`agentnave-meeting` 只含会议所需的内容（boss 会话归属、开会议室、讨论范围、等待与补跑、审核、保存、收尾两行）；`agentnave-manager` 仍含会议部分，供其他档开会。规则块按档位指向对应的 Skill。
- **单一来源**：两份 Skill 的共用段落在仓库里只维护一份，用脚本拼出两份 Skill 的文件，并用测试检查生成结果是最新的。
- **共用目录取 manager**：同一个 Skill 目录被档位不同的 boss 共用时（如多个宿主指向同一配置目录），装 `agentnave-manager`；只开会档的规则块因此写明找不到 `agentnave-meeting` 时改读 manager 的开会部分。评审 Skill 依赖 manager 的私聊派发，只开会档下不派评审工位，由 boss 提示用户切档。宿主还会读其他宿主目录里的 Skill 时，App 管不到那一份，这时以规则块指向的 Skill 为准。
- **决策模型档不拆**：`agentnave-manager` 主文件改为不偏向任何档位，固定工位的选法移到 `references/fixed-seats.md`，与 `references/decision.md` 并列，主文件只给分流表。
- **无头宿主**：仍按 `references/headless.md` 手动复制；用户选「只开会」时复制 `agentnave-meeting`，否则复制 `agentnave-manager`。

## 取舍

- 只开会档失去了「用户点名 CLI 就私聊派发」的便利：用户要切换档位，换来的是这一档的行为与名字一致，boss 手里也没有派活说明可以误用。
- 不选「两份都装，由规则块决定授权」：装着 manager 时，开会也会读进整份派活说明，问题仍在。
- 不选「同名 Skill 按档位装不同内容」：宿主的 Skill 列表看不出这个 boss 是哪一档，规则块写的名字也对不上实际内容，排查困难。
- 多一份 Skill 与一个生成步骤；切换档位要宿主重开会话才认得新 Skill，和规则块的生效时机相同。
