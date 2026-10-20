# 办公室美术资源来源

`ui/art/` 下的角色立绘与办公室道具是为 AgentNave 原创设计的二次元 Q 版形象，2026 年 10 月由 Codex 的图像生成工具按统一规范绘制，再缩放为 WebP：

- `s/<角色>/<状态>.webp`：工位尺寸立绘，与 `office/` 道具共用 1536×1200 舞台的同一锚点（椅子 → 角色 → 桌子 → 笔记本四层叠放）。
- `m/<角色>/<状态>.webp`：对话栏立绘（320px）。
- `h/<角色>/<状态>.webp`：菜单栏圆形头像，按同一方框裁出头部（96px）。
- `logo-default.svg`：没有专属标志的模型或 CLI 在笔记本盖子上使用的默认终端标志。

角色按模型家族区分：claude、codex（GPT）、deepseek、gemini、glm、grok；没有专属形象的模型使用 `default`（「新同事」）。状态共 7 个：idle、blink、working、speaking、review、done、sleep。

设计参考了社区作品的比例与可爱程度，但造型、服装与配色均为原创，未使用各厂商的官方吉祥物。
