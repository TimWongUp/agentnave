# ADR-0034：模型与档位以 CLI 自己给出的为准

- 状态：已接受
- 日期：2026-10-30
- 修订 [ADR-0014](0014-tray-members-and-one-click-update.md) 第 2 条的“只运行列举／帮助命令、档位用静态表”。

## 背景

设置页的努力档位原先按 CLI 用一张静态表给出，只有 Codex 按模型取自缓存。实际各 CLI 按模型给档位：agy 的模型 ID 已含档位，配上别的 `--effort` 直接报错；Claude 的 Haiku 4.5 没有档位，Opus 4.6 没有 xhigh，不支持的档位被静默降级；Grok、CodeBuddy、Pi、dsh 声明的网关模型与 ZCode 也各有每个模型的档位。Claude 也不止四个别名，它自己的模型选单还列出各个固定版本。页面给出 CLI 不提供的档位，用户选了会失败或不生效。

## 决定

设置页的模型和每个模型的档位都取 CLI 自己给出的，与它自己的模型选单一致：

- Claude：以 stream-json 发 `initialize` 控制请求，取回执里的模型选单（除 `default`）与各自的 `supportedEffortLevels`；启动时用 `--settings` 关闭 hooks、不保存会话，拿到回执即终止。失败时退回别名。
- Pi：以 `--mode rpc --no-session` 发 `get_available_models`，按 Pi 自己的规则从 `reasoning` 与 `thinkingLevelMap` 算出每个模型的档位；失败时退回 `--list-models`。
- Grok 取 `models_cache.json` 中每个模型的 `reasoning_efforts`；CodeBuddy 取其缓存的产品配置中的 `reasoning.supportedEfforts`，没有列出的模型按 CodeBuddy 的做法给全部档位；dsh 声明的模型取 `reasoningEfforts` 的键，没有即不推理，官方模型沿用路由的档位；ZCode 取模型规则里的 `reasoningLevel`；Codex 照旧取缓存；Antigravity 不给档位，档位就是模型 ID 的一部分。
- 模型自带的档位列表（包括空列表）优先于 CLI 级的列表；只有一档或没有档位时不显示档位条。

## 取舍

- Claude 与 Pi 的探测从“空 stdin 跑列举命令”变成“启动 CLI 发一条元数据请求后终止”，比列举命令慢、启动面更大，但仍不发起模型调用；为此关掉 Claude 的 hooks，避免用户的 SessionStart 钩子随每次探测运行。不用 `--bare`，因为它换掉认证与设置后列出的模型和用户看到的不同。
- CodeBuddy 的产品配置缓存不是公开接口，读取失败时退回 CodeBuddy 的全部档位，与它没有列出档位时的行为一致。
