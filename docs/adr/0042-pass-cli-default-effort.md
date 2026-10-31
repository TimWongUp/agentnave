# ADR-0042：组员模型带档位，服务每次传模型与档位

- 状态：已接受
- 日期：2026-11-01
- 修订 [ADR-0028](0028-follow-cli-default-model.md)：不再有“跟随 CLI 档位”的组员设置；CLI 自身的默认值只在 App 没配模型时临时兜底。

## 背景

看板上 codex、grok、pi 等工位不显示档位：本机历史约 350 个任务只有 7 个记了 effort。设置页在用户选中的档位等于 CLI 当前档位时把它存成空，表示跟随 CLI；boss 也常不传或自行挑模型。于是服务不传 effort，CLI 按自己的配置跑，记录为空。

## 决定

- **App 是来源**：设置页给有可选档位的模型（默认模型、评审与 Explorer／Worker 工位）选定模型时同时写入档位，预选 CLI 当前档位，只有一档的自动写入；选中值等于 CLI 当前设置也照写，不再存空。没有可选档位的模型（如 Antigravity，档位写在模型 ID 里）不写。选了默认模型却没有档位、而模型有多档时，设置页报错并要求选一档。
- **boss 不挑模型**：用户没点名模型或档位时，boss 不传 `model`、`effort`，由服务按 App 设置传入；`list_members` 只用来选 CLI 和工位。用户点名时再传。
- **CLI 默认值兜底**：调用在套用 App 默认值、工位与权限档位后仍无 effort 时（App 没配模型、用户点名了未配置的模型、旧配置未补档位），服务按 `cli-defaults.json` 补上：App 每次探测后在各 CLI 条目里写出 `efforts: {<模型 ID>: <档位>}`（该 CLI 不传档位时对这个模型实际用的档位；读不到的不写，文件仍是版本 1），指定了模型取 `efforts.<模型>`，未指定取 CLI 默认模型的 `effort`。补上的值写进记录，回执 `defaults_applied` 注明 `effort (CLI default)`；不在 App 为该模型设的可选档位里时不补。服务没有模型目录，判断不了哪些模型没有档位，所以未配置的模型不因缺档位报错。
- **读得准**：Claude 读 `settings.json` 的 `env.CLAUDE_CODE_EFFORT_LEVEL`（优先），再按完整 ID 读 `modelSettings.<模型>.effortLevel`；选单给的别名（`opus`）用 `initialize` 返回的 `resolvedModel` 换成完整 ID。顶层 `effortLevel` 只对 Claude Code 仍读取它的模型生效（Opus 5 与 Fable 5.1 及更早，另加其他厂商的模型），清单与 magpie 一致。读不到就不补、不猜：dsh 未写 `reasoningEffort`、ZCode 3.14、Claude 未设档位的模型。
- 看板只读记录里的档位。

## 取舍

- 已存成空的旧档位需要按探测结果补一次；本机已在用户同意后补齐，其他安装由兜底保证记录里有档位，用户下次在设置页保存时会被要求补上缺的档位。
- `cli-defaults.json` 可能比 CLI 设置旧，兜底时会传旧档位；App 启动、打开设置页、回到前台时重新探测。
- 此前的历史记录没有档位，保持原样。
