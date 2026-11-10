# ADR-0056：Claude Code 改用交互模式运行

- 状态：已接受
- 日期：2026-11-04
- 启动信任与外部导入确认修订见 [ADR-0078](0078-startup-confirmations.md)。
- 修订 [ADR-0019](0019-no-running-messages.md)：Claude 不再只有一次性 print 调用；交互模式会让会话出现在 Claude Code 自带的历史与 `/resume` 列表里。
- 运行中的原生弹框由 boss 代答，见 [ADR-0057](0057-boss-answers-native-prompts.md)。

## 背景

Claude Adapter 现在用 `claude --print` 加 stream-json 运行，原生会话的入口记为 `sdk-cli`。社区实测（anthropics/claude-code#97074）显示，同样的工作在订阅下用 `-p` 消耗的额度约是交互模式（入口 `cli`）的 1.6–2.9 倍。magpie 在 43966a53f 把 Claude 调用从 `-p` 改为驱动交互 TUI，并刻意清掉 `CLAUDE_CODE_ENTRYPOINT`；Anthropic 对第三方入口名改走额外用量，所以给入口改名不是办法。

PTY 驱动交互 Claude Code 的 spike 已跑通：bracketed paste 投递任务，transcript 的 `turn_duration` 行判定回合结束，`--session-id`／`--resume` 续接；权限框和提问框通过 `--settings` 注入的 hooks 结构化应答，不用读屏按键。读屏只在启动阶段需要。用 hooks 应答弹框，没有找到社区先例。

## 决定

- **macOS 与 Linux 默认打开**。App 在 Claude 员工卡片上提供「交互模式」开关，私聊与会议共用，默认打开；关掉后行为与 `-p` 完全相同。Linux 没有 App，默认即交互模式，boss 可显式传 `interactive=false`。Windows 没有交互运行时，App 不显示开关，服务也不套用，一律用 `-p`。订阅登录和 API key 一样走交互模式；长期 token 不传给子代理（[ADR-0058](0058-claude-stored-login.md)）。隔离会议席位和受限工具运行继续用 `-p`。开关存于数据根 `settings.json`，缺省即打开（App 写入，旧服务忽略多出的键，损坏时与同文件其他设置一样返回 App 修复错误）；服务在入口把它转成 Claude Adapter 的选项并在 `defaults_applied` 注明，调用方显式传这个选项时以显式值为准。它不是权限键，不受权限档位约束。
- **形状**。开关打开时，Claude Adapter 的 Provider argv 换成 AgentNave 自带的 helper，由 helper 在同一 supervisor 进程组里建 PTY、启动 `claude`；进程所有权、取消和清理与现在相同。helper 就是 Provider 进程：它把 transcript 和 hooks 归一成事件写到 stdout，由 Claude Adapter 解析；hooks 与 helper 之间的通道是 helper 私有的，只在该调用自己的临时目录里。核心看到的仍是一个 Provider 进程和一串事件。一次调用一个进程，回合结束即退出，不保活。模型、档位、权限档位的映射与 `-p` 相同；hooks 设置文件属于协议必需参数，与用户自己的 hooks 合并生效。
- **投递内容不变**。服务追加的派发前缀和任务原文与 `-p` 完全相同，整段 bracketed paste 投递，不拆成按键加粘贴，也不放进系统提示。粘贴前清理控制字符和会提前结束粘贴的转义序列；以 `/`、`!`、`#`、`&` 开头时加防护，不被当作斜杠命令、Bash 模式或记忆写入。
- **环境**。AgentNave 不设置 `CLAUDE_CODE_ENTRYPOINT`。服务现在已对所有路径清掉宿主会话 ID `CLAUDE_CODE_SESSION_ID`；交互路径另由 helper 在启动 `claude` 前清掉宿主带下来的其余 Claude 变量（`CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、`CLAUDE_CODE_CHILD_SESSION` 等具名清单）：继承 `CLAUDE_CODE_CHILD_SESSION` 会关掉 transcript，属于协议必需。服务的进程环境和 `-p` 路径不变。不使用 `CLAUDE_CODE_SKIP_PROMPT_HISTORY`，它会同时关掉 transcript 和续接。
- **启动阶段**。粘贴前必须先排除已知启动屏，就绪判定不能只看提示符：
  - 首次引导（文字风格、安全须知、终端设置）：按默认走完。文字风格和安全须知回车，终端设置按 Esc 跳过，不改用户的终端；同一屏应答三次仍在，报 BLOCKED。
  - 文件夹信任框：自动选「信任」。选项默认停在「No, exit」，先移到「Yes」并等画面稳定再回车，盲按回车会退出。回执 `warnings` 注明已信任的目录。
  - API key 确认框：环境里的 `ANTHROPIC_API_KEY` 第一次出现时，Claude Code 问是否使用它，选「Yes」。选项默认停在「No (recommended)」，同样先移到「Yes」等画面稳定再回车。Claude 会把这个 key 的末 20 位记进用户配置的已批准列表；`-p` 本来不问、直接用它。回执 `warnings` 注明。
  - 权限档位的确认框（完全放开档的免责框、自动档的启用确认）：用户在 App 选档位就是同意；helper 在自己的 `--settings` 里写 `skipDangerousModePermissionPrompt`、`skipAutoPermissionPrompt`，属于协议必需参数，不写用户配置。仍然出现就报 BLOCKED。
  - 项目 `.mcp.json` 的新服务确认框：`-p` 在已信任的目录里直接启动这些服务；helper 在自己的 `--settings` 里写 `enableAllProjectMcpServers`，不弹框，也不写用户配置。
  - 冷恢复额度框（「Resume this conversation? … will use about N% of your 5-hour usage limit」）：调用方传了 `session_id` 就是要续接，按 Esc 原样恢复，回执 `warnings` 写明闲置时长、token 数和额度百分比。
  - 从摘要恢复框：报 BLOCKED。永不选摘要（会改上下文），永不选「不再询问」（会写用户配置）；能实测后再评估是否改为只选原样恢复。
  - 登录或不认识的屏：报 BLOCKED，不按键。
  - 投递顺序：确认弹框已消失、输入框为空后粘贴；确认粘贴在输入框里后回车；回车后用 transcript 核对 user 行。
- **运行阶段不按键**。弹框只经 hooks 应答（ADR-0057）。画面出现弹框而 hook 没收到，按通道异常报 BLOCKED；hook 待答期间不向终端写任何键。
- **需要用户操作的工具**。提问框和计划确认（`AskUserQuestion`、`ExitPlanMode`）按 Claude Code 文档的做法，由 PreToolUse hook 以「允许加原样带回的输入」应答，交给 boss（ADR-0057）；批准计划后 Claude 回到派发时的权限档位。MCP elicitation 表单由 Elicitation hook 直接取消，与 `-p` 的结果相同。
- **终态**。回合结束取 transcript 最终回答；`StopFailure` 的 `error`（认证、限流、服务端错误等）映射到现有失败类别；进程意外退出按失败处理。探测或启动失败报 BLOCKED，不自动退回 `-p`；boss 可以显式关掉该选项，本次改用 `-p`。
- **后台任务**。回合要等本回合发起的后台调用都收到完成通知才算结束，与 `-p` 一致；Claude 自行转入后台的 Agent 调用（工具结果标为异步启动）同样要等。Claude 的 Bash 工具给每条命令单开进程组，helper 运行期间记下这些进程组，收尾时与 Claude 一并终止。
- **缓存**。服务不为缓存保活进程、不定时续期、不主动压缩。prompt cache 在服务端按前缀命中，跨进程可复用；TTL 由认证方式、计费状态和 CLI 策略决定（2.1.308：订阅主线程 1 小时，超额或 API key 5 分钟），AgentNave 不保证。idle compact 只在进程存活时触发，本方案不会触发它，上下文策略仍归 Manager。

## 取舍

- 交互会话对用户可见：入口记为 `cli`，出现在 Claude Code 的 `/resume` 列表里，任务原文进入 `~/.claude/history.jsonl` 的输入历史。`-p` 没有这些副作用。
- 依赖 Claude Code「整条消息都是粘贴时按用户请求执行」的行为。长粘贴会被包成 `<pasted_content>`，由远程开关控制；Haiku 实测对遵守度没有可测影响，Sonnet、Opus 没测。
- 自动信任让 Claude 把目录的信任永久写进用户的 `~/.claude.json`，之后用户自己在该目录打开 Claude 时，项目里的 hooks、MCP 和设置直接生效。`-p` 本来不问信任、按信任运行，当次风险相同，多出的是这条永久记录；换来的是普通用户开箱即用，不必逐个目录手动信任。用户定的原则是尽量少打扰用户。
- 启动屏和读屏规则跟随 Claude Code 版本和远程开关变化，比 stream-json 脆弱。用户在 2026-11-05 为省额度定为默认打开；失败一律报 BLOCKED，不猜，也不退回 `-p`，用户可以在 App 关掉开关。Linux 在真实验收前就默认打开。
- 每次调用多一个 helper 进程和约 4–9 秒启动时间。
- 用户自己的 hook 拒绝、本服务允许时，以拒绝为准；这种拒绝只出现在工具结果的文字里，回执不计入被拒的工具调用。

## 考虑过的方案

- **保持 `-p`**：最稳，但额度消耗高。作为开关关掉时和 Windows 上的行为保留。
- **设置 `CLAUDE_CODE_ENTRYPOINT` 为自定义名**：第三方入口改走额外用量，magpie 也刻意清掉它。放弃。
- **读屏按键应答弹框**：不用 hooks 也能做，但选项文案和焦点随版本变化，误按会静默改变行为。只留作启动阶段。
- **`SessionStart` 返回 `initialUserMessage` 投递任务**：只在 `-p` 生效。
- **固定指令按键、任务正文粘贴，或指令放进 `--append-system-prompt-file`**：实测遵守度与整段粘贴相同；后者只多一点系统提示缓存，还要 Adapter 多加 prompt 参数。不作为默认。
- **保活进程借 idle compact、续接前主动 `/compact`、定时请求续期缓存**：续接间隔多在 1 小时内，现有缓存已命中；保活改变进程所有权和运行名额，续接时压缩为时已晚，定时请求无人值守地消耗额度。放弃。重新评估的条件：Claude「≥200k 上下文且间隔超过 1 小时」的续接明显增多，默认 TTL 下调，Claude 从会议席位扩展到私聊，Claude Code 提供非交互的官方压缩入口，或回执开始提供上下文大小。

## 尚未真实验收

用户在 2026-11-05 定为默认打开，以下情形尚未真实验收，出现时按上面的规则报 BLOCKED 或按现有映射处理：真实限流菜单（只观察不操作）；auto 模式分类器连续拦截后的恢复提示；冷恢复额度框原样恢复后的完整回合；从摘要恢复框；Linux。
