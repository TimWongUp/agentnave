# ADR-0119：撤掉 Devin CLI 支持

- 状态：已接受
- 日期：2026-11-24
- 撤销：[ADR-0070](0070-devin-runs-over-acp.md) 整篇（Devin 的 ACP 运行方式随 Provider 一起移除）。
- 沿用：其他 ADR 中对 Devin 的提及只是当时的对照或接入范围记录，正文不改。

## 背景

Devin CLI 曾同时接入为组员和宿主（boss），派发走 ADR-0070 的 ACP helper。

## 决定

- **移除 Provider 与宿主**。这是用户的决定。服务不再提供 `devin` Provider：Adapter、ACP helper、Provider 枚举、选项说明与相关测试一并删除。托盘不再检测、接入或显示 Devin，员工、boss、用量、图标中都没有它。Skill 与文档删去 Devin 的参考与说明。
- **旧数据照常可读**。历史里 Devin 做过的私聊任务、会议席位和条目照常加载和显示，CLI 名称原样显示，没有专属图标。这些线程不能续接，用它们的 `session_id` 续接或指定 `provider="devin"` 时，调用以「CLI 不受支持」报错。
- **旧配置不报错**。员工、工位、默认值和权限档位文件里遗留的 Devin 条目在读取时忽略，不阻止 App 或服务启动。工位的候选里只有 Devin 时，按工位不可用处理。
- **清理自己写过的接入**。AgentNave 曾写进 Devin 配置的接入内容（MCP 条目、规则块、Skill），App 启动时删除一次，卸载清理同样覆盖；只删 AgentNave 自己写的部分，Devin 的其他配置不动。

## 取舍

- 已经在用 Devin 的用户升级后会失去它，包括作为 boss 的接入；之前的历史只能查看，不能接着用。
- 保留一小段只用于清理旧接入的路径信息，换来升级后 Devin 配置里不留下失效的 AgentNave 条目。
