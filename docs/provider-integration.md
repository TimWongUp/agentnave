# 新增 Provider CLI：员工与 boss 接入清单

一个 CLI 要成为 AgentNave 员工，也必须能作为 boss 接入。接入以代码、Adapter、MCP schema
和测试为准；架构边界见 [architecture.md](context/architecture.md)。以下步骤用于新增
Provider，不是安装现有 CLI 的用户操作说明。

1. **确认原生 CLI 合同。** 核对安装与登录、非交互调用、stdin/stdout、续接、模型、权限档位、
   原生 session ID 和会话记录。Adapter 在 `src/agentnave/adapters/` 只处理该 CLI 的协议、
   参数与结果解析；不静默指定模型、权限或追加 prompt。把 Provider 注册到
   `src/agentnave/models.py`、`adapters/__init__.py`，并检查 `core.py` 的调用路径。
2. **接员工。** 在 `src/agentnave/members.py` 登记 CLI 的可用选项与权限档位，在
   `src/agentnave/orchestration_roles.py` 的 Provider 类型中开放编排工位；App 的
   `apps/tray/src/detect.rs` 与 `detect/probe.rs` 负责可执行文件、模型和档位探测，
   `apps/tray/src/members.rs` 负责员工配置。员工须经 App 确认后才可派发。
3. **接 boss。** 在 `src/agentnave/host.py` 识别宿主，读取原生会话与模型的逻辑放在
   `native_sessions.py`；同类 CLI 的单任务排除与会议规则要保持一致。App 的
   `apps/tray/src/hosts/` 负责宿主目录、MCP 条目、接入、重连和进程检测，
   `apps/tray/src/rules.rs` 负责全局规则入口。接入时安装同版本的服务与 Manager Skill；
   宿主启动后用 `list_members` 核对 `host`、`host_source` 和员工可用性。
4. **补编排与展示。** 高级编排配置在数据根 `orchestration.json`，默认
   `~/.agentnave/orchestration.json`，自定义数据根由 `AGENTNAVE_DATA_DIR` 指定；
   员工模型池在同目录 `members.json`，评审工位在 `review.json`。App 的「设置 → 高级 → 编排」
   保存工位并更新宿主全局规则；`list_members.orchestration` 应显示当前 boss 的有效工位。
   若新增宿主标志或办公室素材，按 `apps/tray/ART-SOURCES.md` 记录来源、透明底与锚点，
   检查 `apps/tray/ui/common.js`、`chat-office.js` 和 `laptop.js` 的标志与模型参数。
5. **验收两端。** 使用 fake executable 验证 Adapter、错误与终态、原生会话回读、
   App 探测与接入、前端资产加载。另须取得当次用户授权，至少通过 AgentNave 向该 CLI
   发起一次真实员工调用，并从该 CLI 宿主真实调用 AgentNave 工具；分别核对结果、
   原生 session ID、模型回读和 `host`。真实调用未经核验时，不标记为接入完成。

本地检查命令见仓库 `AGENTS.md`。用户安装和接入现有 CLI 的步骤见
[installation.md](installation.md)。
