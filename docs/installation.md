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

**Paired release:** `v0.20.0` contains the runtime source, `agentnave-manager` Skill and all six
CLI reference files. Install both components from this tag. The older `v0.5.0` contains only
the runtime; it is not a complete paired installation. `v0.10.0` is the first paired release with
native Windows support.

## 1. Check and reuse the runtime

Current AgentNave source supports Windows, macOS, and Linux. Install `uv` and Git. Set
`AGENTNAVE_RELEASE` to a chosen published tag that supports the target platform, and use the same
value for the Skill in step 4. The current published paired release is `v0.20.0`, which supports
Windows, macOS, and Linux.

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

The same package also installs the `agentnave-studio-mcp` launcher for AgentNave Studio (saved
shows, review and a local dashboard; see the [studio guide](studio.md)). It includes every core
tool. To use it, point `AGENTNAVE_MCP` at that launcher instead:

```bash
AGENTNAVE_MCP="$(uv tool dir --bin)/agentnave-studio-mcp"
test -x "$AGENTNAVE_MCP"
```

Register one of the two launchers per host under the `agentnave` name; the studio replaces the core
there rather than running beside it. Studio history lives in `~/.agentnave/` unless
`AGENTNAVE_DATA_DIR` sets another absolute directory; the core launcher keeps no history, opens no
port and never loads the studio code.

GitHub Release publication does not publish the package to a package index.

The remaining commands reuse these variables; run them in the same shell or define them again.
`uv` manages the isolated Python 3.12 runtime. Use a fixed release rather than mutable `main`, and
register the absolute launcher path rather than a source checkout or development `.venv`.

For a paired release that includes Windows support, locate its launcher in PowerShell with:

```powershell
$AgentNaveMcp = Join-Path (uv tool dir --bin) "agentnave-mcp.exe"
if (-not (Test-Path -LiteralPath $AgentNaveMcp -PathType Leaf)) {
    throw "AgentNave launcher not found: $AgentNaveMcp"
}
```

Use `$AgentNaveMcp` wherever the POSIX examples below use `$AGENTNAVE_MCP`. Native Windows
invocations use Job Object process-tree ownership; WSL is not required.

Provider CLIs are separate programs. Install and authenticate only those you intend to call, using
their official instructions. On macOS and Linux, AgentNave looks for each CLI on the inherited
`PATH`, then in `~/.local/bin`, `~/.grok/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.bun/bin`,
`~/.npm-global/bin`, `~/.volta/bin` and `~/.cargo/bin`. For the provider process only, it puts the
CLI's directory first, then the inherited `PATH`, then those directories. Desktop hosts may have a
different environment from your terminal; set an explicit `PATH` in the server's host configuration
if a CLI or its runtime lives elsewhere, or on Windows.
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
6. Pi Coding Agent (`pi`)

The user may also answer “none”. If a selection was requested, wait for the answer before locating provider executables; do not
probe every provider, scan the disk, or infer the selection from the host or model name.

For each selected CLI only, run `command -v <command>` on POSIX or
`(Get-Command <command> -CommandType Application).Source` in PowerShell, using the command in
parentheses above. For example, selection `1, 3` means checking only `grok` and `codebuddy`.
Confirm each result is an executable file or Windows command shim, not a shell alias or function.
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

服务在启动时先读取 `AGENTNAVE_HOST`，否则在 macOS／Linux 沿祖先进程最多 32 层识别宿主；Windows 只读取该环境变量。允许值为 `codex|claude|codebuddy|workbuddy|grok|antigravity|pi|gemini|opencode|hermes`，未知值使启动失败；无法自动识别时保持未知、不自我排除。`describe_provider` 与 `list_members` 的 `host`／`host_source` 可核对识别结果。

宿主对应的 Provider 与已有 `AGENTNAVE_EXCLUDED_PROVIDERS` 取并集，仅限制 task；discussion 仍按 App 中的组员名单决定。已有 MCP 配置无需修改，以下显式排除项继续有效，与宿主当前模型无关：

| Host product | Exclusion value |
| --- | --- |
| Codex | `codex` |
| Claude Code | `claude` |
| CodeBuddy Code / WorkBuddy | `codebuddy` |
| Grok CLI | `grok` |
| Antigravity | `antigravity` |
| Pi Coding Agent | `pi` |
| Other hosts | Explicitly choose exclusions, or use an empty value |

Gemini CLI and OpenCode do not have same-product providers in the current registry; they need no
automatic exclusion. Sharing a model vendor is not the same as sharing a host product.

Values are comma-separated provider IDs: `codex,claude` excludes both. Whitespace, letter case, and
duplicates are normalized; empty items are ignored. An unset or empty value adds no exclusions beyond the detected host provider.
Unknown IDs prevent server startup so a typo cannot silently disable the intended restriction.
Every provider may be excluded; the tools remain discoverable, but `start_agent` then accepts none.
Groups and shows still run, since exclusions apply to single tasks only.

Exclusions are fixed when the server starts. Restart it after changing configuration. MCP tool
metadata lists permitted and excluded providers. A permitted provider is not necessarily installed
or authenticated. `start_agent` rejects an excluded provider before creating an invocation; a call
cannot override the exclusion and the server never substitutes a different provider. If no permitted
provider is usable, the Manager reports the blocker. Exclusions apply to single tasks only: the host
can run its own native subagents for those, while groups and shows may seat any provider, including
the host's own CLI.

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

### 由 App 接入（保留下方手动方式）

托盘设置的“宿主接入”分区管理用户级 `agentnave`：默认工作室入口，高级中可切到核心，并可保存该宿主的绝对配置根目录。根目录依次取 App 用户覆盖（`user`）、GUI 环境变量（`env`）、登录 shell 探测（`shell`）、默认值（`default`）。覆盖和首次发现时间保存在 App 自有数据目录的 `hosts.json`；恢复自动仅移除覆盖，不删除宿主配置。

登录 shell 使用固定白名单打印脚本，空 stdin、三秒超时，输出只按变量名和绝对路径解析，绝不执行；结果缓存至下次“刷新检测”。失败或输出不完整时不自动接管，界面显示 `root_unconfirmed`，可显式接入默认目录或保存根目录。服务定位、安装、升级与版本检查共用缓存的有效 `UV_TOOL_DIR` / `XDG_DATA_HOME`，同一变量优先取 GUI 环境、再取 shell 结果。命令链接目录依次取有效 `UV_TOOL_BIN_DIR`、`XDG_BIN_HOME`、`XDG_DATA_HOME` 的同级 bin、默认目录，并与工具环境目录一起显式传给 uv。首次探测完成前不安装，重启后的自动升级等待初次检测完成。App 不编辑 shell 配置。GET 不写文件、不运行探测；“开始使用”使用缓存，按已安装服务、connected 宿主、已保存或迁移的组员设置、带 manager_session 的记录计算。

启动及刷新只接管已有条目：补宿主标识，命令确实缺失时修复同类 launcher；保留原参数、环境、启停与工具过滤。完好但未接管的条目为 connected，另标 `has_host_env=false`。裸命令优先按条目自己的 PATH 解析；无法确定为 unknown，不替换。新增宿主需显式接入，不自动增加组员；孤留条目需显式移除。所有宿主在完成 App 真实接入验收前均为“未实测”。

十个宿主全部由 App 定点编辑文件，不调用宿主 CLI 写配置。JSON/JSONC 保留非目标区间，TOML 使用保留格式的编辑，Hermes 定点编辑 YAML 块。读取字节与哈希后在内存编辑、暂存；替换前重读比较，变化时基于最新内容重做，最多三次，仍冲突报 conflict。替换前备份至 App 数据目录 backups/，全局保留最近 30 份。写后重新解析并校验其他语义不变；仅当文件仍等于本次写入字节才回滚，否则保留外部新内容并报 conflict。替换前比对与 rename 之间仍有极短竞态窗口，不能保证跨应用的完整事务。

MCP 配置路径的全部祖先（家目录本身除外）及目标都拒绝符号链接，显示 `symlink_path` 并要求手动处理。外部 Skill（包括符号链接）仅显示 external，跳过写入，不影响宿主接入状态或 MCP 接入、移除；只有实际写入 App 管理的 Skill 时才检查该路径。App 只更新版本标记与内容清单匹配的 Skill 副本；外部内容不覆盖，目录未知返回 manual。接入及刷新会更新 App 管理的旧版副本。服务缺失时，“安装本机服务”复用 uv 安装逻辑，进度由 `/app/status` 返回，不运行 Provider 会话。

| 宿主 ID | App 写入格式 | 默认 MCP 配置 | Skill 根目录 | tested |
| --- | --- | --- | --- | --- |
| codex | TOML 定点编辑 | `${CODEX_HOME:-~/.codex}/config.toml` | 同配置根目录的 `skills` | false |
| claude | JSON 定点编辑 | `~/.claude.json`；设置 `CLAUDE_CONFIG_DIR` 时为该目录的 `.claude.json` | `${CLAUDE_CONFIG_DIR:-~/.claude}/skills` | false |
| codebuddy | JSON 定点编辑 | `${CODEBUDDY_CONFIG_DIR:-~/.codebuddy}/.mcp.json` | 同配置根目录的 `skills` | false |
| workbuddy | JSON 定点编辑 | `${CODEBUDDY_CONFIG_DIR:-~/.workbuddy}/mcp.json` | 同配置根目录的 `skills` | false |
| grok | TOML 定点编辑 | `${GROK_HOME:-~/.grok}/config.toml` | 同配置根目录的 `skills` | false |
| antigravity | JSON 定点编辑 | `~/.gemini/config/mcp_config.json` | `~/.gemini/config/skills` | false |
| pi | JSON 定点编辑 | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/mcp.json` | 同配置根目录的 `skills` | false |
| gemini | JSON 定点编辑 | `~/.gemini/settings.json`，`mcpServers` | `~/.gemini/skills` | false |
| opencode | JSON/JSONC 定点编辑 | `~/.config/opencode/opencode.json(c)`；支持 `XDG_CONFIG_HOME`、`OPENCODE_CONFIG_DIR`、`OPENCODE_CONFIG` 和 App 覆盖 | 有效根目录的 `skills` | false |
| hermes | YAML 定点块编辑 | `${HERMES_HOME:-~/.hermes}/config.yaml`，`mcp_servers` | 同配置根目录的 `skills` | false |

layers 返回用户、覆盖和已知项目层。移除删除所有用户级定义（包括 OpenCode 的全局、环境覆盖、App 覆盖），逐文件备份并在回执列出；不会在移除高层后悄然退回另一用户层。项目级只读检查限于 App 启动目录及其祖先，实际宿主的其他项目目录仍须手动核对。多文件不是原子整体，冲突时保留外部新内容，依据回执重试余下层。

Pi 的 `agentDir/skills` 已从安装包代码核实；Hermes 新条目 `enabled: true` 且省略工具过滤表示启用全部工具，接管保留原 `include/exclude`，尤其 `include: []`。Gemini 与 OpenCode 按 [Gemini MCP](https://geminicli.com/docs/tools/mcp-server/)、[Gemini Skill](https://geminicli.com/docs/cli/using-agent-skills/)、[OpenCode MCP](https://opencode.ai/docs/mcp-servers/) 和 [OpenCode Skill](https://opencode.ai/docs/skills/) 官方文档实现，这些信息不是 App 接入实测证据。Hermes 流式 MCP 映射、锚点/别名、文档指令等结构仍拒绝自动写入并提示手动处理。真正的 MCP/Skill 发现须按第 5 节验证。

### Codex

```bash
codex mcp add agentnave --env AGENTNAVE_EXCLUDED_PROVIDERS=codex -- "$AGENTNAVE_MCP"
codex mcp get agentnave
```

PowerShell uses the same Codex interface with the Windows launcher variable:

```powershell
codex mcp add agentnave --env AGENTNAVE_EXCLUDED_PROVIDERS=codex -- $AgentNaveMcp
codex mcp get agentnave
```

Start a new session and confirm the eleven core tools (seventeen with the studio) and the `codex` exclusion.
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
tools, then asks whether to enable them. Complete that choice for all AgentNave tools (eleven, or seventeen with the studio); a
successful connection alone does not prove tools are enabled. On upgrade, recheck any saved
per-tool selection so it includes newly added tools. In noninteractive
installation, handle the documented prompt explicitly and verify the saved enabled-tool
selection. Restart the host and verify Skill discovery separately.

### Other hosts

Use the host's documented MCP registration interface. Configure the absolute command, empty
arguments, and the appropriate exclusion environment value. For CodeBuddy Code, Grok CLI, or
Antigravity hosts, use the corresponding value from the table above. These are configuration
requirements, not claims that every host/version combination has been live-tested.

## 4. Install the matching Skill

Use the host's Skill installer or existing deployment manager to install
`skills/agentnave-manager/` from the **same `AGENTNAVE_RELEASE` tag** as the runtime.
Install the whole directory, including `SKILL.md` and every file in `references/`.
The Skill content is shared across hosts; the destination and discovery scope belong to the host.

The checked macOS user-level discovery locations are listed below as examples; use the host's
installed-version settings and your deployment manager as authority, especially with custom
configuration directories.

| Host | User Skill directory |
| --- | --- |
| Codex | `~/.codex/skills` |
| Claude Code | `~/.claude/skills` |
| CodeBuddy Code | `${CODEBUDDY_CONFIG_DIR:-~/.codebuddy}/skills` |
| WorkBuddy | `~/.workbuddy/skills` |
| Grok CLI | `~/.grok/skills` |
| Antigravity | `~/.gemini/config/skills` |
| Hermes | `~/.hermes/skills` |
| Pi | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/skills` |
| Gemini CLI | `~/.gemini/skills`（官方文档，未实测） |
| OpenCode | 有效配置目录的 `skills`，默认 `~/.config/opencode/skills`（未实测） |

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

The v0.6.0 Skill explicitly requests 120-second waits, also supported by v0.5.0.
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

主控先调用 `list_members`，按 task／discussion 模式的组员及说明选择，再用 `describe_provider` 核对参数与宿主身份。App 将默认值存于数据根（`AGENTNAVE_DATA_DIR`，默认 `~/.agentnave`）下的 `members.json`；服务每次调用现读，未传的 model／effort／options 自动补齐并在 `defaults_applied` 中注明。显式参数优先，但超出模式范围会拒绝。未配置字段继续继承 CLI 原生设置；要请求完全原生的对应字段，须同时移除 App 默认值，单纯省略调用参数并不跳过默认值。

配置文件缺失时保留原有行为、不创建目录；损坏时返回明确错误，需在 App 修复。服务不读取旧 `profiles.json`，迁移由 App 负责；核心仍为 11 个工具，工作室仍为 17 个工具。已有 PATH 与排除环境变量不需改写。

After registration or upgrade:

1. Use the host's MCP list/get interface to verify the command and exclusion environment.
2. Restart the server/session and confirm `list_members`, `describe_provider`, `start_agent`,
   `wait_agent`, `wait_any`, `cancel_agent`, `list_invocations`, `open_group`, `run_round`,
   `wait_round` and `cancel_round` are visible (11 total).
   The studio adds six [show and history tools](studio.md) (17 total).
   Confirm the host also discovers `agentnave-manager` in the intended
   scope and can read its five referenced CLI files. Check for older user/project copies that
   could shadow it. For hosts without Skill support, record the installation as MCP-only.
3. Call `describe_provider` only for the selected CLI to confirm its permitted status and supported
   options. This read does not launch a CLI.
4. A `start_agent` call selecting an excluded provider must return a Tool error without starting a CLI.
5. With authorization for any provider quota consumption, run a small task through a permitted
   provider and verify its final result using `wait_agent`. Check actual task evidence as well as
   status: a provider can finish its conversation successfully after a tool was denied.

For Antigravity project work, use the absolute project directory as `cwd` and follow the
[directory handoff guidance](../skills/agentnave-manager/references/antigravity.md): require the
terminal's explicit `Cwd`, verify it with `pwd`, and stop project operations on a mismatch.
`provider_options.project` selects a native project, not a terminal working directory.
For CodeBuddy, `--print` cannot display permission prompts. Preserve native permissions and resolve
any denied tool with the user before retrying; do not automatically enable permission bypass.
The companion Skill's provider references give the calling guidance for these cases.

Report installation results per host as **added**, **reused**, **updated**, or **pending** for
runtime, MCP registration, and Skill separately. Keep four verification levels distinct:
configuration inspection; protocol connection/tool discovery; actual host loading in a new session;
and an authorized real provider invocation. A standalone MCP client test proves only its own
protocol/provider path, not every desktop host's discovery or long-result rendering. The macOS
versions above are observations from 2026-09-08, not an all-version compatibility guarantee.

Repository tests verify MCP contracts with fake providers, including STDIO startup, exclusions,
and the allowed-provider lifecycle. CI checks that the Git source archive contains the runtime and complete Skill with all six CLI
references, matching the source-tag installation route. The workflow also installs a built wheel and lists tools
through its installed launcher on Windows, macOS, and Linux. These checks do not establish live compatibility
with every host or availability of each account's models.

### Upgrading from v0.6.0 to v0.7.0

The v0.7.0 runtime changes wait default/maximum to 600 seconds and replaces the lifecycle
responses with a flat `status`/`reason`/`elapsed_ms` template. Running replies include at most
1,000 characters from the latest public reply; recognized blockers return early without stopping
the CLI. Completed replies retain full final output within the existing capture limit. Usage/cost
and raw event details are omitted. This is a breaking MCP response change from v0.6.0.
Update runtime and Skill together and restart the connection; a source-linked Skill alone does
not upgrade an installed v0.6.0 runtime. The Skill checks the connected schema for older response shapes. A host yielding a background
call handle requires resuming that call through its own wait mechanism.

### Upgrading from v0.7.0 to v0.8.0

The v0.8.0 runtime removes `start_agent.timeout_seconds`, the corresponding runtime cutoff,
and the `timed_out` result status. Remove this argument from callers. AgentNave no longer stops
an invocation based on elapsed runtime. Wait expiry only returns a running response; continue
waiting or use `cancel_agent` to stop work explicitly. Provider-native limits still apply.

Update the runtime and complete Skill together, then restart MCP connections to refresh the
tool schemas. Updating the Skill alone does not remove the older runtime's deadline parameter.
Rolling back to v0.7.0 restores that parameter and its explicit-deadline behavior.

### Upgrading from v0.8.0 to v0.9.0

`wait_agent` now accepts only `invocation_id` and waits for a fixed 300-second window.
Remove `wait_timeout_seconds` from callers and refresh the MCP connection after upgrading
the runtime. Completion and recognized blockers still return early; wait expiry leaves the
CLI running. Update the runtime and Skill together; editing a source-linked Skill does not
upgrade an installed runtime. On older schemas that still expose the wait argument, the Skill
uses 300 seconds. In Codex, resume `Script running with cell ID ...` with `functions.wait`
and the returned `cell_id` until the original call completes.

### Upgrading from v0.10.3 to v0.11.0

Ordinary `start_agent` calls now save private task metadata, native session bindings and final output
under `~/.agentnave/` by default. `AGENTNAVE_DATA_DIR` selects another fixed directory. This is a
new persistence behavior; uninstalling or downgrading does not remove those records. Task grouping
does not share context or change provider permissions. Active invocation handles do not survive restart.

Update runtime and Skill together, reconnect MCP, then use `open_workbench`. Discussion rooms are
explicitly created; use `blind` for independent answers. Only published dialogue is shared between
seats. Ordinary host exclusions remain in force; explicit discussion rooms permit every registered
provider, including the host CLI. The Dashboard stays local and read-only. See the
[studio guide](studio.md) for native restriction limits and recovery behavior.

### Upgrading from v0.11.0 to v0.12.0

Reconnect MCP after upgrading to discover `delete_conversation(room_id)`. This explicit, destructive
tool requires an archived conversation with no unresolved work. After user authorization it removes
AgentNave history and finished invocation handles, while preserving native CLI files and external
root directories. Archiving still preserves history. There is no automatic deletion or migration.

### Upgrading from v0.12.0 to v0.13.0

No new tools or history migration. Hosts sharing one data directory no longer lock every saved
conversation at startup: each indexes history read-only and locks a record only on its first write.
`list_conversations` adds `writable`; `false` means another running service holds that record.
Update runtime and Skill together and reconnect MCP on every host so all services use the new
locking behavior. The paired Skill now defaults Codex to `gpt-6.1-sol` / `high`.

### Upgrading from v0.13.0 to v0.14.0

Adds the `pi` provider (Pi Coding Agent) and its Skill reference; no new tools or history
migration. Update runtime and Skill together and reconnect MCP so `start_agent`, discussion seats
and `describe_provider` accept `pi`. Hosts that want Pi available must install and log in to it
separately; review `AGENTNAVE_EXCLUDED_PROVIDERS` if a host should not launch it.

### Upgrading from v0.19.0 to v0.20.0

No history migration. Update the menu bar app and the runtime together: the app now owns
`members.json` (per-mode members, default models, efforts, options and notes) and migrates an
existing `profiles.json` on first open, while the runtime replaces `list_profiles` with
`list_members` (core 11 tools, studio 17) and applies the App defaults it reports in
`defaults_applied`. A v0.19.0 app updates itself to v0.20.0 in the app; then use the panel's
upgrade action, or reinstall the runtime from the tag, and update the Skill. From v0.20.0 the
app's "全部更新" installs a newer app first and upgrades the runtime after the restart.
Reconnect MCP on each host to see `list_members` and the updated Skill text. Provider CLIs are
now located automatically, and Codex/Antigravity tool output no longer counts toward the 1 MiB
retained-output limit.

### Upgrading from v0.18.0 to v0.19.0

No history migration. Reinstall the runtime and Skill from the tag; each relay switches to the new
server on its own once idle. Reconnect MCP on a host to refresh the tool schemas (core 11
tools, studio 17) and the updated Skill text. A server started inside a provider launched by
AgentNave now exposes no tools, so a CLI that registers AgentNave globally can no longer nest
orchestration. Studio conversations release their lock once idle, so another host can continue
them without restarting the first. The menu bar app gains conversation cleanup and a settings page
that originally wrote `profiles.json` in the data root. Current member settings use `members.json`,
and `list_members` replaces the former profile tool; the App handles migration.

### Upgrading from v0.17.0 to v0.18.0

No tool or history migration. Reinstall the runtime and Skill from the tag, then restart MCP on
every host once more: v0.18.0 is the first release whose launchers start a small relay that runs
the server as a child. After that, a later reinstall is picked up by each relay on its own once
its connection is idle (no request in flight, no running invocation or unread result, and, for
the core launcher, no group opened), without reconnecting. Hosts still see new tools and Skill
text only after they reconnect. `list_invocations` adds `result_read` to finished entries.
Studio task history now also saves the prompt the host sent.

### Upgrading from v0.16.x to v0.17.0

The studio is no longer a separate `agentnave-studio` package: `agentnave` installs both
`agentnave-mcp` and `agentnave-studio-mcp`. Tools, history and launcher names are unchanged, so
host registrations keep working. Replace the two installations with one, then reconnect MCP on
every host:

```bash
uv tool uninstall agentnave-studio  # if installed
uv tool install --force --python 3.12 \
  "git+https://github.com/TimWongUp/agentnave.git@${AGENTNAVE_RELEASE}"
```

### Upgrading from v0.14.0 to v0.15.0

Breaking release. The package splits in two:

- `agentnave` (core) keeps `describe_provider`, `start_agent`, `wait_agent`, `cancel_agent` and adds
  in-memory group rounds: `open_group`, `run_round`, `wait_round`, `cancel_round`. It no longer
  saves history, starts a dashboard or accepts `start_agent` `title`/`conversation_id`.
  `describe_provider` no longer returns `discussion_permitted`.
- `agentnave-studio` adds saved shows and tasks, review, host messages, isolated seats and the
  dashboard; see the [studio guide](studio.md). The thirteen discussion and workbench tools are
  removed from the core; the studio keeps the conversation tools and its `start_agent` keeps
  `title`/`conversation_id`.
- The `discussion_mode` provider option is renamed `restricted_tools`. Antigravity's restricted
  mode now requires a dedicated cwd that already contains its profile; the studio's isolated
  seats prepare it.

History saved by earlier versions in `~/.agentnave/` is not migrated; the studio lists it as
unavailable. Delete it once no longer needed. Update the runtime (choose core or studio) and Skill
together, then reconnect MCP on every host.

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
Skill context, then repeat step 5. From v0.18.0 the launcher relays switch to a reinstalled
runtime on their own when idle; restart to pick up new tools or Skill text. Updating the runtime alone does not update the Skill.

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
uv tool uninstall agentnave-studio  # releases before the single package
```

For OpenCode, remove only `mcp.agentnave`; for other hosts, use their documented removal interface.
Remove the companion Skill through its deployment manager, or remove only its installed directory
or symlink, preserving user-authored additions. Uninstalling the runtime does not delete private
history in `~/.agentnave/` (or `AGENTNAVE_DATA_DIR`) or explicitly selected conversation directories.
Keep these records unless the user separately requests their deletion. Provider CLIs, authentication,
configuration, sessions, and user projects remain owned by the user and providers.
