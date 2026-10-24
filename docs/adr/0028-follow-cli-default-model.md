# ADR-0028：默认模型与档位跟随 CLI 自身设置

- 状态：已接受
- 日期：2026-10-24
- 取代 [ADR-0020](0020-prefill-main-model-defaults.md)；修订 [ADR-0021](0021-keep-default-models-when-unrestricted.md) 中“重新开启自选时按检测补主力模型”。

## 背景

ADR-0020 为了让办公室少出现“默认模型”工位，在首次启动、启用某个 CLI 和开启自选时，把探测到的“主力模型”写成 App 默认值。设置页新配置某个模型时，还会把档位的中间档写成默认档位。主力模型里混有猜测（Codex 取优先级最高的 `gpt-` 模型、Claude 取 `opus`、否则取列表第一项）。这些值一经写入就覆盖 CLI 自己的设置：用户改了 CLI 的模型或档位，App 仍按旧值传参，而用户并没有在 App 里设置过它们。

## 决定

- App 不替用户写默认模型或默认档位。没有在设置页设置时，boss 省略 model 或 effort 就使用 CLI 自身的默认值。
- 探测给出每个 CLI 自身的默认值 `native`：
  - **模型**：取 CLI 设置所写的模型，即 Codex `config.toml` 的 `model`、Claude `settings.json` 的 `model`、Pi 的 `defaultModel`、Antigravity CLI 的 `model` 标签、CodeBuddy `settings.json` 的 `model`、dsh headless 配置的 `agent-default-model`；没有设置时，取 CLI 列举中自己标为默认的模型（Grok 的 `(default)`、CodeBuddy 支持列表的第一项）。
  - **档位**：取 Codex 的 `model_reasoning_effort`、Claude 的 `effortLevel`（先看该模型的 `modelSettings`）、Pi 的 `defaultThinkingLevel`、dsh 的 `reasoningEffort`，以及 Grok 模型缓存为默认模型标注的默认档位；Antigravity 的档位已含在模型 ID 里。
  - 读不到就是未知，不做猜测。
- 设置页显示这些值：
  - 每张 CLI 卡片用一句话说明 boss 不指定时用什么，并标明每一项是谁定的：“boss 不指定时用 模型（你设的默认／CLI 默认）· 档位 …（你设的默认／CLI 默认）”；读不到时写“CLI 自己选的模型”“档位由 CLI 决定”。
  - CLI 自身的默认模型行标出“CLI 默认”；App 的选项叫“设为默认”，避免两个“默认”混淆。
  - 未设置默认档位时，档位条显示“CLI 默认 档位”或“档位由 CLI 决定”；设置后可一键“改回 CLI 默认”。
- 办公室里，没有请求模型、CLI 也尚未回报的工位，先按 CLI 自身的默认模型显示名字和形象，标记来源为 `cli_default`；CLI 回报后以回报为准。App 启动时在后台探测一次，供这一显示使用；办公室只读探测缓存，不为显示触发探测。
- 没有 `members.json` 时不再写出首份配置；旧档案迁移不变。
- 用户在 CLI 默认模型那一行设的档位与参数也要生效。服务不知道 CLI 的默认模型，所以 App 每次探测后把各 CLI 的 `native` 写到数据根的 `cli-defaults.json`（版本 1，`clis.<provider>.{model, effort}`；内容不变不写，不创建数据根；探测失败的 CLI 保留上次结果，未安装或读不到默认模型的不写）。调用未指定模型、App 也没设默认模型时，服务按该文件里的模型套用它的设置，但不传 `model`，模型仍由 CLI 自己选。文件缺失或损坏时按无设置处理。

## 取舍

- 已有 `members.json` 里的默认值不区分是预填还是用户设置，App 不清除；用户在设置页再次点击“默认”即可取消，改为跟随 CLI。
- 工位按 CLI 设置显示的模型可能与实际不同（例如账号侧改了默认模型），以 CLI 回报为准；Claude 未在设置中写档位时，其内置默认档位读不到，只显示模型。
- `members.json` 格式不变；`defaults_applied` 仍只报告用户在 App 里设置的值。`cli-defaults.json` 可能比 CLI 当前设置旧（用户改了 CLI 设置而 App 尚未重新探测），这时套用的是旧默认模型的设置；App 启动、打开设置页和回到前台时都会重新探测。
