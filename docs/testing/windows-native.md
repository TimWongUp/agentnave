# Windows 本机验证

在 `apps/tray` 下运行默认质量门：

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

默认测试使用临时目录，覆盖真实 CodeBuddy/Pi shim 内容、旧启动器仍存在时的三个宿主配置迁移，以及 PowerShell 独占句柄释放后同一次原子写入的重试。不会修改用户配置、切换服务版本或启动 Provider 会话。

安装了全部十个宿主且活动服务与当前版本相同的验证机，可显式启用本机检查：

```powershell
cargo clippy --locked --all-targets --features windows-live-tests -- -D warnings
cargo test --locked --features windows-live-tests windows_live_ -- --nocapture --test-threads=1
```

若任务环境裁剪过 PATH，先补回本机持久化配置，避免把已安装但不在任务 PATH 上的 CLI 误判为 `orphaned`：

```powershell
$env:PATH = [Environment]::ExpandEnvironmentVariables(
    [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
    [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + $env:PATH)
```

该 feature 不用于普通 CI，也不使用 `#[ignore]`：

| 测试 | 读取或修改范围 | 验收条件 |
| --- | --- | --- |
| `windows_live_snapshot` | 只读真实宿主配置、reload 记录、进程及连接回执 | 服务版本匹配、十个宿主均无 `required`；真实待重载状态也会使验收失败，需依据输出判断原因 |
| `windows_live_shims` | 读取 `%LOCALAPPDATA%/Programs/nodejs/codebuddy.cmd`、`~/.pi/agent/bin/pi.cmd` 和进程快照 | 两个 shim 均解析为存在的实际脚本 |
| `windows_live_active_lock` | PowerShell 独占真实 `service/active.json`，仅写回原 manifest | 持续占用时可控失败且旧指针不变；重试窗口内释放后同一次写入成功，无暂存文件遗留 |

应用自己的 `hosts.json` 位于 AppData，不应假定它在服务历史数据根 `~/.agentnave`。`reload_*` 的 `v` 是记录格式版本，`token` 是 MCP 配置摘要；二者都不是服务发行版本。判定需同时结合活动 launcher、连接版本与接口摘要、配置变更时间以及宿主启动时间，不能仅比较字符串是否等于当前发行版本。

原来的 `windows_live_refresh` 和 `windows_live_upgrade` 会改写真实配置，后者还会切换活动服务，可能中断执行测试的宿主。它们已由默认运行的临时目录迁移测试替代；端到端升级仍须在允许宿主重连的独立会话中通过 App 验证，并记录升级前后的实际配置。

设置页版本和接入页徽标必须另做界面目测。后端快照通过不等于 UI 已验收；无法获得界面访问时应记录阻碍，不用代码推断替代截图。

## 宿主需要应用内确认信任

部分宿主（目前确认：WorkBuddy 桌面版）把 MCP 服务器置于应用内的信任确认之后：`mcp.json` 写入成功不等于服务会启动，首次必须由用户在该应用自己的界面里点「信任 / 允许 agentnave」。

表现为接入页显示「待确认生效」且 `reload.confirmed` 一直为空、没有连接回执，此时 App 无法区分「等待重启」与「等待授权」。接入页对该情况会额外提示需要在宿主应用内确认信任。

App 无法代点这个确认框：这是宿主的权限边界。排查时先确认 `agentnave-mcp` 进程是否由该宿主拉起（任务管理器中同一时间应有两个 MCP 进程：宿主一个、其它宿主各一个）。
