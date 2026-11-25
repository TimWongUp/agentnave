# ADR-0121：接入 T3 Code：T3 里由 T3 执行单独指派，AgentNave 只选工位、开会和转交

- 状态：已接受
- 日期：2026-11-24
- 2026-11-25 自身修订：T3 工位目录支持生图工位（第 3、4 节），原条款「没有生图工位，T3 里不派生图」作废。Codex 与 Antigravity 在 T3 子线程里实测能出图并存到指定路径；Antigravity 经自带的生图子代理出图，所以简报附言要放行它。生图候选不限定哪家 CLI，能不能出图由 T3 里的 CLI 决定。
- 2026-11-25 自身修订：T3 里的 Antigravity（`agy acp`）与 Codex 同样识别为主线程与子线程，后台等收件箱（第 1、6 节），第一版「只认 Claude 和 Codex」作废；它的选工位回执直接指向 T3 的 ACP 终端派发（第 4 节）。
- 2026-11-25 自身修订：T3 里的 CodeBuddy（ACP）与 Codex 同样识别为主线程与子线程，但它的后台命令结束后不会被唤醒，主线程仍用阻塞 `wait`（第 1、6 节）；Grok 不识别（第 1 节），T3 启动它的 `grok agent` 模式按非交互处理，不后台等（第 6 节）。
- 2026-11-25 自身修订：T3 页有自己的一组 boss 勾选，只列 T3 与 AgentNave 都支持的 CLI（第 2、5、8 节）。「T3 跟随该 boss 的编排档位」改为：档位决定写什么，勾选决定谁在 T3 里按这套走；T3 里认得出的四家照第 5 节写，认不出的 Grok、Pi 写另一节，派活按 T3 自己的规则；勾了但通用页关闭的 boss 写 T3 专用块。第一版「T3 范围跟着通用页勾选」作废。
- 2026-11-25 自身修订：取消首次预填（第 8 节），首次打开只有空白的通用探索、通用执行和生图工位；添加的备选不自动选模型。
- 修订（仅在 T3 主线程且用户开启 T3 编排时）：
  - [ADR-0037](0037-app-writes-orchestration-rules.md)、[ADR-0041](0041-orchestration-mode-per-boss.md)、[ADR-0068](0068-rules-hold-authorization-skill-holds-method.md)、[ADR-0074](0074-app-seat-model-source-and-claude-family-aliases.md)：T3 编排的对象由 T3 页单独勾选，写什么跟该 boss 的编排档位；开启时勾选的 boss 在分工协作、决策模型、自定义三档的规则块多一节 T3 派法，通用页关闭的 boss 写 T3 专用块（第 2、5 节）。
  - [ADR-0087](0087-decision-model-recommends-seats.md)、[ADR-0089](0089-seat-fallback-order.md)、[ADR-0093](0093-decision-seat-random-candidates.md)：T3 主线程用单独的工位目录；选工位时当场定好候选顺序，由 boss 交给 T3 执行（第 3、4 节）。
  - [ADR-0090](0090-quota-group-cooldown-and-host-cli-choice.md)、[ADR-0108](0108-multi-account-isolation.md)：T3 主线程不做宿主同类处理，账号由 T3 实例表示（第 3 节）。
  - [ADR-0030](0030-optional-review-gate-skill.md)、[ADR-0038](0038-role-applies-app-seat.md)：T3 主线程的评审工位读 T3 工位文件（第 3 节）。
  - [ADR-0114](0114-tui-runtime-like-a-user.md)：T3 主线程可用后台命令等会议和派发；派出的进程剥离 T3 标识（第 1、6 节）。
  - [ADR-0117](0117-private-dispatch-without-skill.md)：T3 主线程与子线程用各自的 MCP 说明（第 5 节）。
- 不改变：[ADR-0031](0031-prompt-only-orchestration.md) 的单向编排；[ADR-0032](0032-cli-permission-tiers.md) 不追加权限参数；[ADR-0091](0091-meetings-only-skill-and-auto-meeting-switch.md) 的 Skill 安装；[ADR-0099](0099-handover-links-workspaces.md) 的转交。

## 背景

用户在 T3 Code 里用 Claude、Codex 和 Antigravity 当 boss。T3 有自己的子代理、账号、隔离目录、等待和任务展示，AgentNave 有工位目录、决策模型选工位、会议和跨应用转交。两边都能派单独任务，规则互相冲突：T3 的说明要求同一家 CLI 用原生子代理，AgentNave 的规则要求经 AgentNave 派发。

T3 支持的提供方远多于 AgentNave（含 ACP Registry 里的 CodeBuddy 等），模型名、账号和档位也是它自己的一套，两边共用一份列表对不上。

2026-11-24 在 T3 0.1.11 上实测：

- T3 里的 Claude 总在首步之后才连上 AgentNave，MCP 说明赶不上首次派发；规则文件和会话启动时的内容赶得上。
- 用本篇第 5 节前身的规则句，Opus、Sonnet 和 Codex 的 gpt-6.2-astra、gpt-6-luna 共派 14 次，全部先选工位再用 `delegate_task`，没有改用原生子代理；Haiku 当 boss 时 3 次里 2 次绕过。
- 后台命令结束后 T3 会叫醒主线程，Claude 与 Codex 都是。
- 2026-11-25 在 T3 Code Beta、agy 1.5.16（macOS）上实测：T3 用 `agy acp --gemini-dir <profile> --app-data-dir antigravity-acp` 启动 agy，agy 及其拉起的服务进程环境里有 `T3CODE_THREAD_ID`（子线程以 `thread:delegated-task:` 开头）；agy 的 `run_command` 要填 `WaitMsBeforeAsync`，超时后命令转后台，命令结束时 agy 自己注入通知并开始新一轮，同一个 T3 run 里不需要 T3 叫醒。T3 给 Codex（`t3-code-direct`）和 Pi 直接提供 `orchestrator_capabilities`、`delegate_task`、`task_status`；agy 的工具列表里少了 `delegate_task`、`schedule_task`、`t3_thread_launch`、`html_render`，直接调用报 `not enabled`，按 T3 说明里的 `acp-mcp-call` 终端备用调用能派出，子任务完成后 T3 以新消息叫醒 agy。
- T3 只拒绝模型不识别的推理档；没有的推理档不传时，同实例同模型的子任务会沿用主线程的档位，明确传空列表才用默认。
- 2026-11-25 实测：Grok 在 T3 里用 T3 自带的 `orchestrator_capabilities`、`delegate_task` 派活完全正常，子任务完成后 T3 发消息唤醒它；它不能靠后台命令自己醒来。T3 的 Pi 同样直接提供这些工具，但没有验证过，按认不出处理。
- 2026-11-25 在 T3 Code Beta（macOS）上实测：T3 用 `node …/codebuddy-code --acp` 启动 CodeBuddy，它拉起的服务进程环境里有 `T3CODE_THREAD_ID`（子线程以 `thread:delegated-task:` 开头）；`Bash` 的 `run_in_background` 能把命令放后台，但主线程和子线程里命令结束后都不会唤醒 CodeBuddy 开新一轮。T3 用 `grok agent --always-approve stdio` 启动 Grok（grok 1.0.92），它和它拉起的服务进程里都没有 `T3CODE_THREAD_ID`；`run_terminal_command` 的 `block_until_ms=0` 能放后台，命令结束后也不唤醒。现有的 Grok 探测对这个进程树返回真，会让 T3 里的 Grok 收到后台等待说明却永远醒不过来。

## 决定

1. **识别 T3**。每个聊天有自己的服务进程，启动时判断一次：
   - Claude：宿主是 Claude，`CLAUDE_CODE_ENTRYPOINT=t3-code` 且 `T3CODE_THREAD_ID` 非空，并且祖先进程里没有第二个 `claude`（嵌套 `claude -p` 会继承这两个变量）。Windows 不查祖先，接受罕见误判。
   - Codex、Antigravity、CodeBuddy：宿主是 Codex、Antigravity 或 CodeBuddy 且 `T3CODE_THREAD_ID` 非空，不查祖先进程。Grok 不识别：它的进程（含它拉起的服务）都没有 `T3CODE_THREAD_ID`，分不清主线程和子线程。
   - 线程 ID 以 `thread:delegated-task:` 开头是 T3 派出的子线程（子线程里嵌套的 `claude` 也是），其余是主线程。
   - 服务启动的 Provider 进程与托盘终端剥离 `T3CODE_` 开头的变量，避免误判。`T3CODE_HOME` 由服务自己读取后再剥离。
   - 其他宿主不识别。
2. **T3 的写法跟随编排档位，对象单独勾选**。档位决定谁选工位、三个开关决定授权（[ADR-0122](0122-merge-split-and-parallel-presets.md)），T3 开关只决定在 T3 主线程里由谁执行：分工协作和自定义由 boss 选 T3 工位，决策模型由决策模型选；只开会档没有 T3 模式，在 T3 里和别处一样。不识别、开关没开、该 boss 没勾选、当前 boss 的档位没有 T3 模式或 T3 工位目录没有可用工位时，除第 6 节的后台等外一切照旧。T3 内置终端、桌面端、命令行、派生会话都按普通连接处理。子线程不受开关影响，总是只给子线程说明。
   - **勾选**：数据根 `t3-seats.json` 的可选字段 `bosses`，存 CLI 标识数组；没有这个字段时一直按通用页推导：模式不是关闭、且在支持列表里的 boss（同一 CLI 的多个账号只要有一个不是关闭就算勾选）。用户在 T3 页改过并保存后，只按存的写，两边各管各的，之后新接入的 boss 不自动勾选。
   - **支持列表**是静态表，T3 与 AgentNave 都支持的 CLI：`claude`（T3 的 `claudeAgent`）、`codex`、`antigravity`、`codebuddy`（`acpRegistry_codebuddy_code` 实例）、`grok`、`pi`；T3 的 cursor、opencode 等 AgentNave 没有对应 boss，Trae、WorkBuddy 等 T3 不支持的不出现在 T3 页，也不写任何 T3 内容。不用 T3 实际检测到的实例推导：缓存缺失或还没刷新时勾选会消失，已保存的选择也会跟着丢；T3 页本来就只在检测到 T3 时显示。
   - **识别与不识别**：第 1 节认得出的 `claude`、`codex`、`antigravity`、`codebuddy` 照第 5 节写；认不出的 `grok`、`pi` 写第 5 节的另一节，服务端不改变它们的行为。
   - **档位与勾选的组合**：勾选且档位是分工协作、决策模型或自定义，按档位写；勾选但该 boss 在通用页是关闭，写 T3 专用块（第 5 节），服务端按分工协作处理；勾选但档位是只开会，不加任何 T3 内容（只开会不通过 AgentNave 派单独任务，与 T3 派法冲突）；通用页开着、T3 没勾，在 T3 里就按通用规则走。
3. **T3 工位目录单独一份**，数据根文件 `t3-seats.json`，App 写、服务读：开关、同档候选随机开关、任务工位、评审工位。任务工位的专业、说明、读写、难度沿用自定义工位的含义，各自保存，不引用现有卡片。
   - 候选只存 T3 账号实例 ID、T3 模型名和推理档；每个工位 1–4 个候选（还没选模型的工位可以保存，但不进候选；没有可用工位时不能打开开关），账号就是实例，同一 CLI 的两个账号是两个候选。
   - 推理档只认 T3 标为推理的选项，以及 `effort`、`reasoningEffort`；不存权限、沙箱、模式类选项。没选推理档时明确传空列表。
   - 决策模型与密钥沿用现有设置，会议成员不变。
   - 不做宿主同类处理：工位的账号、模型和推理档是用户指定的设置，T3 的说明对这种情况用 `delegate_task`，选到 boss 自己的 CLI 或账号也一样。
   - 生图工位可按画风设多个，各有 1–4 个候选，存的仍是账号、对话模型和推理档；出图由该 CLI 用自带的生图工具或生图子代理完成。没有配好的生图工位时直接告诉用户，不拿其他可写工位顶替。
4. **选工位与执行分开**。
   - T3 主线程里 `recommend_seat` 接受一项或多项（各带摘要、读写，可指定工位或评审角色），当场返回每项排好序的 T3 候选与请求号，不启动任何东西。标准专业的工位也可用固定工位的名字指定（如 `worker-easy`）。分工协作和自定义档必须指定工位，没指定就报错并列出可选工位，不问决策模型、不需要密钥；决策模型档没指定才问决策模型。指定工位或评审角色时不问决策模型；同档候选随机打开时，同一批里优先避开已排首位的「实例＋模型」；关闭时按保存的顺位。生图工位始终按保存的顺位，也不占同批避重的名额；选中时回执附一段交给子线程的说明：可用自带的生图子代理，不用代码或别的办法顶替，成品存到简报给的路径。
   - 排除 T3 标为停用、未安装、出错或未登录的实例，以及按 T3 的判断、候选模型的额度窗口已满且未重置的候选（移植 T3 自己的判断函数，覆盖 T3 有额度数据的所有账号；只限某个模型或模型组的窗口只排除对应模型）。T3 缓存读不到或格式不认识时不排除，由 T3 判断。
   - T3 明确拒绝了实例、模型或档位且没有建任务时，boss 用同一请求号换同工位下一个候选，并在汇报里说明；已有任务号、超时、断线、权限被拒一律不换、不重派。候选全部不可用就直说，不退回 AgentNave 启动。被拒的推理档不悄悄去掉。
   - 三份列表各管一件事：`recommend_seat` 不带任务调用时返回 T3 工位与评审工位目录，按档位派发只看它；用户点名时只看 T3 自己的模型列表；`list_members` 只列会议成员，只在开会时用。
   - `start_agent`、`start_agents` 在 T3 主线程直接拒绝并指路。开会、收件、转交和历史工具不变。
   - T3 任务由 T3 展示，不进 AgentNave 历史、冷却和同目录提醒。只读靠回执附的只读说明写进简报，不是权限隔离。
   - T3 里的 Antigravity 拿不到可直接调用的 `delegate_task`，`recommend_seat` 回执在派法后加一句：不试 MCP 调用，直接用 T3 的 `acp-mcp-call` 终端备用调用派发。
   - T3 子线程里，选工位、派发、开会和转交工具都拒绝，说明它只完成自己的任务。
5. **规则与说明**。
   - 开关打开且档位有 T3 模式时，App 在勾选的 Claude、Codex、Antigravity 和 CodeBuddy 各自规则块里把派法分成两节：共用条目只写一份，后面依次是「不在 T3 Code 里时这样派」（普通派法、固定工位行和生图行）和「在 T3 Code 里这样派」（见下，第一条按档位取一种）。自定义档只在用户正文后追加 T3 一节，不改正文、不给正文加标题。开关关着或其他档位不加任何标题。按「在不在 T3 Code 里」分节，不按有没有 `delegate_task` 工具：T3 的系统说明从首步就写明运行环境，工具可能晚加载。首次派发只靠这一节。
   - 授权、小事边界、并行写入、开会、评审和验收都用该档正文，T3 只换派法。
   - 用户点名 CLI、T3 账号或模型时不选工位，按 T3 的模型列表直接派；只点名 CLI 时由 boss 选模型并说明，不保证是工位里存的那个。
   - MCP 说明在 T3 主线程和子线程各用一段专用文字放在最前面；普通连接的说明不变。
   - 弱模型（如 Haiku）当 boss 不保证先选工位。
   - 勾选了认不出的 CLI（Grok、Pi）时，规则块多一节，节首点名这些 CLI；分工协作、决策模型和自定义档同样把派法分成「不在 T3 Code 里时这样派」和这一节：

   > ### 在 T3 Code 里，Grok CLI 按 T3 自己的规则派
   >
   > - 在 T3 Code 里，Grok CLI 不用 AgentNave 的派法和工位，也不用本文件其他地方写的 `recommend_seat`、`start_agent`、`start_agents`；派活、等子任务、隔离和收结果全按 T3 自己的工具和回执（`orchestrator_capabilities`、`delegate_task`）。
   > - 开会仍用 AgentNave 的 `run_round`（T3 没有开会功能），用阻塞的 `wait` 一直等到会议结束并收会议结果。
   > - 自己是 T3 派来的子任务时，只完成自己的任务，不再派发、不开会。

   - 同一个规则文件被两类读者读到时（如 Grok 读 Claude 的 `CLAUDE.md`），第 5 节的 T3 派法在前、这一节在后，这一节点名的 CLI 不用前一节。文件里没勾选的读者也会读到这些节，与生图行同一个限制。
   - T3 专用块给勾选但通用页关闭的 boss：共用条目（授权与小事边界、评审与验收）、只在用户要求时并行写入和开会、一句「以上只在 T3 Code 里适用，不在 T3 Code 里只按用户当次明确的要求派发」，再接 T3 派法；认得出的 CLI 用「由 boss 选工位」那一种，没有「不在 T3 Code 里时这样派」一节，节标题也不写「不用上面的派法和工位」。认不出的 CLI 在它后面接这一节。

   T3 派法：

   > ### 在 T3 Code 里这样派，不用上面的派法和工位
   >
   > - 任何派出的任务（含只读调查和评审）先调 AgentNave `recommend_seat`【决策模型：「 由决策模型选工位」】【分工协作、自定义：「：不带任务先取 T3 工位，按任务类型和难度选一个填进 `seat`」】，再按回执用 `delegate_task`；工位是用户指定的设置，同 CLI 也这样派，不用 `start_agent`、`start_agents`。评审把角色名填进 `seat`，只把允许同时执行的独立任务一次交出。
   > - 选位工具没列出就先加载或稍等，仍不可用就告诉用户，不跳过选位，也不因此自己做完。
   > - 用户点名 CLI、T3 账号或模型时，按 T3 的模型列表直接派，说明用了哪个模型；用不了就告诉用户，不换别的。
   > - 开会仍用 AgentNave 并按会议回执收件，`list_members` 只用来选会议成员；其余等待、隔离和收件按 T3。

6. **后台等**。只要识别为 T3 主线程，不看 T3 编排是否打开，Claude、Codex 和 Antigravity 的会议和 AgentNave 派发都用后台命令等收件箱；Claude 与 Codex 在命令结束时由 T3 叫醒，Antigravity 用 `run_command` 把 `WaitMsBeforeAsync` 设成 500 让命令转后台，命令结束时 agy 自己唤醒，不调用 `t3_background_wait`；Antigravity 仍要求 `agy --version` 不低于 [ADR-0114](0114-tui-runtime-like-a-user.md) 的门槛，探测不通过时退回阻塞等待，探测只放行 T3 启动用的 `acp` 子命令；CodeBuddy 在 T3 主线程用阻塞 `wait`，因为它的后台命令结束后不会被唤醒（2026-11-25 实测）；`grok agent …` 模式按 [ADR-0114](0114-tui-runtime-like-a-user.md) 的非交互宿主处理，只能前台等，探测不放行，交互式 `grok` 不变；Windows 上命令的引号没有核实过能在 T3 用的终端里运行，等待不变；其他宿主在 T3 里没有识别，等待不变；提前返回（如卡住提醒）处理后要重新挂上。
7. **转交**。T3 内换账号、分叉、新线程用 T3 的；跨出或跨进 T3 用 AgentNave 转交；没说去哪就问。T3 里换账号后聊天 ID 会变，旧会议不能续开，第一版接受。
8. **App 读 T3 的模型列表**。App 读 `T3CODE_HOME`（缺省 `~/.t3`）下的实例缓存和隐藏模型设置，只取列模型需要的字段，不读也不展示邮箱、账号和凭据；T3 已移除的实例可能残留缓存，已保存的候选读不到时保留并标出。通用探索、通用执行和生图工位始终列出、不能删除，没填的不保存；首次打开全部留空，不从其他工位带入，专业工位和自定义工位由用户添加。添加的备选由用户自己选账号和模型，没选完不能保存。没检测到 T3 时不显示这一块。档位和三个开关在页面顶部，普通与 T3 共用；「通用 / T3 Code」只切换工位，当前档位没有 T3 模式时不显示。「写入的规则」显示实际写入的全文。T3 页顶部有自己的 boss 勾选，样式同通用页，只列支持列表里已接入的 CLI，没勾过时显示推导出的勾选；预览按所选档位和当前勾选，不展示 T3 专用块。

## 取舍

- **T3 跟随档位而不是另设一套**：两套档位写进同一个规则文件会有两套互相冲突的授权。代价是不能普通编排关着、只开 T3。
- **勾选独立而不是跟通用页**：T3 里的派法和普通连接是两件事，用户可能只想在 T3 里用某个 boss 派发，或相反。代价是多一份要维护的选择，以及通用页关闭的 boss 要有自己的 T3 专用块和一句限定授权范围的话。
- **静态支持列表而不是 T3 实例**：见第 2 节。代价是 T3 新增提供方时要手工补一行。
- **另立目录而不是共用一份**：两套模型名、账号和提供方对不上，共用要做账号对应、模型换算和退回启动。代价是用户维护两份工位。
- **不退回 AgentNave 启动**：状态不明时可能重复执行同一任务。代价是 T3 没有的 CLI 在 T3 里用不了。
- **规则分节而不是会话启动钩子**：两节之间真正不同的只有派法几条；Codex 的新钩子要用户先信任，没信任就一条规则都没有，T3 又让一个 Codex 进程服务多个线程，钩子认不出主线程和子线程；其他宿主仍要规则文件。代价是 boss 能看到另一节，弱模型会绕过。
- **一律 `delegate_task` 而不是同账号用原生子代理**：普通连接里同类 CLI 用原生子代理，T3 里不照搬；T3 的说明对用户指定的设置就用 `delegate_task`，只有一条路，任务也都能在线程里看到。代价是比原生子代理慢一点。
- **读 T3 缓存文件而不是调 T3 接口**：App 没有 T3 的连接，服务也不该依赖 App。代价是 T3 改格式会失效，失效时退化为不排除、由 T3 判断。
- **没有采用**：让用户在规则里写自然语言分工（没有决策模型、随机和额度排除）；决策模型只选难度、模型由 T3 决定（T3 没有按难度选模型的设置）；把 T3 工位表写进规则块（每段会话都读一遍，改工位就要重写规则文件）；点名 CLI 时按工位里的候选派（选位不能按 CLI 筛，点名的 CLI 可能没配工位）。
