# ADR-0076：权限档位缺省为完全放开

- 状态：已接受
- 日期：2026-11-10
- 修订 [ADR-0032](0032-cli-permission-tiers.md) 的缺省档位。

## 决定

`permissions.json` 或 CLI 条目缺失时使用「完全放开」；用户显式保存的「自动」保持不变。用户仍可在 App 选择「自动」。Claude 交互模式的 bypass 警告通过本次启动的 `--settings` 消除，设置与 hooks 合并，不写用户配置。

## 取舍

未保存档位的现有用户也会采用更宽的默认权限；调用回执继续注明套用的参数。权限仍由 Provider 原生机制执行，AgentNave 不是沙箱。
