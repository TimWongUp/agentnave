# ADR-0052：Worker 增加界面两档

- 状态：已接受
- 日期：2026-11-04
- 修订 [ADR-0037](0037-app-writes-orchestration-rules.md) 中「Explorer 与 Worker 各有简单、困难两档」的条款；`orchestration.json` 的版本、其余内容与写入方式不变。

## 背景

模型多、擅长不同，只按难度分两档，用户没法把界面类实现交给更擅长的模型。分得越细，boss 每次派发要判断的就越多，用户不希望增加 boss 的思考负担。

## 决定

- Worker 增加界面简单、困难两档，seat 名 `worker-frontend-easy`、`worker-frontend-hard`，存于 `orchestration.json` 的 `worker.frontend-easy`、`worker.frontend-hard`。界面指页面、样式、交互、看截图或设计稿；原有 Worker 两档改为界面以外的实现。Explorer 不变。
- 界面位没设时，沿用 Worker 同难度的工位：`list_members` 与派发套用同一结果，App 在空位上显示沿用的是哪个。所以不设界面位时，行为与之前一致。
- boss 先按难度选档，实现涉及界面时改用界面两档；规则正文只增加这一句。

## 取舍

- 只分出界面：模型差异明显、任务常见、boss 一眼能判断的，只有这一类。文档常与代码改动一起出现，单列会让 boss 多判断一次；Explorer 的探索、规划、评审不分前后端。
- 不把原有两档改名为「后端」并取消通用：内容几乎相同，却要停用旧 seat 名、升级文件版本并迁移；保留原名后，这次改动是纯加法。代价是原有 Worker 两档的含义收窄，名字仍是 Worker。
- 以后其他类型需要专门的模型时，可用同样的方式再加一组。
