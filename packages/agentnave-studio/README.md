# AgentNave Studio

AgentNave Studio is the AgentNave MCP server plus saved history, optional answer review and a
local read-only dashboard, for multi-CLI shows that a human watches. It includes every core
AgentNave tool; use the plain `agentnave` package when an agent only needs subagents.

Entry point: `agentnave-studio-mcp`. History lives in `~/.agentnave` (override with
`AGENTNAVE_DATA_DIR`). The dashboard starts only when the manager calls `open_dashboard`.

See the [repository](https://github.com/TimWongUp/agentnave) for installation and the
show guide.
