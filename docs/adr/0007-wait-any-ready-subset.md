# ADR-0007：`wait_any` 返回就绪子集并只消费回复中的阻塞

- 状态：已接受
- 日期：2026-10-15
- `wait_any`、`wait_agent` 与 `wait_round` 由 [ADR-0024](0024-consolidated-tools-and-private-review.md) 合并为 `wait`，就绪子集语义不变。

## 背景

Manager 常并行启动多个 Invocation。只有 `wait_agent` 时，它要么轮流等待各个 ID，先返回的那个会被前面的五分钟窗口延后发现；要么由宿主并发发起多次等待，而不少宿主不支持并发工具调用。在借鉴 herdr（终端多路复用器）的多 CLI 评审中，各方一致同意新增一个多 ID 等待工具，但不改动 `InvocationResult` 与 `wait_agent` 的稳定合同。

阻塞提醒是每个 Invocation 内的队列，读取即消费。若在服务端对多个单 ID 等待做 `asyncio.wait` 再丢弃输家，同时就绪的输家已经弹出的阻塞会丢失，Manager 再也看不到它。

## 决定

- 新增 `wait_any(invocation_ids)`，`wait_agent` 不变。名单非空、不重复、全部已知，否则在等待前整批报错；窗口沿用固定五分钟。
- 返回 `reason=ready` 时，`updates` 是此刻全部就绪项（已终态或有未读阻塞）的现有 Invocation 回复形状，而不只是第一个。聚合在 InvocationManager 内实现：唤醒后不挂起地扫描整份名单，只弹出写进回复的阻塞，每项最多一条；终态优先于阻塞，且可重复读取。
- 窗口到期返回 `reason=wait_elapsed` 与每项运行快照，不弹出任何阻塞。
- 同一 Invocation 的多个等待者沿用现有消费语义，先取先得，不承诺广播。
- 同时新增只读 `list_invocations`，只给句柄元数据，供 Manager 找回句柄或核对在途调用；正文仍只通过等待工具读取。

## 后果

- 一次调用即可得到全部已就绪项，不会因只报第一个而让其他已结束的结果多等一个窗口；未进入回复的阻塞不会丢失。
- 已终态 ID 每次都就绪，Manager 必须在处理后把它移出名单，否则下一次会立即返回。
- 不提供订阅、广播或推送；Manager 仍以请求／响应方式循环等待。
