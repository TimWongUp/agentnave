# ADR-0098：托盘可在仓库锁与租约协议下手动删除隔离目录

- 状态：已接受
- 日期：2026-11-16
- 修订 [ADR-0085](0085-service-managed-isolated-dirs.md) 中「App 也不提供清理入口」一条；扩展 [ADR-0012](0012-tray-conversation-cleanup.md) 的托盘写入范围，沿用 [ADR-0085](0085-service-managed-isolated-dirs.md) 与 [ADR-0092](0092-auto-release-untouched-isolated-dirs.md) 的仓库级锁、租约、确认摘要与禁用 hooks/filter/fsmonitor 边界。

## 背景

[ADR-0085](0085-service-managed-isolated-dirs.md) 将隔离目录的释放完全交给 boss 调用 `isolated_dirs`，App 只读展示目录列表，理由是第一版不把清理入口放到人面前。实际使用中，遗留目录往往在对应的 boss 会话已经关闭或用户直接从托盘办公室检视项目时被发现；若必须再开一次 boss 对话才能清理，操作链路过长，且需要处理的失败残留（`deleting` / `needs_attention`）也难以直接在界面里收尾。

## 决定

- **App 原生执行手动删除**：托盘 App 在办公室工作区的「隔离目录」抽屉内为每个隔离目录提供手动删除入口，由 Rust 端直接操作 git worktree、受管分支与 `worktrees.db`，不经过 Python 服务或 MCP（与 [ADR-0012](0012-tray-conversation-cleanup.md) 保持一致，在无服务运行或 Python 版本不同时仍可清理）。
- **沿用服务的锁、租约与安全边界**：
  - 删除前后的检查与操作必须在仓库公共目录下的同一把 `agentnave/lock` 仓库级锁内串行执行，并核对 `agentnave/leases/<id>/` 无活跃租约；目录正在被任务使用（存在活跃租约或卡片处于使用中）时，删除按钮不可用，服务端亦拒绝删除。
  - 执行 Git 检查与清理时沿用服务的隔离约定：跳过仓库配置的 hooks、自定义或已配置的 filter 驱动及 `fsmonitor`，不进入嵌套仓库或符号链接/junction，状态流转（`ready` -> `deleting` -> 删除登记，或失败转 `needs_attention`）与服务一致。
  - `deleting` 与 `needs_attention` 状态的目录同样允许在 App 中再次删除，按服务的再次释放与回滚逻辑处理。
- **两步检查与带上下文的确认**：用户在界面点击删除时总是先展示确认框再执行删除，且全程不需要用户手填任何内容：
  - 第一步在仓库锁下检查目录并生成绑定当前状态的确认摘要存入 `worktrees.db`；确认框写明删除会永久丢弃目录内被 git 忽略的文件，并在存在未提交源码改动、未合入提交、嵌套仓库、已脱离 git worktree（orphan）或上次清理失败说明时列出具体情况。
  - 第二步携带确认摘要再次在仓库锁下核对租约与当前工作树摘要；若状态已变化则拒绝删除并要求重新确认。
- **范围与预览版约束**：App 只提供用户触发的手动删除，不做自动回收，也不创建或重建隔离目录；预览版（`AGENTNAVE_PREVIEW`）照常拒绝任何会修改正式数据根或用户仓库的写操作。

## 取舍

- **原生 Rust 实现而非调用 Python/MCP**：托盘不依赖本机服务进程或特定 Python 环境即可完成清理，代价是仓库锁、租约探针、Git 中和参数与摘要计算须在 Rust 侧与服务端保持严格一致。
- **所有目录删除前都弹出确认框，而非像 MCP `release` 对干净目录免确认**：MCP 调用方是刚完成任务的 boss，清楚忽略目录里是否还有临时产出；托盘面对的是可能已遗忘上下文的人类用户，删目录会一并丢弃被 git 忽略的文件，统一经过两步摘要确认既能明确告知风险，也能防止点击与确认之间目录状态发生变化。
