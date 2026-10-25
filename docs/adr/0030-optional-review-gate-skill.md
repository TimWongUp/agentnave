# ADR-0030：App「技能」页提供可选 Skill，评审门控为首个，评审工位在 App 设置

## 背景

用户希望把 PR／合并前的分级评审流程（定级、按风险派评审者、裁决、复核、留记录）用到 Codex 以外的宿主，并通过 AgentNave 派不同 CLI 做评审。原架构约定 MCP 与 Skill 只提供调用方式，不规定调用方的审核策略；App 只内嵌并自动安装 `agentnave-manager`。用户还希望在 App 里直接为每个评审角色指定 CLI 和模型，不必每次在对话里点名。

## 决定

- 评审流程写成独立 Skill `agentnave-review`，与 `agentnave-manager` 分开维护，调用方式仍以后者为准。流程由调用方 Agent 执行，运行时、MCP 工具和 `agentnave-manager` 不承担评审判断。
- App 构建时同时内嵌两个 Skill。可选 Skill 在 App「技能」页集中展示说明和完整写法，供用户决定是否使用；`agentnave-review` 是第一个，按宿主开关安装，默认关闭，开关存于 `hosts.json` 的 `skills`；安装、更新、备份和所有权校验沿用 `agentnave-manager` 的 App 副本规则，外部管理的副本不改动，开关置灰。
- 评审工位存于独立的 `review.json`：按八个固定角色记录 `provider`、`model` 与可选 `effort`。不放进 `members.json`，因为旧版服务按严格结构整份解析组员文件，App 先更新、服务尚未换新进程时会整份拒绝；`review.json` 损坏只影响评审工位，`list_members` 以 `review_error` 报告。App 只做结构校验，允许引用已不在组员里的模型；`list_members` 返回每个工位的可用性、原因和提供同一模型 ID 的其他可用 CLI。
- Skill 选工位的顺序：当前对话里用户点名 > App 评审工位 > 按组员自动选。App 工位在当前宿主不可用时，同类 CLI 被排除改用宿主自带子代理，模型不在组员里改找提供同一模型的 CLI，并在评审记录里注明。

## 取舍

- 架构边界从“Skill 不规定审核策略”放宽为“可选 Skill 可以规定，但由调用方执行”；不需要它的用户不开开关即可，不影响 `agentnave-manager`。
- 评审工位不随组员删除而清理：避免保存组员时因引用失效被拒，代价是失效工位要到派发时才替换，App 只在私聊名单未包含时提示。
- 角色列表固定在 App、服务和 Skill 三处，新增角色需同时修改。
- 新增可选 Skill 只需放进 `skills/` 并在托盘的可选目录登记；它们同样只给推荐写法，由调用方执行。
