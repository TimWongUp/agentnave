# Linux 装完 AgentNave 后：bot 带用户完成设置的脚本

**读者是 bot**（例如 Grok Bot）。你刚按用户要求在 Linux 上装好了 AgentNave，现在按下面的步骤带用户一步步完成设置。**只问两类问题**：一是要接入哪些 CLI、要不要开编排、要不要装评审 skill；二是**所有跟模型有关的选择**（默认模型、思考档位、工位用哪个模型、评审角色用哪个模型），这些必须问用户，不能替用户定。另外只有 CLI 没登录时才问一次要不要登录。权限、清理、并发等其余设置直接用默认值，最后在第 10 步的总结里列出所有默认值，告诉用户以后想改随时说。字段细节见 [linux-headless.md](linux-headless.md)，下文的“§”都指这份文档。

## 总规则

- **一次只问一件事**，每个问题都给具体选项，选项按本机实际检测到的结果列，并标出推荐项。
- 下文问题里的 CLI 名和模型名（如 `<CLI>`、`<模型>`）都是占位，必须换成用户本机检测到、用户接入的那些。**不要照搬任何一台机器的组合**，也不要推荐用户没装或没接入的 CLI。
- 除模型相关的选择外，有推荐默认值的设置不要问，直接用默认值；用户主动提出要改时再改。模型相关的必须问，问的时候标出推荐项。
- 用户没回答第 3 步之前，**不要启用任何 CLI**；权限只写默认的 `auto`，用户没明确要求之前绝不写 `full`。用户暂停过的 CLI，要等用户明确说恢复才动。
- 登录只有两种做法：把桌面交给用户，让用户自己扫码或登录；或者走 CLI 自带的登录流程。**绝不在聊天里要 token 或密码**。API key 一律用密钥表单收，写进文件后 `chmod 600`，永远不要回显。
- 数据根目录默认 `~/.agentnave/`；设过 `AGENTNAVE_DATA_DIR` 时，连接器也要传入同一目录。每次改配置前，先创建目录并备份：
  ```bash
  mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
  B="$HOME/agentnave-backup-$(date +%m%d%H%M)"
  mkdir -p "$B"
  cp -a "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/." "$B/"
  echo "$B"
  ```
- 改文件用 python 或 jq，先写到 `.tmp` 再 `mv` 过去。服务每次调用都会重新读文件，**改完不用重启**。
- 文中的验证都用连接器的 `list_members`。连接器暂时用不了时，改用独立客户端做检查（§5.3）。Grok Bot 的命令是：
  `~/.local/share/uv/tools/agentnave/bin/python /workspace/an-client/client.py list`

## 0. 确认安装

自己执行：
```bash
which agentnave-mcp; uv tool list | grep agentnave
```
验证：二进制在 `~/.local/bin/agentnave-mcp`，版本和预期一致（例如 v1.0.0）。不对的话按 §1.1 重装，这一步不用问用户。

## 1. 接入宿主

用户让你装 AgentNave 就已经包含接入，这一步不用问，直接执行：
- 先看宿主里是否已有 `agentnave` 连接器：已有且 `connected`、工具数对，就直接跳到确认聊天身份；已有但工具对不上，先按 §2.2 重连并检查。
- Grok Bot：用 `AddMcpServer`，名字填 `agentnave`，命令填 `/home/box/.local/bin/agentnave-mcp`（写完整路径）。
- 其他宿主：添加 stdio 服务器，`{"command":"/home/<user>/.local/bin/agentnave-mcp","args":[]}`。
- 先读取当前聊天的原生会话 ID；只有宿主不提供时，才生成一个 UUID，作为本聊天固定的 `manager_session_id`（§2.3、§6.4）。也可按聊天注入 `AGENTNAVE_HOST_SESSION_ID`，不要跨聊天复用：
  `python3 -c "import uuid;print(uuid.uuid4())"`
- 升级按 §2.2：App 安装后需宿主重连，uv 原地升级会在空闲时切换。从 1.0 以前升级时，先断开同一数据目录的所有旧服务连接并备份，再升级、重连；新服务启动会迁移会话库到 v2，并删除旧身份记录及关联数据和席位目录，迁移期间不要让旧服务继续连接。无需例行删除连接器。

验证：连接器状态显示 `connected`，有 9 个工具。

## 2. 检测已安装的 CLI

自己执行，只看装没装，这一步不检查登录：
```bash
for c in pi codebuddy dsh codex agy claude grok zcode; do printf "%-10s " $c; command -v $c || echo "未安装"; done
```
注意命令名和组员名不一样的：`dsh` 在 AgentNave 里叫 `deepseek`，`agy` 叫 `antigravity`。写配置时用组员名（§3.1）。

## 3. 选组员

问用户：「检测到已安装：<CLI 列表>。要接入哪些？」只列本机检测到的 CLI，不预设推荐组合。
用户暂停过的 CLI 标注“已暂停”，不默认勾选。没装的 CLI 不列出；用户点名要用时再装（§1.2）。

按回答写 `members.json`：没选的 CLI 不写进 `modes`，就等于暂停。 这段写法会整份重写文件，只适合首次设置；之后修改用 [linux-headless.md §7](linux-headless.md) 的增量写法。
```bash
python3 - <<'PY'
import json
import os
from pathlib import Path

root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
root.mkdir(parents=True, exist_ok=True)
chosen = ["<组员1>", "<组员2>"]  # ← 用户的回答（组员名）
d = {
    "version": 1,
    "clis": {c: {"models": {}} for c in chosen},
    "modes": {m: {c: {"models": []} for c in chosen} for m in ("task", "discussion")},
}
json.dump(d, open(root / "members.json.tmp", "w"), indent=2)
(root / "members.json.tmp").replace(root / "members.json")
PY
```
验证：`list_members` 返回 `configured: true`，`members` 里正好是选中的这些 CLI。

## 4. 逐个设置接入的 CLI

只对第 3 步选中的 CLI 做，一次处理一个 CLI；之后用户新接入 CLI 时，也只对新接入的那个走这一步。没接入的 CLI 不检查、不提问。

### 4a. 登录

自己执行一次“回复：好了”，能正常回答就算已登录，直接跳到 4b，不用问用户：
- `pi -p "回复：好了"`
- `codebuddy -p "回复：好了"`
- `dsh --profile headless --json "回复：好了"`（需要 Node ≥ 22.19，不够的话单独装一份 Node 22 到 `~/.local/node`，软链到 `~/.local/bin`，见 §1.2）
- `codex exec "回复：好了"`
- 其他 CLI（claude、grok、agy、zcode）用各自的非交互模式（`-p` 或 `exec`）跑同一句，见 §1.2。

没登录时才问用户：「<CLI> 还没登录，现在登录吗？」
- 要扫码或网页登录的 CLI，先运行它自己的登录命令，再把桌面交给用户。例如 `codebuddy` 要微信扫码，`codex login` 走网页授权。
- dsh 的 key 用密钥表单收，写进 `~/.dsh/.env`。
- 用户不登录，就把这个 CLI 从 `members.json` 里去掉。

验证：再跑一次“回复：好了”，能回答就算登录成功。

### 4b. 默认模型

自己执行：查出这个 CLI 能用的模型和它的默认模型。
- codebuddy：`codebuddy --help`，看 `--model` 那一行。
- dsh：`dsh --profile headless --dump-config`，看 `agent-default-model`。
- pi：`pi --list-models`，写配置时用 `provider/model` 格式，例如 `opencode-go/deepseek-v4.1-flash`。

问用户（每次只问一个 CLI）：「<CLI> 默认用哪个模型？①<CLI 自己的默认模型>（推荐）②<其他可用模型>…」选项按上面查到的实际可用模型列，CLI 自己的默认模型标为推荐。

按回答修改（下面以 pi 为例，换成实际的 CLI 和模型）：选了具体模型，就把它写进 `clis.<cli>.models.<m>`，并在两个模式里设好 `models` 和 `default`；选“CLI 自己的默认”，就保持 `"models": {}`（§3.1）。
```bash
jq --arg m "opencode-go/deepseek-v4.1-flash" '.clis.pi.models[$m]={} |
  (.modes.task.pi, .modes.discussion.pi) = {"models":[$m],"default":$m}' \
  "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/members.json" > /tmp/m.json && mv /tmp/m.json "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/members.json"
```
验证：`list_members` 里这个 CLI 显示了选定的模型，并且标为 `default`。

### 4c. 思考档位

只在 4b 选了具体模型、且这个模型支持多个档位时问；用 CLI 自己的默认模型就跳过。
问用户：「<CLI> 的 <模型> 默认用哪个思考档位？①CLI 默认（推荐）②…」列出的档位以该模型实际支持的为准。

按回答修改：选了档位就写 `clis.<cli>.models.<m>.effort`；选“CLI 默认”就不写（§3.1）。
```bash
jq --arg m "opencode-go/deepseek-v4.1-flash" '.clis.pi.models[$m].effort="high"' \
  "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/members.json" > /tmp/m.json && mv /tmp/m.json "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/members.json"
```
验证：`list_members` 里这个模型显示了选定的 `effort`。

## 5. 要不要用编排功能

先单独问用户：「要开启编排功能吗？开启后我会按任务难度，自动把只读的探索、评审交给 Explorer 工位，把实现交给 Worker 工位，不用每次点名用哪个 CLI；不开的话，每次派活我会先跟你确认用谁。①开启（推荐）②先不开」
开启编排就等于授权你按工位主动派活，这一点要在问题里说清楚。

用户选②：写 `{"version":1,"preset":"off"}` 到 `orchestration.json`，跳过本步剩下的内容。

用户选①，再问工位用哪个 CLI 和模型（一次给出整套方案，用户可以直接选或改）：「工位怎么分？
①推荐：探索·简单=<CLI>（<模型>），探索·困难=<CLI>（<模型>），实现·简单=<CLI>（<模型>），实现·困难=<CLI>（<模型>）
②全部用 <CLI>（<模型>）
③自定义」
推荐方案由你按用户接入的组员生成：简单工位用更便宜、更快的组员和模型，困难工位用更强的；只接入了一个 CLI 时，只给“全部用它”和“自定义”两个选项。
只能从第 3 步选中的组员和第 4 步定下的模型里组合；某个工位的 CLI 有多个可用模型时，要列出模型让用户选。

按回答写 `orchestration.json`（下面是示例值，换成用户选的）。工位必须填实际的模型 ID，**不要写 `effective_apps`**（§3.2）：
```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
cat > "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/orchestration.json" <<'J'
{"version":1,"preset":"split",
 "explorer":{"easy":{"provider":"pi","model":"opencode-go/deepseek-v4.1-flash"},
             "hard":{"provider":"deepseek","model":"deepseek-flash"}},
 "worker":{"easy":{"provider":"pi","model":"opencode-go/deepseek-v4.1-flash"},
           "hard":{"provider":"<组员>","model":"<模型 ID>"}}}
J
```
验证：`list_members.orchestration.enabled` 符合预期，4 个工位都在。

## 6. 要不要加评审功能（agentnave-review skill）

先单独问用户：「要额外装上 agentnave-review 吗？装了以后，代码评审会拆成正确性、安全、性能等 8 个角色分头审，更细，但更费额度。①装（推荐写代码多的时候用）②不装」

用户选②：不写 `review.json`，第 9 步也不装 agentnave-review，跳过本步剩下的内容。

用户选①，再问：「评审角色用哪个模型？①全部用 <最便宜的组员>（<模型>，推荐，省额度）②正确性、安全用 <更强的组员>（<模型>），其余用 <最便宜的组员> ③自定义」只接入了一个 CLI 时，不给②。

按回答写 `review.json`（示例值，换成用户选的），共 8 个角色，名字见 §3.3：
```bash
python3 - <<'PY'
import json
import os
from pathlib import Path

root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
root.mkdir(parents=True, exist_ok=True)
seat = {"provider": "pi", "model": "opencode-go/deepseek-v4.1-flash"}
roles = (
    "correctness architecture security performance test-reliability simplicity specialist".split()
)
d = {
    "version": 1,
    "seats": {**{r + "-reviewer": seat for r in roles}, "adversarial-verifier": seat},
}
json.dump(d, open(root / "review.json.tmp", "w"), indent=2)
(root / "review.json.tmp").replace(root / "review.json")
PY
```
验证：`list_members.review` 有这些角色，`review_error` 为空。

## 7. 权限档位

不问用户，第 3 步接入的每个 CLI 都写默认的 `auto`。用户没有明确要求之前，**不要写 `full`**（§3.4）。

```bash
python3 - <<'PY'
import json
import os
from pathlib import Path

root = Path(os.environ.get("AGENTNAVE_DATA_DIR") or Path.home() / ".agentnave")
root.mkdir(parents=True, exist_ok=True)
chosen = ["<组员1>", "<组员2>"]  # ← 第 3 步的组员
json.dump(
    {"version": 1, "clis": {c: "auto" for c in chosen}},
    open(root / "permissions.json.tmp", "w"),
    indent=2,
)
(root / "permissions.json.tmp").replace(root / "permissions.json")
PY
```
验证：`list_members.permissions` 返回全部 7 个有档位的 CLI，不限于已选组员，也不含 Pi。默认 claude、grok、codex 显示 `auto`；codebuddy、deepseek、antigravity、zcode 的 `auto` 档本身就是全权限，会显示为 `full`，这是正常的。

## 8. 其他设置

不问用户，直接写默认值：同时运行上限 8，历史自动清理 90 天。第 10 步总结里要说明超过 90 天的旧对话记录会被自动删除，可以改成永不、30 或 180 天。

```bash
mkdir -p "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}"
jq -n '{version:1,max_running:8,history_retention_days:90}' > "${AGENTNAVE_DATA_DIR:-$HOME/.agentnave}/settings.json"
```
验证：用 `python3 -m json.tool` 检查格式。`version` 必须是整数 1，`max_running` 必须是 1–32 的整数，否则派发报 `SettingsFileError` 并关闭清理；仅 `history_retention_days` 非法时关闭清理并记录 warning，派发仍可用。清理设置在服务启动或下一轮清理时生效，最长 24 小时（§7.1）。

## 9. 安装 skill

`agentnave-manager` 必装，不用问；`agentnave-review` 只在第 6 步选了装时才装。`agentnave-handover` 是给桌面 App 宿主交接工作区用的，Linux 上的 bot 用不上，不装。已有的旧版会先备份。

直接执行（第 6 步选了装时先设 `EXTRA=agentnave-review`，否则留空）：
```bash
EXTRA=""                                # 第 6 步选了装：EXTRA=agentnave-review
SRC=~/.local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave/skills
DEST="$HOME/agent-data/workflows"      # Grok Bot 的全局 skill 仓库；其他 bot 换成自己的目录
for s in agentnave-manager $EXTRA; do   # 第 6 步选了装时 EXTRA=agentnave-review
  [ -d "$DEST/$s" ] && mv "$DEST/$s" "$HOME/$s-backup-$(date +%m%d%H%M)"; cp -a "$SRC/$s" "$DEST/$s"; done
```
验证：`ls $DEST/agentnave-*/SKILL.md` 能列出装了的那几个（§6.5）。

## 10. 自检与总结

这一步不用问用户，直接执行：
1. 调用 `list_members`。期望：`configured: true`，各 `*_error` 为空。
2. 在同一个连接里先 start 再 wait：
   ```
   start_agent(prompt="回复：好了", seat="explorer-easy",   # 没开编排时改用 provider="<第 3 步选的第一个 CLI>"
               role="自检", cwd="/tmp",
               manager_session_id="<固定 UUID>")
   wait(ids=["<invocation_id>"], wait_seconds=600, include_output=true)
   ```
   期望：`succeeded`，输出“好了”（本机实测由 Pi 执行，大约 5 秒）。返回 `Boss model unknown` 警告是正常的（§8.4），不要因此重复调用。

最后用一段话向用户总结：
- 启用了哪些 CLI，各用什么模型
- 工位和评审工位怎么分配
- 权限档位（默认 auto）
- 历史自动清理 90 天、同时运行上限 8，说明旧记录会被删，可以改
- 一句话：权限、清理、并发这些默认值想改随时说
- 连接器名字
- skill 装在哪里
- 自检结果
- 备份目录

出问题时先看 §8；以后要改设置，按 §7 来。
