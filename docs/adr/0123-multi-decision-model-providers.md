# ADR-0123：决策模型支持多平台与本机 System One 端点

- 状态：已接受
- 日期：2026-11-25
- 修订 [ADR-0087](0087-decision-model-recommends-seats.md) 中「先只接 Gemini Decision 1」与单一 Gemini 密钥／外发说明的条款。
- 修订 [ADR-0029](0029-settings-without-free-text.md) 的不手填约束：允许在「决策模型」档填写各平台密钥、`ollama`／`custom` 地址与可变平台的模型 ID（与 [ADR-0044](0044-meeting-voice-cloud-and-key-input.md) 口语摘要口径一致）。
- 不改变：单向派发、候选先按可用性过滤与调不通不自动回落（ADR-0087）、同档候选随机选（[ADR-0093](0093-decision-seat-random-candidates.md)）、T3 决策模式共用同一份决策模型配置（[ADR-0121](0121-t3-code-orchestration.md)）、20 秒超时与网络失败重试一次（ADR-0087 2026-11-17 修订）。

## 背景

ADR-0087 最初只接入了 Google `gemini-decision-1`。实际使用中，Google 存在可用地区限制（不含中国大陆和香港）、免费层输入可能用于产品改进以及单一厂商绑定问题；已有 OpenAI、OpenRouter、TypeSafe 密钥或希望完全在本地运行的 Ollama 用户无法使用「决策模型」档。

截至 2026 年 11 月，纯选择型决策模型（System One / Decisions API）已广泛可用，协议收敛为三套：Google Decisions API、OpenAI Decisions API（`/v1/decisions`）以及 System One 事实标准（`/v1/systemone`，覆盖 TypeSafe Jev、OpenRouter 聚合目录、Ollama ≥ 0.35 与 `llama.cpp` 等自托管服务）。

## 决定

- **支持六种决策来源，缺省仍为 Google Gemini。**
  - `gemini`（默认）：Google Decisions API，模型 `gemini-decision-1`，需 `gemini` 密钥。
  - `openai`：OpenAI Decisions API（`POST /v1/decisions`），模型 `gpt-6-luna`，需 `openai` 密钥。
  - `typesafe`：TypeSafe AI（`POST /v1/systemone`），默认 `jev-latest`，支持拉取列表、内置推荐或手填模型 ID，需 `typesafe` 密钥。
  - `openrouter`：OpenRouter（`POST /api/v1/systemone`），默认 `liquid/d1.2-ultra`，托盘按需从 `output_modalities=decisions` 拉取模型目录并提供精选推荐与手填模型 ID，需 `openrouter` 密钥。
  - `ollama`：Ollama 本机／兼容服务（默认 `http://127.0.0.1:11434`，可修改地址，免密钥；托盘按需从 `/api/tags` 筛选带 `decision` 能力的模型，服务调用 `/v1/systemone`）。
  - `custom`：自定义 System One 兼容端点（必填地址，可选 `custom` 密钥，支持尝试拉取模型列表或手填模型 ID；服务按地址规范补全 `/v1/systemone` 或 `/systemone`）。
- **保持 `decision-seats.json` version 1 与向后兼容。** 新增可选字段 `decision_provider`（缺省为 `gemini`）与 `decision_base_url`（仅 `ollama` 与 `custom` 允许）。使用默认 `gemini` 时 App 省略这两个字段，不破坏旧版服务对既有文件的读取。`decision-keys.json` 沿用 `keys` 对象按平台名分存各平台密钥（`gemini`、`openai`、`typesafe`、`openrouter`、`custom`），服务只读、托盘仍不回传密钥原文；`POST /app/decision-key` 缺省 `provider` 时仍按 `gemini` 处理。
- **地址边界与联网时机。** `decision_base_url` 与口语摘要自定义地址采用同一校验规则：只允许 `https://` 或本机回环地址的 `http://`，拒绝账号信息、查询参数与片段；服务访问回环地址时绕过系统代理直连。托盘仅在设置页选中对应平台或点击刷新时才按需联网读取该平台模型目录，预览版允许只读查询目录，仍拒绝保存配置与密钥。
- **服务按三套协议直连，不自动跨平台回落。** 服务内置 Google Decisions、OpenAI Decisions 与通用 System One 三个客户端，共用超时、单次网络重试与密钥脱敏约束；不接入 Magpie 网关；缺密钥、配置无效或调用失败仍报错，由 boss 从工位目录自选。
- **设置页输入与提示边界。** 允许输入密钥、自定义／Ollama 地址以及可变平台的模型 ID；`gemini` 与 `openai` 仅提供固定选择。数据去向提示随所选平台准确说明（本机 `ollama` 提示派发内容不出本机，云端平台标明对应服务方，Gemini 保留免费层与地区提示）。

## 取舍

- **新增显式 `decision_provider` 而非仅靠模型名路由。** OpenRouter、Ollama 与自定义端点可能出现同名模型，且密钥与协议归属需要无歧义；代价是切换到非 Gemini 来源后，未升级的旧版服务会把 `decision-seats.json` 判为无效，需重连新版服务。
- **聚合优先走 OpenRouter，本机与长尾走 Ollama 与自定义 `/v1/systemone`，不逐家接入小众厂商也不接 Magpie。** 一套 System One 客户端即可覆盖数十个决策模型与本地推理服务，维护面小；代价是只使用 System One 的 `choice` 子集，不支持厂商私有扩展字段或跨平台自动故障转移。
- **模型目录由托盘按需拉取，服务派发时不拉目录。** 避免发版硬编码快速变化的 OpenRouter／本机模型清单，也不增加派发路径的网络延迟；目录暂时拉不到时仍可用内置推荐或手填模型 ID。
