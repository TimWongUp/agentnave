# Antigravity CLI

Antigravity 作宿主时按回执 `next` 选择等待方式（App 内外终端一致，不再由 App 键入唤醒）：已验收版本的交互 `agy` CLI（1.5.14 及以上）会在 `next` 里给出只读等待命令，照常原样运行（超过 10 秒自动转入后台，不挂阻塞 `wait`），命令返回（含 10 秒内前台直接返回）后按输出行处理——`ready` 或 `expired` 调 `wait(wait_seconds=0)` 收取并按新回执续等，`already_watching` 或 `all_done` 直接结束本轮，`service_gone` 回到当前连接核对；Antigravity App、非交互模式或未确认版本的 `agy` 仍按 `next` 前台 `wait`。

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "antigravity"`；用户点名时的写法示例：

```json
{"model": "gemini-3.8-flash", "effort": "high"}
```

对应 `agy` CLI。原生 `print_timeout` 由 CLI 执行，与 AgentNave 的单次等待时长是不同参数；仅在用户明确要求时设置。

服务默认给 Antigravity 派发带上 `"dangerously_skip_permissions": true`（`--dangerously-skip-permissions`），自动批准全部原生工具权限请求，CLI 已存配置不变，回执 `defaults_applied` 会注明；App 不提供其他档位。要某次调用守原生权限，只能由用户本轮明确要求，boss 显式传任一权限键（`mode`、`sandbox` 或 `dangerously_skip_permissions: false`）时整档不套用。`mode` 与 `sandbox` 是独立选项，不代表 YOLO。

macOS／Linux 上权限放开时默认以交互模式运行（App 关掉「交互模式运行」或显式 `interactive=false` 时改用 `-p`），回执 `defaults_applied` 注明 `interactive`；不放开权限、Windows、受限工具路径，或传了 `print_timeout`、`disable_slash_commands` 时用 `-p`。交互模式要求 agy 1.5.10 或更新版本，通过原生终端事件判断进度。出现原生提问或需要用户输入的弹窗时，任务报受阻并保留会话；请按回执判断是否续接。需要用户决定的事项仍应写进最终回答。

准备阶段信任屏显示目录与派发 `cwd` 为同一目录时，服务自动确认；agy 会将选择永久写进 `~/.gemini/antigravity-cli/settings.json`，这是持久放行先问用户的限定例外。目录不符或未知屏受阻。

## 显式受限工具

`restricted_tools=true` 使用默认 Agent，不传 `--agent` 也不向目录写入 `.agents/agents/agentnave-discussion.md`（自 agy 1.5.17 起自定义 Agent 按高倍率扣额，默认 Agent 维持原价）。Adapter 通过本次会话的 `--settings` 传入不落盘的收紧配置：`permissions.deny` 拒绝 `command(*)`、`read_file(*)`、`write_file(*)`、`read_url(*)`、`execute_url(*)`、`download_url(*)`、`mcp(*)`，并关闭非工作区访问与 slash commands、开启终端沙箱、工具权限设为 `request-review`，同时保留 `--sandbox=true`、`--disable-slash-commands=true`。

这是权限层拒绝执行，不是移除定义或操作系统沙箱：`read_file`、`write_file`、`edit_file`、`run_command`、MCP 工具调用（含派生子代理读文件）会被拒绝，不写 stderr 且回合正常返回；`--conversation` 续接持续生效且不改写 `settings.json`。但工具声明仍在默认 Agent 上下文中，未走权限表的内置工具（`search_web`、`list_resources`/`read_resource`、`generate_image`、`schedule`、`invoke_subagent` 等）不受该 deny 规则拦截；且默认 Agent 在非空 `cwd` 中仍会加载项目规则（含 `@[label](path)` 文件引用）、hooks，子代理入口也可发现目录内的 `.agents/agents/*.md`。单任务需要内容或规则隔离时，应使用独立空目录或 `isolated=true`，不要在不可信项目目录下仅依赖 `restricted_tools=true`。

## 项目目录

在项目内执行任务时，将项目绝对路径传给 `cwd`，并在交接 prompt 中明确同一路径。要求 CLI 将终端工具的 `Cwd` 显式设为该绝对路径，先执行只读 `pwd`，确认一致后再进行项目操作；后续项目命令沿用该目录。目录不符或无法设置时，停止项目操作并报告实际目录。临时交接文件的位置不改变目标目录。

`cwd` 设置 CLI 进程启动目录，不限制内部 Agent 为终端另选 `Cwd`；上述交接要求是行为约束，不是目录隔离或强制保证。验收时核对实际执行证据，不能仅凭启动参数判定目录正确。

`provider_options.project` 用于选择原生项目 ID 或名称，仅在已核对对应项目且调用方明确选择时传入。它不等同于文件系统工作目录；不要把项目路径自动填入该参数作为目录修复。会话续接保持已选择的原生项目，并明确本轮目标目录；不同目录的任务不沿用旧的目录核验结论。
