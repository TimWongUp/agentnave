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

**Paired release:** `v0.55.0` contains the runtime source, `agentnave-manager` Skill and all eight
CLI reference files. Install both components from this tag.

## Subagent orchestration guidance

The runtime prepends a fixed instruction to every dispatched task and meeting-seat prompt,
including native-session continuations: complete the assigned task, report back to the calling
manager, and do not call AgentNave tools or delegate through AgentNave. Blockers go back to the
manager. This is an instruction for the model, not a tool-permission boundary. The server does
not inspect process ancestry to disable tools; host identification remains best-effort.

## 1. Check and reuse the runtime

Current AgentNave source supports Windows, macOS, and Linux. Install `uv` and Git. Set
`AGENTNAVE_RELEASE` to a chosen published tag that supports the target platform, and use the same
value for the Skill in step 4. The paired release for this version is `v0.55.0`, which supports
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

Register `agentnave-mcp` under the `agentnave` name. It is the only launcher and includes saved
tasks, group rounds, optional review and viewing. History lives in `~/.agentnave/`, unless
`AGENTNAVE_DATA_DIR` sets another absolute directory. Provider sessions remain provider-owned.

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
AgentNave does not install providers or change their login or configuration. It passes each CLI's
permission tier on every dispatch (its automatic approval reviewer by default, or full access when
the CLI has no such reviewer or the App chooses full access) without changing the CLI's saved
settings.

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
7. DeepSeek Harness (`dsh`; on macOS bundled in `DeepSeek Harness.app`)
8. ZCode (`zcode app-server --stdio`; on macOS the official `ZCode.app` Node bundle is also supported with Node.js installed)

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

服务在启动时先读取 `AGENTNAVE_HOST`，否则在 macOS／Linux 沿祖先进程最多 32 层识别宿主；Windows 只读取该环境变量。允许值为 `codex|claude|codebuddy|workbuddy|grok|antigravity|pi|deepseek|hermes|zcode`，未知值使启动失败（v0.46.0 起不再支持 Gemini CLI 宿主，`gemini` 也属未知值，旧配置的处理见下文升级说明；OpenCode 未经实测，已不再作为宿主接入，`opencode` 同样是未知值，旧的 OpenCode 条目请在其配置中删除 `mcp.agentnave`，App 曾装在 OpenCode 配置目录 `skills/agentnave-manager`（默认 `~/.config/opencode/skills/agentnave-manager`）的 Skill 也需手动删除；两者的历史会话仍可读取）；无法自动识别时保持未知、不自我排除。`list_members` 的 `host`／`host_source` 可核对识别结果。

宿主对应的 Provider 与已有 `AGENTNAVE_EXCLUDED_PROVIDERS` 取并集，仅限制 task；discussion 仍按 App 中的组员名单决定。除带 `AGENTNAVE_HOST=gemini` 或 `AGENTNAVE_HOST=opencode` 的旧条目外，已有 MCP 配置无需修改，以下显式排除项继续有效，与宿主当前模型无关：

| Host product | Exclusion value |
| --- | --- |
| Codex | `codex` |
| Claude Code | `claude` |
| CodeBuddy Code / WorkBuddy | `codebuddy` |
| Grok CLI | `grok` |
| Antigravity | `antigravity` |
| Pi Coding Agent | `pi` |
| DeepSeek Harness | `deepseek` |
| ZCode | `zcode` |
| Other hosts | Explicitly choose exclusions, or use an empty value |

Values are comma-separated provider IDs: `codex,claude` excludes both. Whitespace, letter case, and
duplicates are normalized; empty items are ignored. An unset or empty value adds no exclusions beyond the detected host provider.
Unknown IDs prevent server startup so a typo cannot silently disable the intended restriction.
Every provider may be excluded; the tools remain discoverable, but `start_agent` then accepts none.
Meetings still run, since exclusions apply to single tasks only.

Exclusions are fixed when the server starts. Restart it after changing configuration. MCP tool
metadata lists permitted and excluded providers. A permitted provider is not necessarily installed
or authenticated. `start_agent` rejects an excluded provider before creating an invocation; a call
cannot override the exclusion and the server never substitutes a different provider. If no permitted
provider is usable, the Manager reports the blocker. Exclusions apply to single tasks only: the host
can run its own native subagents for those, while meetings may seat any provider, including
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

托盘顶部导航的“接入”分区同时展示版本更新和宿主接入，管理用户级 `agentnave`：统一使用 `agentnave-mcp`。配置根目录自动检测，App 不提供手填；以前手动指定过的可以“恢复自动”。根目录依次取 App 用户覆盖（`user`）、GUI 环境变量（`env`）、登录 shell 探测（`shell`）、默认值（`default`）。覆盖、首次发现时间及各宿主的重启确认保存在 App 自有数据目录的 `hosts.json`；恢复自动仅移除覆盖，不删除宿主配置。

更新后宿主卡片只在需要操作时提示“需要重新加载”，没有提示就无需操作：界面更新无需重启宿主，接口不变的服务更新会在空闲时自动切换。接口仍旧时，先重新连接 AgentNave MCP，必要时才完整退出并重新打开宿主。v0.31.0 之前启动的中继在新版宿主协议下不会换用新服务，安装新版本超过十分钟仍未切换的连接也会提示重连（重连会结束该连接上运行中的任务）。Skill 更新不提示重新加载，Agent 使用时会读取最新文件；接入配置变化后，只有变化之前就已在运行的连接会提示，重新打开宿主或新开会话后自动消除；当前没有活跃连接时不提示，下次连接自然使用新配置。不报告接口的旧中继无法自动检测，可在操作后手动确认；手动确认不会覆盖仍未同步的真实连接。

登录 shell 使用固定白名单打印脚本，空 stdin、三秒超时，输出只按变量名和绝对路径解析，绝不执行；结果缓存至下次“刷新检测”。失败或输出不完整时不自动接管，界面显示 `root_unconfirmed`，可显式接入默认目录或保存根目录。服务定位、安装、升级与版本检查共用缓存的有效 `UV_TOOL_DIR` / `XDG_DATA_HOME`，同一变量优先取 GUI 环境、再取 shell 结果。命令链接目录依次取有效 `UV_TOOL_BIN_DIR`、`XDG_BIN_HOME`、`XDG_DATA_HOME` 的同级 bin、默认目录，并与工具环境目录一起显式传给 uv。首次目录探测完成前不安装；重启后的自动升级仅等待三秒超时的 shell 目录探测，目录快照使用独立缓存，完整宿主刷新与接管在后台运行，不阻塞升级。App 不编辑 shell 配置。GET 不写文件、不运行探测；“开始使用”使用缓存，按已安装服务、connected 宿主、已保存的组员设置、带 manager_session 的记录计算。

启动、刷新检测及安装成功后只接管已有条目：补宿主标识，统一运行时就绪后修复缺失的统一命令；保留原参数、环境、启停与工具过滤。完好但未接管的条目为 connected，另标 `has_host_env=false`。裸命令优先按条目自己的 PATH 解析；无法确定为 unknown，不替换。新增宿主需显式接入，不自动增加组员；孤留条目需显式移除。Codex、Claude Code、CodeBuddy、Grok、Antigravity（agy）、Pi 和 Hermes 已通过 App 接管及宿主真实调用 `describe_provider` 验收，WorkBuddy 桌面 GUI 已通过真实调用 `list_members` 验收，DeepSeek Harness 桌面 App 已通过真实调用 `list_members` 与 `start_agent` 验收，均标为已实测；ZCode Desktop 已用手写条目通过真实调用 `list_members` 验收，尚未经 App 接管验收，仍标为未实测。WorkBuddy 桌面会停用配置改动后的 MCP 条目，接入或接管后须在其“自定义连接器 › MCP 服务管理”里点“信任”，App 的结果提示会注明。接入配置变化时，已在运行的会话需要重开才会生效；需要时宿主卡片会提示具体方式，没有提示就无需操作。接管验收不代表所有模型或宿主版本都可用。

十个宿主全部由 App 定点编辑文件，不调用宿主 CLI 写配置。接管只编辑变更字段：JSON/JSONC 保留原键序与对象缩进，TOML 保留表的位置和原键序并在表末尾追加新键，Hermes 在块式 env 中按原缩进追加标识，DeepSeek Harness 只增删自己的 `- insert:` 列表项；新建条目采用现有缩进风格的可读块。读取字节与哈希后在内存编辑、暂存；替换前重读比较，变化时基于最新内容重做，最多三次，仍冲突报 conflict。替换前备份至 App 数据目录 backups/，宿主配置文件与 Skill 文件分开计数，各保留最近 30 份。写后重新解析并校验其他语义不变；仅当文件仍等于本次写入字节才回滚，否则保留外部新内容并报 conflict。替换前比对与 rename 之间仍有极短竞态窗口，不能保证跨应用的完整事务。

MCP 配置路径的全部祖先（家目录本身除外）及目标都拒绝符号链接，显示 `symlink_path` 并要求手动处理。Skill 所在目录经过符号链接时显示“无法安装”、不写入，不影响宿主接入状态或 MCP 接入、移除。本服务的 Skill 只由 App 管理：Skill 路径上不是 App 副本的内容（包括符号链接）会在接入或刷新时整体移入备份目录再替换，移除宿主时同样整体移走；目录未知返回 manual。接入时自动安装缺失的 App Skill；启动和刷新检测还会为已有 `agentnave` 用户级入口的宿主自动补装，并更新 App 管理的旧版副本。安装失败会在下次刷新检测时继续自动重试。服务缺失时，“安装本机服务”复用 uv 安装逻辑，进度由 `/app/status` 返回，不运行 Provider 会话。

| 宿主 ID | App 写入格式 | 默认 MCP 配置 | Skill 根目录 | tested |
| --- | --- | --- | --- | --- |
| codex | TOML 定点编辑 | `${CODEX_HOME:-~/.codex}/config.toml` | 同配置根目录的 `skills` | true |
| claude | JSON 定点编辑 | `~/.claude.json`；设置 `CLAUDE_CONFIG_DIR` 时为该目录的 `.claude.json` | `${CLAUDE_CONFIG_DIR:-~/.claude}/skills` | true |
| codebuddy | JSON 定点编辑 | `${CODEBUDDY_CONFIG_DIR:-~/.codebuddy}/.mcp.json` | 同配置根目录的 `skills` | true |
| workbuddy | JSON 定点编辑 | `${CODEBUDDY_CONFIG_DIR:-~/.workbuddy}/mcp.json` | 同配置根目录的 `skills` | true |
| grok | TOML 定点编辑 | `${GROK_HOME:-~/.grok}/config.toml` | 同配置根目录的 `skills` | true |
| antigravity | JSON 定点编辑 | `~/.gemini/config/mcp_config.json` | `~/.gemini/config/skills` | true |
| pi | JSON 定点编辑 | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/mcp.json` | 同配置根目录的 `skills` | true |
| deepseek | YAML 补丁列表项 | `${DSH_HOME:-~/.dsh}/cordis.patch.yml`，`- insert:` 插入 `id: agentnave` 的 `@deepseek-ai/dsh-mcp-client` | 同配置根目录的 `skills` | true |
| hermes | YAML 定点块编辑 | `${HERMES_HOME:-~/.hermes}/config.yaml`，`mcp_servers` | 同配置根目录的 `skills` | true |
| zcode | JSON/JSONC 定点编辑 | `~/.zcode/cli/config.json`，`mcp.servers` | `~/.zcode/skills` | false |

layers 返回用户、覆盖和已知项目层。移除删除所有用户级定义，逐文件备份并在回执列出；不会在移除高层后悄然退回另一用户层。项目级只读检查限于 App 启动目录及其祖先，实际宿主的其他项目目录仍须手动核对。多文件不是原子整体，冲突时保留外部新内容，依据回执重试余下层。

Pi 的 `agentDir/skills` 已从安装包代码核实；Hermes 新条目 `enabled: true` 且省略工具过滤表示启用全部工具，接管保留原 `include/exclude`，尤其 `include: []`。DeepSeek Harness 的家目录级补丁层对桌面 App 与 `dsh` 各 profile 都生效（桌面 App 自己的设置写在 `profiles/desktop/cordis.patch.yml`，App 不碰）；新条目带 `toolCallTimeoutMs: 310000`，否则 dsh 默认 60 秒就截断 `wait`；接管已有条目不改这项。补丁文件必须是 YAML 列表，移除最后一项时留下 `[]`，空文件会让 dsh 启动失败；dsh 会热重载这一层，改动后自动换新 MCP 连接。其他列表项（含 `!!js` 表达式）不解析、原样保留；以下情况交回手动处理：`agentnave` 行与其他条目合在同一个 `insert` 里；`id: agentnave` 的 `name` 不是 `@deepseek-ai/dsh-mcp-client`；文件不是顶格书写的块式列表，比如整表缩进、带 BOM、以 `---` 开头，或写成 `[{...}]` 流式列表。要恢复自动接入，去掉 BOM 和 `---`，把每个列表项改成从行首 `- ` 开始的块式写法即可。Hermes 流式 MCP 映射、锚点/别名、文档指令等结构仍拒绝自动写入并提示手动处理。真正的 MCP/Skill 发现须按第 5 节验证。

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

Start a new session and confirm the sixteen tools and the `codex` exclusion.
Reference: [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

### Claude Code

```bash
claude mcp add agentnave --transport stdio --scope user \
  --env AGENTNAVE_EXCLUDED_PROVIDERS=claude -- "$AGENTNAVE_MCP"
claude mcp get agentnave
```

Reference: [Claude Code MCP](https://code.claude.com/docs/en/mcp).

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
tools, then asks whether to enable them. Complete that choice for all AgentNave tools (sixteen); a
successful connection alone does not prove tools are enabled. On upgrade, recheck any saved
per-tool selection so it includes newly added tools. In noninteractive
installation, handle the documented prompt explicitly and verify the saved enabled-tool
selection. Restart the host and verify Skill discovery separately.

### DeepSeek Harness

在 `${DSH_HOME:-~/.dsh}/cordis.patch.yml`（顶层 YAML 列表，不存在时新建）追加：

```yaml
- insert:
    - id: agentnave
      name: "@deepseek-ai/dsh-mcp-client"
      config:
        serverName: "agentnave"
        transport: "stdio"
        command: "/absolute/path/to/agentnave-mcp"
        env:
          AGENTNAVE_HOST: "deepseek"
        toolCallTimeoutMs: 310000
```

`toolCallTimeoutMs` matters: dsh ends each MCP tool call after 60 seconds by default, while `wait`
blocks for up to 300 seconds. This home-level patch applies to the desktop App and every `dsh` profile. dsh clears inherited
`DSH_*` and secret-looking variables before starting stdio servers; the `env` block above is
merged back on top. Restart DeepSeek Harness, then check that the tools appear as
`mcp__agentnave__*`.

### ZCode

Use `~/.zcode/cli/config.json`, keeping any other settings and servers:

```json
{
  "mcp": {
    "servers": {
      "agentnave": {
        "type": "stdio",
        "command": "/absolute/path/to/agentnave-mcp",
        "args": [],
        "env": {"AGENTNAVE_HOST": "zcode"},
        "timeoutMs": 310000
      }
    }
  }
}
```

Use the installed release launcher's absolute path. ZCode stops an MCP tool call after 30
seconds unless the entry sets `timeoutMs`; 310000 covers `wait`'s 300-second limit. The App
writes it only when it creates the entry; takeover of an existing entry leaves it unchanged.
ZCode reads `~/.agents/mcp.json` only while `~/.zcode/cli/config.json` names no MCP server, so
the App refuses a first connection while that fallback holds servers: move them under
`mcp.servers` first. New sessions load this entry; in Desktop,
inspect the connection under Settings → MCP. ZCode's native permission prompts still apply to
boss tool calls. This host configuration does not change its permission mode.

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
| ZCode | `~/.zcode/skills` |
| Pi | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/skills` |
| DeepSeek Harness | `${DSH_HOME:-~/.dsh}/skills` |

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
not as part of a release. Keep the installed runtime's stable launcher registration.

## 5. Verify MCP and Skill discovery

boss 先调用 `list_members`，按 task／discussion 模式的组员及说明选择，再用 `list_members(provider=...)` 核对该 CLI 的允许状态与参数。App 将默认值存于数据根（`AGENTNAVE_DATA_DIR`，默认 `~/.agentnave`）下的 `members.json`；服务每次调用现读，未传的 model／effort／options 自动补齐并在 `defaults_applied` 中注明。显式参数优先，但超出模式范围会拒绝。未配置字段继续继承 CLI 原生设置；要请求完全原生的对应字段，须同时移除 App 默认值，单纯省略调用参数并不跳过默认值。

配置文件缺失时保留原有行为、不创建目录；损坏时返回明确错误，需在 App 修复。工具为 9 个。已有 PATH 与排除环境变量不需改写。

After registration or upgrade:

1. Use the host's MCP list/get interface to verify the command and exclusion environment.
2. Restart the server/session and confirm `start_agent`, `wait`, `cancel`, `list`,
   `list_members`, `run_round`, `decide_answers`, `handover` and `read_conversation` are
   visible (9 total).
   Confirm the host also discovers `agentnave-manager` in the intended
   scope and can read its eight referenced CLI files. Check for older user/project copies that
   could shadow it. For hosts without Skill support, record the installation as MCP-only.
3. Call `list_members(provider=...)` only for the selected CLI to confirm its permitted status and supported
   options. This read does not launch a CLI.
4. A `start_agent` call selecting an excluded provider must return a Tool error without starting a CLI.
5. With authorization for any provider quota consumption, run a small task through a permitted
   provider and verify its final result using `wait`. Check actual task evidence as well as
   status: a provider can finish its conversation successfully after a tool was denied.

For Antigravity project work, use the absolute project directory as `cwd` and follow the
[directory handoff guidance](../skills/agentnave-manager/references/antigravity.md): require the
terminal's explicit `Cwd`, verify it with `pwd`, and stop project operations on a mismatch.
`provider_options.project` selects a native project, not a terminal working directory.
For CodeBuddy, `--print` cannot display permission prompts and its own `auto` mode still denies
commands, so its permission tier passes `bypassPermissions`. A terminal result's `warnings` reports
tool calls a CLI denied while still finishing successfully.
The companion Skill's provider references give the calling guidance for these cases.

Report installation results per host as **added**, **reused**, **updated**, or **pending** for
runtime, MCP registration, and Skill separately. Keep four verification levels distinct:
configuration inspection; protocol connection/tool discovery; actual host loading in a new session;
and an authorized real provider invocation. A standalone MCP client test proves only its own
protocol/provider path, not every desktop host's discovery or long-result rendering. The macOS
versions above are observations from 2026-09-08, not an all-version compatibility guarantee.

Repository tests verify MCP contracts with fake providers, including STDIO startup, exclusions,
and the allowed-provider lifecycle. CI checks that the Git source archive contains the runtime and complete Skill with all eight CLI
references, matching the source-tag installation route. The workflow also installs a built wheel and lists tools
through its installed launcher on Windows, macOS, and Linux. These checks do not establish live compatibility
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
registration, without deleting the source directory, then repeat step 5. Agents read the updated
Skill from disk on use, so a Skill update needs no restart. The launcher relays switch to a
reinstalled runtime on their own when idle; reconnect MCP only when the tool interface changed.
Updating the runtime alone does not update the Skill.

Use this paired procedure for repair and rollback too. Host registrations retain the stable
launcher path. Recheck configuration and restart after changes. There is no cross-host
transaction or automatic host configuration update.

### Upgrading from v0.54.0 to v0.55.0

"Update all" updates the App, the runtime and App-managed Skills; for a host not connected through
the App, replace its `agentnave-manager` and `agentnave-review` Skills with the complete v0.55.0
directories. The orchestration presets, the rules writing and the interface changes ship in the
App, so updating only the runtime does not bring them. The orchestration preset on the 「高级」
page defaults to off, so upgrading leaves every global rules file untouched; choosing a preset
writes a marked block into the user-level rules file each connected host reads, after backing the
file up in the App's `backups/` folder. `orchestration.json` is created in the data root only when
orchestration settings are first saved. The `list_members` and `run_round` tool descriptions
changed: reload or restart each host the App marks as needing it. `members.json` keeps its
structure, and history, member configuration and settings need no migration. Upgrading from
v0.53.1 or earlier also brings the v0.54.0 changes, including the App taking over the Skill
directories of connected hosts; see [v0.54.0 release notes](releases/v0.54.0.md). Upgrading from
v0.52.0 or earlier also brings the v0.53.0 changes, including manual cleanup of an OpenCode host
entry; see [v0.53.0 release notes](releases/v0.53.0.md). Upgrading from v0.45.0 or earlier also
drops Gemini CLI as a host, which needs manual cleanup of its old entry; see
[v0.46.0 release notes](releases/v0.46.0.md). See [v0.55.0 release notes](releases/v0.55.0.md).

## Uninstall

For each target host, remove AgentNave's MCP registration and uninstall `agentnave-manager` through
its Skill installer or deployment manager. Remove the shared runtime only after all hosts stop
using it. These are separate operations; the MCP removal commands do not uninstall the Skill:

```bash
# Run the relevant commands for your configured hosts.
codex mcp remove agentnave
claude mcp remove --scope user agentnave
uv tool uninstall agentnave
```

For other hosts, use their documented removal interface.
Remove the companion Skill through its deployment manager, or remove only its installed directory
or symlink, preserving user-authored additions. Uninstalling the runtime does not delete private
history in `~/.agentnave/` (or `AGENTNAVE_DATA_DIR`) or explicitly selected conversation directories.
Keep these records unless the user separately requests their deletion. Provider CLIs, authentication,
configuration, sessions, and user projects remain owned by the user and providers.
