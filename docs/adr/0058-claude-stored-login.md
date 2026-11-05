# ADR-0058：Claude 子代理统一用 Claude Code 自己存的登录

- 状态：已接受
- 日期：2026-11-05

## 背景

Claude Code 启动子进程（包括 MCP 服务）时，会从环境里删掉自己的凭据变量，`CLAUDE_CODE_OAUTH_TOKEN` 在其中；在 MCP 配置的 `env` 里写 `${CLAUDE_CODE_OAUTH_TOKEN}` 也只展开成空字符串，普通变量照常展开。其他宿主（如 Antigravity）原样把环境传给 AgentNave。于是子代理 Claude 用哪种认证取决于 boss 是哪个宿主：Claude 当 boss 时用 Claude Code 存的登录（macOS Keychain，或 `~/.claude/.credentials.json`），其他宿主下可能用 boss 环境里的长期 token。

## 决定

- 服务启动 Provider 进程前，从子进程环境删掉 `CLAUDE_CODE_OAUTH_TOKEN`，与删宿主会话 ID 一样对所有 Provider 生效，`-p` 与交互模式相同。子代理 Claude 一律用 Claude Code 自己存的登录。服务自身的进程环境不变。
- AgentNave 不保存、不读取用户的 Claude 凭据：不从登录 shell 取 token，App 也不提供填写 token 的地方。
- Claude Code 没有存的登录时，按现有认证失败上报：`-p` 报认证失败，交互模式在登录屏报 BLOCKED；用户先在 Claude Code 里 `/login` 一次。
- `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN` 不删：它们常和 `ANTHROPIC_BASE_URL` 一起用于 API 计费或第三方网关，是用户有意为 CLI 选的通道。

## 取舍

- 只用长期 token、从没 `/login` 过的用户，在任何 boss 下派 Claude 都要先 `/login`；之前从不过滤凭据的宿主派活还能用。
- 存的登录在多个工具共用时可能因刷新冲突而过期，长期 token 不会；换来的是认证不随 boss 变化，也不必绕开 Claude Code 对子进程的凭据屏蔽。

## 考虑过的方案

- **统一用长期 token**：Claude 当 boss 时 token 已被删掉，AgentNave 拿不回来，只能另找来源。
- **服务从登录 shell 读 token**：对把 token 写进 shell 配置的用户开箱即用，但有意绕开 Claude Code 的屏蔽，每次派发多跑一次 shell 配置，读不到临时 export 的值，也没有先例。
- **App 保存用户粘贴的 token**（magpie 的做法）：需要让用户手填，AgentNave 开始管理密钥；Linux 只有服务、没有 App。
- **保持现状**：同一用户换个 boss，子代理就换一种认证，出问题时难以判断。
