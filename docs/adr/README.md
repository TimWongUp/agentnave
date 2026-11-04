# ADR 索引

现行边界以 [docs/context/architecture.md](../context/architecture.md) 为准，ADR 记录取舍历史。

| 编号 | 标题（链接） | 状态 | 被谁修订/取代 |
| --- | --- | --- | --- |
| 0001 | [执行运行时边界](0001-execution-runtime-boundary.md) | 已被 ADR-0002 取代 | [ADR-0002](0002-lightweight-cli-subagent-adapter.md) |
| 0002 | [AgentNave 收敛为轻量 CLI 子代理适配器](0002-lightweight-cli-subagent-adapter.md) | 部分取代 | [ADR-0003](0003-agent-only-stdio-mcp-server.md)、[ADR-0008](0008-tray-app-reads-studio-snapshots.md)、[ADR-0016](0016-single-core-and-mcp-entrypoint.md)、[ADR-0026](0026-sqlite-conversation-store.md)、[ADR-0027](0027-deepseek-official-route-stored-credentials.md)、[ADR-0049](0049-service-initiated-termination.md) |
| 0003 | [只提供面向 Agent 的 STDIO MCP 接口](0003-agent-only-stdio-mcp-server.md) | 部分取代 | [ADR-0024](0024-consolidated-tools-and-private-review.md) |
| 0004 | [Windows 使用 Job Object 监督 Provider 进程树](0004-windows-job-object-supervision.md) | 已接受 | — |
| 0005 | [由主控 Agent 导演的公共发言区与独立工作台](0005-director-controlled-discussion-rooms.md) | 已被 ADR-0006 取代 | [ADR-0006](0006-core-rounds-and-studio-split.md)、[ADR-0008](0008-tray-app-reads-studio-snapshots.md) |
| 0006 | [核心收敛为子代理与协作轮次，节目、历史与看板拆为工作室包](0006-core-rounds-and-studio-split.md) | 部分取代 | [ADR-0009](0009-single-package-two-entry-points.md)、[ADR-0013](0013-members-defaults-and-host-identity.md)、[ADR-0016](0016-single-core-and-mcp-entrypoint.md)、[ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0025](0025-session-desks-and-running-limit.md)、[ADR-0045](0045-remove-browser-dashboard.md)；2026-10-20 代码修订（席位上限 7） |
| 0007 | [`wait_any` 返回就绪子集并只消费回复中的阻塞](0007-wait-any-ready-subset.md) | 部分取代 | [ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0039](0039-retry-lost-seat-within-round.md)、[ADR-0049](0049-service-initiated-termination.md)、[ADR-0057](0057-boss-answers-native-prompts.md)（提议） |
| 0008 | [托盘应用只读展示工作室数据，工作室写出视图快照与运行文件](0008-tray-app-reads-studio-snapshots.md) | 部分取代 | [ADR-0010](0010-launcher-relay-swaps-upgraded-server.md)、[ADR-0012](0012-tray-conversation-cleanup.md)、[ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md)、[ADR-0026](0026-sqlite-conversation-store.md)、[ADR-0044](0044-meeting-voice-cloud-and-key-input.md)、[ADR-0045](0045-remove-browser-dashboard.md)、[ADR-0050](0050-v1-compatibility-baseline.md) |
| 0009 | [核心与工作室合为一个发行包，保留两个入口](0009-single-package-two-entry-points.md) | 已被 ADR-0016 取代 | [ADR-0016](0016-single-core-and-mcp-entrypoint.md) |
| 0010 | [入口经中继启动，升级后在空闲时自动换用新服务](0010-launcher-relay-swaps-upgraded-server.md) | 部分取代 | [ADR-0014](0014-tray-members-and-one-click-update.md)、[ADR-0016](0016-single-core-and-mcp-entrypoint.md)、[ADR-0045](0045-remove-browser-dashboard.md)、[ADR-0047](0047-versioned-service-install.md) |
| 0011 | [被派出的 CLI 只能使用零工具 AgentNave 服务](0011-disable-nested-orchestration.md) | 已被 ADR-0031 取代 | [ADR-0031](0031-prompt-only-orchestration.md) |
| 0012 | [托盘在原生会话锁下清理记录](0012-tray-conversation-cleanup.md) | 部分取代 | [ADR-0013](0013-members-defaults-and-host-identity.md)、[ADR-0014](0014-tray-members-and-one-click-update.md)、[ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0026](0026-sqlite-conversation-store.md)、[ADR-0045](0045-remove-browser-dashboard.md)、[ADR-0046](0046-service-history-retention.md) |
| 0013 | [App 组员默认值、模式强制与宿主归属](0013-members-defaults-and-host-identity.md) | 部分取代 | [ADR-0016](0016-single-core-and-mcp-entrypoint.md)、[ADR-0017](0017-manager-chat-session-identity.md)、[ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md)、[ADR-0020](0020-prefill-main-model-defaults.md)、[ADR-0021](0021-keep-default-models-when-unrestricted.md)、[ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)、[ADR-0038](0038-role-applies-app-seat.md) |
| 0014 | [托盘维护组员配置、探测 CLI 模型，并一键全部更新](0014-tray-members-and-one-click-update.md) | 部分取代 | [ADR-0029](0029-settings-without-free-text.md)、[ADR-0034](0034-models-and-efforts-from-each-cli.md)、[ADR-0047](0047-versioned-service-install.md)、[ADR-0050](0050-v1-compatibility-baseline.md) |
| 0015 | [App 定点编辑宿主配置，保留外部 Skill 所有权](0015-app-host-integration.md) | 部分取代 | [ADR-0029](0029-settings-without-free-text.md)、[ADR-0036](0036-app-owns-service-skills.md)、[ADR-0040](0040-boss-app-from-session-records.md) |
| 0016 | [统一核心与 MCP 入口](0016-single-core-and-mcp-entrypoint.md) | 部分取代 | [ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0031](0031-prompt-only-orchestration.md)、[ADR-0033](0033-boss-workspace-handover.md)、[ADR-0045](0045-remove-browser-dashboard.md) |
| 0017 | [按主控聊天归集 App 群聊](0017-manager-chat-session-identity.md) | 部分取代 | [ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md)、[ADR-0033](0033-boss-workspace-handover.md)、[ADR-0050](0050-v1-compatibility-baseline.md) |
| 0018 | [主控工作区、工位与会议室](0018-office-workspace-desks-and-meeting-rooms.md) | 部分取代 | [ADR-0023](0023-explicit-desk-continuation.md)、[ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0025](0025-session-desks-and-running-limit.md)、[ADR-0038](0038-role-applies-app-seat.md)、[ADR-0050](0050-v1-compatibility-baseline.md) |
| 0019 | [不提供运行中消息](0019-no-running-messages.md) | 部分取代 | [ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0056](0056-claude-interactive-runtime.md)（提议）、[ADR-0057](0057-boss-answers-native-prompts.md)（提议） |
| 0020 | [首次预填各 CLI 的主力模型](0020-prefill-main-model-defaults.md) | 已被 ADR-0028 取代 | [ADR-0028](0028-follow-cli-default-model.md) |
| 0021 | [关闭自选时保留各 CLI 的默认模型](0021-keep-default-models-when-unrestricted.md) | 部分取代 | [ADR-0028](0028-follow-cli-default-model.md)、[ADR-0035](0035-member-cards-one-row-per-cli.md)、[ADR-0054](0054-members-need-confirmation.md) |
| 0022 | [新会话自动消除宿主重新加载提醒](0022-auto-clear-host-reload.md) | 已接受 | — |
| 0023 | [私聊续接改为显式开启](0023-explicit-desk-continuation.md) | 已被 ADR-0025 取代 | [ADR-0024](0024-consolidated-tools-and-private-review.md)、[ADR-0025](0025-session-desks-and-running-limit.md) |
| 0024 | [工具收敛为 8 个，审核改为公开／不公开](0024-consolidated-tools-and-private-review.md) | 部分取代 | [ADR-0033](0033-boss-workspace-handover.md)、[ADR-0037](0037-app-writes-orchestration-rules.md)、[ADR-0038](0038-role-applies-app-seat.md)、[ADR-0045](0045-remove-browser-dashboard.md)、[ADR-0057](0057-boss-answers-native-prompts.md)（提议） |
| 0025 | [工位即原生线程，并发上限决定显示上限](0025-session-desks-and-running-limit.md) | 部分取代 | [ADR-0038](0038-role-applies-app-seat.md) |
| 0026 | [会话存储改为 SQLite](0026-sqlite-conversation-store.md) | 部分取代 | [ADR-0033](0033-boss-workspace-handover.md)、[ADR-0048](0048-sqlite-schema-evolution.md) |
| 0027 | [DeepSeek 官方路由自动恢复已存认证](0027-deepseek-official-route-stored-credentials.md) | 已接受 | — |
| 0028 | [默认模型与档位跟随 CLI 自身设置](0028-follow-cli-default-model.md) | 部分取代 | [ADR-0035](0035-member-cards-one-row-per-cli.md)、[ADR-0042](0042-pass-cli-default-effort.md) |
| 0029 | [设置页只提供选择，不提供手填输入](0029-settings-without-free-text.md) | 部分取代 | [ADR-0032](0032-cli-permission-tiers.md)、[ADR-0035](0035-member-cards-one-row-per-cli.md)、[ADR-0037](0037-app-writes-orchestration-rules.md)、[ADR-0044](0044-meeting-voice-cloud-and-key-input.md) |
| 0030 | [App「技能」页提供可选 Skill，评审门控为首个，评审工位在 App 设置](0030-optional-review-gate-skill.md) | 部分取代 | [ADR-0036](0036-app-owns-service-skills.md)、[ADR-0037](0037-app-writes-orchestration-rules.md)、[ADR-0038](0038-role-applies-app-seat.md) |
| 0031 | [单向编排使用派发指令约束](0031-prompt-only-orchestration.md) | 已接受 | [ADR-0043](0043-service-prompt-instructions.md) |
| 0032 | [App 按 CLI 设权限档位](0032-cli-permission-tiers.md) | 已接受 | [ADR-0057](0057-boss-answers-native-prompts.md)（提议） |
| 0033 | [boss 工作区交接](0033-boss-workspace-handover.md) | 部分取代 | [ADR-0048](0048-sqlite-schema-evolution.md) |
| 0034 | [模型与档位以 CLI 自己给出的为准](0034-models-and-efforts-from-each-cli.md) | 已接受 | — |
| 0035 | [员工卡片一行设好一个 CLI，需要时再分开两个场景](0035-member-cards-one-row-per-cli.md) | 部分取代 | [ADR-0042](0042-pass-cli-default-effort.md)、[ADR-0050](0050-v1-compatibility-baseline.md) |
| 0036 | [本服务的 Skill 只由 App 管理](0036-app-owns-service-skills.md) | 已接受 | — |
| 0037 | [App 写入编排规则，Explorer 与 Worker 分简单、困难两档](0037-app-writes-orchestration-rules.md) | 部分取代 | [ADR-0038](0038-role-applies-app-seat.md)、[ADR-0041](0041-orchestration-mode-per-boss.md)、[ADR-0044](0044-meeting-voice-cloud-and-key-input.md)、[ADR-0051](0051-parallel-preset-without-recheck.md)、[ADR-0052](0052-orchestration-specialty-seats.md) |
| 0038 | [私聊的 role 指明工位时，服务套用 App 里的工位设置](0038-role-applies-app-seat.md) | 部分取代 | [ADR-0043](0043-service-prompt-instructions.md)、[ADR-0050](0050-v1-compatibility-baseline.md)、[ADR-0052](0052-orchestration-specialty-seats.md)、[ADR-0053](0053-disabled-cli-runs-for-no-seat.md) |
| 0039 | [单席失败提醒一次，并联轮内可补跑](0039-retry-lost-seat-within-round.md) | 已接受 | — |
| 0040 | [工作区按会话记录显示 boss 的入口，接入按配置文件划分](0040-boss-app-from-session-records.md) | 已接受 | — |
| 0041 | [编排档位按 boss 选择，读同一文件的 boss 共用一档](0041-orchestration-mode-per-boss.md) | 已接受 | — |
| 0042 | [组员模型带档位，服务每次传模型与档位](0042-pass-cli-default-effort.md) | 已接受 | — |
| 0043 | [服务统一追加非交互与只读说明](0043-service-prompt-instructions.md) | 已接受 | — |
| 0044 | [会议语音使用云端引擎并允许配置摘要接口](0044-meeting-voice-cloud-and-key-input.md) | 已接受 | — |
| 0045 | [去掉浏览器看板](0045-remove-browser-dashboard.md) | 已接受 | [ADR-0046](0046-service-history-retention.md) |
| 0046 | [历史自动清理由服务执行](0046-service-history-retention.md) | 已接受 | — |
| 0047 | [App 按版本安装服务并在启动时跟进](0047-versioned-service-install.md) | 已接受 | — |
| 0048 | [SQLite schema 演进与迁移归属](0048-sqlite-schema-evolution.md) | 已接受 | — |
| 0049 | [服务主动终止任务的情形](0049-service-initiated-termination.md) | 已接受 | — |
| 0050 | [1.0 兼容基线](0050-v1-compatibility-baseline.md) | 已接受 | — |
| 0051 | [并行档不再逐次复核，评审时机交给用户](0051-parallel-preset-without-recheck.md) | 已接受 | — |
| 0052 | [Worker 增加界面两档](0052-orchestration-specialty-seats.md) | 已接受 | — |
| 0053 | [员工里停用的 CLI 不能当工位](0053-disabled-cli-runs-for-no-seat.md) | 部分取代 | [ADR-0054](0054-members-need-confirmation.md) |
| 0054 | [CLI 要在员工里确认后才能用，接入 boss 时自动确认](0054-members-need-confirmation.md) | 已接受 | — |
| 0056 | [Claude Code 可选改用交互模式运行](0056-claude-interactive-runtime.md) | 提议 | — |
| 0057 | [运行中的原生弹框由 boss 代答](0057-boss-answers-native-prompts.md) | 提议 | — |
