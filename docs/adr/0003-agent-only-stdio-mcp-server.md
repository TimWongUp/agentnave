# ADR-0003：只提供面向 Agent 的 STDIO MCP 接口

- 状态：已接受
- 日期：2026-08-30

AgentNave 的唯一用户是上层 Agent Manager。公开接口因此只保留 STDIO MCP 的 `start_agent`、`wait_agent` 和 `cancel_agent`，不再维护面向人的前台 `run` 命令；`agentnave-mcp` 仅是 MCP Host 启动 server 进程的入口。

同时维护 CLI 与 MCP 会形成两套生命周期和错误合同，还会让产品被误解为人用 CLI。失去前台单次调用的便利是可接受代价，开发和诊断通过官方 MCP Client、Inspector 与自动化测试完成。ADR-0002 的轻量适配器和 Manager 所有权边界继续有效，仅其中的双公开接口决定被本 ADR 取代。
