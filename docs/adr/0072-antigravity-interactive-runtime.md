# ADR-0072：Antigravity CLI 在权限放开时改用交互模式运行

- 状态：已接受
- 日期：2026-11-09
- 信任屏确认与用户配置持久化例外修订见 [ADR-0078](0078-startup-confirmations.md)。
- 运行活动可由屏幕状态行更新，见 [ADR-0079](0079-agy-screen-activity.md)。
- 沿用 [ADR-0056](0056-claude-interactive-runtime.md) 的形状：helper 在同一 supervisor 进程组里用伪终端驱动 CLI，一次调用一个进程，回合结束即退出。

## 背景

Antigravity Adapter 用 `agy --input-format stream-json --output-format stream-json` 运行。用户为省额度要求像 Claude 一样改走交互界面，并限定只在权限放开（`--dangerously-skip-permissions`）时使用，运行中不处理权限框。

实测（agy 1.5.3，`gemini-3.8-flash-low`）：

- 伪终端启动 → 整段 bracketed paste 投递多行任务 → 回车，单回合约 16–19 秒；`--conversation` 续接保留上下文；放开权限后工具直接执行。
- 回合结束只有 hooks 能可靠告知：`Stop` hook 带 `conversationId`、`modelName`、`terminationReason`、`error` 与 `fullyIdle`（后台命令是否跑完）。agy 的运行日志里没有回合结束标记。
- 会话记录在 `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/`：`transcript.jsonl` 会截断长字段，`transcript_full.jsonl` 不截断。
- hooks 只从工作区的 `.agents/hooks.json` 或全局 `~/.gemini/config/hooks.json` 加载；`--settings` 拒绝 `hooks` 键。用 `--add-dir` 加一个带 hooks 的临时目录能生效，但 agy 按路径字母序排列工作区，临时目录可能排到项目前面，实测模型会把命令跑在临时目录里。
- hook 进程继承 agy 的环境变量；hook 命令以 `exec` 起头时，其 `$PPID` 就是运行它的 agy 进程。
- 不在信任列表里的目录会弹信任框；`--settings` 传入的 `trustedWorkspaces` 只对本次会话生效，不写入用户配置。
- 回合发起后台命令时，`Stop` 先带 `fullyIdle=false`；命令结束后 agy 再唤醒模型写一段回复，然后补发一次 `fullyIdle=true` 的 `Stop`。后台命令运行期间页脚已经显示空闲的 `? for shortcuts`，只看页脚会提前收尾。
- 长粘贴在输入框里折叠成 `> [Pasted text #1 +121 lines]`。
- 在 200×50 的伪终端里，agy 1.5.3 用相对光标移动、`CSI X`（擦除字符）、`CSI P`（删除字符）和 `ESC M`（反向换行）重绘，没有发出滚动区序列 `CSI r`；不处理前三者，旧文字会残留在新画面旁，页脚和输入行也会被弄乱。

成熟项目里，awslabs 的 cli-agent-orchestrator 用 tmux 加 `--dangerously-skip-permissions` 驱动 agy，只读渲染后的底部页脚判断状态（`esc to cancel` 或盲文转圈为运行中，`? for shortcuts` 为空闲，`↑/↓ navigate`、`[y/n]` 为等待用户），并维护用户全局 `mcp_config.json` 里自己的条目；它从屏幕抠取回答，不判断后台命令。codex-agy-bridge 在任务末尾追加完成标记，本项目不静默增补 prompt，不采用。

## 决定

- **何时使用**。macOS／Linux 默认打开，App 在 Antigravity 员工卡片上提供「交互模式运行」开关，存于数据根 `settings.json`，缺省打开；Windows 没有交互运行时。服务只在本次运行已带 `dangerously_skip_permissions=true`、且没有 `-p` 专有选项（`print_timeout`、`disable_slash_commands`）时套用，并在 `defaults_applied` 注明；App 的两个权限档位对 Antigravity 都写放开，所以开着开关时私聊与非受限会议席位都走交互模式。调用方显式传 `interactive=true` 却没放开权限，Adapter 拒绝。受限工具席位继续用 `-p`。
- **形状**。Adapter 的 Provider argv 换成服务自带的 helper（`agentnave.agy_tui`），helper 用伪终端启动 `agy`，把 transcript 与 hook 事件归一成 agy `-p` 的 stream-json 事件（`init`、`step_update`、`result`）写到 stdout，Adapter 的解析和活动映射与 `-p` 共用。模型、档位、模式等参数映射与 `-p` 相同。
- **hooks 走全局文件的门控条目**。helper 每次运行前确认全局 `hooks.json` 里有名为 `agentnave-interactive` 的条目，缺失或内容不同就补上，保留其他条目；首次写入在回执 `warnings` 注明。条目只在 helper 给 agy 设置了专用环境变量时起作用，否则立即返回空结果，不影响用户自己、Antigravity App 与 IDE 的会话。它包含：
  - `SessionStart`、`Stop`：把事件写到本次调用的私有临时目录，文件名带 `$PPID`，helper 只认自己启动的 agy；任务里嵌套运行的 agy 进程号不同，被忽略。同一 agy 里子代理会话的 `Stop` 按会话 ID 忽略。
  - `PreToolUse`（只匹配 `ask_question`）：拒绝并说明无人能答、把问题写进最终回答，与 `-p` 一致；没有环境变量时返回 `allow`，交给 agy 默认策略。
  - 文件不是合法 JSON 或条目被用户停用时报 BLOCKED，不覆盖。
- **信任**。helper 用 `--settings` 只对本次会话把工作目录加入 `trustedWorkspaces`，属于协议必需参数，不弹信任框，也不写用户配置。
- **分工**。页脚判断状态，transcript 提供回复和投递确认，`Stop` 证明回合连同后台命令已经结束。证据不足时限时报 BLOCKED，不推定成功，也不无限等待。
- **画面识别**。helper 用 agy 专用的画面模型，在 Claude 的模型上补 `CSI X`、`CSI P`、`ESC M`，Claude 路径不变；只补实际字节里出现过的序列，滚动区等出现时再加。状态只看渲染后最下面四行非空行，按菜单、运行中、空闲、未知的顺序判定，对话里引用的这些字眼不算数。
- **启动与投递**。页脚空闲后整段粘贴，粘贴前清理控制字符，以 `/`、`!`、`#`、`&` 开头时加防护；输入框（最后两条分隔线之间）出现任务开头或折叠标记后回车，3 秒未见投递再补一次回车。投递只认 transcript 里含本次任务开头的用户行，`SessionStart` 只用来取会话 ID。启动时页脚出现菜单（登录、信任等）或 60 秒未就绪报 BLOCKED，不按键。
- **运行与终态**。运行中不按键。本会话的 `Stop` 且 `fullyIdle` 为真时回合结束，从 `transcript_full.jsonl` 取本次用户行之后最后一段回复作为结果，`modelName` 作为回执模型；`fullyIdle` 为假时继续等下一次 `Stop`，不看页脚。以下情形报 BLOCKED 并附画面摘录：页脚菜单停留超过 2 秒；回复已写出、页脚空闲 15 秒仍没有 `Stop`（保留回复，提示检查 hooks.json 的条目）；还没有 `Stop` 时画面空闲或无法识别、且画面和 transcript 60 秒没有变化。运行中的页脚不设时限。`Stop` 带 `error` 或没有回复按失败处理；进程意外退出按失败处理。结束时先发 SIGTERM，留 5 秒让 agy 写完会话，再交给原有清理。

## 取舍

- 写入用户与 Antigravity App／IDE 共用的全局 `hooks.json`。条目在 AgentNave 之外不起作用，但用户自己的每个回合会多起一个 `sh`（毫秒级）。用户在 2026-11-09 选定这一做法，放弃了零写入但会把命令跑错目录的 `--add-dir` 方案，也放弃了只靠读屏判断回合结束。
- 只支持权限放开：运行中出现权限框没人能答，按 BLOCKED 处理。
- 交互会话与用户自己的会话一样进 agy 的输入历史；读屏规则跟随 agy 版本变化，比 stream-json 脆弱，失败一律报 BLOCKED，不退回 `-p`，用户可以在 App 关掉开关。
- 运行中只按 transcript 推进活动，不逐字流式；工具开始与结束按 transcript 的步骤顺序对应。
- 没有 `Stop` 时宁可报 BLOCKED 也不报成功：画面看不出后台命令，空闲页脚不能证明回合真正结束；代价是 hook 出问题时已经写出的回复也要由 boss 判断是否可用。
- 2026-11-09 开会讨论（Codex、Gemini、Grok 三席两轮）。三席一致保留门控 hook、改为锚定页脚读屏、不引入 pyte；后两项分歧（没有 `Stop` 时成功还是受阻、`fullyIdle` 为假后怎么收尾）按随后的实测定为上面的规则。

## 尚未真实验收

Linux；更大或更小的终端尺寸下 agy 是否改用滚动区；首次使用、未登录或有更新提示时的启动画面；额度、登录报错是否发 `Stop`。出现时按上面的规则等待或报 BLOCKED。
