# ADR-0017：按主控聊天归集 App 群聊

原 `manager_session` 是 MCP 连接的 UUID，常驻宿主可能在多个聊天间共用一个连接，重连又会把同一聊天拆开。App 群聊因此改用调用方每次创建任务或协作组时提供的主控聊天 ID；宿主未公开原生 ID 时，由主控为当前聊天生成并复用一个 UUID。明确按聊天注入的环境变量仅作兜底，不扫描宿主日志或依据 CLI、目录、时间推测归属。

保存的分组键由宿主身份与原始 ID 确定性散列，并加 `s-` 前缀，与旧版连接 UUID 区分。真实身份未知时 `start_agent` 与 `run_round` 拒绝调用，不再产生无归属的新记录（v0.45.0 修订：漏传会让同一聊天的历史散成多条，比报错更难发现）；旧历史没有足够信息重建主控边界，保留内容并独立展示，不伪造归属。live 顶层改用 `connection_id` 表示服务连接。此决定修订 ADR-0013 的进程级主控身份。

Provider 原生 session 继续用于 CLI 续接，与主控聊天身份独立；自动寻找任务历史绑定限于当前主控会话。跨主控续接同一个 Provider session 可以创建当前主控的新记录，但不能把另一主控的任务记录并进来；无归属的旧记录仍可读取，带当前身份用原生 `session_id` 续接时创建当前主控的新记录，不追加到旧记录。任务续接在独立于聊天的原生 session 启动锁内扫描活跃绑定并落盘，避免不同聊天在首次记录尚未保存时并发启动同一原生 session。主控聊天环境变量不传给子 CLI。

补充：Claude Code 为每个会话启动的 MCP 进程自带 `CLAUDE_CODE_SESSION_ID`，与该会话 transcript 文件名一致，作为注入变量之后的第二个兜底，使漏传 `manager_session_id` 的 Claude Code boss 也能归入真实聊天并读到模型。代价是进程级变量在 `/clear` 等不重启 MCP 的会话切换后可能仍是旧值，新聊天会并入旧聊天、模型读自旧 transcript；因此显式传入仍优先，Skill 要求 boss 每个聊天从 Bash 读取当前值传入。未采用随每次刷新上报会话与模型的宿主钩子（statusLine／SessionStart），它要改用户的宿主配置。Codex 不给 MCP 进程传线程 ID，只能由 boss 从 shell 读 `CODEX_THREAD_ID` 显式传入。
