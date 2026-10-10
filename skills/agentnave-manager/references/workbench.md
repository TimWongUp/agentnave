# 工作台与讨论会话

## 能力与保存

先核对实时工具列表。v0.11.0 起提供工作台；版本文档不等于当前宿主已加载；缺工具时报告运行时版本差异，不更改全局注册来绕过。`open_workbench` 返回本机私有工作台地址及保存位置；`list_conversations` 列出共享数据目录中可读取的会话，`read_conversation(room_id)` 查看私有状态。

默认保存在 `~/.agentnave/`，可通过服务启动环境 `AGENTNAVE_DATA_DIR` 指定固定绝对目录。省略 `open_discussion.directory` 使用该目录下的新会话位置；显式指定目录须自行确保是长期保存位置。普通任务只保存任务元信息、原生会话绑定和最终回复，不保存完整任务 prompt、思考或原始工具事件。运行中回复尾部只用于实时展示。记录跨重启保留，进程和 invocation_id 不恢复；中断后由主控检查再决定续聊，不自动重试。

多个 MCP 服务可共享同一目录并读取全部历史，首次写入会话的服务独占写入到退出。`writable=false` 的会话由其他服务占用：可读不可写，续接或修改会报错，回到占用它的宿主继续或等该服务退出，不抢占、不合并进程，也不改开新会话拆散历史。手动在终端启动的 CLI 不会自动接入。

## 做任务

直接 `start_agent(provider, prompt, cwd, provider_options, title?, conversation_id?)`。首个任务创建私有会话，返回 `conversation_id` 与工作台 URL；同一件事后续任务传相同 conversation_id。可并行调用不同会话或不同 CLI；同一原生 session 不可同时执行两个任务。续用原生 session_id 时会尽量归入对应尚未结束的任务会话，也可明确提供 conversation_id。

按主 Skill 等待或取消 invocation。程序自动采集回复和终态，不要求主控复述正文给 Dashboard。任务页无公共 URL，不把任务输出传给其他 CLI。若将结果拿去讨论，由主控明确选定材料并按主持人引用发布，不能冒充选手新发言。保留普通 CLI 原有工具与权限，不套用讨论室的 discussion_mode；宿主对普通 start_agent 的排除规则保持原样。

`read_conversation` 提供运行任务的 invocation_id 和已知原生 session_id，仅供主控恢复调度；网页不显示这些内部 ID。已回复不代表已经验收。

## 一起聊与各自答

1. `open_discussion(title, seats, mode, directory?)` 创建；每席指定 id、label、provider、model、effort。支持全部五种 CLI，包括主控同厂商 CLI，使用独立选手会话。重开已有目录使用保存的当前 title/seats/mode。
2. `post_discussion_message(room_id, text)` 发布主持人公开题目；本轮角色指令通过 `start_discussion_turn` 单独发给对应席位。私有指令不要放到公开题目里。
3. 主控调用 `start_discussion_turn(room_id, seat_id, instruction)` 点名，程序向它提供本席未读公开消息和自己的指令，续用本席原生会话。按返回的 invocation_id 等待或取消。
4. 一起聊：有效回复成为 `ready` 候选；用 `decide_discussion_turn(..., action="publish"|"discard")` 按 ID 决定，不复制或改写正文。
5. 各自答：有效回复自动成为 `held` 并进入私有 pending_answers，直接调度下一席，无需主控逐条收存。收集期间不能改题、重复提交或单独公开；公共页面、JSONL 和其他席位输入均无封存答案。
6. 全员提交后，以状态中的 blind_round_id 调用 `decide_discussion_round(..., action="publish")` 一次揭晓。`discard` 可结束未收齐的一轮而不公开已收内容。丢弃不会擦掉各席自己的原生记忆。
7. 有人失败、取消或无法继续时，不自动跳过。先取消并等待其在途调用终态，解决 ready 候选，再调用 `mark_discussion_absent(room_id, seat_id, reason)`。揭晓时自动附“本轮结果不完整”和未作答名单；缺席者不能补交到同一轮。
8. 已揭晓后 `continue_discussion(room_id)` 单向进入一起聊，保留原会话及公开历史；不能反向清洗已见信息。

`reset_discussion_seat` 仅在主控明确决定新身份或原生历史不确定时使用。先解决在途或待发布状态；已封存席位必须先处理整轮。它不删除 CLI 自己的历史文件。

## 查看、结束和边界

`update_conversation(room_id, title?, archived?)` 改名、结束或重新打开；先解决运行任务、候选及未完盲答轮次，结束只隐藏活动状态，不删历史。工作台页面保持只读，调度仍由主控通过 MCP 完成。

私有工作台、导演指令、候选/封存答案、原生 ID、其他会话目录和地址不可交给参演 CLI。公共内容只有当前会话已发布台词。一般 CLI 规则可存在，原生工具限制不等于操作系统隔离。根据用户是否需要独立回答选择模式，不能因已经有公共 Dashboard 就默认逐条公开。
