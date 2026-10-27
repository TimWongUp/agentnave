# AgentNave

<p align="center">
  <img src="docs/assets/agentnave-banner.png" alt="AgentNave — a thin local bridge to CLI subagents" width="100%">
</p>

**A thin local bridge from Agent Managers to CLI subagents.**

AgentNave lets an Agent Manager launch Antigravity CLI, Claude Code, CodeBuddy Code, Codex CLI,
Grok CLI, Pi Coding Agent, or DeepSeek Harness as subagents, one at a time or as a group answering in parallel or
serial rounds. Each provider keeps its native authentication,
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
| POSIX process groups / Windows Job Objects | Parallelism, review, and synthesis |
| Normalized terminal results | Retries, permissions, and worktrees |
| Saved parallel and serial group rounds | Who answers, in what order, and acceptance |

## Requirements

- Windows, macOS, or Linux
- `uv` and Git (for installation from a release tag)
- At least one authenticated provider CLI

`uv` installs AgentNave in an isolated Python 3.12 environment. A separately managed system
Python is not required.

## Install

Install the MCP runtime and companion [agentnave-manager Skill](skills/agentnave-manager/SKILL.md)
as a pair. The runtime exposes the tools; the Skill teaches the calling Agent how to use other
CLIs. The optional [agentnave-review Skill](skills/agentnave-review/SKILL.md) adds a risk-graded
review gate for PRs and merges that dispatches read-only reviewer CLIs through AgentNave. The tray
App lists it on its Skills page, installs it per host when you switch it on, and lets you pick the
CLI and model for each reviewer role.

Follow the [installation guide](docs/installation.md) to:

1. Check the existing runtime's source, version and launcher; reuse it when suitable, or install
   the missing runtime with `uv tool`.
2. Inspect the target host's effective MCP registration; reuse matching settings and add only
   a missing entry, with the host's provider exclusions.
3. Reuse the matching Skill or install its complete directory, including `references/`, from
   the same release.
4. Verify both MCP tools and Skill discovery; restart connections or sessions after changes.

**The paired release is `v0.47.0`.** Install the runtime and complete Skill directory from that
tag. Hosts under the same OS user and `uv` tool directories share one runtime installation, but
each MCP connection runs its own STDIO service process. Provider CLIs must be installed and authenticated separately; `uv`
manages only the runtime and does not install the Skill or change provider configuration.

Tasks and groups save history under `~/.agentnave/` (or `AGENTNAVE_DATA_DIR`). Provider
authentication, configuration and native sessions remain provider-owned.

## The MCP surface

| Tool | Purpose |
| --- | --- |
| `start_agent` | Start one provider task; returns an `invocation_id` immediately and saves the task |
| `run_round` | Open a saved group of 2–7 seats, run a parallel or serial round, or post a host remark |
| `wait` | Wait up to five minutes on task and round handles; returns early when any is ready |
| `cancel` | Stop a task or a round and return its terminal result |
| `decide_answers` | With `review=true`, publish each answer or keep it private |
| `list` | List this server's handles and saved conversations, and start the read-only dashboard |
| `read_conversation` | Read one conversation back as the App shows it |
| `list_members` | Read the member defaults set in the App, and one provider's options |

A seat in a group is an ordinary subagent with its own native session. In a `parallel` round all
seats answer independently; in a `serial` round each sees the earlier answers. Seats share only
committed answers, and later rounds receive earlier ones. Independent answers, several rounds,
turn-taking or "answer first, then discuss" are combinations of seat lists and orders, not modes.

Omitted model, effort and options use the member defaults set in the tray App, reported in
`defaults_applied`; explicit parameters win within the mode's allowed range. The
[agentnave-manager Skill](skills/agentnave-manager/SKILL.md) covers CLI selection, options,
waiting and continuation; the [conversation guide](docs/studio.md) covers review, host remarks,
isolated seats and the dashboard; [architecture](docs/context/architecture.md) lists the stable
contracts, including output limits and blocker reporting.

## Lifecycle and security

Invocation handles live only in the current MCP server process and expire seven days after
completion. When the server stops, AgentNave makes a best-effort attempt to terminate processes
that remain in the provider process tree. A restart cannot recover old handles, but saved groups
continue and a retained provider `session_id` can be supplied to a new `start_agent` call.

AgentNave uses dedicated POSIX process-group supervisors on macOS/Linux and kill-on-close Windows
Job Objects on Windows. Windows providers are created suspended, assigned to the Job Object, and
only then resumed so descendants cannot start outside the owned process tree.

AgentNave is not a sandbox. On POSIX, a same-user provider with command permission can deliberately
daemonize, kill its supervisor, or otherwise escape ordinary process-group cleanup. Windows Job
Objects provide tree ownership but do not isolate the provider from the user account or the rest of
the machine. Provider-native permissions remain the security boundary; use OS-level isolation when
adversarial containment is required.

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
