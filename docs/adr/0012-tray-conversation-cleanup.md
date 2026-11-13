# ADR-0012：托盘在原生会话锁下清理记录

- 状态：部分取代
- 日期：2026-10-16
- 按天数的自动清理归属由 [ADR-0046](0046-service-history-retention.md) 取代：由服务执行，托盘保留手动清理。
- 托盘写入范围由 [ADR-0024](0024-consolidated-tools-and-private-review.md) 修订：另可在同一把锁下改名。
- 托盘写入范围由 [ADR-0088](0088-relay-records-service-exit-fault.md) 扩展：另可删除用户已确认的连接故障记录（`connection-faults/`，不取会话锁）。
- 锁位置与删除对象由 [ADR-0026](0026-sqlite-conversation-store.md) 取代：使用 `locks/<会话ID>.lock`，级联删除数据库会话行。
- 独立的管理页面已并入办公室：右键工作区或会议室改名、删除，可多选；路由与锁不变。
- `profiles.json` 增补由 [ADR-0013](0013-members-defaults-and-host-identity.md) 取代
- 托盘配置写入对象由 [ADR-0014](0014-tray-members-and-one-click-update.md) 修订
- 保留只读浏览器看板的条款由 [ADR-0045](0045-remove-browser-dashboard.md) 取代

托盘需在没有工作室服务运行、Python 未安装或版本不同的机器上管理历史，因此由 Rust 原生删除，不调用 Python 或 MCP。此决定修订 [ADR-0008](0008-tray-app-reads-studio-snapshots.md) 中托盘“不写数据根”的边界：托盘可删除会话的 `state.json` 与 `views.json`，保留会话目录、`.lock` 和全部席位文件；浏览器看板仍只读。

托盘与工作室必须锁同一个 `.lock` 文件，使用 `File::try_lock` 并对探针冲突短暂重试；锁内重读状态。未结束但空闲的会话可直接删除，有待审回答拒绝；拿到锁后仍标 running 的记录属于崩溃残留，允许清理。删除和其他宿主的续接由同一锁串行化，工作室发现状态缺失后拒绝写入，不从缓存恢复。保留锁文件避免删除再创建导致不同 inode 同时被锁。Windows 的 `LockFileEx` 与 `msvcrt.locking` 第 0 字节重叠区间预期互斥，跨语言互斥仍须 Windows 实机验证。

会话管理使用托盘专属页面与只接受带操作请求头的 POST 路由，先在页面内确认，再逐条显示结果；不把写入加入共享看板前端，不开放 IPC，不放宽 CSP。与工作室 MCP 的“先结束再删除”相比，托盘承担用户主动清理空闲历史的操作；删除范围更窄，不处理角色定义、空席位目录或服务的调用句柄。先删派生视图再删权威状态；文件错误可能留下部分结果，保留的状态允许重试。

修订（2026-10-16）：托盘写入边界自然扩展到用户维护的编排档案 `profiles.json`，通过同样带操作请求头的专属 POST 路由校验结构并原子替换；核心与工作室只读，主控按档案显式传参，权限参数仍须用户本轮明确要求。详见架构文档。
