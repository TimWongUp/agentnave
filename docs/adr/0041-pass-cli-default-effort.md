# ADR-0041：未指定档位时，服务传 CLI 自身的默认档位

- 状态：已接受
- 日期：2026-10-31
- 修订 [ADR-0028](0028-follow-cli-default-model.md)：档位不再“省略即跟随 CLI”，改为服务显式传 CLI 自身设置给出的档位。

## 背景

boss 和 App 都没给档位时，服务不传 effort，CLI 按自己的设置运行，历史与看板只记到空值。本机历史约 350 个任务里只有 7 个记下了档位，codex、grok、pi 的工位因此一律不显示档位，而它们的 CLI 设置其实写明了档位（如 Codex `model_reasoning_effort`、Grok 模型缓存的默认档位）。在显示端另读探测缓存补齐，会让同一个值有两个来源，且旧记录随 CLI 设置变化而改变显示。

## 决定

- App 每次探测后，在 `cli-defaults.json` 每个 CLI 的条目里增加 `efforts`：`{<模型 ID>: <档位>}`，即该 CLI 不传档位时对这个模型实际使用的档位，来源与设置页所写的“CLI 默认”相同；读不到的模型不写。文件仍是版本 1，旧读者忽略新字段。
- 调用在套用 App 默认值、评审工位与权限档位之后仍没有 effort 时，服务按 `cli-defaults.json` 补上：指定了模型取 `efforts.<模型>`，未指定模型取该 CLI 默认模型的 `effort`。补上的值写进任务／席位记录，并在回执 `defaults_applied` 里注明 `effort (CLI default)`。
- 显式参数、App 默认档位和工位档位都优先；补上的档位不在 App 为该模型设的可选档位里时不补，交给 CLI。
- 读不到就不补，不猜：dsh 未写 `reasoningEffort`、ZCode（3.14 起配置里没有默认档位）、Claude 未写 `effortLevel` 的模型。
- Claude 读取顺序补上 `settings.json` 的 `env.CLAUDE_CODE_EFFORT_LEVEL`，它优先于其他设置。Claude 选单给的是 `opus` 这类别名，而 `modelSettings` 按完整 ID 保存档位；探测用选单里每项的 `resolvedModel` 查找，别名的档位仍按别名发布。顶层 `effortLevel` 只对 Claude Code 仍读取它的模型生效（Opus 5 与 Fable 5.1 及更早，另加其他厂商的模型）；Opus 5.5 及之后的 Claude 只读 `modelSettings`，清单与 magpie 一致。
- 看板只读记录里的档位，不另读探测缓存。

## 取舍

- `cli-defaults.json` 可能比 CLI 当前设置旧：用户改了 CLI 档位而 App 尚未重新探测时，服务会传旧档位并覆盖新设置。App 启动、打开设置页和回到前台时都会重新探测，窗口很短；需要跟随 CLI 的新设置时重新打开设置页即可。
- 档位成了每次调用的显式参数，CLI 不再自行决定；这是为了让每次运行都有可追溯的档位。
- 此前的历史记录没有档位，保持原样，不回填。
