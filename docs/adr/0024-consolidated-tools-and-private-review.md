# ADR-0024：工具收敛为 8 个，审核改为公开／不公开

- 状态：已接受
- 日期：2026-10-23
- 修订 [ADR-0007](0007-wait-any-ready-subset.md) 的等待工具名、[ADR-0016](0016-single-core-and-mcp-entrypoint.md) 的工具数、[ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md) 的访问等级修订与被丢弃回答的说法、[ADR-0023](0023-explicit-desk-continuation.md) 中按访问等级筛选续接线程的条款；扩展 [ADR-0012](0012-tray-conversation-cleanup.md) 的托盘写入范围。不改变 Manager 拥有编排决策、Provider 原生权限是安全边界的约定。

## 背景

服务曾公开 16 个工具，多数成对重叠：`wait_agent`／`wait_any`／`wait_round`、`cancel_agent`／`cancel_round`、`open_group`／`run_round`／`post_host_message`、`describe_provider`／`list_members`、`list_invocations`／`list_conversations`。工具 schema 约 20.6K 字符，`list_members` 结果另有约 9.5K 字符，每次都进入 Manager 上下文；结果文本还是带缩进与 null 字段的 JSON。

审核中的“丢弃”有歧义：被丢弃的回答从未删除，App 也能看到它，实际是工位只回给 boss 的私聊；该席却不知道自己的回答没进会议室，下一轮可能引用别人没见过的内容。`access` 在工作区内以首次声明为准并拒绝不同声明，但它不改变 CLI 权限，强制只制造拒绝，挡不住真正的写入。看板随每次保存操作启动，而日常查看已由 App 承担。`read_conversation` 返回原始状态，Manager 要自己拼出 App 里那样的消息顺序。会话改名与编排偏好也不属于 Manager：标题在创建时就已知，怎样分工是用户写在宿主全局规则里的事。

## 决定

- 工具为 `start_agent`、`wait`、`cancel`、`list`、`list_members`、`run_round`、`decide_answers`、`read_conversation`。`wait`、`cancel` 同时接受任务 `invocation_id` 与轮次 `round_id`，`wait` 在任一项就绪时返回此刻全部就绪项。不带 `group_id` 的 `run_round` 以 `title`、`members` 开节目，不带 `seats` 时发主持人发言。`list_members(provider)` 另返回该 CLI 的允许状态与支持参数。
- 中继的空闲探测依赖 `list_invocations`，旧中继也会每 5 秒调用它，因此它保留为可调用的隐藏工具，不进入工具列表。
- 结构化结果另以去掉空值的紧凑 JSON 文本返回。
- 审核决定为 `publish` 与 `private`。不公开的回答留在 boss 与该席之间，App 显示为工位给 boss 的「🔒 未公开」回复；该席下一次续接时，若上一轮成功回答未公开，prompt 附一句说明。取消与退出时未决回答按不公开处理；旧记录的 `discard` 读作 `private`。
- `access` 只记录：保存、回显，工位显示最近一次声明；不拒绝调用，不参与续接线程的选择。
- 看板只在 `list` 时启动；保存操作不再启动看板或返回链接。
- `read_conversation` 返回与 App 相同的最终消息时间线，可用 `since_round` 只读后续轮次，`include_private=true` 另含私密指令与不公开回答。
- 标题由 Manager 创建时给出，之后只在 App 中改名；改名在会话锁内改写会话与视图快照的标题（存储见 [ADR-0026](0026-sqlite-conversation-store.md)）。MCP 不提供改名工具。
- 编排偏好属于用户的宿主全局规则。Skill 只写调用方式，不再规定分工或默认轮数；App 设置页给出可复制的推荐模板，并列出、打开已存在的全局规则文件，不创建或写入。

## 取舍

- 改名、删除工具与参数是破坏性变更：宿主需更新服务并重连，Skill 同步重写。旧中继仍能换用新进程，因为探测工具保留。
- `wait` 只有“任一就绪”一种模式，等待全部由 Manager 循环调用；这保留 ADR-0007 的语义，不另加模式参数。
- 放弃访问等级的强制后，只读与可写的区分完全依赖 Provider 原生权限与 Manager 的派发选择；此前的拒绝本就不构成安全边界。
- 席位提示只覆盖上一轮成功回答被设为不公开的情况，不追溯更早的轮次；作者的原生会话仍记得自己的回答，审核不回滚工具副作用。
- 全局规则文件位置是各宿主的常见位置，不保证与每个宿主的实际配置一致，App 只打开已存在的文件。
- 收敛后工具 schema 约 16.6K 字符，`list_members` 结果约 3.7K 字符。
