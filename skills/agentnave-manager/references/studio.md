# 工作室节目

仅在当前连接提供 AgentNave Studio 工具（`open_dashboard`、`decide_answers` 等）时使用。核心协作轮次的规则照常适用，本文件只补充工作室多出的部分。

## 保存与查看

工作室把普通任务和节目都保存到数据目录（默认 `~/.agentnave/`，由服务启动环境 `AGENTNAVE_DATA_DIR` 指定）。只保存任务元信息、原生会话绑定、条目和最终回复，不保存思考或原始工具事件；运行中回复尾部只用于实时展示。记录跨重启保留，进程和 invocation_id 不恢复；中断后由主控检查再决定，不自动重试。

- `start_agent` 另接受 `title`（新任务会话名）和 `conversation_id`（归入同一件事）；续用原生 `session_id` 会归入对应未结束的任务会话。分组只用于展示，不向其他 CLI 转发内容。同一原生会话不可同时执行两个任务。
- `list_conversations` 列出会话；`writable=false` 表示其他服务占用：可读不可写，回到占用它的宿主继续或等该服务退出，不抢占、不另开会话拆散历史。`unavailable` 中的旧版记录不迁移。
- `read_conversation` 返回完整状态，含私密指令、待审回答和原生会话 ID，只给主控。
- `update_conversation` 改名、结束（`archived=true`）或重开；有运行中工作或待审回答时不能结束。`delete_conversation` 只删除已结束的会话，执行前须取得用户授权。
- 用户要观看时调用 `open_dashboard`，把返回的首页地址交给用户；看板在此之前不启动。

## 开节目

`open_group(title, seats, cwd, review?, isolated?)` 返回的 `group_id` 也是会话 ID，后续 `run_round` 直接使用；重启后仍可用同一 ID 继续，各席续接自己的原生会话。

- **审核**（`review=true`）：成功回答先待审，`wait_round` 以 `reason=review_needed` 提前返回、席位状态为 `pending_review`。读取正文后用 `decide_answers(round_id, {seat_id: "publish"|"discard"})` 决定；并联轮全部决定后一次公开，串联轮决定后才轮到下一席。被丢弃的回答其他席位看不到，但该席自己记得。有待审回答时不能开新轮次、发主持人消息或结束会话；`cancel_round` 或服务退出时未决回答丢弃。只能发布或丢弃原文，不改写答案；需要重答就丢弃后再开一轮。
- **隔离**（`isolated=true`）：每席使用工作室准备的独立空目录并启用 `restricted_tools`，只能指定 `model`、`effort`（Pi 另可 `provider`）；调用方 `cwd` 被忽略。只有隔离节目可以加入宿主被排除的 CLI，包括宿主自己的 CLI。用于防作弊的节目；需要读代码或改文件的协作不要隔离。
- **主持人**：`post_host_message(conversation_id, text)` 在轮次之间发公开发言，所有席位下一轮作为数据收到。私密信息放 `run_round` 的 `instructions`，不要写进公开任务或主持人发言。

## 边界

导演页地址、`read_conversation` 结果、待审回答、私密指令、原生 ID 和数据目录路径不可交给参演 CLI；公开页只含已提交条目。原生工具限制和隔离目录不等于操作系统沙箱，原生通用规则仍可能加载。按用户是否需要独立回答选择并联或串联，不因有看板就默认逐条公开。
