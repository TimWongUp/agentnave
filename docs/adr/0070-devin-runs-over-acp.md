# ADR-0070：Devin 改用 ACP 运行

- 状态：已接受
- 日期：2026-11-09
- 扩展 [ADR-0057](0057-boss-answers-native-prompts.md)：Devin 运行中的权限请求同样交给 boss 代答。

## 背景

Devin Adapter 用 `devin -p --prompt-file … --export <文件>` 运行。`-p` 没有流式输出，stdout 只在结束时打印最终文本，运行期间 AgentNave 拿不到任何事件，托盘全程不显示状态，回复也不会边写边出。`-p` 下需要确认的工具调用直接被拒，任务报受阻。

`devin acp` 是 Devin 自带的 ACP（Agent Client Protocol）stdio 服务端。按 `devin --help` 的说明，`-p` 本身也是在 `devin acp` 子进程里运行 agent。实测（Devin 3000.12.32，免费模型 `swe-2-flash-medium`）：

- 握手、建会话、发 prompt 都正常；关掉 stdin 后进程自行退出，返回 0。
- 运行中推送思考片段、工具调用（带 `kind` 和内部工具名）、工具状态和逐段回复；回合结束返回 `stopReason`。
- `session/load` 续接后上下文保留；载入时会先把整段历史当作 update 推一遍。
- 权限请求以 `session/request_permission` 发给客户端，选项有允许一次、本会话允许、永久允许、切到放开模式、拒绝。
- 模式只有 `accept-edits`、`smart`、`ask`、`plan`、`bypass` 五种，没有 `-p` 的 `auto`。
- `--model` 只对新会话生效，名字无效时不报错，悄悄改用默认模型；续接时也会被忽略。`session/set_config_option` 只接受本账号目录里的模型，无效值明确报错。
- `devin acp` 没有沙箱参数，但会读取 `DEVIN_SANDBOX`：设为 `true` 后，往工作区外写文件被拦截。
- ACP 运行不做工作区信任检查。

## 决定

- **运行方式**。Devin Adapter 的 Provider 进程换成服务自带的 helper，与 ZCode 的 app-server 桥同一做法：helper 在同一 supervisor 进程组里启动 `devin acp`，完成握手，新建或载入会话，按选项设定模式和模型，发出一次 prompt，把协议事件转成 JSONL 写到 stdout，回合结束即关闭连接退出。一次调用一个进程，不保活。核心看到的仍是一个 Provider 进程和一串事件。
- **续接**。`session_id` 存在时用 `session/load`。载入响应返回前推来的 update 都是历史回放，一律丢弃，只统计本次 prompt 之后的事件。
- **模型**。调用方给了 `model` 时，会话建立后若当前模型不同，用 `session/set_config_option` 切换，并核对返回的当前值。Devin 拒绝或核对不一致，调用失败并列出 Devin 当前可用的模型，不悄悄改用别的模型。不传 `--model`。回执里的模型取切换后的当前值。
- **权限模式**。`permission_mode` 接受 ACP 的五种模式，另把 `dangerous` 作为 `bypass` 的别名，App「完全放开」档仍写 `dangerous`，「自动」档仍写 `smart`。`-p` 才有的 `normal`、`auto`、`yolo`、`autonomous` 不再接受，因为 ACP 没有对应模式，按名字猜测映射会改变权限含义。未传时沿用 Devin 自己的默认模式。
- **沙箱**。`sandbox=true` 时，helper 只给 `devin acp` 子进程设置 `DEVIN_SANDBOX=true`；这是显式选项的协议翻译，不写用户配置。
- **工作区信任**。`respect_workspace_trust` 移除：ACP 运行不检查信任，这个选项没有可控制的对象。
- **权限请求交给 boss**。`session/request_permission` 转成 ADR-0057 的待答弹框：工具名取内部工具名或 `kind`，输入取该次工具调用的原始参数，打码后交出。boss 允许时回「允许一次」，拒绝时回「拒绝」，选项缺失时回「已取消」。永不回本会话允许、永久允许或切到放开模式。回合结束仍未答的弹框按关闭处理。Devin 发来的其他请求一律以「方法不存在」拒绝，helper 握手时声明不提供文件和终端能力。
- **事件与回答**。思考片段只标记「思考中」，正文不外传；工具状态按调用编号上报，工具名不取 title，因为 title 里带命令内容。最终回答取最后一个工具调用之后的回复；之后没有文字时，取最后一段非空回复。`end_turn` 为成功，`refusal` 为受阻，`cancelled` 为取消，其余停止原因按失败处理；认证类错误按受阻处理。

## 取舍

- 依赖 ACP 协议和 Devin 的私有扩展通知；后者只被忽略，不被依赖。协议升级到新版本时，握手核对版本号，不一致直接失败，不按旧格式猜测。
- 原生 ATIF 导出不再用于派发结果，读取老会话的 ATIF 与会话库逻辑不变。
- 只接受 ACP 的模式名，显式传 `-p` 专有模式的调用方会收到错误。App 两档写的值都保留，现有用户不受影响。
- 「自动」档下 `smart` 判断不了的操作，从直接被拒变成等 boss 代答；boss 长时间不 `wait` 时任务会停在弹框上，与 Claude 交互模式相同。
- Windows 未实测。

## 考虑过的方案

- **继续用 `-p`，读会话库轮询进度**：要依赖 Devin 私有的数据库结构，轮询延迟大，也拿不到权限请求。
- **用 `--model` 选模型**：名字无效或续接时会悄悄换模型，违背「不替换为别的模型」的边界。
