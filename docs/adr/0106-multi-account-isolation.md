# ADR-0106：支持按账号隔离运行，第一期开放 Claude

- 状态：已接受
- 日期：2026-11-21
- 修订 [ADR-0029](0029-settings-without-free-text.md)（允许手填账号显示名／编号；关联已有目录仍用选择器或扫描，不手填路径）、[ADR-0056](0056-claude-interactive-runtime.md)（非 TUI `-p` 路径同样剥离 Claude 宿主会话变量；具名账号配置与会话隔离在 `CLAUDE_CONFIG_DIR` 内）、[ADR-0062](0062-service-owned-member-names.md)（非默认账号在成员称呼的 CLI 段后增加账号消歧段）、[ADR-0078](0078-startup-confirmations.md)（具名账号的信任与 API Key 确认写入其 `CLAUDE_CONFIG_DIR` 内的 `.claude.json`）、[ADR-0081](0081-account-cooldown-duration.md)（`provider-health.json` 不升版本，具名账号嵌套在 `accounts.<id>` 下独立冷却与清除，认证冷却按整账号记）、[ADR-0089](0089-seat-fallback-order.md)（工位与备选带可选账号并按 `(provider, account, model)` 判重与追踪，同 CLI 不同账号顺位由服务直接启动）、[ADR-0090](0090-quota-group-cooldown-and-host-cli-choice.md)（配额冷却按账号独立；私聊同类宿主收紧为「同 CLI 且同账号」，同 CLI 不同账号由服务直接启动）、[ADR-0100](0100-app-uninstall.md)（托管目录位于数据根外，卸载清理其中的 AgentNave 接入与 Skill 但默认保留目录）、[ADR-0103](0103-tray-embedded-terminal.md)（内嵌终端支持账号选择并按 `workspaces.owner_account` 续接，具名账号经 `/usr/bin/env` 传 `CLAUDE_CONFIG_DIR`；独立登录终端与办公室 agent 分离）。
- 扩展 [ADR-0048](0048-sqlite-schema-evolution.md)（`workspaces` 表新增可空 `owner_account` 列并提升 `user_version`）；沿用 [ADR-0058](0058-claude-stored-login.md)（不存不读 token，API Key 与自定义网关变量不阻断不剥离）。

## 背景

用户有多个 Claude 订阅或配置目录（如工作账号与个人账号），希望在派发子代理（工位、备选顺位、会议席位）与办公室终端中同时在线、各自独立冷却，并在主选账号受阻时自动切换备选账号顶上。其他 Provider CLI（如 Codex 的 `CODEX_HOME`）日后也会有同样诉求，需要在第一期把数据模型与跨进程边界一次定好。

## 决定

- **通用账号模型，第一期只开放 Claude**：数据根新增 `accounts.json`（App 写、服务只读；Linux 无 App 时允许手写），结构预留通用 CLI 账号，第一期只开放 Claude。默认账号不入 `accounts.json`（`account=None`），保证老配置与单账号用户零变化。同一 CLI 的多个账号共用 `members.json` 的可用模型池与 `permissions.json` 的权限档位。
- **默认账号原样继承环境，具名账号显式传入目录**：参考 t3code 的做法，默认账号启动时不设置也不清除 `CLAUDE_CONFIG_DIR`，原样继承服务进程环境，绝不写入字面量 `~/.claude`；具名账号设登记的绝对路径。内嵌终端经 login shell 启动时，为避免 shell 启动脚本覆盖环境变量，具名账号以 `/usr/bin/env CLAUDE_CONFIG_DIR=<dir> claude ...` 启动。始终不修改 `HOME`（改动 `HOME` 会丢失 macOS Keychain 凭据）。
- **托管目录放在数据根外，登录与关联分离**：App 自动创建的托管目录放在数据根外的固定位置 `~/.agentnave-accounts/claude/<id>`；登录在独立登录终端（不属于办公室 agent，不传 `--session-id`，不进入唤醒和续接列表）中带对应 `CLAUDE_CONFIG_DIR` 执行。同时支持通过目录选择器或扫描关联已有目录（不手填路径），关联时检查重复 UUID 与路径冲突并提示。
- **手填短名称与本地展示边界**：允许用户在 App 手填账号显示名／编号；底层以不可变 `id` 关联，改名只改显示名。`claude auth status --json` 返回的邮箱、组织与套餐信息仅在 App 本地展示，不进入 MCP 回执、决策模型输入与日志。非默认账号在成员称呼中加账号消歧段（如 `@opus·Claude Code（工作）·评审`）。
- **配置同步、接入行身份与宿主识别**：托管目录只自动同步 AgentNave 接入（MCP 配置、编排规则与 AgentNave Skill），用户个人配置不跨账号同步；接入行复用 `AGENTNAVE_HOST_APP=claude@<id>`（与 `codex@<folder>` 同构），不新增单独的账号环境变量。ADR-0090 的同类宿主判定收紧为「同 CLI 且同账号」，以服务进程继承到的 `CLAUDE_CONFIG_DIR` 规范化路径为准（与 `AGENTNAVE_HOST_APP` 冲突时提示修复）；账号 A 作为 boss 派发账号 B 时由 AgentNave 直接启动 B。
- **全链路透传与剥离宿主变量**：Adapter 声明账号环境变量名（Claude 为 `CLAUDE_CONFIG_DIR`，预留 Codex `CODEX_HOME`）；服务在启动、权限模式回退、`/usage` 探测与结果回读（含 transcript 读取）全链路透传，不改服务自身 `os.environ`。派发 Claude 时按 t3code 剥离宿主会话变量，补齐非 TUI（`-p`）与探测路径。
- **按解析后的配置目录禁止跨账号续接**：任务、会议席位及 `workspaces` 表新增的可空 `owner_account` 列（按 ADR-0048 升 `user_version`，托盘续接内嵌终端直接读取）记录账号。按解析后的配置目录比对归属，禁止跨账号续接同一原生线程；第一期原生锁与 `boss_key` 不改三端编码。
- **按账号独立冷却与 `(provider, account, model)` 判重**：`provider-health.json` 不升版本，具名账号嵌套在 `accounts.<id>` 下独立冷却与解冷，认证冷却按整账号记；补齐 Claude 交互登录屏报错向认证失败的归类，使未登录账号能触发冷却并走备选。自动顶上只在新线程启动前生效，不重试已执行任务。工位、备选与随机避重统一按 `(provider, account, model)` 判重和追踪。
- **API Key 共存**：存在 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN` 或自定义 `ANTHROPIC_BASE_URL` 时沿用 ADR-0058 不阻断、不剥离；App 卡片按 `auth status` 如实展示认证方式，派发回执附 warning。
- **移除、卸载与预览版约束**：移除账号默认只解绑，外部目录永不删除；托管目录仅在用户勾选确认时删除（删除前先检查活动任务、会议与终端，并校验真实路径、符号链接与 Windows junction），且删除时不自动执行 `claude auth logout`（登出为独立动作）。卸载默认保留托管目录。预览版禁止建账号、登录、同步与删除。

## 取舍

- **默认账号原样继承环境，不写入 `~/.claude` 也不记录首次状态再恢复**：外层 shell 或启动器可能已为默认环境设好自定义 `CLAUDE_CONFIG_DIR`，写死 `~/.claude` 会覆盖用户的显式环境；记录首次启动状态再恢复则会在外部环境变化后失准且增加状态维护成本。
- **托管目录放在 `~/.agentnave-accounts/` 而不放数据根内**：`AGENTNAVE_DATA_DIR` 允许覆盖和迁移，卸载时数据根也可能按开关删除；把账号目录放在数据根外并默认在卸载时保留，避免换数据根路径失效或误删用户的登录态与聊天记录。代价是用户主目录下新增固定隐藏目录，需要独立的清理确认与路径安全校验。
- **允许手填账号名但仍禁止手填路径**：ADR-0029 禁止手填是为了防止用户填错复杂参数与文件路径；账号短名称／编号没有语法陷阱且是多账号辨识必需，关联外部目录仍坚持选择器／扫描。
- **按配置目录禁止跨账号续接**：Claude Code 将会话历史与索引保存在各自配置目录内，不同目录之间线程隔离；沿用 t3code 按解析后的配置目录比对归属，避免续接报找不到会话或串联上下文。
- **按 `(provider, account, model)` 判重并按账号冷却**：允许同 CLI 同模型以不同账号作为备选顺位自动顶上，且 A 账号耗尽或未登录不阻塞 B 账号；代价是冷却文件结构多一层嵌套，且旧版服务会忽略新结构。
- **删除托管目录时不自动 `claude auth logout`**：Keychain 凭据或本地登录态可能与其他关联配置或排障需求相关，自动登出是不可逆外部副作用；将登出留作独立操作，删除托管目录只清理目录本身。
- **同一 CLI 多账号共用 `members.json` 与 `permissions.json`**：降低多账号配置重复度；代价是无法按账号区分可用模型池或权限档位，如有需求须在后续版本单独拆分。
- **不改 `HOME`、不新增 `AGENTNAVE_HOST_ACCOUNT`、不改 `boss_key` 编码**：保留 macOS Keychain 访问能力、复用既有 `AGENTNAVE_HOST_APP=<cli>@<id>` 接入行模型并避免跨端历史键迁移。
