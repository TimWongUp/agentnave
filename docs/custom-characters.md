# 自制与更换办公室 CSS 小人教程

AgentNave 托盘应用的办公室与菜单栏面板支持在「设置 → 外观」切换两种小人渲染方式：

1. **CSS 小人（默认）**：开箱即用，包含 12 个模型家族角色与 1 个「新同事」（`default`），自带眨眼、呼吸、部件摆动与飘出小图标等 CSS 动画；支持按模型家族单独替换为你自制的 27 张 WebP 小人。
2. **双档选装视频小人包（`272p` / `544p`）**：在「设置 → 外观」中一键下载安装 `272p` 或 `544p` 官方视频包后，即可在「显示方式」中切换启用。视频小人在办公室窗口打开时会持续解码透明视频，比 CSS 小人多占用约 150–300 MB 内存与一定 GPU/CPU（`544p` 约为 `272p` 的两倍）；关闭或隐藏办公室窗口、滚出屏幕或开启「减少动效」时会自动释放解码器。

**两者的关系**：
- 「CSS 小人（按模型家族更换）」只替换 **CSS 小人** 方式下对应模型家族的立绘与头像。
- 当「显示方式」选为 **视频 272p** 或 **高清视频 544p** 时，所有角色统一播放内置视频包；切回 **CSS 小人** 时，已替换的模型家族照常显示你上传的自定义 CSS 小人，点击「恢复默认」则回到内置 CSS 小人。

---

## 一、目录结构与 27 张 WebP 规格

在「设置 → 外观 → CSS 小人（按模型家族更换）」点击对应家族的 **更换小人**，选择包含 `s/`、`m/`、`h/` 三个子文件夹的目录（或直接包含这三个子文件夹的上一级目录）。应用会在本地解码校验全部 27 张图的尺寸与格式，全部通过后才会原子替换，不会出现半套混用的情况。

所选文件夹内必须包含以下 **27 个 `.webp` 文件**（正方形、真透明底、单张文件大小 **≤ 1 MB**）：

```text
my-character/
├── s/                  # 工位与会议室立绘（266×266，共 10 张）
│   ├── idle.webp
│   ├── blink.webp
│   ├── working.webp
│   ├── speaking.webp
│   ├── review.webp
│   ├── done.webp
│   ├── sleep.webp
│   ├── sad.webp
│   ├── walk-1.webp
│   └── walk-2.webp
├── m/                  # 中尺寸预览立绘（320×320，共 10 张，状态同 s/）
│   ├── idle.webp
│   └── ...（共 10 张）
└── h/                  # 菜单栏与列表圆形头像（96×96，共 7 张，无 sad/walk）
    ├── idle.webp
    ├── blink.webp
    ├── working.webp
    ├── speaking.webp
    ├── review.webp
    ├── done.webp
    └── sleep.webp
```

| 子目录 | 分辨率 | 张数 | 状态文件名 | 用途 |
| :--- | :--- | :--- | :--- | :--- |
| `s/` | `266×266` | 10 | `idle`、`blink`、`working`、`speaking`、`review`、`done`、`sleep`、`sad`、`walk-1`、`walk-2` | 办公室工位与会议室席位 |
| `m/` | `320×320` | 10 | 同上 10 个状态 | 外观设置页预览与中尺寸立绘 |
| `h/` | `96×96` | 7 | `idle`、`blink`、`working`、`speaking`、`review`、`done`、`sleep` | 菜单栏面板与头像徽章 |

---

## 二、AI 生图规范与提示词模板

### 1. 立绘绘制与舞台对齐规范

- **真透明底 PNG 原图**：生成或绘制原图时直接输出带 Alpha 通道的真透明底正方形 PNG（建议 `1024×1024`），人物之外完全透明，避免白底或色底事后抠图残留白边。
- **舞台与桌椅对齐**：工位按「椅子 → 角色 → 桌子 → 笔记本」四层叠放：
  - 头顶（发顶，不含高出的角或耳朵）建议落在画面高度约 `12%` 处，鞋底落在约 `97.8%` 处。
  - `idle`、`blink`、`sleep` 为安静坐姿，双手揣进口袋或缩进袖口（不露手），避免与桌面合上的笔记本穿插。
  - `working`、`speaking`、`review`、`done`、`sad` 的手部动作或小道具抬高到桌面高度以上，避免被前方的办公桌挡住。
  - `walk-1`、`walk-2` 为去会议室时的正面迈步两帧，鞋底高度与坐姿保持一致。
- **头部头像裁切框（`h/`）**：同一个角色的 7 张 `h/<状态>.webp` 使用**同一个正方形裁切框**裁出头部与面部表情，切换状态时头像不会跳动。

### 2. 提示词模板

可将以下提示词模板交给支持透明底生图的图像模型或 Agent，一次性生成同一角色的 10 张状态原图（`idle.png` … `walk-2.png`）：

```text
2D flat cel-shaded chibi anime character sprite, full body, front-facing, centered in a 1024x1024 square canvas, true transparent background (RGBA PNG).
Alignment: top of hair at ~12% from top edge, shoe soles at ~97.8% from top edge, consistent proportions across all 10 frames.
Generate 10 state frames for the same character:
- idle: seated quietly, hands tucked in sleeves/pockets (not visible), calm smile, eyes open
- blink: identical to idle but eyes gently closed
- working: seated, hands raised above desk height typing/working, focused expression
- speaking: seated, mouth open talking, expressive hand gesture above desk height
- review: seated, holding a magnifying glass or checklist above desk height, inspecting carefully
- done: seated, cheerful celebratory pose with hands raised above desk height
- sleep: seated quietly, eyes closed sleeping, hands hidden
- sad: seated, droopy/upset expression, wiping a tear above desk height
- walk-1: standing front-facing, left foot stepping forward, shoe soles at ~97.8%
- walk-2: standing front-facing, right foot stepping forward, shoe soles at ~97.8%
```

---

## 三、一键裁切导出 27 张 WebP 脚本

准备好 10 张正方形透明底 PNG 原图（命名为 `idle.png`、`blink.png`、`working.png`、`speaking.png`、`review.png`、`done.png`、`sleep.png`、`sad.png`、`walk-1.png`、`walk-2.png`）后，可用以下 Python（Pillow）脚本一次性导出符合校验要求的 `s/`、`m/`、`h/` 文件夹：

```python
#!/usr/bin/env python3
"""从 10 张正方形透明底 PNG 原图生成单个模型家族的 27 张 s/m/h WebP。
用法: python build_css_character.py <原图目录> <输出目录> [头部框 left,top,size 占原图比例，默认 0.22,0.06,0.56]
"""
import sys
from pathlib import Path
from PIL import Image

STATES_7 = ["idle", "blink", "working", "speaking", "review", "done", "sleep"]
STATES_10 = [*STATES_7, "sad", "walk-1", "walk-2"]

src_dir, out_dir = Path(sys.argv[1]), Path(sys.argv[2])
hx, hy, hw = (float(x) for x in (sys.argv[3] if len(sys.argv) > 3 else "0.22,0.06,0.56").split(","))

for folder in ("s", "m", "h"):
    (out_dir / folder).mkdir(parents=True, exist_ok=True)

for state in STATES_10:
    im = Image.open(src_dir / f"{state}.png").convert("RGBA")
    w, h = im.size
    im.resize((266, 266), Image.Resampling.LANCZOS).save(out_dir / "s" / f"{state}.webp", "WEBP", quality=92, method=6)
    im.resize((320, 320), Image.Resampling.LANCZOS).save(out_dir / "m" / f"{state}.webp", "WEBP", quality=92, method=6)
    if state in STATES_7:
        box = (round(w * hx), round(h * hy), round(w * (hx + hw)), round(h * (hy + hw)))
        im.crop(box).resize((96, 96), Image.Resampling.LANCZOS).save(
            out_dir / "h" / f"{state}.webp", "WEBP", quality=92, method=6
        )

print(f"已生成 27 张 WebP -> {out_dir}")
```
