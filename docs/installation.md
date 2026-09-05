# Installation

Install the `agentnave-mcp` runtime once, then register it with each host that should use it.
AgentNave delivers its usage, model-selection, and lifecycle instructions through MCP. It does not
require or distribute a Skill.

## 1. Install the runtime

AgentNave supports macOS and Linux. Install `uv` and Git, then choose a published release tag:

```bash
AGENTNAVE_RELEASE=v0.4.0
uv tool install --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"

AGENTNAVE_MCP="$(uv tool dir --bin)/agentnave-mcp"
test -x "$AGENTNAVE_MCP"
```

The remaining commands reuse these variables; run them in the same shell or define them again.
`uv` manages the isolated Python 3.12 runtime. Use a fixed release rather than mutable `main`, and
register the absolute launcher path rather than a source checkout or development `.venv`.

Provider CLIs are separate programs. Install and authenticate only those you intend to call, using
their official instructions. The MCP server must inherit a `PATH` that can locate them. Desktop
hosts may have a different environment from your terminal; set an explicit `PATH` in the server's
host configuration if necessary, retaining the directories needed by the CLIs and their runtimes.
AgentNave does not install providers or change their login, permissions, or configuration.

## 2. Configure provider exclusions per host

Set `AGENTNAVE_EXCLUDED_PROVIDERS` in each host's MCP server environment to exclude the CLI matching
that host product, regardless of which model the host is currently using:

| Host product | Exclusion value |
| --- | --- |
| Codex | `codex` |
| Claude Code | `claude` |
| CodeBuddy Code | `codebuddy` |
| Grok CLI | `grok` |
| Antigravity | `antigravity` |
| Other hosts | Explicitly choose exclusions, or use an empty value |

Gemini CLI and OpenCode do not have same-product providers in the current registry; they need no
automatic exclusion. Sharing a model vendor is not the same as sharing a host product.

Values are comma-separated provider IDs: `codex,claude` excludes both. Whitespace, letter case, and
duplicates are normalized; empty items are ignored. An unset or empty value excludes nothing.
Unknown IDs prevent server startup so a typo cannot silently disable the intended restriction.
All five providers may be excluded; the tools remain discoverable, but no invocation is allowed.

Exclusions are fixed when the server starts. Restart it after changing configuration. MCP tool
metadata lists permitted and excluded providers. A permitted provider is not necessarily installed
or authenticated. `start_agent` rejects an excluded provider before creating an invocation; a call
cannot override the exclusion and the server never substitutes a different provider. If no permitted
provider is usable, the Manager reports the blocker.

This is a restriction on calls through this MCP server, not a sandbox preventing the host from
executing commands through its other tools.

## 3. Register the MCP server

Any host that can launch a local STDIO MCP process can register the same absolute executable with
its own environment. No host-specific Skill, plugin, or global instruction file is needed. A host
that only accepts remote HTTP MCP endpoints cannot directly use this local server.

For hosts documenting the `mcpServers` JSON format, this example excludes the `codex` provider:

```json
{
  "mcpServers": {
    "agentnave": {
      "command": "/absolute/path/from/uv/tool/dir/bin/agentnave-mcp",
      "args": [],
      "env": {"AGENTNAVE_EXCLUDED_PROVIDERS": "codex"}
    }
  }
}
```

Replace the path and exclusion value for your host. Configuration formats differ; this JSON is an
example for hosts that document this shape, not a universal MCP configuration standard. Preserve
unrelated server entries. JSON does not expand shell variables. Codex uses its own configuration
format; register it with the official CLI command below instead of copying this JSON into Codex.

### Codex

```bash
codex mcp add agentnave --env AGENTNAVE_EXCLUDED_PROVIDERS=codex -- "$AGENTNAVE_MCP"
codex mcp get agentnave
```

Start a new session and confirm the three tools and the `codex` exclusion.
Reference: [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

### Claude Code

```bash
claude mcp add agentnave --transport stdio --scope user \
  --env AGENTNAVE_EXCLUDED_PROVIDERS=claude -- "$AGENTNAVE_MCP"
claude mcp get agentnave
```

Reference: [Claude Code MCP](https://code.claude.com/docs/en/mcp).

### Gemini CLI

```bash
gemini mcp add agentnave "$AGENTNAVE_MCP" --scope user --env AGENTNAVE_EXCLUDED_PROVIDERS=
gemini mcp list
```

Reference: [Gemini CLI commands](https://geminicli.com/docs/cli/cli-reference/).

### OpenCode

Merge this entry into the existing `mcp` object in `~/.config/opencode/opencode.json` or
`opencode.jsonc`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "agentnave": {
      "type": "local",
      "command": ["/absolute/path/from/uv/tool/dir/bin/agentnave-mcp"],
      "environment": {"AGENTNAVE_EXCLUDED_PROVIDERS": ""},
      "enabled": true
    }
  }
}
```

Run `opencode mcp list` and start a new session.
Reference: [OpenCode MCP servers](https://opencode.ai/docs/mcp-servers).

### Other hosts

Use the host's documented MCP registration interface. Configure the absolute command, empty
arguments, and the appropriate exclusion environment value. For CodeBuddy Code, Grok CLI, or
Antigravity hosts, use the corresponding value from the table above. These are configuration
requirements, not claims that every host/version combination has been live-tested.

## Model selection and verification

The Manager chooses a permitted provider and explicitly passes `model` and `effort` in
`provider_options`. MCP instructions and the parameter description carry the default model/effort
pairs and supported option names. User-specified values override the corresponding defaults.
When the user requests native settings, the Manager omits those options. The adapters do not inject
defaults; omitted values retain provider-native behavior. Permissions and tools remain inherited
unless explicitly changed by the user.

After registration or upgrade:

1. Use the host's MCP list/get interface to verify the command and exclusion environment.
2. Restart the server/session and confirm `start_agent`, `wait_agent`, and `cancel_agent` are visible.
3. Inspect the tool metadata to confirm the permitted/excluded providers and model guidance.
4. A call selecting an excluded provider must return a Tool error without starting a CLI.
5. With authorization for any provider quota consumption, run a small task through a permitted
   provider and verify its final result using `wait_agent`.

Repository tests verify MCP contracts with fake providers, including STDIO startup, exclusions,
and the allowed-provider lifecycle. The CI workflow also installs a built wheel and lists tools
through its installed launcher on macOS and Linux. These checks do not establish live compatibility
with every host or availability of each account's models.

## Upgrade, repair, and rollback

Choose the desired published tag and reinstall through `uv`:

```bash
AGENTNAVE_RELEASE=v0.4.0
uv tool install --force --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"
```

The same command can repair a damaged runtime or restore a previous version. Restoring `v0.3.0`
loses exclusion enforcement and MCP model guidance, so it is not an equivalent policy rollback.
Host registrations retain the stable launcher path. Recheck configuration and restart after changes.
There is no cross-host transaction or automatic host configuration update.

When upgrading from v0.3.0, updating the runtime alone leaves the old host registrations without
exclusions. Before restarting, explicitly update each existing registration's environment using
the host's supported configuration interface and the values in sections 2 and 3: for example,
`AGENTNAVE_EXCLUDED_PROVIDERS=codex` for Codex and `=claude` for Claude Code. Preserve other server
settings and sibling registrations. Verify the effective entry in the actual project/session;
a same-name project registration may override the user-level entry. After restart, confirm the
exclusions in tool metadata and test rejection as described above. These restrictions apply to
that configured server process, not to a replacement registration with a different environment.

When migrating from the former two-part installation, first install and verify the MCP-only
runtime. Then remove the old `agentnave-manager` Skill from the host's discovered Skill directories
(including user or project copies). Preserve any user-authored additions before removal. A managed
symlink should be unlinked without deleting its target. Remove its deployment-manifest entry too,
if a separate Skill manager would otherwise reinstall it. Keeping the old Skill can supply stale
model-selection instructions.

## Uninstall

Remove only AgentNave's registrations, then remove the shared runtime after all hosts stop using it:

```bash
# Run the relevant commands for your configured hosts.
codex mcp remove agentnave
claude mcp remove --scope user agentnave
gemini mcp remove agentnave --scope user
uv tool uninstall agentnave
```

For OpenCode, remove only `mcp.agentnave`; for other hosts, use their documented removal interface.
Remove any legacy Skill as described above. AgentNave creates no durable user data and needs no
purge operation. Provider CLIs, authentication, configuration, sessions, and user projects remain
owned by the user and providers.
