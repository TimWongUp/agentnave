# ADR-0117：普通私聊派发不必先读 Skill，用法放进工具描述与回执

- 状态：已接受
- 日期：2026-11-23
- 修订 [ADR-0068](0068-rules-hold-authorization-skill-holds-method.md) 的「归属」：原来 MCP 说明要求派发或组织协作之前先加载 `agentnave-manager`，现在只在少见场景才要求加载；规则块只放授权这一条不变。
- 沿用 [ADR-0028](0028-follow-cli-default-model.md)、[ADR-0032](0032-cli-permission-tiers.md)：不新增默认值和权限参数；[ADR-0043](0043-service-prompt-instructions.md)：给组员的非交互说明仍由服务附加；[ADR-0074](0074-app-seat-model-source-and-claude-family-aliases.md)：Claude 同系列映射的规则不变，只是把它写进回执；[ADR-0056](0056-claude-interactive-runtime.md) 的不保活和 [ADR-0114](0114-tui-runtime-like-a-user.md) 的宿主自唤醒也不变。

## 背景

2026-11-23 做了一次对比：boss 都是 Claude Sonnet 5.5，组员是 Codex 和 Grok，题目一样，包含并行查询、续接和隔离提交。

- T3 Code 在线程里用 `delegate_task`，61 秒完成，第 16 秒就派出了组员。它自己启动 Agent 会话，往系统提示里注入约 3KB 的编排说明。
- AgentNave 用了约 90 秒，约第 25 秒才派出组员。boss 先读了 `SKILL.md` 和三份参考文件，共约 30KB。

AgentNave 不拥有 boss 会话，没法往宿主的系统提示里写东西，所以说明只能通过 MCP 说明、工具描述、回执、规则块和 Skill 这几条路给出。实测 Claude Code 2.1.330 的情况是：MCP 说明在第 4096 字符截断；工具描述也是 4096，经 ToolSearch 加载后放宽到 16384。T3 源码里另记录了 Codex 会截断超过 4000 字节的条目。

## 决定

1. **何时加载 Skill**。普通私聊（派一个或几个任务、续接、收结果、验收）按工具描述和回执 `next` 来做，不必先读 Skill。只有以下场景才加载 `agentnave-manager`：开会、评审、转交、多个可写任务并行、流水线类方法、回执报错或返回 `not_started`。这条同时写进 MCP 说明、规则块和 Skill 触发描述，三处不能互相矛盾。
2. **MCP 说明只写授权与身份**：授权从哪来、工具可用不等于有编排授权、本聊天的 `manager_session_id`、不得自行追加权限参数，以及何时加载 Skill。展开各宿主提示后总长不超过 3,500 UTF-8 字节，用单测守住。
3. **私聊用法写进工具描述**，关键约束放在首句和对应字段里。每个工具的顶层描述不超过 2,000 字符，具体是：
   - 首派带 `title`，prompt 写成自足的简报；
   - 用户没点名就不传模型、档位和权限参数，被拒时按报错列出的可选项改；
   - 调查任务声明只读，续接时重新传 `access`；
   - 写任务用隔离目录，验收后释放；
   - `succeeded` 不等于完成。
4. **随 boss 宿主变化的用法放进回执**：
   - Codex 外部终端的 `@exec` 让出提示，写进 `wait` 的 `next`；
   - `native_seat` 的自带子代理调用方式和模型映射（Claude 同系列别名、Codex `spawn_agent` 参数、agy `inherit` 边界），由服务入口按 boss 宿主查表，写进 `not_started` 回执。

   参考文件里的同一段删掉，只留回执这一处。这里是按调用方宿主分支，不是按组员 Provider 分支，不违反「核心不按 Provider 名分支」。
5. **不做的事**：
   - 续接不复用常驻会话，沿用 ADR-0056 的不保活；
   - 完成后不推送唤醒 boss，沿用 ADR-0114；
   - 续接冷启动的耗时另行调查。

## 取舍

- 首派前少读约 30KB，代价是三处说明要同版发布。版本错配（新规则块配旧服务，或反过来）时会退回先读 Skill，慢一些，但不出错。
- boss 少读了 Skill 里的简报模板、细度和流水线方法。普通私聊靠工具描述里的底线兜住，复杂场景仍按第 1 条加载 Skill。
- 回执按 boss 宿主生成提示，服务要多知道一点宿主差异。宿主的子代理语义变了要跟着改，这和以前写在 Skill 里的维护成本一样。
- 没有照搬 T3 的系统提示注入，因为 AgentNave 不拥有 boss 会话。
