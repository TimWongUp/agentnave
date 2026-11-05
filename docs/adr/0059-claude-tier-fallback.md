# ADR-0059：Claude 的两个权限档位互为退路

- 状态：已接受
- 日期：2026-11-05
- 修订 [ADR-0032](0032-cli-permission-tiers.md)：Claude Code 两档所选的权限模式不可用时，改用另一档的模式重跑一次。

## 背景

Claude Code 的 auto 模式会因设置 `disableAutoMode`、模型或套餐不支持、服务端关闭而不可用；bypassPermissions 可被 `permissions.disableBypassPermissionsMode` 禁用。所选模式不可用时，Claude Code 不报错，直接以手动（`default`）模式开始。`-p` 下所有需要审批的工具调用随即被拒，交互运行（[ADR-0056](0056-claude-interactive-runtime.md)）则把每个调用都交给 boss（[ADR-0057](0057-boss-answers-native-prompts.md)），两种情况任务都推不动。

## 决定

- Claude 的「自动」档用 auto，退路是 bypassPermissions；「完全放开」档用 bypassPermissions，退路是 auto。退路随档位一起由服务套用，并在 `defaults_applied` 注明；调用方显式传了权限键时，没有退路。
- 交互运行和 `-p` 判断方式相同：AgentNave 加一个 UserPromptSubmit hook，读 Claude Code 实际所处的模式。实际是 `default` 时，hook 拦下这条提示词（提示词不写入会话），AgentNave 改用退路模式重跑：已有会话续接原会话；新会话在 `-p` 下事先分配 ID 并续接它，交互运行续接不了只含被拦提示词的会话，改用新 ID 并删掉那个空会话文件。每次派发最多重跑一次。重跑后仍是 `default` 时，任务受阻，原因写明两种模式都不可用。实际模式不是 `default`、但与所选不同时，照常运行并给出警告。
- 用了退路的回执带 `warnings`，由 boss 转告用户。App 两档的说明写明各自的退路，「自动」档写明 auto 不可用时改用完全放开。
- 受限工具运行不套用档位（ADR-0032），也没有退路。

## 考虑过的方案

- PermissionRequest hook 返回 `setMode` 改模式（官方支持，不用重跑）：退路模式也不可用时 `setMode` 不生效，可此前已经放行了第一个调用，事先又没有信号能判断退路是否可用，等于越过用户或管理员的策略放行一次。放弃。
- 看 `-p` 的 `system/init` 事件再重跑：init 晚于提示词写入会话，续接时重跑会让同一条提示词出现两次。放弃。
- 退回手动、全部交给 boss：boss 无法正常推进。放弃。

## 取舍

- auto 不可用时，任务以完全放开运行，权限比用户选的宽。用户在 2026-11-05 明确选择宁可放开也不要手动，App 文案和回执警告都写明这一点。
- 需要退路时多一次启动；第一次启动时，用户自己的 UserPromptSubmit hook 也会多跑一次。
- AgentNave 不知道 Claude Code 为何不用所选模式，警告只说明结果。
