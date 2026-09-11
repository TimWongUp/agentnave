# ADR-0004：Windows 使用 Job Object 监督 Provider 进程树

- 状态：已接受
- 日期：2026-09-11

## 背景

AgentNave 原先只在 macOS／Linux 上运行。POSIX supervisor 通过独占进程组，在 Provider
主进程结束后仍持有 PGID，并在完成、取消、输出超限或 MCP server 关闭时清理后代。
Windows 的 `CREATE_NEW_PROCESS_GROUP` 只影响控制台信号，不提供等价的进程树所有权；直接
采用它会破坏 AgentNave 对取消与清理的现有合同。

## 决定

Windows 为每次 Invocation 启动专用 supervisor。supervisor 创建配置了
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object，以挂起状态创建 Provider，将其成功加入
Job 后才恢复主线程。Provider 正常退出后，supervisor 关闭 Job 以清理仍存活的后代，再通过
有界临时状态文件回报 Provider 退出码。AgentNave 取消或 supervisor 异常退出时，Job 句柄关闭
并由系统终止 Job 内的进程树。

创建 Job、设置限制、创建 Provider、加入 Job 或恢复线程任一步失败时，Invocation 返回
`launch_error`；不回退为普通 Windows 子进程，也不用 `CREATE_NEW_PROCESS_GROUP` 冒充树级
所有权。Windows 没有与 POSIX SIGTERM 等价且可移植的温和树级信号，因此显式取消直接终止
supervisor 并触发 kill-on-close。

## 结果

- Windows、macOS 与 Linux 共享同一 MCP 和 Adapter 合同，平台差异收敛在进程监督层。
- Windows Provider 在执行任何用户代码前已经属于 Job，避免启动后再分配产生的抢跑窗口。
- Job Object 提供生命周期所有权，不提供用户隔离或安全沙箱；Provider 原生权限仍是安全边界。
- 状态文件只承载 supervisor 终态，不含 prompt、Provider 输出或会话内容，并在读取后删除。
