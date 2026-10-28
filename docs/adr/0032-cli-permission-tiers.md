# ADR-0032：App 按 CLI 设权限档位

- 状态：已接受
- 日期：2026-10-28

本决定修订 [ADR-0013](0013-members-defaults-and-host-identity.md) 的组员默认值范围：权限除了来自模型级 options，还可来自 App 中按 CLI 选的权限档位；并收窄 [ADR-0029](0029-settings-without-free-text.md) 的取舍“权限类参数不能在 App 里编辑”。

## 背景

AgentNave 以非交互方式派发 CLI，派发时没人能点确认。各 CLI 的原生默认对需要审批的工具直接拒绝：CodeBuddy 在 `-p` 下连 `pwd` 都拒绝，任务仍报 `succeeded`；Codex `exec` 默认只读沙箱，写文件的命令被拒，结果里没有任何信号。ADR-0013 允许权限类参数来自 App 的组员默认 options，但它们挂在单个模型上：boss 不指定模型（跟随 CLI 默认）、不限模型（`*`）或指定未列出的模型时都套不上。ADR-0029 又去掉了 App 里编辑 options 的入口，普通用户没有办法让派出去的 CLI 跑命令。

## 决定

- 设置页每个 CLI 卡片提供三档：原生／自动／完全放开，点选即存，私聊与会议共用。档位存于数据根独立的 `permissions.json`：`{"version": 1, "clis": {provider: "native" | "auto" | "full"}}`。不放进 `members.json`：旧版服务对 `members.json` 严格校验，多一个字段就会拒绝所有调用。文件或条目缺失即原生。
- 服务按 CLI 把档位映射为原生选项，在任务与会议席位入口套用，与模型无关，并在 `defaults_applied` 注明：

  | CLI               | 自动                                 | 完全放开                                        |
  | ----------------- | ------------------------------------ | ----------------------------------------------- |
  | Claude Code、Grok | `permission_mode=auto`               | `permission_mode=bypassPermissions`             |
  | CodeBuddy         | `permission_mode=bypassPermissions`  | 同左                                            |
  | Antigravity       | `mode=auto`                          | `dangerously_skip_permissions=true`             |
  | Codex             | `approve_for_me=true`                | `dangerously_bypass_approvals_and_sandbox=true` |
  | DeepSeek          | `permission_mode=danger-full-access` | 同左                                            |

  Pi 没有审批步骤，不设档位。

- 调用方显式传了该 CLI 的任一权限键时，整档不套用；档位生效时替换模型级默认 options 里的权限键，不与之混用；受限工具运行（`restricted_tools`、隔离节目）不套用；原生档保持原有行为。
- Codex Adapter 新增布尔选项 `approve_for_me`：新任务传 `--approve-for-me`；`exec resume` 没有这个参数，续接改传它实际设置的三项 `--config`（`approval_policy="on-request"`、`approvals_reviewer="auto_review"`、`sandbox_mode="workspace-write"`）。
- 首次引导问一次，推荐「自动」，也可「保持各 CLI 原生」；用户未选择前 App 不写 `permissions.json`。`list_members` 回执列出已选档位。
- Claude 与 CodeBuddy 报成功、但结果的 `permission_denials` 非空时，回执带 `warnings`，状态不改。

## 取舍

- 「自动」对 CodeBuddy 与 DeepSeek 等同完全放开：CodeBuddy 的 auto 在 `-p` 下仍拒绝，DeepSeek 没有自动审核。界面直接写明。
- 自动审核由各 CLI 自己实现，判断标准与可用性随 CLI 版本和账号变化；Codex 续接用的三项设置与 `--approve-for-me` 等效是实测所得，CLI 升级后可能失配。
- 档位只决定派发时传入的参数；用户自己的 CLI 配置（允许列表、Codex `config.toml` 等）仍会叠加生效。其他 CLI 的工具被拒目前没有可读信号，不能都给出警告。
- AgentNave 仍不是沙箱，档位只是代用户选择原生权限；具体行为靠派发 prompt 约束。
