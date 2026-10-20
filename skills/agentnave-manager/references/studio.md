# 会话与审核

统一核心提供完整 16 个工具，本文件补充历史、审核、主持人和只读看板的调用方式；无需切换 MCP 入口。

## 保存与查看

AgentNave 把普通任务和节目都保存到数据目录（默认 `~/.agentnave/`，由服务启动环境 `AGENTNAVE_DATA_DIR` 指定）。只保存任务元信息、原生会话绑定、条目和最终回复，不保存思考或原始工具事件；运行中回复尾部只用于实时展示。记录跨重启保留，进程和 invocation_id 不恢复；中断后由 boss 检查再决定，不自动重试。

- `start_agent` 另接受 `title`（新任务会话名）和 `conversation_id`（归入同一件事）；续用原生 `session_id` 会归入对应的任务会话。分组只用于展示，不向其他 CLI 转发内容；需要时用 `forward` 显式转发已完成的消息。同一原生会话不可同时执行两个任务。
- `list_conversations` 列出会话并启动看板、返回首页地址；`writable=false` 表示其他服务占用：可读不可写，回到占用它的宿主继续或等会话空闲，不抢占、不另开会话拆散历史。`unavailable` 中的旧版记录不迁移。
- `read_conversation` 返回完整状态，含私密指令、待审回答和原生会话 ID，只给 boss。
- `update_conversation` 只改名。会话不需要结束，boss 也不删除会话；清理由用户在托盘 App 的“管理会话”中进行。
- 新建或继续会话会自动返回看板地址，只给 boss 或用户；仅查看旧历史时调用 `list_conversations`。看板失败不回滚任务，不重发已启动的调用。

写入回执保持简短：主持人返回条目序号，改名返回标题，审核返回已保存决定、剩余待审、实际已提交席位和轮次 `status`，失败时另含 `error`。`status=finished` 已完成提交，可直接开始下一轮，无需再确认；`running` 或 `review` 继续用 `wait_round` 等待，`failed`、`cancelled`、`interrupted` 按终态处理。回执不含正文或导演状态；需要完整答案或历史时再读取。AgentNave 共 16 个工具，不另设看板打开工具。

## 开节目

`open_group(title, seats, cwd, review?, isolated?)` 返回的 `group_id` 也是会话 ID，后续 `run_round` 直接使用；重启后仍可用同一 ID 继续，各席续接自己的原生会话。

- **审核**（`review=true`）：成功回答先待审，`wait_round` 以 `reason=review_needed` 提前返回、席位状态为 `pending_review`。读取正文后用 `decide_answers(round_id, {seat_id: "publish"|"discard"})` 决定；并联轮全部决定后一次公开，串联轮决定后才轮到下一席。被丢弃的回答其他席位看不到，但该席自己记得；原文保留，App 办公室标为「🔒 私密」。有待审回答时不能开新轮次或发主持人消息；`cancel_round` 或服务退出时未决回答丢弃。只能发布或丢弃原文，不改写答案；需要重答就丢弃后再开一轮。
- **隔离**（`isolated=true`）：仅在用户明确要求工具层面限制访问，并同时要求服务准备独立席位目录时启用；日常讨论按主 Skill 的“讨论范围与工具”约定处理。每席使用服务准备的独立空目录并启用 `restricted_tools`，只能指定 `model`、`effort`（Pi 另可 `provider`）；调用方 `cwd` 被忽略。节目（隔离与否）都可加入宿主被排除的 CLI，包括宿主自己的 CLI；排除项只约束 `start_agent`。需要读代码或改文件的协作不要隔离。
- **主持人**：`post_host_message(conversation_id, text)` 在轮次之间发公开发言，所有席位下一轮作为数据收到。私密信息放 `run_round` 的 `instructions`，不要写进公开任务或主持人发言。

## 边界

导演页地址、`read_conversation` 结果、待审回答、私密指令、原生 ID 和数据目录路径不可交给参演 CLI；公开页只含已提交条目。工具限制和独立目录不清除旧会话历史，也不统一禁止原生规则或上下文文件加载，不等于操作系统沙箱。按用户是否需要独立回答选择并联或串联，不因有看板就默认逐条公开。
