# Contributing to AgentNave

Contributions are welcome through GitHub Issues and pull requests. Keep changes focused on
AgentNave's role as a local STDIO MCP adapter for CLI-based subagents; planning, orchestration,
review decisions, retries, permissions, and worktree management belong to the calling Agent Manager.
The core (`src/agentnave`, launcher `agentnave-mcp`) keeps no history, starts no web server and never
imports the studio; saved shows, review and the dashboard belong to `src/agentnave_studio` and must
keep public pages limited to committed entries; see
[the studio guide](docs/studio.md).

## Report an issue

Search existing Issues before opening a new one. For a bug, include the operating system, Python
version, provider CLI and version when relevant, a minimal reproduction, and the expected and
actual behavior. Redact credentials, provider output, local paths, and other private data.

Do not report security vulnerabilities in a public Issue. Follow [SECURITY.md](SECURITY.md)
instead.

## Set up the development environment

AgentNave requires Python 3.12 or later, `uv`, and Windows, macOS, or Linux.

```bash
git clone https://github.com/TimWongUp/agentnave.git
cd agentnave
uv sync --locked --all-groups
```

## Make a change

- Keep the change limited to one coherent behavior or documentation concern.
- Preserve provider-native defaults unless the caller explicitly supplies an allowlisted option.
- Add or update the smallest relevant test when behavior changes and existing coverage would not
  catch a regression.
- Automated tests must not require authenticated provider CLIs or consume provider quota. Follow
  the repository's existing fake-executable patterns instead.

Run the project checks before opening a pull request:

```bash
uv run ruff format --check .
uv run ruff check .
uv run pyright
uv run pytest
```

In the pull request, explain the user-visible change, the validation performed, and any remaining
limitations or platform-specific behavior.

## 静态 mock 自查（不启动托盘 App）

从仓库根目录运行以下命令，在临时目录建立静态路由布局；不要直接以 apps/tray/ui 为 HTTP 根目录，否则 /chat/common.js 等绝对路径会 404。此服务只提供静态资源，mock 动作使用浏览器内存，不访问本机真实宿主配置；刷新页面会重置动作结果。

```bash
preview_root=$(mktemp -d)
preview_home=$(mktemp -d)
for section in settings chat panel manage; do
  mkdir -p "$preview_root/$section"
  for asset in "$PWD"/apps/tray/ui/*; do
    ln -s "$asset" "$preview_root/$section/$(basename "$asset")"
  done
  ln -s "$PWD/apps/tray/ui/$section.html" "$preview_root/$section/index.html"
done
ln -s "$PWD/src/agentnave_studio/dashboard_assets/avatars.png" "$preview_root/avatars.png"
HOME="$preview_home" python3 -m http.server 8765 --bind 127.0.0.1 --directory "$preview_root"
```

打开 `http://127.0.0.1:8765/settings/?mock#hosts` 查看宿主卡与根目录覆盖；`/chat/?mock&hosts=fresh`、`/panel/?mock&hosts=fresh` 查看空态引导。`hosts=typical|midway|done` 分别提供异常/孤留、设置中间态、全部完成的数据；`scene=empty|quiet|live` 可单独控制群聊与舞台数据。mock 群链接使用 `?group=...`，不要求静态服务器实现路由回退。只有真实 App 的验收可以改变 tested 标记。

## Version releases

Prepare a release in a pull request: increase the stable `MAJOR.MINOR.PATCH` version in
`pyproject.toml` and `src/agentnave/__init__.py`, refresh `uv.lock` with `uv lock`, update the install
examples, and add `docs/releases/v<version>.md` with user-facing changes and migration instructions.
The CI metadata check rejects inconsistent versions, decreases, and missing release notes.

After the version change is merged into `origin/main`, the existing Python workflow waits for
Windows, macOS, and Linux verification jobs, then creates the version tag and GitHub Release at that
exact push commit. Ordinary changes without a version bump do not publish. The Git installation
channel remains canonical; this workflow does not publish to PyPI or upload binary distributions.

After a transient or external failure is resolved, rerun the original workflow. If recovery requires
code or release metadata changes, submit a new pull request with another version increase: reruns
use the original commit, and an ordinary same-version push does not retry an earlier release.
A published release at the same tag and commit is a no-op; a conflicting tag, missing tag for an
existing release, or existing draft requires manual review and is never overwritten. Confirm that
the release job and GitHub Release have completed before reporting a version as published.
