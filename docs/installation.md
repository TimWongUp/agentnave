# Installation

The standard installation pairs the MCP runtime with the `agentnave-manager` Skill from the
same release. Check existing installations first, then complete only the missing or explicitly
requested updates in the five steps below. Provider CLIs are installed
and authenticated separately.

For the same OS user and `uv` tool directories, hosts share one runtime installation and launcher.
Each host registers that launcher with its own environment and starts an independent STDIO server
process; invocation handles are not shared. Adding another host normally requires only its MCP
registration and Skill discovery entry, not another runtime installation.

The MCP server supplies parameter and lifecycle contracts. The Skill teaches CLI selection,
model options, waiting, cancellation, and session continuation. Neither installs providers or
sets the calling Agent's planning, review, or retry workflow. Hosts without Skill support can
use MCP alone, but must supply their own calling guidance.

**Paired release:** `v0.6.0` contains the runtime source, `agentnave-manager` Skill and all five
CLI reference files. Install both components from this tag. The older `v0.5.0` contains only
the runtime; it is not a complete paired installation.

## 1. Check and reuse the runtime

AgentNave supports macOS and Linux. Install `uv` and Git. Set `AGENTNAVE_RELEASE` to the chosen
published tag; the current paired release is `v0.6.0`. Use the same value for the Skill in step 4.

Inspect the current installation before running an install command:

```bash
uv tool list
uv tool dir
AGENTNAVE_MCP="$(uv tool dir --bin)/agentnave-mcp"
```

Check the installed AgentNave version and source using the `uv` installation receipt or package
source metadata, and verify that the launcher is executable and belongs to that installation.
A matching version string alone does not establish the source release. Also inspect the target
host's existing MCP registration as described in step 3; it may use a custom launcher instead.

- Matching release/source and working launcher: reuse `AGENTNAVE_MCP` and skip installation.
- Neither a runtime nor an existing custom registration: install using the command below.
- Different version, broken launcher, custom installation, or uncertain source: report the
  difference before replacement. Adding a host does not authorize upgrading, repairing, or
  migrating the shared runtime. Reuse the confirmed existing release when it meets the requested
  requirements; otherwise follow an explicitly authorized upgrade or repair.

An upgrade changes the files used by every host pointing at the shared launcher. Identify known
consumers from the relevant host configurations, disclose any unknown coverage, and explain that
they must restart their MCP connections. Reuse current-session authorization when it already
covers that shared change; otherwise obtain it before replacing the runtime. Do not use `--force`
as a response to finding an existing installation.

For a missing runtime only:

```bash
: "${AGENTNAVE_RELEASE:?Set the chosen published release tag first}"
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

### Select installed CLIs before locating them

When an Agent assists with installation, reuse the user's current-session selection of installed
CLIs. If it is missing, ask which are already installed using the list below; accept multiple
numbers, for example `1, 3`:

1. Grok CLI (`grok`)
2. Claude Code (`claude`)
3. CodeBuddy Code (`codebuddy`)
4. Codex CLI (`codex`)
5. Antigravity CLI (`agy`)

The user may also answer “none”. If a selection was requested, wait for the answer before locating provider executables; do not
probe every provider, scan the disk, or infer the selection from the host or model name.

For each selected CLI only, run `command -v <command>` in the user's terminal shell, using the
command in parentheses above. For example, selection `1, 3` means checking only `command -v grok`
and `command -v codebuddy`. Confirm each result is an executable file, not a shell alias or function.
If a selected command is missing, report that specific CLI and ask for its installation location
or let the user install it before checking again. Do not search unselected providers.

When registering the MCP server below, add the discovered executable directories to that server's
host-specific `PATH`, preserving any existing entries and the paths needed by the selected CLIs'
interpreters. Write resolved directory values, not literal `$PATH` or `~`, into JSON configuration.
This selection only scopes installation-time checks; it does not change provider exclusions.
If the user selected none, explain that provider calls require a separately installed CLI.

This is an Agent-assisted installation procedure, not an interactive prompt or automatic discovery
feature in the MCP server. AgentNave continues to inherit the configured `PATH` at runtime.

## 2. Configure provider exclusions per host

Set `AGENTNAVE_EXCLUDED_PROVIDERS` in each host's MCP server environment to exclude the CLI matching
that host product, regardless of which model the host is currently using:

| Host product | Exclusion value |
| --- | --- |
| Codex | `codex` |
| Claude Code | `claude` |
| CodeBuddy Code / WorkBuddy | `codebuddy` |
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
its own environment. Registration installs no Skill; continue to step 4 to install the shared
companion Skill. No global instruction edits are needed. A host that only accepts remote HTTP
MCP endpoints cannot directly use this local server.

Before any `add` command below, inspect the target host's MCP list/get interface and its effective
configuration in the intended project/session. Check user and project scopes, including entries
under another name that resolve to the same launcher; do not create a duplicate alias.

- If the effective entry already has the intended launcher, arguments, enabled state, provider
  exclusions and required `PATH`, leave it unchanged and proceed to Skill discovery.
- If it is absent, add it in the intended scope using the commands below.
- If it differs, preserve unrelated environment values and settings; update only the necessary
  fields through the host's supported interface. Respect existing permission/exclusion choices
  unless their change is authorized. A project entry can shadow a user entry, so editing only
  the user entry may not fix the effective configuration.

The examples below are for missing registrations, not commands to replay on every installation.

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

Start a new session and confirm all four tools and the `codex` exclusion.
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

### CodeBuddy Code and WorkBuddy desktop

CodeBuddy Code's native command manages its CLI registration:

```bash
codebuddy mcp add agentnave --transport stdio --scope user \
  --env AGENTNAVE_EXCLUDED_PROVIDERS=codebuddy -- "$AGENTNAVE_MCP"
codebuddy mcp get agentnave
```

Do not assume this also registers WorkBuddy desktop. In the checked macOS versions
(CodeBuddy Code 2.142.0, WorkBuddy 5.5.3), the CLI used `.mcp.json` under its effective
configuration directory, while WorkBuddy desktop used `mcp.json` under its configuration
directory. With the CLI configured to use `~/.workbuddy`, these were respectively
`~/.workbuddy/.mcp.json` and `~/.workbuddy/mcp.json`. Preserve `CODEBUDDY_CONFIG_DIR` when
configured; do not copy this machine's override as a universal default. Use WorkBuddy's MCP
settings to locate and edit its effective entry, preserving other servers and environment values.
Use the `codebuddy` exclusion for both hosts and restart each connection separately.

### Grok CLI

```bash
grok mcp add agentnave --scope user --transport stdio \
  --env AGENTNAVE_EXCLUDED_PROVIDERS=grok -- "$AGENTNAVE_MCP"
grok mcp list
```

Grok 1.0.13 documents the user configuration as `~/.grok/config.toml`; project configuration
can override the effective entry. Recheck `grok mcp add --help` for the installed version.

### Antigravity

Use Antigravity's MCP settings to register the launcher and the `antigravity` exclusion.
The checked CLI version 1.1.27 uses the shared MCP configuration at
`~/.gemini/config/mcp_config.json`. Verify the effective path in the installed product before
editing it; preserve sibling entries. Restart the MCP connection and check discovery from a
new host session rather than treating the existence of the file as proof of loading.

### Hermes

```bash
hermes mcp add agentnave --command "$AGENTNAVE_MCP" --env AGENTNAVE_EXCLUDED_PROVIDERS=
hermes mcp test agentnave
```

Hermes 0.21.0 writes `mcp_servers` in `~/.hermes/config.yaml`. Its `add` flow connects and discovers
tools, then asks whether to enable them. Complete that choice for all four AgentNave tools; a
successful connection alone does not prove tools are enabled. In noninteractive installation,
handle the documented prompt explicitly and verify the saved enabled-tool selection. Restart
the host and verify Skill discovery separately.

### Other hosts

Use the host's documented MCP registration interface. Configure the absolute command, empty
arguments, and the appropriate exclusion environment value. For CodeBuddy Code, Grok CLI, or
Antigravity hosts, use the corresponding value from the table above. These are configuration
requirements, not claims that every host/version combination has been live-tested.

## 4. Install the matching Skill

Use the host's Skill installer or existing deployment manager to install
`skills/agentnave-manager/` from the **same `AGENTNAVE_RELEASE` tag** as the runtime.
Install the whole directory, including `SKILL.md` and all five files in `references/`.
The Skill content is shared across hosts; the destination and discovery scope belong to the host.

The checked macOS user-level discovery locations are listed below as examples; use the host's
installed-version settings and your deployment manager as authority, especially with custom
configuration directories.

| Host | User Skill directory |
| --- | --- |
| Codex | `~/.codex/skills` |
| Claude Code | `~/.claude/skills` |
| WorkBuddy | `~/.workbuddy/skills` |
| Grok CLI | `~/.grok/skills` |
| Antigravity | `~/.gemini/config/skills` |
| Hermes | `~/.hermes/skills` |

First inspect the host's effective Skill discovery locations and any existing deployment-manager
entry. Check `agentnave-manager` in both user and project scopes, its source/release, and all of
`references/`; an existing name or `SKILL.md` alone is not sufficient.

- Matching source/release and complete content: reuse it and skip copying or re-registering.
- A managed shared source already exists but this host lacks an entry: add only this host's
  discovery entry through that manager, preserving the source and other hosts' entries.
- No installation exists: install from the selected source below.
- Different, incomplete, or uncertain content: compare against the selected release tree and
  preserve user additions. Update only when required and authorized; do not create a second
  copy that shadows it. Changes to a shared source also affect every host linked to that source.

If installing manually, set `AGENTNAVE_SKILLS_DIR` to the absolute Skill directory supported by
that host and desired scope. Obtain a temporary checkout of the selected release:

```bash
: "${AGENTNAVE_RELEASE:?Set the same tag used for the runtime}"
: "${AGENTNAVE_SKILLS_DIR:?Set the absolute Skill directory for the host}"
AGENTNAVE_SOURCE="$(mktemp -d)"
git clone --depth 1 --branch "$AGENTNAVE_RELEASE" \
  https://github.com/TimWongUp/agentnave.git "$AGENTNAVE_SOURCE"
```

Then copy the Skill if missing, or leave identical content unchanged. Differing content is
reported for the update procedure instead of being overwritten:

```bash
(
  set -eu
  case "$AGENTNAVE_SKILLS_DIR" in
    /*) ;;
    *) echo "Use an absolute Skill directory" >&2; exit 1 ;;
  esac
  test -f "$AGENTNAVE_SOURCE/skills/agentnave-manager/SKILL.md"
  test -d "$AGENTNAVE_SOURCE/skills/agentnave-manager/references"
  if test -e "$AGENTNAVE_SKILLS_DIR/agentnave-manager" ||
     test -L "$AGENTNAVE_SKILLS_DIR/agentnave-manager"; then
    if test -d "$AGENTNAVE_SKILLS_DIR/agentnave-manager" &&
       diff -qr "$AGENTNAVE_SOURCE/skills/agentnave-manager" \
         "$AGENTNAVE_SKILLS_DIR/agentnave-manager" >/dev/null; then
      echo "Reuse existing agentnave-manager Skill"
      exit 0
    fi
    echo "Existing Skill differs or is broken; inspect before updating" >&2
    exit 1
  fi
  mkdir -p "$AGENTNAVE_SKILLS_DIR"
  cp -R "$AGENTNAVE_SOURCE/skills/agentnave-manager" "$AGENTNAVE_SKILLS_DIR/"
)
```

A missing source Skill means that release does not provide the paired installation. A differing target
requires inspection through the update procedure below, preserving user additions. After a successful copy or comparison,
the temporary checkout is no longer needed; remove only that temporary directory. If a deployment
manager owns the Skill, register the source and scope there instead of creating a competing copy.

### Development checkout option

Use a local checkout containing this Skill as `AGENTNAVE_SOURCE`, then run the copy block above,
or register that directory with the existing deployment manager. A managed symlink may point at
`skills/agentnave-manager/` in the development checkout, but must not point at a temporary directory.
The target must remain present when switching branches. Record this as a development Skill,
not as part of v0.5.0. Keep the installed runtime's stable launcher registration.

### Upgrading from v0.5.0

The Skill explicitly requests 120-second waits, so it also works with v0.5.0's 30-second default.
In v0.6.0 the runtime wait default itself is 120 seconds. Total `start_agent.timeout_seconds`
is now optional: omitted/null means no AgentNave deadline, replacing the former 1,800-second
cutoff. To retain a deadline, pass an explicit positive total limit; expiry terminates the
invocation. Provider-native limits remain independent. A wait expiry only returns running,
and completion returns early. Respect any shorter host timeout/responsiveness limits.

The v0.6.0 runtime removes `defaults` and `guidance` from `describe_provider`; update callers
that consume those fields. The tool now reports permitted status and supported option names;
the complete Skill and its five references supply model/effort guidance. Running snapshots
add recent native activity, its age, and remaining explicit budget; unknown values are null.
They describe observations rather than all concurrent work or evidence of a stall.

Updating the runtime alone does not install or update the Skill. Update the pair through the
procedure below, restart MCP connections to refresh schemas, and start a fresh Skill context.
A development Skill used with v0.5.0 does not change that older runtime's behavior.

## 5. Verify MCP and Skill discovery

The Manager chooses a permitted provider and explicitly passes `model` and `effort` in
`provider_options`. Read the selected CLI reference from the Skill for model/effort defaults;
call `describe_provider(provider)` for permitted status and supported option names, reusing its
result in the current context. User-specified values override the corresponding defaults.
When the user requests native settings, the Manager omits those options. The adapters do not inject
defaults; omitted values retain provider-native behavior. Permissions and tools remain inherited
unless explicitly changed by the user.

After registration or upgrade:

1. Use the host's MCP list/get interface to verify the command and exclusion environment.
2. Restart the server/session and confirm `describe_provider`, `start_agent`, `wait_agent`, and
   `cancel_agent` are visible. Confirm the host also discovers `agentnave-manager` in the intended
   scope and can read its five referenced CLI files. Check for older user/project copies that
   could shadow it. For hosts without Skill support, record the installation as MCP-only.
3. Call `describe_provider` only for the selected CLI to confirm its permitted status and supported
   options. This read does not launch a CLI.
4. A call selecting an excluded provider must return a Tool error without starting a CLI.
5. With authorization for any provider quota consumption, run a small task through a permitted
   provider and verify its final result using `wait_agent`.

Report installation results per host as **added**, **reused**, **updated**, or **pending** for
runtime, MCP registration, and Skill separately. Keep four verification levels distinct:
configuration inspection; protocol connection/tool discovery; actual host loading in a new session;
and an authorized real provider invocation. A standalone MCP client test proves only its own
protocol/provider path, not every desktop host's discovery or long-result rendering. The macOS
versions above are observations from 2026-09-08, not an all-version compatibility guarantee.

Repository tests verify MCP contracts with fake providers, including STDIO startup, exclusions,
and the allowed-provider lifecycle. CI checks that the Git source archive contains the runtime and complete Skill with all five
references, matching the source-tag installation route. The workflow also installs a built wheel and lists tools
through its installed launcher on macOS and Linux. These checks do not establish live compatibility
with every host or availability of each account's models.

## Upgrade, repair, and rollback

This section replaces an existing installation; it is not part of merely registering another
host. Confirm the shared impact and applicable authorization from step 1 before proceeding.

Choose a published tag containing both components. Set `AGENTNAVE_RELEASE` to that tag and
reinstall the runtime through `uv`:

```bash
: "${AGENTNAVE_RELEASE:?Set the desired published release tag}"
uv tool install --force --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"
```

Then update the Skill from that same tag through its installer or deployment manager. For a manual
copy, stage the new Skill outside the discovered directory, preserve user-authored additions, and
replace only the existing `agentnave-manager` tree, including `references/`; do not merge new files
over old files and leave stale references behind. A managed symlink is updated through its source
registration, without deleting the source directory. Restart the MCP connection and start a fresh
Skill context, then repeat step 5. Updating the runtime alone does not update the Skill.

Use this paired procedure for repair and rollback too. If rolling back to a runtime-only tag such
as v0.5.0, explicitly choose MCP-only or retain the labelled development Skill; it is not a matched
release pair. Restoring `v0.3.0`
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

When migrating from a legacy Skill installation, replace its `agentnave-manager` with the
companion from the selected release, or use the development checkout option above. Check user and project copies, preserving user-authored additions. Update
the deployment manager's source so it does not reinstall legacy model-selection instructions.
A managed symlink should be unlinked without deleting its target.

## Uninstall

For each target host, remove AgentNave's MCP registration and uninstall `agentnave-manager` through
its Skill installer or deployment manager. Remove the shared runtime only after all hosts stop
using it. These are separate operations; the MCP removal commands do not uninstall the Skill:

```bash
# Run the relevant commands for your configured hosts.
codex mcp remove agentnave
claude mcp remove --scope user agentnave
gemini mcp remove agentnave --scope user
uv tool uninstall agentnave
```

For OpenCode, remove only `mcp.agentnave`; for other hosts, use their documented removal interface.
Remove the companion Skill through its deployment manager, or remove only its installed directory
or symlink, preserving user-authored additions. AgentNave creates no durable user data and needs no
purge operation. Provider CLIs, authentication, configuration, sessions, and user projects remain
owned by the user and providers.
