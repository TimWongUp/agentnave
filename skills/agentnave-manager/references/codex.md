# Codex CLI

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "codex"`；用户点名时的写法示例：

```json
{"model": "gpt-6.1-sol", "effort": "high"}
```

Codex 作宿主时，`native=true` 的 Codex 工位交给自带子代理：按 `spawn_agent` 的实时 schema 显式传工位的 `model`；工位 `effort` 有值且工具支持 `reasoning_effort` 时才传该值，空值省略，不自行补档位。模型或已设档位不能选择时，按主 Skill 的工位不可用与同难度兜底规则处理，不用宿主默认模型顶替；用户明确授权特定替代后才采用该替代。

服务总是为 Codex 带上 `--skip-git-repo-check`，非 Git 目录也能直接运行，无需传参；它只跳过仓库检查，不扩大命令权限。

`sandbox`（`read-only`、`workspace-write`、`danger-full-access`）设定本次调用的原生沙箱模式，续接同样生效；它是权限类参数，只能来自 App 默认值或用户明确要求。

`"approve_for_me": true` 由 Codex 的自动审核批准工具调用，命令在 `workspace-write` 沙箱内运行（新任务传 `--approve-for-me`，续接改用等效的 `--config`）；不能与 `sandbox` 或 `dangerously_bypass_approvals_and_sandbox` 同传。它是权限档位「自动」对应的选项，也是权限类参数。

`"dangerously_bypass_approvals_and_sandbox": true` 开启原生 YOLO（`--dangerously-bypass-approvals-and-sandbox`），本次调用及续接都跳过审批并关闭沙箱。它是权限档位「完全放开」（缺省）对应的选项，由服务套用；boss 自己显式传它时，仅在用户同时授权这两项时使用。传 `false` 或省略（且无 App 默认值）不加参数、继承原生设置，不会强制恢复沙箱或审批；CLI 已存配置不变。
