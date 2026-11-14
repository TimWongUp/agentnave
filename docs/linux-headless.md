# 无头 Linux 宿主接入指引（v1.11.0，bot 作 boss）

> 草稿。适用对象：跑在无图形界面 Linux 上的 bot（例如 Grok Bot），只能通过 **MCP stdio 连接器** 和 **shell** 接入 AgentNave，没有托盘 App。
> 托盘 App 平时替你写的配置文件，本文教你手写；每个字段都注明出处（适用于 1.11.0）。

## 0. 心智模型

- `agentnave-mcp` 是 stdio MCP 服务（`pyproject.toml` → `agentnave-mcp = "agentnave.relay:main"`），由宿主（bot）按需拉起。
- 数据根目录 `~/.agentnave/`，可用环境变量 `AGENTNAVE_DATA_DIR` 改（`src/agentnave/members.py` `data_root()`）。
- 服务**只读不写** App 的配置文件（`members.py` 文件头：“never create or modify the data root”）；文件缺失＝未配置，文件损坏＝报错并提示 `Repair it in the AgentNave App, or fix or remove <文件完整路径>.`（`<文件完整路径>` 会替换为实际路径）——在 Linux 上可手动修复 JSON；删除会恢复未配置状态，需先确认影响。
- **重要**：派出的 CLI 进程挂在这个 MCP 会话上。stdio 客户端退出，任务会变成 `cancelled` 或 `interrupted (server_restarted)`。所以 `start_agent` 和 `wait` 必须在同一个连接里完成（Grok Bot 的情况见 §6.7）。

## 1. 安装

### 1.1 AgentNave

```bash
uv tool install --force --python 3.12 "git+https://github.com/TimWongUp/agentnave.git@v1.11.0"
which agentnave-mcp      # 一般是 ~/.local/bin/agentnave-mcp
```

自带的 skill 随包安装在 `~/.local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave/skills/`（`pyproject.toml` 的 force-include 段）。

### 1.2 CLI 前置条件（按需装，至少一个）

| CLI | 安装 | 登录 / 备注 |
|---|---|---|
| Pi | 按 Pi 官方说明安装 | 模型写成 `provider/model`，例如 `opencode-go/deepseek-v4.1-flash`；Pi 没有审批环节（`members.py` `PERMISSION_TIERS` 注释） |
| CodeBuddy | `npm i -g @tencent-ai/codebuddy-code` | `codebuddy` 首次登录要用微信扫码，需要有人看屏幕；登录数据在 `~/.codebuddy`。用 `-p/--print` 非交互运行；可用模型见 `codebuddy --help` 的 `--model` 一行 |
| DeepSeek Harness (dsh) | `npm i -g @deepseek-ai/dsh` | **需要 Node ≥ 22.19**。系统 Node 太旧时，单独装一份到 `~/.local/node`，并把 `node/npm/npx` 软链到 `~/.local/bin`（要排在 PATH 前面）。Key 写到 `$DSH_HOME/.env`（默认 `~/.dsh/.env`）：`DEEPSEEK_API_KEY=...`，`chmod 600`，**不要 echo 出来**。默认模型看 `dsh --profile headless --dump-config` 里的 `agent-default-model` |
| Codex | `npm i -g @openai/codex` | `codex login`（ChatGPT 账号）；见 §8.2 |
| Antigravity | 官方安装 | 部分地区不可用，见 §8.3 |

npm 全局目录若无写权限：`npm config set prefix ~/.local`。

装完先各自在 shell 里直接跑一次，确认 CLI 本身能答，再接 AgentNave。

## 2. 把 bot 接成 MCP 宿主

### 2.1 stdio 连接器

在 bot 的连接器配置里加一个 stdio 服务器：

```json
{
  "command": "/home/<user>/.local/bin/agentnave-mcp",
  "args": [],
  "env": {}
}
```

可选环境变量（`src/agentnave/host.py` `detect_host()`、`mcp_server.py` `_read_excluded_providers()`）：

- `AGENTNAVE_HOST`：宿主名，只能取 `codex|claude|codebuddy|workbuddy|grok|antigravity|pi|deepseek|devin|hermes|zcode`。bot 不属于这些，**不要设**；这时 `list_members` 报 `"host_source":"unknown"`，属正常。（`grok` 指 Grok CLI，不是 Grok Bot。）
- `AGENTNAVE_HOST_SESSION_ID`：当前聊天的会话 ID，优先于宿主原生环境变量；只能按聊天注入，不能给多个聊天共用同一个值。设置后可省略调用中的 `manager_session_id`。
- `AGENTNAVE_HOST_APP`：App 的接入行身份，没有 App 就不设（见 §3.4）。
- `AGENTNAVE_EXCLUDED_PROVIDERS=codex,antigravity`：在 task 模式下禁止这些 CLI，是暂停 CLI 的另一种做法。
- `AGENTNAVE_DATA_DIR`：改数据根目录。

### 2.2 升级与重连

升级前先等任务结束并收取结果。用 App 安装的新版服务，需让宿主重连 AgentNave MCP 或新开会话才会生效；没有 App、用 §1.1 的 `uv tool install --force` 原地升级时，中继会在空闲时自动切换。工具 schema 有变化时，中继会通知宿主刷新，不按协议版本区分。若宿主仍用旧 schema，先重连，再用 `list_members` 检查；不必例行删除连接器。

**从 1.0 以前升级：先断开所有使用同一数据目录的旧服务连接，再备份整个数据目录，然后升级、重连。** 新服务启动时会把会话库迁移到 v2，并删除旧身份记录及其关联数据和席位目录（没有聊天身份，或聊天身份不以 `s-` 开头的记录）；这些记录不会保留展示。迁移期间不要让旧服务继续连接，新库也不能再供旧服务使用。重连会结束该连接上仍在运行的任务。

### 2.3 manager_session_id

`start_agent`、`run_round`，以及交出（`handover(release=true)`）或接手（`handover(workspace_id=...)`）工作区时，需要聊天身份：传 `manager_session_id`，或由 §2.1 的环境变量提供。无参 `handover()` 只查询待接手列表，不需要它。`wait` 指定 `ids` 时会忽略 `manager_session_id`；省略 `ids` 时，它才用于筛选当前服务的收件箱。服务对未知宿主的提示（`manager_identity.py` `session_id_hint`）是：先从宿主上下文或 shell 读取本对话的原生会话 ID；**宿主一个都没提供时，生成一个 UUID，并在这个对话里一直用它**。

bot 一般拿不到原生会话 ID，所以：

```bash
python3 -c "import uuid;print(uuid.uuid4())"   # 只生成一次
```

为每个聊天保存各自的 UUID，并在该聊天的调用中复用；不同聊天不能共用，调用时也不能重新生成，否则收件箱、续接和交接都会对不上。

## 3. 不用 App，手写配置

下面的配置文件都放在数据根目录（默认 `~/.agentnave/`），均为 `version: 1`。设过 `AGENTNAVE_DATA_DIR` 时，连接器也要传入同一目录。动手前先创建目录并备份：

```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cp -a "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}" "$HOME/agentnave-backup-$(date +%m%d%H%M)"
```

### 3.1 members.json：组员（`members.py` `MemberFile`/`CLIConfig`/`ModeCLI`/`MemberConfig`，严格模式，多写字段会报错）

```jsonc
{
  "version": 1,
  "clis": {                         // 各 CLI 已知的模型 → MemberConfig
    "pi": {"models": {"opencode-go/deepseek-v4.1-flash": {"notes": "评审、小修、测试、读代码"}}},
    // MemberConfig 可写的字段：efforts[]、effort、options{}（不能放 model/effort）、notes
    "codebuddy": {"models": {}},    // 空 = 用 CLI 自己的默认模型
    "deepseek":  {"models": {}}
  },
  "modes": {                        // 写了某个 mode 就是白名单：没列出的 CLI 在该 mode 下被拒
    "task": {
      "pi": {"models": ["opencode-go/deepseek-v4.1-flash"], "default": "opencode-go/deepseek-v4.1-flash"},
      "codebuddy": {"models": []},  // [] = 不限模型（list_members 显示 "*"）
      "deepseek":  {"models": []}
      // 不写 codex / antigravity = 暂停它们
    },
    "discussion": { /* 同上，用于 run_round 会议 */ }
  }
  // "defaults": {mode: {cli: model}} 只用于没写在 modes 里的、不受限的 mode
}
```

校验规则（`MemberFile.references`）：`models`、`overrides` 和 `default` 里的模型必须先在 `clis.<cli>.models` 里声明。ProviderName 只能取 `antigravity|claude|codebuddy|codex|deepseek|devin|grok|pi|zcode`。

### 3.2 orchestration.json：通用与专项工位（`members.py` `OrchestrationFile`；App 端 `apps/tray/src/members.rs` `validate_orchestration`；ADR-0037/0041/0065/0068/0069/0073/0083）

```jsonc
{
  "version": 1,
  "preset": "split",               // off | meetings | split | parallel | decision | custom；不是 off 和 meetings 才启用工位
  "meeting": {                      // 可选：开会默认人数（2–6）与席位（留空填 null，服务忽略）
    "count": 3,
    "seats": [{"provider": "codex"}, null]
  },
  "enabled_roles": ["explorer", "worker", "worker_frontend", "explorer_web"], // 可选：开启的角色，不写时只开通用探索、通用执行和界面制作
  "explorer": {                     // 通用探索（只读）
    "easy": {"provider": "pi",       "model": "opencode-go/deepseek-v4.1-flash"},
    "hard": {"provider": "deepseek", "model": "deepseek-flash"}
  },
  "worker": {                       // 通用执行（可写）
    "easy": {"provider": "pi",        "model": "opencode-go/deepseek-v4.1-flash"},
    "hard": {"provider": "codebuddy", "model": "deepseek-v4-pro"}
  },
  "worker_frontend": {              // 专项：界面制作；两档可以写同一个模型
    "easy": {"provider": "antigravity", "model": "gemini-4-argon-medium"},
    "hard": {"provider": "antigravity", "model": "gemini-4-argon-medium"}
  },
  "explorer_web": {                 // 专项：网页调研；没设的一档不提供，boss 改选同难度通用探索
    "hard": {"provider": "deepseek", "model": "deepseek-flash"}
  }
}
```

- 工位（`ReviewSeat`）必须有 `provider`、非空的 `model`，`effort` 可选。工位不能写“默认模型”，要填 CLI 实际的模型 ID。
- 工位可写 `fallbacks`，最多 3 项，每项含 `provider`、`model` 和可选 `effort`；新线程启动前主选不可用时按顺位尝试，显式指定 CLI 或模型、续接及启动后失败不切换。配额硬阻断按 CLI 声明的额度组冷却，无法对应时按模型或 CLI 冷却。
- `decision` 档使用数据根的 `decision-seats.json` 和 `decision-keys.json`，派发用 `seat="auto"` 并传 `access`；prompt 会发送给决策模型。工位目录及调用方式见 [`references/decision.md`](../skills/agentnave-manager/references/decision.md)，无 App 时需自行配置；文件结构与校验见 `src/agentnave/orchestration_roles.py` 的 `DecisionSeatsFile` 和 `src/agentnave/decision_model.py`。
- **无头宿主不要写 `apps` 和 `effective_apps`**：`enabled()` 用 `host.app_id or host.name` 作 key。bot 两者都为空，`apps` 里找不到就回落到 `preset`；但只要文件里有 `effective_apps`，没有 `app_id` 的连接就一律视为关闭（ADR-0041）。
- 角色键：通用组 `explorer`、`worker`；专项组 `worker_frontend`（界面制作）、`worker_writing`（写作）、`explorer_web`（网页调研）、`explorer_social`（社交平台调研）、`worker_documents`（办公文件制作）。每个角色管什么写在 `src/agentnave/orchestration_roles.json` 的 `scope_zh`，`list_members.orchestration.scopes` 会列出开启角色的范围。`worker_code`、`worker_data` 已删除，写了会被 App 拒绝。
- 专项只提供自己绑定的档，不沿用通用工位；App 里「区分难度」关闭时，两档存的是同一个模型。
- 各档规则文字与拼装顺序随 Python 包发在 `agentnave/orchestration_rules.json`（用 `"$(uv tool dir)/agentnave/bin/python" -c 'from pathlib import Path; import agentnave; print(Path(agentnave.__file__).with_name("orchestration_rules.json"))'` 查看路径；角色目录数据在同目录 `orchestration_roles.json`）。有 App 时由 App 按其中的 `order.<档位>` 把正文写进宿主的全局规则文件；bot 宿主没有这一步：非只开会档安装 `agentnave-manager` 后，没有规则块时的默认行为见该 skill 的 [`references/headless.md`](../skills/agentnave-manager/references/headless.md)；只开会档安装 `agentnave-meeting`，它没有这个参考文件，须按下述方法手写规则。要用决策模型档、并行分工、boss 可主动开会、`lead_only`（boss 只负责编排）、「只开会」（`meetings`，对服务算 `orchestration.enabled=false`）或 `meeting`（`count` 为 2–6，`seats` 可选且长度不超过 `count`，每席为 `{"provider": ...}` 或 `null`，模型与档位由服务按开会场景的员工设置补上）默认席位时，须经用户确认后，按 `orchestration_rules.json` 的 `order.<档位>` 从 `sections`、`switches`、`meeting_defaults`、`enabled_roles` 拼出对应文字放进 bot 的指令入口；`orchestration.json` 里的开关字段和 `meeting` 只供 App 生成规则块，bot 不读这些字段，只按指令入口里的文字执行。只开会档的「boss 可主动开会」默认关闭，升级后不再主动开会；要恢复，在 `orchestration.json` 设 `"meetings_options": {"meetings": true}`，并按 `order.meetings` 将对应的开启文字写入 bot 指令入口。
- 派发：`start_agent(seat="explorer-easy", role="具体职责", cwd=..., prompt=...)`，不传 `provider`、`access` 和模型，由服务补齐（Explorer 自动 `access=read`）。

### 3.3 review.json：agentnave-review 评审工位（`members.py` `ReviewFile`/`ReviewRole`）

```json
{"version": 1, "seats": {
  "correctness-reviewer":      {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "architecture-reviewer":     {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "security-reviewer":         {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "performance-reviewer":      {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "test-reliability-reviewer": {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "simplicity-reviewer":       {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "specialist-reviewer":       {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"},
  "adversarial-verifier":      {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"}
}}
```

只有这 8 个角色名有效；可以只写其中一部分。

### 3.4 permissions.json：权限档位（`members.py` `PermissionFile`/`PERMISSION_TIERS`）

```json
{"version": 1, "clis": {"pi": "auto", "codebuddy": "auto", "deepseek": "auto"}}
```

- 只有 `auto` 和 `full` 两档，没写的 CLI 按 `full`。用户要求把某个 CLI 改为自动时才写 `auto`。
- 注意：有几家 CLI 的 `auto` 本身就是全权限（源码注释：这几家没有可用的自动审批器）。`codebuddy` 的 auto 是 `bypassPermissions`，`deepseek` 是 `danger-full-access`，`antigravity` 是 `dangerously_skip_permissions`，`zcode` 是 `yolo`，所以 `list_members` 会把它们显示为 `"full"`。只有 claude、grok、codex、devin 的 auto 比 full 低（codex auto 是 `approve_for_me`，devin auto 是 `smart`）。Pi 没有档位。
- `list_members.permissions` 返回全部 8 个有档位的 CLI（claude、grok、codex、codebuddy、deepseek、devin、antigravity、zcode），不按已选组员过滤，也不含 Pi。
- 调用方传了 `PERMISSION_KEYS` 里的任何参数，档位就不再生效，所以 bot 不要自己追加权限参数。

### 3.5 其余可选文件

- `settings.json`：`{"version":1,"max_running":8}`，同时运行的 CLI 数量上限，取值 1–32；`history_retention_days` 取 30、90 或 180，省略或写 `null` 表示永久保留（`read_max_running`、`read_history_retention_days`）。
- `cli-defaults.json`：App 检测到的各 CLI 默认模型，可以不写。

Codex 在非 Git 目录也可直接派发，无需额外配置；1.0 忽略旧的 `cli-options.json`。

配置改完不用重启：派发设置在下次调用时读取，历史清理设置在服务启动或下一轮清理时读取。

## 4. 安装自带 skill

按档位安装 `agentnave-manager`（调用、开会、续接）或 `agentnave-meeting`（只开会）；`agentnave-review`（分级评审）可选。两者不同时装（ADR-0091）；切档时先备份并移走另一份。包里还带了 `agentnave-handover`（桌面 App 宿主交接工作区用），Linux 上不需要装。复制到 bot 的 skill 仓库读取的目录，每个 skill 一个目录，目录里有 `SKILL.md` 和 `references/`：

```bash
SRC=~/.local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave/skills
DEST=<bot 的 skill 目录>          # Grok Bot 见 §6.5
MAIN=agentnave-manager            # 只开会档改成 agentnave-meeting；先备份并移走另一份
for s in "$MAIN" agentnave-review; do
  [ -d "$DEST/$s" ] && cp -a "$DEST/$s" "/tmp/$s.bak.$(date +%s)"   # 先备份旧版
  rm -rf "$DEST/$s" && cp -a "$SRC/$s" "$DEST/$s"
done
```

每次升级 AgentNave 都要重新复制一遍，因为 skill 和服务的版本要一致。

## 5. 自检

### 5.1 list_members

调用 `list_members`（不带参数），检查：

- `configured: true`，`members` 里有 pi、codebuddy、deepseek
- 分工档下 `orchestration.enabled: true`，开启的角色和工位都在，`orchestration.scopes` 列出它们的范围；只开会档为 `false`，决策档检查自建工位目录
- `review` 有 8 个角色，`permissions` 符合预期
- 各 `*_error` 字段为空

### 5.2 派一个小任务

```
start_agent(prompt="回复：好了", seat="explorer-easy", role="自检",
            cwd="/workspace", manager_session_id="<本聊天的 UUID>")
wait(ids=["<invocation_id>"], wait_seconds=270, include_output=true)
```

此类 bot 的 `wait_seconds` 取 0 或 270；Claude Code 终端交互会话另支持 1500。一直等到 `status` 是 `succeeded`、`failed` 或 `cancelled`，并且 `all_finished: true`。

### 5.3 连接器坏了时：独立 stdio 客户端

用 AgentNave 自带 venv 里的 `mcp` 包，**在同一个连接里**完成 start 和 wait：

```python
# ~/.local/share/uv/tools/agentnave/bin/python check.py
import asyncio, json
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

SID = "<本聊天的 UUID>"
txt = lambda r: "\n".join(getattr(c, "text", "") for c in r.content)


async def main():
    p = StdioServerParameters(command="/home/<user>/.local/bin/agentnave-mcp", args=[])
    async with stdio_client(p) as (r, w), ClientSession(r, w) as s:
        await s.initialize()
        print(txt(await s.call_tool("list_members", {})))
        st = json.loads(
            txt(
                await s.call_tool(
                    "start_agent",
                    {
                        "prompt": "回复：好了",
                        "seat": "explorer-easy",
                        "role": "自检",
                        "cwd": "/tmp",
                        "manager_session_id": SID,
                    },
                )
            )
        )
        while True:
            w_ = json.loads(
                txt(
                    await s.call_tool(
                        "wait",
                        {
                            "ids": [st["invocation_id"]],
                            "wait_seconds": 270,
                            "include_output": True,
                        },
                    )
                )
            )
            if w_.get("all_finished"):
                print(w_)
                break


asyncio.run(main())
```

参考结果（2026-11-03 在本机实测）：`provider: pi`，`defaults_applied` 里有 `explorer-easy seat`、`access (explorer-easy seat)`，约 5 秒后 `succeeded`，输出“好了”。

## 6. Grok Bot 接入

本节把前面的通用步骤落到 Grok Bot 上。打 ⚠️ 的地方尚未逐项核实。

### 6.1 安装位置
Grok Bot 的 box 是一台 Linux 机器，同一用户的所有 bot 共用它，文件、工具和登录状态也都共享。AgentNave 按 §1.1 用 uv 装在 box 上，二进制是 `/home/box/.local/bin/agentnave-mcp`。`~/.agentnave/` 下的配置因此也对所有 bot 生效。

### 6.2 添加连接器
- 在聊天里让 bot 用它的 `AddMcpServer` 工具添加连接器：`name` 填连接器名（如 `agentnave`），`command` 填完整路径 `/home/box/.local/bin/agentnave-mcp`，不带参数，传输方式为 stdio。不要只写 `agentnave-mcp`，宿主的 PATH 里不一定有 `~/.local/bin`。
- 添加以后，宿主会把它显示为 `user-<name>`，例如 `agentnave (server id: user-agentnave) · transport=stdio · tools=9`。
- 添加用 bot 的 `AddMcpServer`，参数是 `name`、`command`（填完整路径 `/home/box/.local/bin/agentnave-mcp`），需要时再加 `args`、`env`；bot 会先请用户确认。这个命令也会在用户的其他 bot 里运行。新工具要到下一条消息才会出现。
- 实测（北京时间 2026-11-03 18:46）：`AddMcpServer(name="agentnave", command="/home/box/.local/bin/agentnave-mcp")`，不需要 `args` 和 `env`，加完马上显示 connected、9 个工具，server id 为 `user-agentnave`，同一轮里就能调用。

### 6.3 连接器命名与升级
- 名字固定用 `agentnave`，不加版本后缀。升级与重连按 §2.2；Grok Bot 可用 `RestartMcpServers` 重连，再检查工具是否刷新。
- 确需移除时，用 bot 的 `UninstallMcpServer`，传 `server_id`（即 `GetMcpServerStatus` 里显示的 id，如 `user-agentnave`）。这一步会删掉连接器，所以 bot 只会在用户亲手打字要求移除时执行；点按钮、回答「是」都不算。这类连接器不属于插件，不适用 `UninstallPlugin`。

### 6.4 manager_session_id
- Grok Bot 不在 `AGENTNAVE_HOST` 的取值范围里，也没有原生会话 ID 的环境变量。所以**每个 bot 聊天生成一个 UUID，长期复用**，写进该 bot 的记忆或 skill。
- `start_agent` 会返回 `Boss model unknown` 警告，原因见 §8.4。不想看到它，就同时传 `manager_model`；它不影响执行，也不要因为它重复调用。

### 6.5 安装 skill
Grok Bot 的全局 skill 仓库在 `/home/box/agent-data/workflows/<name>/SKILL.md`，所有 bot 共用。从 v1.11.0 标签安装：

```bash
TAG=v1.11.0; DEST="$HOME/agent-data/workflows"; TMP=$(mktemp -d)
git clone -q --depth 1 --branch $TAG https://github.com/TimWongUp/agentnave.git "$TMP/an"
MAIN=agentnave-manager            # 只开会档改成 agentnave-meeting；先备份并移走另一份
for s in "$MAIN" agentnave-review; do
  [ -d "$DEST/$s" ] && mv "$DEST/$s" "/workspace/$s-backup-$(date +%m%d%H%M)"
  cp -a "$TMP/an/skills/$s" "$DEST/$s"
done
rm -rf "$TMP"
```

（已经有本地仓库的话，也可以用 `git -C <repo> archive v1.11.0 skills/ | tar -x -C "$TMP"` 导出，或者直接从 §4 的 uv 安装目录复制。）

### 6.6 编排
只用默认的分工协作时，不需要另写宿主规则：按 §3.2 写好 `orchestration.json`（`preset` 设为 `split`，且不写 `apps` 和 `effective_apps`），再装上 `agentnave-manager`（§6.5），bot 在看不到规则块时会按 `agentnave-manager` 的 [`references/headless.md`](../skills/agentnave-manager/references/headless.md) 执行，从 `list_members.orchestration` 选工位传 `seat`。要用决策模型、只开会、并行分工、主动开会、`lead_only` 或开会默认席位时，在用户确认后按 `orchestration_rules.json` 的 `order` 拼出规则文字放进 bot 的指令入口（§3.2）。

### 6.7 一个连接里完成 start 和 wait
- 派出的 CLI 只在它所属的 MCP 连接里存活（§0、§8.6）。
- 用 Grok Bot 的连接器调用时，同一个对话轮次里先 `start_agent`、再 `wait` 按理可行。本次实测用的是独立客户端：在同一个连接里，start 和 wait 都顺利完成。实测：通过 Grok Bot 连接器分两次调用，先 `start_agent`、再 `wait`，任务正常 succeeded（约 4.7 秒），说明两次调用之间服务进程保持不变。跨对话轮次能否保持还没测，遇到 `cancelled` 或 `server_restarted` 就说明连接被重启过。
- 连接器坏了（schema 过期、调用失败）时，改用独立客户端 `/workspace/an-client/client.py`。它在一个进程里完成 `list_members`、`start_agent` 和 `wait`：

```bash
PY="$HOME/.local/share/uv/tools/agentnave/bin/python"
$PY /workspace/an-client/client.py list   # 只查组员
$PY /workspace/an-client/client.py run    # 派 explorer-easy 并等到终态
```

### 6.8 可选：Auto-review 放行规则
Grok Bot 的部分 shell 和 MCP 调用会先经过 Auto-review 检查。按本文配置，权限档位由服务从 `permissions.json` 自动补齐，bot 不需要传危险的权限参数。若你确实要显式传 `dangerously_bypass_approvals_and_sandbox` 这类参数，被拦截后可以在设置里保存一条放行规则（Settings → General → `auto-review-rules`）。⚠️ 规则的具体写法没有核实。规则范围要尽量窄，只放行 `agentnave-mcp` 的调用。

## 7. Grok Bot 这边的服务设置（替代 App 设置页）

托盘 App 设置页里的每一项，在 Linux 上都要靠改 `~/.agentnave/` 下的文件来完成。服务在相关调用时读取派发设置，改完通常不用重启或重连；历史清理等例外见下表。

### 7.0 通用做法

```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cd "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cp -a members.json members.json.bak.$(date +%m%d%H%M)      # 1) 先备份要改的文件
python3 - <<'PY'                                             # 2) 用 python（或 jq）改
import json

p = "members.json"
d = json.load(open(p))
# …… 修改 d ……
json.dump(d, open(p + ".tmp", "w"), ensure_ascii=False, indent=2)
PY
mv members.json.tmp members.json                             # 3) 原子替换
```

然后调用 `list_members` 检查：`configured`、各 `*_error` 字段为空、内容符合预期。文件写坏了，服务会直接报错，不会悄悄回退到别的设置（`_read_app_file`）。这时把备份 `mv` 回去即可。

### 7.1 设置对照表

| App 设置（页面） | 文件 / 键 | 出处 | 校验 | 重启/重连 |
|---|---|---|---|---|
| 员工、各模式白名单、默认模型 | `members.json`：`clis.<cli>.models`、`modes.{task,discussion}.<cli>.{models,default,overrides}`、`defaults` | `members.py` 的 `MemberFile`；`apps/tray/src/members.rs` | `list_members.members`/`restricted` | 不需要 |
| 某个模型的默认档位和参数 | `members.json`：`clis.<cli>.models.<m>.{efforts,effort,options,notes}` | `MemberConfig`（`options` 里不能放 model/effort） | `list_members` 的组员 `options`/`effort` | 不需要 |
| 每个 CLI 的权限档位 | `permissions.json`：`clis.<cli>` = `auto`/`full` | `PermissionFile`、`PERMISSION_TIERS` | `list_members.permissions`（auto 本身就是全权限的 CLI 会显示为 full） | 不需要 |
| 编排档位（关闭/只开会/分工协作/并行分工/决策模型/自定义） | `orchestration.json`：`preset`、`custom`（自定义正文）、`split_options.meetings`（分工协作下 boss 可主动开会，默认关）、`parallel_options.meetings`（并行分工下 boss 可主动开会，默认开）、`decision_options`（决策档并行写入与主动开会开关）、`meetings_options.meetings`（只开会档下 boss 可主动开会，默认关）、`lead_only`（分工协作与并行分工共用，默认关；打开后小事也派出）、`meeting`（开会默认人数与席位）、`apps`/`effective_apps` | `OrchestrationFile`；`orchestration_rules.json`；`members.rs` 的 `validate_orchestration`；ADR-0041/0073/0083 | `list_members.orchestration.enabled`（`off` 与 `meetings` 为 `false`） | 不需要；**不要写 `apps` 和 `effective_apps`**（§3.2）；开关、`meeting` 和 `meetings` 正文只影响 App 写出的规则块，无头宿主按 `orchestration_rules.json` 与 `references/headless.md`（§3.2）写指令入口 |
| 通用与专项工位、角色开关 | `orchestration.json`：`explorer`、`worker` 与五个专项键的 `.{easy,hard}` → `{provider,model,effort?,fallbacks?}`；`enabled_roles` | `ReviewSeat`；`orchestration_roles.json` | `list_members.orchestration`（含 `scopes`） | 不需要 |
| 评审工位（技能 → agentnave-review） | `review.json`：`seats.<role>` | `ReviewFile`；`ui/skills.js`、`ui/advanced.js` | `list_members.review`/`review_error` | 不需要 |
| 评审门（是否启用 agentnave-review） | **没有对应的服务配置文件**。App 会把这个 skill 按宿主开关（`ui/skills.js`，`/app/hosts/skill`），bot 这边就是装或卸 skill 目录（§6.5） | `apps/tray/src/hosts.rs` | 看 bot 的 skill 列表 | bot 下次读 skill 时生效 |
| 同时运行上限（默认 8） | `settings.json`：`max_running`，1–32 | `settings.rs`；`members.py` 的 `read_max_running`；`conversations.py` 在每次启动前读取 | 超出时报 `CapacityError`；`version` 或 `max_running` 非法时派发报 `SettingsFileError` 并给出文件路径。`list_members` 不显示这个值 | 不需要 |
| 历史自动清理 | `settings.json`：`history_retention_days`，取 `null`/30/90/180 | `settings.rs`；`retention.py`（服务启动时执行一次，之后每 24 小时一次） | `version` 或 `max_running` 非法会阻止派发并关闭清理；仅保留天数非法时关闭清理、记录 warning，派发仍可用 | 下一轮清理时生效，最长要等 24 小时；想立刻生效就重启服务 |
| 手动清理或重命名对话 | 没有对应文件。App 直接改 `agentnave.db`，同时持有对话锁（`manage.rs`，ADR-0012/0026） | — | — | ⚠️ 不建议手改数据库 |
| CLI 默认模型检测 | `cli-defaults.json`（App 检测到后写入，可以不写） | `detect.rs` 的 `NATIVE_FILE`；`members.py` 的 `cli_default_model` | `defaults_applied` | 不需要 |
| boss 全局规则文件 | 有 App 时按 `orchestration_rules.json` 写进各宿主的规则文件；无头宿主默认见 `agentnave-manager` 的 `references/headless.md`，其他档位按 `orchestration_rules.json` 写进 bot 指令入口（§3.2、§6.6） | `orchestration_rules.json` | — | — |
| 宿主接入（MCP 配置、skill 开关） | App 会改各宿主自己的配置文件（`hosts.rs`）。bot 这边就是 §6.2 和 §6.5 | — | — | 重新添加连接器 |
| 语言、外观、模型名称显示（简短/完整）、头像、提醒、语音、开机自启 | 只给 App 自己用：`preferences.json`、`avatars.json` 放在 App 的配置目录（`preferences.rs`、`avatars.rs`、`ui/*-settings.js`），**服务不读** | — | — | 无头环境下没有效果 |
| 版本与更新 | App 自带的更新器（`update.rs`）。bot 这边就是 §1.1 的 `uv tool install --force` | — | `list_members` 能正常调用 | App 安装需重连；uv 原地升级空闲切换；跨 1.0 升级先断开旧服务（§2.2） |

⚠️ 未核实：App 界面每一项和上表的对应关系，是按 `ui/*.js` 文件头注释和 `settings.html` 的标题整理的，没有逐个按钮核对；以 Rust/Python 里的校验函数为准。

### 7.2 示例

把并发上限改成 4：
```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cd "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"; [ -f settings.json ] && cp -a settings.json settings.json.bak.$(date +%m%d%H%M)
jq -n --argjson old "$(cat settings.json 2>/dev/null || echo '{}')" '$old + {version:1, max_running:4}' > settings.json.tmp && mv settings.json.tmp settings.json
```

换掉 `worker-hard` 工位：
```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cd "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"; cp -a orchestration.json orchestration.json.bak.$(date +%m%d%H%M)
jq '.worker.hard = {"provider":"pi","model":"opencode-go/deepseek-v4.1-flash"}' orchestration.json > o.tmp && mv o.tmp orchestration.json
```

暂停一个 CLI（从白名单里去掉）：
```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cd "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"; cp -a members.json members.json.bak.$(date +%m%d%H%M)
jq 'del(.modes.task.codebuddy, .modes.discussion.codebuddy)' members.json > m.tmp && mv m.tmp members.json
```

每改一次都调用一次 `list_members` 检查。

### 7.3 改之前要先问用户

- **权限档位**（`permissions.json`，尤其是改成 `full`），以及往调用参数里加任何权限类参数：必须用户明确同意。
- **启用或恢复 CLI**（往 `modes` 里加回去，或者从 `AGENTNAVE_EXCLUDED_PROVIDERS` 里去掉）：要问。用户暂停过的 CLI（目前是 Codex 和 Antigravity），要等用户说恢复才动。
- 开启编排（把 `preset` 从 `off` 改成别的），等于授权 bot 主动派活，也要先问。
- 改历史保留天数会删掉旧记录，同样要先问。
- 其他像换工位模型、调整并发上限这类小改动，在用户已确认的任务范围内可以直接做，做完告诉用户。

## 8. 常见报错

### 8.1 连接器 schema 过期
先按 §2.2 确认升级方式并重连宿主，再调用 `list_members` 检查。Grok Bot 的操作见 §6.3；连接器暂时不可用时可用 §5.3 的独立客户端检查。

### 8.2 Codex：`workspace routing discovery failed`
Codex 用 ChatGPT 账号登录后，会去后台查询账号所属的工作区，这次请求临时失败了，本地配置没有问题。等一会儿重试即可；一直失败的话重新 `codex login`。

### 8.3 Antigravity：`User location is not supported`
机器出口 IP 所在的地区不在服务范围内，改 AgentNave 配置解决不了。在 `members.json` 的 `modes` 里不写它，或者设置 `AGENTNAVE_EXCLUDED_PROVIDERS=antigravity`。

### 8.4 `Boss model unknown` 警告
`start_agent` 返回 `warnings: ["Boss model unknown: ..."]`（`manager_identity.py` `model_warnings`）。这是因为服务从会话记录里推断不出 boss 用的是什么模型。bot 宿主**即使传了固定的 manager_session_id 也会出现这条警告**（实测）。它只是提醒，任务照常运行；想消掉它就同时传 `manager_model="<bot 名称>"`。**不要因为这条警告重复调用**，提示原文也说了 “Do not repeat this call”。

### 8.5 OpenAI 网络安全过滤
Codex 会返回 `provider_error`，内容是 cyber-risk 拦截。原因是任务内容（例如复现漏洞、写利用代码）被 OpenAI 判为网络安全风险，和 AgentNave 无关。不要改写 prompt 去绕过拦截，把任务交给别的 CLI（例如 Pi）或者 bot 自己做。

### 8.6 任务被 cancelled，或 `server_restarted; previous invocation cannot resume`
发起任务的 stdio 连接已经退出。start 和 wait 要放在同一个连接里（§0、§5.3）。

### 8.7 `... is not allowed in task; allowed members: ...`
这个 CLI 不在 `members.json` 的 `modes.task` 白名单里。如果是故意暂停的，这条报错就是预期结果。
