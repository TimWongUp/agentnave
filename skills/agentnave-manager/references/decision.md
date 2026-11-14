# 决策模型档

用户选了「决策模型」档时，工位是用户自建的目录，由服务把派发内容交给外部决策模型来选。boss 写简报、声明读写类型、验收结果；选哪个工位、用哪个模型交给服务，派发前不用先查 `list_members`。拆成几项、能否并行写入、何时开会仍按规则块；开会席位和评审工位不走决策模型，按主文件。

## 派发

| 情况 | 调用 |
| --- | --- |
| 一项任务 | `start_agent`，`seat="auto"`，`access` 按是否改文件填；`role` 写具体职责，不传 `provider` 和模型 |
| 几项互不依赖的任务（如一项调查拆成几份） | `start_agents`，每项各写 `access` 和 prompt；服务用一次决策请求为各项选工位并同时启动 |
| 接着同一线程干活 | 传 `session_id`，不传 `seat`，沿用原工位 |
| 想先看选谁再定 | `recommend_seat`，`task` 直接传打算派发的 prompt；再按返回的 `seat` 派发 |
| 生成图片 | 先查 `list_members`，目录里有 `kind` 为 `image` 的工位才派：`seat="auto"`、`access="write"`，简报开头写明要生成图片和画风，写清产物路径；没有生图工位时告诉用户缺这个工位 |

`start_agents` 里有两个及以上可写项时，服务自动给每个可写项建隔离目录（显式传了 `isolation` 的以它为准）；boss 验收后合入当前分支并释放。

## 简报就是决策依据

决策模型读到的是 prompt 原文，超过 4000 字时只有开头 3000 字和结尾 1000 字。所以「目标」放在最前面，写清交付什么、难在哪、主要靠读代码、上网查还是改文件；密钥和个人信息不写进 prompt。`forward` 带上的内容不会发给决策模型。

## 读回执

回执的 `recommendation` 给出选中的 `seat`、`role`、`difficulty`、`confidence` 和前几名 `candidates`（各带 `difficulty`）。选择是数据，boss 照常对结果负责：

- `confidence` 低于 0.6，或前两名概率接近：核对选中的工位是否对题、难度是否合适（直接看回执里的 `difficulty`）。
- 明显不对题：`cancel`，查 `list_members` 的 `custom_seats`，按 `role`、`description`、`difficulty`、`access` 直接指定 `seat` 重派，并在回报里说明改选的理由。

## 没启动时

报错即任务没有启动，同一请求不重发。`start_agents` 逐项返回，`status` 为 `not_started` 的项按它自己的 `error.code` 处理，其余项照常等待。

| `code` | 处理 |
| --- | --- |
| `native_seat` | 选中的是宿主同类工位：按回执里给出的模型和推理档交给宿主自带子代理 |
| `no_candidates` | 没有能派的工位（自定义工位有任一顺位可用即算可派，全部顺位都不可用才排除）：boss 自己完成，或说明缺少能力 |
| `start_failed` | 这一项启动失败（如自动隔离建不成）：按报错原因修正后单独重派 |
| 其余（`no_key`、`network`、`rate_limited`、`auth`、`rejected`、`bad_response`、`config_error`） | 查 `list_members`，从 `custom_seats` 里直接指定 `seat` 派发，并把错误转告用户一次 |
