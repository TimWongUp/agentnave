# 各宿主的会话位置

以下路径是默认值，括号里是覆盖目录的环境变量。按修改时间找最近的；有会话 ID 时按 ID 找，不要换成另一个会话。

| 宿主             | 位置                                                                                                                             | 怎么认当前目录                       | 用户和 boss 的文字                                                                                              |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| Claude Code | `~/.claude/projects/<目录编码>/<ID>.jsonl`（`CLAUDE_CONFIG_DIR`） | 行内 `cwd`；目录编码把 `/` 和 `.` 都换成 `-` | `type` 为 `user`、`assistant` 的行，`message.content` 是字符串或 `text` 块；跳过 `isMeta`、`isSidechain` |
| Codex            | `~/.codex/sessions/年/月/日/rollout-*-<ID>.jsonl`（`CODEX_HOME`）；可能压缩成 `.jsonl.zst`，或移到 `~/.codex/archived_sessions/` | 首行 `payload.cwd`                   | `type` 为 `response_item`、`payload.type` 为 `message` 的行，`payload.role` 为 `user`、`assistant`              |
| Antigravity | `~/.gemini/antigravity-cli/brain/<ID>/.system_generated/logs/transcript_full.jsonl`；桌面端在 `~/.gemini/antigravity/brain/` | 同一根目录下 `conversation_summaries.db` 的 `workspace_uris` | `type` 为 `USER_INPUT` 的 `content` 里的 `<USER_REQUEST>`；`PLANNER_RESPONSE` 的 `content`，可能为空 |
| CodeBuddy        | `~/.codebuddy/projects/<目录编码>/<ID>.jsonl`（`CODEBUDDY_CONFIG_DIR`）                                                          | 行内 `cwd`                           | `type` 为 `message` 的行，看 `role` 和 `content`                                                                |
| WorkBuddy        | `~/.workbuddy/projects/<目录编码>/<ID>.jsonl`（`CODEBUDDY_CONFIG_DIR`）                                                          | 行内 `cwd`                           | 同 CodeBuddy                                                                                                    |
| Grok | `~/.grok/sessions/<URL 编码的目录>/<ID>/chat_history.jsonl`（`GROK_HOME`） | 目录名 | `type` 为 `user` 且带 `prompt_index` 的行是用户原话，文字在 `content[].text`；`assistant` 行是回复 |
| Pi               | `~/.pi/agent/sessions/--目录--/*_<ID>.jsonl`（`PI_CODING_AGENT_DIR`）                                                            | 目录名                               | `type` 为 `message`，看 `message.role`；会话是树，沿最后一行的 `parentId` 往回走才是当前分支                    |
| DeepSeek Harness | `~/.dsh/sessions/--目录--/session-<ID>/session.v*.jsonl.zstd`（`DSH_HOME`），用 `zstd -dc` 读                                    | 目录名                               | `user/message` 且 `data.source.kind` 为 `user` 的 `data.content`；`assistant/message` 的 `data.message.content` |
| Hermes           | `~/.hermes/state.db`（`HERMES_HOME`），表 `messages`，只读打开                                                                   | 表 `sessions`                        | 按 `session_id` 查，`role` 为 `user`、`assistant`，`active = 1`                                                 |
| ZCode | `~/.zcode/cli/db/db.sqlite`（`~/.zcode/cli/config.json` 的 `storage.sessionDbPath` 可覆盖），表 `message` 和 `part`，只读打开 | 表 `session` 的 `directory` | `message.data` 的 `semantics.origin` 为 `real_user` 是用户原话；文字在 `part.data` 里 `type` 为 `text` 的部分 |

Antigravity 的会话如果只有 `conversations/<ID>.db`，那是 protobuf，读不了，告诉用户。

Hermes、ZCode 的数据库没有 `-wal` 文件时，普通只读打开可能失败，改用 `file:<路径>?immutable=1`。
