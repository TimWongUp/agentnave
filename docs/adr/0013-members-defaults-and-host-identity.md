# ADR-0013：App 组员默认值、模式强制与宿主归属

- 状态：部分取代
- 日期：2026-10-16
- 核心不持久化的条款由 [ADR-0016](0016-single-core-and-mcp-entrypoint.md) 取代
- 进程级主控身份由 [ADR-0017](0017-manager-chat-session-identity.md) 修订
- 组员身份、模式命名与群聊展示由 [ADR-0018](0018-office-workspace-desks-and-meeting-rooms.md) 修订
- 无配置时的模型预填行为由 [ADR-0020](0020-prefill-main-model-defaults.md) 修订；后由 ADR-0028 取代该预填。
- 不受限模式的默认模型由 [ADR-0021](0021-keep-default-models-when-unrestricted.md) 补充
- 工具名与工具集合由 [ADR-0024](0024-consolidated-tools-and-private-review.md) 修订
- 权限默认值范围及无配置时的原生行为由 [ADR-0032](0032-cli-permission-tiers.md) 修订
- 工位默认模型、档位套用与名单校验例外由 [ADR-0038](0038-role-applies-app-seat.md) 修订

本决定取代 [ADR-0012](0012-tray-conversation-cleanup.md) 中关于 `profiles.json` 的增补：App 改为维护唯一的 `members.json`，核心与工作室只读、每次调用现读；旧档案迁移只由 App 负责，服务不兼读两种真源。顶层 `version`、`clis`、`modes` 三项必填，与第一期共享契约修订 1 一致。

组员以 `(provider, model)` 为身份，共享配置由 task／discussion 模式按出现字段整体覆盖。服务强制模式名单与模型／effort 范围，补齐调用方未传的用户默认值，并在回执 `defaults_applied` 逐键注明；显式值超出范围时拒绝，最终参数仍由 Adapter 校验。模式模型列表非空、无默认模型且调用方未传 model 时，必须拒绝并列出允许模型，不能退回 CLI 原生模型绕过白名单。空模型列表不限模型，`list_members` 除已知组员外以 `model="*"` 的空配置条目说明这一放行范围。

这改变了原先“权限参数仅在用户本轮要求时显式传入”的安全语义：权限类参数（如 `dangerously_*`、`permission_mode`、`sandbox`）可来自用户在 App 中设定的默认 options，或用户本轮的明确要求，主控不得自行追加。App 权限默认值与其他 options 一样在回执注明；`session_id` 非空的续接也现读配置、强制范围并套用默认值，不保证延续上次启动参数。显式字段优先，无配置文件时保持原生行为。

隔离节目显式增加 `restricted_tools`；为保持受限选项合同，只套用 model／effort（Pi 另可套用模型后端 provider），跳过其他默认 options，也不将跳过的键写入 `defaults_applied`。调用方显式提供的非法受限选项仍由 Adapter 拒绝，不静默删除。

宿主身份以显式 `AGENTNAVE_HOST` 优先，POSIX 祖先进程最多 32 层兜底，Windows 只读环境变量。已知解释器或包装层可按第一个非选项参数识别 CLI 脚本；原生 Claude 安装的 `claude/versions/<版本号>` 路径也可识别。读不到一层 argv 时仍尝试父进程；读不到父进程或遇到不匹配宿主的非包装进程则停止，避免跨未知进程误判外层 GUI。AgentNave 自身的 Python 中继脚本作为包装层跳过。

宿主映射的 Provider 与既有环境排除项取并集，只限制 task；discussion 可容纳同类 CLI，但仍受自身组员配置约束。工作室 `describe_provider.show_permitted` 每次按 discussion 的 CLI 名单计算，文件缺失或模式不受限时为 true。选择按模式强制而非全局自我排除，是为了同时保留单任务的同类编排边界与协作讨论的席位需求。

工作室新会话保存创建宿主及进程级 UUID `manager_session`，会话摘要与运行快照保留归属；后续宿主接手不改写创建者，live 顶层单独标识当前服务。旧记录允许缺失，不迁移历史、不恢复运行进程。核心仍不持久化，宿主 MCP 配置不由本次变更修改。
