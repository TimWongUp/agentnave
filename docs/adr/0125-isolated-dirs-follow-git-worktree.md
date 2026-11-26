# ADR-0125：建隔离目录和 git worktree add 一样执行仓库的 hooks、filter 与 LFS

- 状态：已接受
- 日期：2026-11-26
- 修订 [ADR-0085](0085-service-managed-isolated-dirs.md)「环境」一条中跳过 git hooks 与 LFS 下载、拒绝有子模块或自定义 filter 的仓库的规定，以及 [ADR-0092](0092-auto-release-untouched-isolated-dirs.md) 沿用的「不执行 hooks」边界。
- 不改变 [ADR-0085](0085-service-managed-isolated-dirs.md) 的「服务不执行 setup 脚本」，也不改变 [ADR-0098](0098-tray-deletes-isolated-dirs.md) 检查与删除时不执行 hooks、filter 与文件监视程序的边界。

## 背景

服务建隔离目录时绕开了仓库配置的 hooks，跳过 LFS 下载，并拒绝有子模块或自定义 filter 的仓库。Claude Code、T3 Code 等宿主建 worktree 时直接执行 `git worktree add`，这三样都照 git 的默认行为处理。结果是同一个仓库里，宿主建的 worktree 有 `post-checkout` 钩子准备好的环境（例如 CodeGraph 索引）和完整的 LFS 文件，AgentNave 的隔离目录却没有；有子模块或自定义 filter 的仓库更是无法隔离。

## 决定

- **建目录跟 git 一致**：新建与续接时原位重建隔离目录，都和 `git worktree add` 一样执行仓库配置的 hooks、filter 与 LFS 下载；有子模块的仓库照常建，子模块和 git 默认一样不初始化。这是用户的决定。
- **检查与删除不变**：判断能否释放、自动释放与托盘删除时，仍不执行仓库的 hooks、filter 与文件监视程序，不进入嵌套仓库。`git worktree remove` 本来也不执行 hooks。

## 取舍

- **服务会执行仓库里的代码。** 此前服务建目录时不跑任何仓库代码。现在仓库的钩子和 filter 会在服务进程下运行，与用户自己或宿主执行 `git worktree add` 时一样；AgentNave 本来就不是沙箱，Provider 原生权限才是安全边界。
- **建目录可能变慢或失败。** 钩子和 LFS 下载要花时间，LFS 需要联网；钩子失败或下载失败按建目录失败处理，回滚半成品。
- **检查仍不跑 filter，靠建目录时记下的文件状态判断干净。** 检查沿用不执行 filter 的边界，经 filter 改写过内容的文件（如 LFS 下载的文件）只能靠 git 索引里的文件时间戳判断没被动过；所以仓库配了 filter 时，建目录多等到下一秒再刷新一次索引，约慢 1 秒，否则这类文件会被误判为有改动、失去自动释放。这些文件之后被改动过或时间戳变了，就判为有改动，多一次确认，不会误删。LFS 尚未用 git-lfs 实测。
- **钩子写出的未忽略文件会挡住自动释放。** 钩子在目录里产生的、未被忽略规则覆盖的文件算作改动，这类目录只能确认后释放，不会自动释放。
