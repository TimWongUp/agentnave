# 会话与审核

本文件补充会议室的审核、主持人、隔离，以及历史和只读看板。

## 保存与查看

AgentNave 把私聊任务和会议室都保存到数据目录（默认 `~/.agentnave/`，由服务启动环境 `AGENTNAVE_DATA_DIR` 指定），只保存任务元信息、原生会话绑定、条目和最终回复，不保存思考或原始工具事件。记录跨重启保留，进程和句柄不恢复；中断后由 boss 检查再决定，不自动重试。

- `start_agent` 的 `title` 是新任务会话名，在第一次派发时给出，最多 48 列（中文算 2 列）；本聊天的第一个会话标题同时是工作区名，见 SKILL 主文件；之后改名由用户在 App 办公室里右键进行，boss 不改。`conversation_id` 把新线程归入已有任务会话；续用原生 `session_id` 会归入对应会话。同一原生会话不可同时执行两个任务。
- `list` 按最近活动列出会话（`kind`、`limit` 过滤，`total` 是总数），并启动只读看板、返回首页链接；`writable=false` 表示其他服务占用：可读不可写，回到占用它的宿主继续或等会话空闲，不另开会话拆散历史。
- `read_conversation` 默认返回会话信息和最终消息：任务、已公开回答、缺席、转发、主持人发言和待审回答（`pending_answer`）；`include_private=true` 另含各席私密指令（`instruction`）和不公开的回答（`private_answer`）；`since_round` 只取某轮之后；`limit` 只取最近若干条，略去的条数在 `omitted`。私聊会话返回每个任务的 prompt、最终回复和原生 `session_id`。结果只给 boss。
- 会话不需要结束，boss 也不删除会话；清理由用户在 App 中进行。

## 开会议室

`run_round(title, members, cwd, review?, isolated?)` 新建会议室，返回的 `group_id` 也是会话 ID；同一次调用可带 `seats` 和 `prompt` 直接开第一轮，不带 `prompt` 只建会议室。之后用 `run_round(group_id, seats, order, prompt)` 继续，重启后仍可用同一 ID，各席续接自己的原生会话；只能继续本聊天 `manager_session_id` 建的会议室，无归属的旧会议室只读，需新建。

- **审核**（`review=true`）：成功回答先待审，`wait` 以 `reason=review_needed` 提前返回，席位状态为 `pending_review`。读取正文后用 `decide_answers(round_id, {seat_id: "publish"|"private"})` 决定；并联轮全部决定后一次提交，串联轮决定后才轮到下一席。`private` 的回答只在 boss 与该席之间：其他席位看不到，原文保留，App 显示为该席对 boss 的未公开回复；该席下一轮会被告知它的回答没有公开。有待审回答时不能开新轮次或发主持人发言；`cancel` 或服务退出时未决回答按不公开处理。只能公开或不公开原文，不改写答案；需要重答就不公开后再开一轮。
- **隔离**（`isolated=true`）：仅在用户明确要求工具层面限制访问，并同时要求服务准备独立席位目录时启用。每席使用服务准备的独立空目录并启用 `restricted_tools`，只能指定 `model`、`effort`（Pi 另可 `provider`）；调用方 `cwd` 被忽略。会议室（隔离与否）都可加入宿主被排除的 CLI；排除项只约束 `start_agent`。需要读代码或改文件的协作不要隔离。
- **主持人**：`run_round(group_id, prompt)` 不带 `seats` 时，在轮次之间发一条公开发言，所有席位下一轮作为数据收到。私密信息放 `instructions`，不要写进公开任务或主持人发言。

回执保持简短：主持人发言返回条目的 `message_id`，审核返回已保存决定、剩余待审、实际提交的席位和轮次 `status`，失败时另含 `error`。`status=finished` 已完成提交，可直接开始下一轮；`running` 或 `review` 继续 `wait`。

## 边界

看板链接、`read_conversation` 结果、待审或不公开的回答、私密指令、原生 ID 和数据目录路径不交给参会 CLI；公开页只含已提交条目。工具限制和独立目录不清除旧会话历史，也不统一禁止原生规则或上下文文件加载，不等于操作系统沙箱。
