# 办公室美术资源来源

`ui/art/` 下的角色立绘与办公室道具是为 AgentNave 原创设计的二次元 Q 版形象，2026 年 10 月由 Codex 的图像生成工具按统一规范绘制，再缩放为 WebP：

- `s/<角色>/<状态>.webp`：工位尺寸立绘，与 `office/` 道具共用 1536×1200 舞台的同一锚点（椅子 → 角色 → 桌子 → 笔记本四层叠放）。
- `m/<角色>/<状态>.webp`：对话栏立绘（320px）。
- `h/<角色>/<状态>.webp`：菜单栏圆形头像，按同一方框裁出头部（96px）。
- `office/laptop-closed.webp`：非工作状态桌上合上的笔记本，按 `office/laptop.webp` 的配色和描边用脚本绘制，与其共用锚点；`office/desk.webp`、`office/boss-desk.webp` 的前挡板已镂空，露出桌下的腿和脚，并补画了两条后桌腿，前桌腿底部重画为直腿；`office/chair.webp` 补画了气杆、五星脚和轮子，桌下不再悬空。
- `logo-default.svg`：没有专属标志的模型或 CLI 在笔记本盖子上使用的默认终端标志。

角色按模型家族区分：claude、codex（GPT）、deepseek、gemini、glm、grok；没有专属形象的模型使用 `default`（「新同事」）。状态共 7 个：idle、blink、working、speaking、review、done、sleep。另有 3 个可选帧：`sad`（上次失败）、`walk-1`、`walk-2`（去会议室时的正面走路两帧，鞋底与坐姿对齐），只放 `s/` 与 `m/`；缺少时失败显示灰度 idle，走路改为坐姿滑行。7 个角色都已画了可选帧；idle、blink、sleep 也一并重画为看不到手（揣进口袋或缩进袖口），桌上只放合上的笔记本。

设计参考了社区作品的比例与可爱程度，但造型、服装与配色均为原创，未使用各厂商的官方吉祥物。
