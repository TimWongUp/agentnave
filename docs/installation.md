# Installation

An AgentNave installation has exactly two AgentNave-owned parts:

1. the `agentnave-mcp` runtime, installed once with `uv tool`; and
2. the `agentnave-manager` Skill, installed in each host agent that should use the runtime.

Keep both parts on the same published release tag. Provider CLIs such as Claude Code, Codex CLI,
or Grok CLI are separate programs: install and authenticate only the providers you intend to call,
using their own official instructions.

The examples below use `v0.3.0`, which supports the `antigravity`, `claude`, `codebuddy`, `codex`,
and `grok` providers. A host agent and a provider CLI are different roles even when both are Codex
or Claude Code.

## 1. Install the runtime

AgentNave supports macOS and Linux and requires `uv`:

```bash
AGENTNAVE_RELEASE=v0.3.0
uv tool install --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"

AGENTNAVE_MCP="$(uv tool dir --bin)/agentnave-mcp"
test -x "$AGENTNAVE_MCP"
```

The remaining commands reuse `AGENTNAVE_RELEASE` and `AGENTNAVE_MCP`; run them in the same shell or
define those variables again.

Always use a published tag rather than the mutable `main` branch. The absolute launcher path is the
stable boundary between `uv` and the MCP host; do not point a long-lived host registration at a
source checkout or its `.venv`.

## 2. Install the Manager Skill

Codex, Gemini CLI, and OpenCode all discover the shared Agent Skills user directory. One copy there
serves all three hosts:

```bash
AGENTNAVE_SKILL_DIR="$HOME/.agents/skills/agentnave-manager"
AGENTNAVE_SKILL_TMP="$(mktemp)"
curl -fsSL \
  "https://raw.githubusercontent.com/TimWongUp/agentnave/${AGENTNAVE_RELEASE}/src/agentnave/resources/agentnave-manager/SKILL.md" \
  -o "$AGENTNAVE_SKILL_TMP"
install -d "$AGENTNAVE_SKILL_DIR"
install -m 0644 "$AGENTNAVE_SKILL_TMP" "$AGENTNAVE_SKILL_DIR/SKILL.md"
rm "$AGENTNAVE_SKILL_TMP"
```

Claude Code uses its own personal Skill directory, so install a second copy only when Claude Code is
one of the host agents:

```bash
AGENTNAVE_SKILL_DIR="$HOME/.claude/skills/agentnave-manager"
AGENTNAVE_SKILL_TMP="$(mktemp)"
curl -fsSL \
  "https://raw.githubusercontent.com/TimWongUp/agentnave/${AGENTNAVE_RELEASE}/src/agentnave/resources/agentnave-manager/SKILL.md" \
  -o "$AGENTNAVE_SKILL_TMP"
install -d "$AGENTNAVE_SKILL_DIR"
install -m 0644 "$AGENTNAVE_SKILL_TMP" "$AGENTNAVE_SKILL_DIR/SKILL.md"
rm "$AGENTNAVE_SKILL_TMP"
```

The Skill is release-managed. Update it by replacing `SKILL.md` from a newer published tag rather
than editing the installed copy.

## 3. Register the MCP server

Choose every host agent that should use AgentNave. Personal/user scope is shown because AgentNave is
normally useful across projects.

### Codex

```bash
codex mcp add agentnave -- "$AGENTNAVE_MCP"
codex mcp get agentnave
```

Start a new Codex session. It should discover `agentnave-manager`, and the MCP server should expose
`start_agent`, `wait_agent`, and `cancel_agent`. Codex CLI, the Codex IDE extension, and the Codex app
share the same MCP configuration.

Official references: [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) and
[Codex Skills](https://learn.chatgpt.com/docs/build-skills).

### Claude Code

```bash
claude mcp add --transport stdio --scope user agentnave -- "$AGENTNAVE_MCP"
claude mcp get agentnave
```

Run `/skills` in Claude Code to confirm Skill discovery. If `~/.claude/skills` did not exist when the
current session started, restart Claude Code once.

Official references: [Claude Code MCP](https://code.claude.com/docs/en/mcp) and
[Claude Code Skills](https://code.claude.com/docs/en/slash-commands).

### Gemini CLI

Register the MCP server at user scope:

```bash
gemini mcp add agentnave "$AGENTNAVE_MCP" --scope user
gemini mcp list
```

Run `gemini skills list` to verify the shared Skill, then run `/skills reload` in an already open
Gemini CLI session if necessary.

Official references: [Gemini CLI commands](https://geminicli.com/docs/cli/cli-reference/) and
[Gemini CLI Skills](https://geminicli.com/docs/cli/skills/).

### OpenCode

Run OpenCode's interactive MCP setup and choose a local server. Use `agentnave` as the name and the
literal absolute value of `$AGENTNAVE_MCP` as its command:

```bash
opencode mcp add
opencode mcp list
```

For a non-interactive or manually reviewed setup, merge this entry into the existing `mcp` object in
`~/.config/opencode/opencode.json` (or `opencode.jsonc`) without replacing unrelated configuration:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "agentnave": {
      "type": "local",
      "command": ["/absolute/path/from/uv/tool/dir/bin/agentnave-mcp"],
      "enabled": true
    }
  }
}
```

Use the literal absolute value printed by `uv tool dir --bin`; JSON does not expand shell variables.

Start a new OpenCode session and confirm that it discovers the shared Skill.

Official references: [OpenCode CLI](https://opencode.ai/docs/cli/),
[OpenCode MCP servers](https://opencode.ai/docs/mcp-servers), and
[OpenCode Skills](https://opencode.ai/docs/skills).

### Other compatible agents

A host is compatible when it can both:

- launch a local STDIO MCP server from an absolute executable path; and
- discover an [Agent Skills](https://agentskills.io/specification) directory containing
  `agentnave-manager/SKILL.md`.

Register `$AGENTNAVE_MCP` through the host's supported MCP interface, then install the Skill into a
documented user or project Skill root. If a host supports MCP but not Skills, AgentNave's tools can
still be called, but the Manager routing and lifecycle instructions will not be installed. A Skill
without the MCP tools is not a working AgentNave installation.

## Upgrade

Choose a newer published tag and replace both AgentNave-owned parts from that same tag:

```bash
AGENTNAVE_RELEASE=vNEXT
uv tool install --force --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"
```

Repeat the Skill installation for each host. Existing MCP registrations keep using the stable
launcher path. Restart the host, verify its MCP entry, confirm Skill discovery, and confirm that the
three lifecycle tools are available.

## Uninstall

Remove only the registrations and Skill files you installed, then remove the shared runtime.

```bash
# Run the command for each configured host.
codex mcp remove agentnave
claude mcp remove --scope user agentnave
gemini mcp remove agentnave --scope user

# Remove each installed Skill file. rmdir refuses to remove a directory containing other files.
rm -f "$HOME/.agents/skills/agentnave-manager/SKILL.md"
rmdir "$HOME/.agents/skills/agentnave-manager"
rm -f "$HOME/.claude/skills/agentnave-manager/SKILL.md"
rmdir "$HOME/.claude/skills/agentnave-manager"

uv tool uninstall agentnave
```

For OpenCode, remove only the `mcp.agentnave` entry from its configuration.

AgentNave creates no durable user data, so there is no separate purge step. Removing AgentNave does
not remove provider CLIs, their authentication, configuration, or sessions.
