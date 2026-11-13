# 办公室美术资源来源

`ui/art/` 下的角色立绘与办公室道具是为 AgentNave 原创设计的二次元 Q 版形象，2026 年 10 月由 Codex 的图像生成工具按统一规范绘制，再缩放为 WebP：

**生图规范：新生成的立绘、部件和道具必须直接输出真透明底 PNG（带 alpha 通道），物件之外完全透明，收图时检查透明通道；不用白底、品红等色底事后抠图。** 以现有帧为底做局部编辑时同样要求透明底。

- `s/<角色>/<状态>.webp`：工位尺寸立绘，与 `office/` 道具共用 1536×1200 舞台的同一锚点（椅子 → 角色 → 桌子 → 笔记本四层叠放）。
- `m/<角色>/<状态>.webp`：中尺寸立绘（320px），原用于对话栏，现已不显示，保留备用。
- `h/<角色>/<状态>.webp`：菜单栏圆形头像，按同一方框裁出头部（96px）。
- `office/portrait-frame.png`：通用香槟银金挂像相框，由 GPT Image 2 生成为独立 PNG，外部与中央开口均为真透明；原图直接使用，角色头像由界面单独放在内框后方。
- `office/laptop-closed.webp`：非工作状态桌上合上的笔记本，按 `office/laptop.webp` 的配色和描边用脚本绘制，与其共用锚点；`office/desk.webp` 为通用光面工位桌，杯子（6 款 `mug*.webp`）和盆栽花瓶（6 款 `plant.webp`、`pot-*.webp`、`vase-*.webp`）拆为独立图层并按工位固定分配；9 套 Boss 桌椅与笔记本（`claude`、`codex`、`agy`、`deepseek`、`grok`、`codebuddy`、 `zcode`、`pi`、`hermes`，同品牌命令行与桌面端／WorkBuddy 共用）按宿主及对应角色立绘配套（整套含桌面摆件的办公桌 `boss-desk-<host>.webp`、配套老板椅 `boss-chair-<host>.webp`、配套开合笔记本 `boss-laptop-<host>.webp` 与 `boss-laptop-closed-<host>.webp`），桌面高度统一对齐 1536×1200 舞台，未识别宿主沿用默认 `office/chair.webp`、`office/boss-desk.webp` 与 `office/laptop*.webp`；`office/chair.webp` 补画了气杆、五星脚和轮子，桌下不再悬空。
- 桌子统一为 14° 视角：9 张 Boss 桌（`boss-desk*.webp`，Hermes 除外）已由 Codex 按 Hermes 的视角重画，清理 alpha 后覆盖原文件，道具有轻微重绘；工位 `desk.webp` 用脚本拉伸后沿对齐。笔记本改由 `ui/laptop.js` 以 3D 绘制，不再使用 `boss-laptop*.webp`，各桌只保留高度参数。 Devin 的 `boss-desk-devin.webp` 与 `boss-chair-devin.webp` 由 Codex 图像生成工具直接生成真透明 PNG，再缩放为 WebP；桌面遵守相同视角，以深蓝、青绿和浅绿六边形装饰呼应 Devin。
- `office/decision-flow.webp`：设置页「决策模型」档的四格流程插图（boss 交出任务、筛出能派的工位、决策模型选一个、选中的工位开工），2026 年 11 月由 Codex 的图像生成工具按现有画风生成 1600×400 透明底 PNG，脚本清掉低透明度杂点后转为 WebP；图里没有文字，四步说明由界面写在图下方。
- `logo-default.svg`：没有专属标志的 CLI 在笔记本盖子上使用的默认终端标志。
- `cli/<provider>.svg`：各 CLI 的标志，用在笔记本盖子、boss 工位和 CLI 徽章上，取自 LobeHub 开源图标库 `@lobehub/icons-static-svg@1.100.0`（MIT，商标归各自所有者），除下述两项外未改动：claude ← `claudecode-color.svg`、claude-desktop ← `claude-color.svg`（boss 在 Claude 桌面端的 Code 标签里时，用 Claude 应用标志；文件未改，界面按应用图标显示成橙底白色）、codex ← `codex-color.svg`（笔记本盖子改用 `cli/lid/codex.svg`，由脚本去掉白底）、codex-desktop ← `openai.svg`（boss 在 Codex 桌面端时，桌面端随 ChatGPT 应用发布，用厂商标志，白底深色，与应用图标一致）、codebuddy ← `codebuddy-color.svg`、antigravity ← `antigravity-color.svg`（原文件靠模糊滤镜画渐变，托盘的 WebKit 里边缘发糊，所以渲染成 256px 透明 WebP 后包回 SVG）、devin ← `devin-color.svg`、grok ← `grok.svg`、pi ← `pi.svg`、deepseek ← `deepseek-color.svg`、workbuddy ← `workbuddy-color.svg`、hermes ← `hermesagent.svg`、zcode ← `zai.svg`（库里没有 Grok CLI、DeepSeek Harness 与 ZCode 专属标志，用厂商标志）。接入页的宿主图标按宿主 ID 取同一目录的文件。小人形象与模型头像按模型家族对应，不用 CLI 标志。
- `platform/<id>.svg`：语音摘要平台的标志，取自同一图标库同一版本，未改动：chatgpt、openai ← `openai.svg`、google ← `gemini-color.svg`、deepseek ← `deepseek-color.svg`、moonshot ← `kimi.svg`、zhipu ← `zhipu-color.svg`、qwen ← `qwen-color.svg`、minimax ← `minimax-color.svg`、volcengine ← `volcengine-color.svg`、openrouter ← `openrouter.svg`、siliconflow ← `siliconcloud-color.svg`。

角色按模型家族区分：claude、codex（GPT）、deepseek、gemini、glm、grok、qwen、kimi、minimax、hunyuan、doubao、muse；没有专属形象的模型使用 `default`（「新同事」）。状态共 7 个：idle、blink、working、speaking、review、done、sleep。另有 3 个可选帧：`sad`（上次失败）、`walk-1`、`walk-2`（去会议室时的正面走路两帧，鞋底与坐姿对齐），只放 `s/` 与 `m/`；缺少时失败显示灰度 idle，走路改为坐姿滑行。12 个角色都已画了可选帧；idle、blink、sleep 也一并重画为看不到手（揣进口袋或缩进袖口），桌上只放合上的笔记本。

Claude、Grok、Gemini 三套立绘于 2026 年 10 月重绘：Claude 是抱着小螃蟹的橙发少女（螃蟹只在待机与眨眼帧出现），Grok 是戴蝙蝠发饰的金发哥特少女（同月再修：双马尾剪到胸口、紫色瞳孔，加两节恶魔尾巴 `devil-tail` 与 `devil-tail-tip`，尾尖带紫），Gemini 是猫娘。它们会动的部位另存为图层，只放 `s/` 与 `m/`：`<部件>-<状态>.webp`，部件为呆毛 `tuft` 或猫耳 `ear-l`、`ear-r`（猫娘的 sleep 帧歪头、一只耳朵下垂，不拆耳朵）。对应状态帧里挖去了这些部位：呆毛整根挖去，部件图层在根部多带几行头发；猫耳只挖去摆动幅度大的耳尖，靠近转轴和头顶的部分留在原帧上，转动时都不露空；blink 帧与 idle 共用 idle 的部件。`parts.json` 按角色、部件、状态记录转轴位置（占画面宽高的百分比），前端把部件叠在同状态帧上，绕转轴摆动。没有部件图层的角色与设置里替换的小人套装照旧显示整张帧。

GPT（`codex`）各帧去掉了呆毛，坐姿帧的蝙蝠翅膀拆成 `wing-l`、`wing-r` 图层，叠在身体后面，在头发下面多延伸一段翅膀的颜色，扇动时根部不露空；走路帧的翅膀不动。DeepSeek 的呆毛压在白色头饰前面，拆成 `tuft` 后原位补上头饰的白色，并沿边补一条深蓝描边。它的鲸鱼尾巴拆成 `tail` 图层（blink 共用 idle 的），像翅膀一样叠在身体后面，底图里被尾巴挡住的裙摆已补画，前端让尾巴以尾根为轴摆动。同月又给 GPT 的龙尾、Gemini 的猫尾拆出 `tail` 图层（同样叠在身体后面、补画了被挡住的裙摆和头发，摆幅是鲸尾的一半），DeepSeek 两只鳍状耳朵拆成 `lop-l`、`lop-r`，沿用垂耳的轻晃。

Kimi、MiniMax、混元、豆包、Muse 五套于 2026 年 10 月新增：Kimi 后来改为银紫长发、深蓝女仆裙与月亮发饰的形象；坐姿及头像取自 544p 视频包的透明帧，走路帧由 Codex 图像生成工具参照视频形象绘制，不再使用兔耳分层。混元是天蓝高马尾的小道姑，额前祥云呆毛拆成 `tuft`；MiniMax 是戴白色波形耳机的玫红短发女生，豆包是豆包官方头像形象的画风适配（深棕齐肩直发、黑色圆领上衣，未另加饰品；头发外缘重画为连续描线，深色底上不出毛刺），Muse 是戴月桂冠、抱竖琴的酒红麻花辫少女（工作帧和走路帧不拿竖琴）。后三者没有部件，改为「飘出」动效：`emit.json` 按角色记录小图标（`m/<角色>/` 下的音符、豆包 logo 配色的豆形泡泡、金色桂叶）和各坐姿状态的发射点（占画面宽高的百分比），前端按办公室状态定时从发射点放出，音符往右上飘、泡泡往左上飘后破掉、桂叶打旋落下；打盹、失败、走路时、窗口隐藏或系统要求减少动态效果时不放。

坐姿动画另有一套视频（2026 年 11 月，试用中）：macOS 为 `video/<角色>/<状态>.mov`，Windows 与 Linux 为 `video/<角色>/<状态>.webm`，13 个角色各 7 个状态（idle、working、speaking、review、done、sleep、sad），外加闲置摸鱼 `slack-1`、`slack-2` 与睡觉 `sleep-1` 三段，空闲和睡觉时偶尔插播一次。以各角色 `m/` 帧为首尾帧，用 MiniMax H3 图生视频在纯绿底上生成（768px、24fps、124 帧），抠成真透明底后编码为 272px 与 544px、24fps 带 alpha 的 HEVC（`.mov`）及同源转码的 VP9 + Alpha（`.webm`）。待机与睡眠是安静版：身体几乎不动，待机只在中段慢慢眨一次眼，有耳朵、尾巴、翅膀的角色这些部位轻晃，睡眠只有呼吸起伏。道具与特效一律平涂实色、不透明、带描边，不加光效；道具只从她身上掏出并收回或吃掉，动作抬到桌面以上。同目录的 `<状态>.webp` 是从该状态视频取出的第 0 帧（还原为普通透明底），视频加载期间只显示这张静止图，第一段从第 0 帧开播，切换时画面不跳。有视频的角色播放时隐藏帧图、部件和飘出图标，走路和视频加载前仍用帧图；进出场不另做视频，由前端统一动画完成。

2026 年 11 月起，除走路帧外的 `s/` 坐姿帧（7 个状态加 blink）改用该角色 544 视频包同状态的第 0 帧：按前端的视频对位参数（缩放与锚点）烘焙成 544×544 透明 WebP，与视频模式同位；blink 以 idle 帧为底，只把视频中段闭眼一帧的眼部对位贴入。坐姿帧的会动部件随之改为「软铰链」图层：部件整块放在帧图后面，帧图里该部件贴着头发或身体的根部保留不透明、离根部越远越透明，部件绕根部小幅转动时不露切口；摆动周期按各角色视频实测（约 1.25 秒，睡眠减半），幅度小于视频。qwen 的发钗吊坠在头发前面单独成层：只取人偶与挂钩，链子按旧帧样式补画为从耳后垂下的 1 像素细链，帧图里人偶后面的头发用其下方同列发丝上移补齐；GPT（codex）的视频里没有龙尾，坐姿不再叠龙尾；Grok 坐姿的恶魔尾改为单段 `tail`。走路帧与其部件未改。

设计参考了社区作品的比例与可爱程度，除豆包按官方头像适配外，造型、服装与配色均为原创，未使用各厂商的官方吉祥物。

`dmg/background.tiff` 是 macOS 安装盘窗口背景（660×400，内含 @2x），2026 年 11 月用脚本把 `ui/art/m/<角色>/walk-1.webp` 中的 gemini、codex、deepseek、claude、grok、glm、hunyuan 七个角色排在浅灰底下方，加暖色光晕、投影和指向「应用程序」的箭头，没有新生成美术；整窗不透明，不适用透明底要求。
