# AgentNave

<p align="center">
  <img src="docs/assets/agentnave-banner.png" alt="AgentNave — a thin local bridge to CLI subagents" width="100%">
</p>

**A thin local bridge from Agent Managers to CLI subagents.**

AgentNave lets an Agent Manager launch Antigravity CLI, Claude Code, CodeBuddy Code, Codex CLI, or
Grok CLI through three lifecycle tools and one read-only provider-description tool. Each provider keeps its native authentication,
configuration, permissions, and session model; AgentNave supplies the adapter and process
supervision around it.

The boundary is intentional. AgentNave does not plan tasks, assign roles, build DAGs, choose
parallelism, review results, synthesize answers, retry work, or manage worktrees. Those decisions
belong to the calling Agent Manager, where the full task context already exists.

AgentNave has no human-facing CLI. The `agentnave-mcp` command is only the STDIO entry point used by
a compatible MCP host.

## What AgentNave owns

| AgentNave | Calling Agent Manager |
| --- | --- |
| Provider command adapters | Planning and task decomposition |
| In-memory invocation lifecycle | Provider and model selection |
| POSIX process-group supervision | Parallelism, review, and synthesis |
| Normalized terminal results | Retries, permissions, and worktrees |

## Requirements

- macOS or Linux
- `uv` and Git (for installation from a release tag)
- At least one authenticated provider CLI

`uv` installs AgentNave in an isolated Python 3.12 environment. A separately managed system
Python is not required.

AgentNave supports POSIX process supervision on macOS and Linux. Native Windows support requires
Job Object ownership first.

## Install

Install the MCP runtime and companion [agentnave-manager Skill](skills/agentnave-manager/SKILL.md)
as a pair. The runtime exposes the tools; the Skill teaches the calling Agent how to use other
CLIs. Planning, scheduling, review, retry decisions, and synthesis remain with the calling Agent.

Follow the [installation guide](docs/installation.md) to:

1. Check the existing runtime's source, version and launcher; reuse it when suitable, or install
   the missing runtime with `uv tool`.
2. Inspect the target host's effective MCP registration; reuse matching settings and add only
   a missing entry, with the host's provider exclusions.
3. Reuse the matching Skill or install its complete directory, including `references/`, from
   the same release. Existing shared sources need only a discovery entry for the new host.
4. Verify both MCP tools and Skill discovery; restart connections or sessions after changes.

Hosts under the same OS user and `uv` tool directories share installation files, but each MCP
connection runs its own STDIO service process. Adding a host does not require reinstalling the
runtime or authorize upgrading it for other hosts. The guide covers differences and shared updates.

The guide covers host registration, provider paths, Skill installation, paired upgrades, rollback,
and removal. Provider CLIs must be installed and authenticated separately. `uv` manages only the
runtime; it does not install the Skill or modify provider permissions and configuration.

**The paired release is `v0.7.0`.** Install the runtime and complete Skill directory from that
tag. `uv tool` installs only the runtime; Skill discovery is a separate step. Hosts without
Skill support can still use MCP alone. The older `v0.5.0` tag contains only the runtime.

AgentNave creates no durable user data. Provider authentication and configuration remain owned by
their respective CLIs.

## The MCP surface

This section describes `v0.7.0`. Start, wait and cancel now use flat lifecycle responses;
callers upgrading from v0.6.0 must update their response handling and paired Skill.
Restart the MCP connection after updating the runtime to refresh its schemas.

AgentNave exposes four tools. The initial tool metadata contains a compact provider directory;
provider-specific options are returned only when requested. Model defaults live in the Skill's
per-CLI reference files, loaded only for the selected CLI.

### `describe_provider`

Call with the selected `provider` before its first use in the current context. Returns that
provider's permitted status and supported options.
Reuse the result for later calls with the same provider. This tool neither launches a CLI nor
checks installation or authentication; it does not consume provider quota or change the tool list.

For example: `describe_provider({"provider": "grok"})` → `start_agent(...)` → `wait_agent(...)`.

### `start_agent`

Starts one provider invocation and immediately returns an in-memory `invocation_id`. It requires
`provider`, `prompt`, and an absolute existing `cwd`; `session_id`, `timeout_seconds`, and explicit
`provider_options` are optional.

Supported providers are `antigravity`, `claude`, `codebuddy`, `codex`, and `grok`. The Skill provides model and effort defaults for the Manager to pass explicitly through
allowlisted options. User choices override that guidance; omitted options still inherit native
settings. Exclusions are configured per host process, independently of the model it uses. For Codex calls outside a
Git repository, the Manager must pass
`{"skip_git_repo_check": true}` in `provider_options`.

### Choosing a model and reasoning effort

To override the defaults for one task, tell your calling Agent the provider, model ID, and
reasoning effort. For example: “Use Codex CLI with model `gpt-6-astra` and effort `medium`.”
The Agent passes `{"model": "gpt-6-astra", "effort": "medium"}` in `provider_options`.
Only specified fields override the Skill guidance. To keep your choices across tasks, put the
same preference in your calling Agent's personal instructions. To use the CLI's native settings,
explicitly ask the Agent to omit the corresponding options.

When a new model becomes available, use its exact ID from that provider's model list; updating
AgentNave is not required to pass a new model ID. If you maintain a source installation and want
to change the bundled defaults, edit the selected CLI file in
`skills/agentnave-manager/references/`, then reload the Skill in a fresh context. An unavailable
model should be reported rather than silently replaced.

### `wait_agent`

The runtime waits up to 600 seconds by default (and at most 600). It returns early
when the invocation finishes or reports a recognized execution blocker. Reconnect after
upgrading from v0.6.0, which used a 120-second default and a 300-second maximum, and respect
any shorter host timeout.

Start, wait and cancel use one flat response: `invocation_id`, `status`, `reason`, `elapsed_ms`,
plus applicable `activity`, `error`, `output`, `output_age_ms` and `session_id` fields. Reasons are
`started`, `wait_elapsed`, `execution_blocked` and `finished`. Running replies include the latest
public reply tail (at most 1,000 Unicode characters), retained across tool events, and its age.
No new public reply means the same tail can recur; unavailable fields are omitted. There is no
cursor, pagination or separate output-reading tool. Final replies are not subject to the tail limit.
Provider usage/cost, native event names, tool call IDs, tool payloads and thinking are not returned.
Public replies may still contain task data; this is not a redaction service.

`execution_blocked` leaves `status=running`: the Manager decides whether to keep waiting or cancel.
Each recognized blocker category wakes once per invocation, avoiding repeated immediate returns
from the same retry loop. Ordinary tool failures, transient retries and silence do not imply a
blocker; unrecognized errors may only become visible in output or the final result. There is no
unsolicited completion/error push without a pending wait request.

Wait expiry never terminates the invocation. `start_agent.timeout_seconds` remains a separate
optional total budget: omitted/null means no AgentNave deadline; an explicit positive value
(up to 86,400 seconds) terminates the invocation when reached. Provider-native limits still apply.

### `cancel_agent`

Stops an invocation and returns its terminal result. Use it only when the Manager intends to end
active provider work; `wait_agent` observes without cancelling.

All four tools publish input and output JSON Schemas. Agent-correctable request errors are MCP Tool
errors with retry guidance; provider launch and execution outcomes remain structured Invocation
Results.

## Lifecycle and security

Invocation handles live only in the current MCP server process. When the server stops, AgentNave
makes a best-effort attempt to terminate processes that remain in the provider process group. A
restart cannot recover old handles, but a retained provider `session_id` can be supplied to a new
`start_agent` call.

Running responses help inspect health and direction using a bounded public reply and native
activity. They do not show every active operation or guarantee progress. Raw streams remain
bounded in process memory; no output log/database is added. A 1,000-character reply tail is kept
per invocation for waiting responses. Final results remain available for this server process.

Use the project directory as `cwd` so the CLI can load its native project rules. Temporary
handoff files do not change that directory. Antigravity can choose a different terminal `Cwd`:
state the absolute project directory in the handoff, require that terminal directory explicitly,
and verify it with `pwd` before project operations. This is a behavioral instruction, not enforced
directory isolation. `provider_options.project` selects a native project ID or name; it is not a
working-directory override. The companion Skill guides task handoffs and keeps
intermediate files in OS temporary storage without imposing Markdown or a result-file format.
AgentNave itself uses stdin/in-memory output except for Grok's temporary prompt file, which is
removed after use. Provider-owned history and caches remain under provider control.

AgentNave is not a sandbox. A same-user provider with command permission can deliberately daemonize,
kill its supervisor, or otherwise escape ordinary POSIX process-group cleanup. Provider-native
permissions remain the security boundary; use OS-level isolation when adversarial containment is
required.

## Verify

These checks are for a development checkout, not the `uv tool` installation above. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the complete contributor workflow.

```bash
uv sync --locked --all-groups
uv run ruff format --check .
uv run ruff check .
uv run pyright
uv run pytest
```

## Contributing and security

Contributions are welcome through GitHub Issues and pull requests. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and validation requirements.

Do not report security vulnerabilities in a public Issue. Follow [SECURITY.md](SECURITY.md) to use
the repository's private vulnerability reporting channel.

## License

AgentNave is licensed under the [MIT License](LICENSE).
