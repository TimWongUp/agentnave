# AgentNave

AgentNave is a local STDIO MCP server for agents that need to launch Antigravity CLI, Claude Code,
CodeBuddy Code, or Grok CLI as subagents. It deliberately leaves planning, parallelism, review,
synthesis, retries, permissions, and worktree management to the calling Agent Manager.

AgentNave has no human-facing CLI. The `agentnave-mcp` command only starts the MCP process for a
compatible host.

## Requirements

- macOS or Linux
- `uv`
- At least one authenticated provider CLI

`uv` installs AgentNave in an isolated Python 3.12 environment. A separately managed system
Python is not required.

AgentNave 0.2 does not claim Windows process-tree supervision. Native Windows support requires Job
Object ownership first.

## Install

Install a tagged release with `uv tool` so the MCP launcher does not depend on a source checkout:

```bash
uv tool install --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@v0.2.0"
```

The release tag is part of the install source. Do not replace it with the mutable `main` branch.
`uv` owns the isolated runtime, launcher, upgrades, and removal.
It does not modify host Skills, global instructions, permissions, or provider configuration.

## Connect an MCP host

Register the installed launcher as a local STDIO server. Codex provides an official MCP management
command, so no configuration file needs to be edited by hand:

```bash
AGENTNAVE_MCP="$(uv tool dir --bin)/agentnave-mcp"
test -x "$AGENTNAVE_MCP"
codex mcp add agentnave -- "$AGENTNAVE_MCP"
codex mcp get agentnave
```

Restart Codex after registration. A new session should expose `start_agent`, `wait_agent`, and
`cancel_agent`. Other compatible hosts should likewise point their STDIO MCP registration at the
absolute launcher reported by `uv tool dir --bin`; their activation commands and scopes are
host-specific.

AgentNave creates no durable user data. Provider authentication and configuration remain owned by
their respective CLIs.

## Upgrade and uninstall

Replace `vNEXT` with a published release tag, install it into the same `uv`-owned tool environment,
then restart the MCP host:

```bash
uv tool install --force --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@vNEXT"
codex mcp get agentnave
```

To remove AgentNave from Codex, remove the host registration before uninstalling the runtime:

```bash
codex mcp remove agentnave
uv tool uninstall agentnave
```

Uninstalling AgentNave does not remove or sign out any provider CLI.

## MCP tools

### `start_agent`

Starts one provider invocation and immediately returns an in-memory `invocation_id`. Required
arguments are `provider`, `prompt`, and an absolute existing `cwd`. Optional arguments are
`session_id`, `timeout_seconds`, and explicit `provider_options`.

Supported providers are `antigravity`, `claude`, `codebuddy`, and `grok`. Provider-native settings
are inherited unless the Manager explicitly supplies allowlisted options.

### `wait_agent`

Waits for at most `wait_timeout_seconds`. A `running` response leaves the invocation active and
includes a lifecycle snapshot. A `finished` response contains the normalized provider result.

### `cancel_agent`

Stops one invocation and returns its terminal result. Use it only when the Manager intends to stop
active provider work; `wait_agent` observes work without cancelling it.

All tools publish input and output JSON Schemas. Agent-correctable request errors are returned as MCP
Tool errors with retry guidance; provider launch and execution outcomes remain structured Invocation
Results.

## Lifecycle and security

Invocation handles exist only for the current MCP server process. When the server stops, AgentNave
makes a best-effort attempt to terminate processes that remain in the provider process group. Old
handles cannot be recovered after restart, although a provider `session_id` can be supplied to a new
`start_agent` call if the provider retained it.

A running snapshot reports the lifecycle phase, elapsed time, and age of the latest official provider
stream event. It does not claim semantic task progress. Terminal `output` contains the provider's
final response rather than streamed intermediate narration.

AgentNave is not a sandbox. A same-user provider with command permission can deliberately daemonize,
kill its supervisor, or otherwise escape ordinary POSIX process-group cleanup. Provider-native
permissions are the security boundary; use OS-level isolation when adversarial containment is
required.

## Verify

The commands below are for a source checkout used for development, not for the `uv tool`
installation above. See [CONTRIBUTING.md](CONTRIBUTING.md) for the complete contributor workflow.

```bash
uv sync --locked --all-groups
uv run ruff format --check .
uv run ruff check .
uv run pyright
uv run pytest
```

## Contributing and security

Contributions are welcome through GitHub Issues and pull requests. See [CONTRIBUTING.md](CONTRIBUTING.md)
for the development workflow and validation requirements.

Do not report security vulnerabilities in a public Issue. Follow [SECURITY.md](SECURITY.md) to use
the repository's private vulnerability reporting channel.

## License

AgentNave is licensed under the [MIT License](LICENSE).
