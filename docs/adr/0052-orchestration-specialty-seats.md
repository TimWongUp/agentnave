# ADR-0052：Explorer、Worker 按前端、后端分类，各分简单、困难两档

- 状态：已接受
- 日期：2026-11-04
- 修订 [ADR-0037](0037-app-writes-orchestration-rules.md) 中「Explorer 与 Worker 各有简单、困难两档」的条款；`orchestration.json` 的其余内容与写入方式不变。

## 背景

模型多、擅长不同，只按难度分两档，用户没法把界面类任务交给更擅长的模型。分得越细，boss 每次派发要判断的就越多；用户不希望给 boss 增加思考负担，理想是 8 个工位。

## 决定

- Explorer 与 Worker 各分前端、后端两类，每类有简单、困难两档，共 8 个工位，seat 名为 `<explorer|worker>-<frontend|backend>-<easy|hard>`。前端指界面相关的任务：页面、样式、交互、看截图或设计稿；后端是其余全部任务，含服务、脚本、构建、测试，以及写文档、调研、出方案。boss 只判断任务是否与界面有关，再按难度选档。
- 没设的工位沿用同一角色已设的工位，顺序为：同类另一档、另一类同档、另一类另一档。`list_members` 与派发套用同一结果，App 在空位上显示沿用的是哪个；一个角色一个工位都没设时，该角色没有工位。
- `orchestration.json` 升为版本 2，`explorer`、`worker` 的键改为 `frontend-easy`、`frontend-hard`、`backend-easy`、`backend-hard`。App 读到版本 1 时一次性迁移：原 `easy`、`hard` 改为 `backend-easy`、`backend-hard`，前端由沿用规则补齐，行为与迁移前一致。服务只接受版本 2，读到版本 1 时报告需要 App 迁移或按新格式改写；旧 seat 名 `explorer-easy` 等不再接受（[ADR-0050](0050-v1-compatibility-baseline.md)：只迁移，不留兼容分支）。

## 取舍

- 不留通用档：通用和后端几乎重合，boss 要多判断一次。代价是没有界面、也不算典型后端的任务（如写文档）归在后端，名字和内容不完全对应。
- 不按更多专长（写作、联网、长上下文）分：每多一类，boss 就多一次判断，工位也翻倍。
- 沿用规则让用户可以只设少数工位；但空位实际用的模型要在 App 里看，`defaults_applied` 只写 seat 名。
- 无 App 的 Linux 宿主要手动把版本 1 文件改写为版本 2。
