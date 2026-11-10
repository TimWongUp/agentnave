# ADR-0078：启动确认屏由服务自动处理

- 状态：已接受
- 日期：2026-11-10
- 修订 [ADR-0056](0056-claude-interactive-runtime.md) 的启动信任、[ADR-0057](0057-boss-answers-native-prompts.md) 的按键与持久放行边界，以及 [ADR-0072](0072-antigravity-interactive-runtime.md) 的信任与用户配置边界。

## 决定

boss 派发时指定 `cwd` 即授权启动该目录。Claude Code 与 Antigravity 的工作区信任屏显示的目录与 `cwd` 经文件系统核对为同一目录时，服务在交互准备阶段自动选择信任；不一致或无法核对时受阻。Claude Code 的外部 CLAUDE.md 导入审批屏自动选择允许，包括未展示和未来新增的导入，不检查路径。未知启动屏受阻，不猜按键。这些启动屏不产生 `prompt_pending`；运行时弹框沿用 ADR-0057。

确认后 CLI 会将选择永久写入各自用户配置：Claude Code 的 `~/.claude.json` 和 Antigravity 的 `~/.gemini/antigravity-cli/settings.json`。这是持久放行先问用户规则的限定例外；服务不写持久放行规则。

## 取舍

用户认为进入的仓库均由用户自己持有或授权拉取，自动确认避免启动卡住。导入批准涵盖未来项目新增的外部文件；CLI 版本变化可能影响屏幕识别，无法确认时受阻。目录同一性依赖文件系统，不能识别的路径不放行。
