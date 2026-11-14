# Grok CLI

通常省略 `model`、`effort`，由服务按 App 里的组员设置传入；用户点名模型或档位时才传，并按 `list_members` 核对，超出模式范围会被拒绝。

`provider: "grok"`；用户点名时的写法示例：

```json
{"model": "grok-4.7", "effort": "high"}
```
