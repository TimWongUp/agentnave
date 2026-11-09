"use strict";
(function () {
  const { h, art, L } = AN;
  const root = document.getElementById("avatar-settings");
  const status = h("p", { role: "status" });
  // Upload order the tray expects: s and m carry every frame, heads (h) only the seven moods.
  const FRAMES = [
    ...["s", "m"].flatMap((size) => [...art.STATES, ...art.EXTRA].map((state) => `${size}/${state}`)),
    ...art.STATES.map((state) => `h/${state}`),
  ];
  let busy = false, replaced = {}, packs = {}, packsLoaded = false, pollTimer = 0;
  let styleChoice = document.documentElement.dataset.characterStyle || "static";
  const refreshers = [];
  const videoBox = h("div.video-packs");
  const megabytes = (bytes) => `${((bytes || 0) / 1048576).toFixed(1)} MB`;

  const PROMPT_TEMPLATE = `2D flat cel-shaded chibi anime character sprite, full body, front-facing, centered in a 1024x1024 square canvas, true transparent background (RGBA PNG).
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
- walk-2: standing front-facing, right foot stepping forward, shoe soles at ~97.8%`;

  const CROP_SCRIPT = `#!/usr/bin/env python3
# pip install pillow
# Usage: python build_css_character.py <input_png_dir> <output_dir> [head_box: left,top,size]
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
        im.crop(box).resize((96, 96), Image.Resampling.LANCZOS).save(out_dir / "h" / f"{state}.webp", "WEBP", quality=92, method=6)
print("Done ->", out_dir)`;

  const PACK_BYTES = { "272": 189160043, "544": 551420875 };

  function packStatusText(pack, tier) {
    const dl = pack?.download, size = megabytes(pack?.bytes || PACK_BYTES[tier]);
    if (dl?.state === "downloading") {
      const done = megabytes(dl.downloaded);
      const total = dl.total ? megabytes(dl.total) : size;
      return L(`下载中 ${done} / ${total}`, `Downloading ${done} / ${total}`);
    }
    if (dl?.state === "extracting") return L(`解压校验中… · ${size}`, `Extracting… · ${size}`);
    if (dl?.state === "failed") return `${size} · ${dl.error || L("下载失败", "Download failed")}`;
    if (pack?.installed) return L(`已安装 · ${size}`, `Installed · ${size}`);
    return L(`未安装 · ${size}`, `Not installed · ${size}`);
  }

  function ensureInstalledStyle(persist) {
    if (!packsLoaded || !styleChoice.startsWith("video-")) return;
    const tier = styleChoice.slice(6);
    if (packs[tier]?.installed) return;
    styleChoice = "static";
    window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: { character_style: "static" } }));
    if (persist) AN.post("/app/preferences", { general: { character_style: "static" } }).catch(() => {});
  }

  async function applyPacks(data, announce) {
    const next = { ...(data?.packs || {}) };
    const stampKey = (obj) => ["272", "544"].map((t) => `${obj[t]?.installed}:${obj[t]?.stamp || ""}`).join("|");
    const changed = stampKey(next) !== stampKey(packs);
    const hadVideo = styleChoice.startsWith("video-") && Boolean(packs[styleChoice.slice(6)]?.installed);
    packs = next;
    packsLoaded = true;
    ensureInstalledStyle(hadVideo || changed);
    renderVideoPacks();
    if (changed) {
      await art.videoPacksChanged?.();
      refreshers.forEach((f) => f());
    }
    if (announce && !data?.cancelled) status.textContent = announce;
    clearTimeout(pollTimer);
    if (["272", "544"].some((t) => ["downloading", "extracting"].includes(packs[t]?.download?.state))) {
      pollTimer = setTimeout(loadPacks, 600);
    }
  }

  function loadPacks() {
    return fetch("/app/video-packs", { cache: "no-store" })
      .then((r) => (r.ok ? r.json() : {}))
      .then((data) => applyPacks(data))
      .catch(() => {});
  }

  async function saveStyle(choice) {
    if (busy || choice === styleChoice) return;
    if (choice.startsWith("video-") && !packs[choice.slice(6)]?.installed) return;
    busy = true; renderVideoPacks();
    try {
      await AN.post("/app/preferences", { general: { character_style: choice } });
      styleChoice = choice;
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: { character_style: choice } }));
      if (choice === "static") {
        status.textContent = L("已切换为 CSS 小人", "Switched to CSS characters");
      } else {
        const tier = choice.slice(6);
        status.textContent = L(`已切换为 ${tier}p 视频小人`, `Switched to ${tier}p video characters`);
      }
    } catch (error) {
      status.textContent = error.message;
    } finally {
      busy = false; renderVideoPacks();
    }
  }

  async function packAction(tier, action, done) {
    if (busy) return;
    busy = true; renderVideoPacks();
    try {
      const data = await AN.post(`/app/video-packs/${tier}/${action}`, {});
      await applyPacks(data, done);
    } catch (error) {
      status.textContent = error.message;
      await loadPacks();
    } finally {
      busy = false; renderVideoPacks();
    }
  }

  function renderVideoPacks() {
    const label = L("显示方式", "Display mode");
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [choice, text, tier, missingHint] of [
      ["static", L("CSS 小人", "CSS"), "", ""],
      ["video-272", L("视频 272p", "Video 272p"), "272", L("请先安装 272p 视频包", "Install the 272p video pack first")],
      ["video-544", L("高清视频 544p", "HD video 544p"), "544", L("请先安装 544p 高清视频包", "Install the 544p HD video pack first")],
    ]) {
      const installed = !tier || Boolean(packs[tier]?.installed);
      const active = styleChoice === choice;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled: busy || !installed,
        title: !installed ? missingHint : null,
      }, text);
      btn.addEventListener("click", () => void saveStyle(choice));
      control.append(btn);
    }
    const tipText = L(
      "视频小人在办公室窗口打开时会持续解码透明视频，比 CSS 小人多占用约 150–300 MB 内存与一定 GPU/CPU（544p 约为 272p 的两倍）；关闭或隐藏办公室窗口、滚出屏幕或开启「减少动效」时会自动释放解码器。",
      "Video characters continuously decode alpha video while the office window is open, using ~150–300 MB more memory and extra GPU/CPU than CSS characters (544p uses roughly twice as much as 272p); decoders are released automatically when the window is hidden, scrolled offscreen, or Reduce motion is on.",
    );
    const tip = h("span.setting-tip", { tabindex: "0", role: "note", "aria-label": tipText }, "?");
    const rows = [h("div.setting-row", {}, h("div.video-pack-info", {}, h("h2", {}, label), tip), control)];
    for (const [tier, title] of [["272", L("272p 视频包", "272p video pack")], ["544", L("544p 高清视频包", "544p HD video pack")]]) {
      const pack = packs[tier] || {};
      const activeDl = ["downloading", "extracting"].includes(pack.download?.state);
      const actions = h("div.video-pack-actions");
      if (activeDl) {
        const cancel = h("button", { type: "button", disabled: busy }, L("取消下载", "Cancel"));
        cancel.addEventListener("click", () => void packAction(tier, "cancel", L("已取消下载", "Download cancelled")));
        actions.append(cancel);
      } else {
        if (!pack.installed) {
          const dl = h("button", { type: "button", disabled: busy }, L("一键下载", "Download"));
          dl.addEventListener("click", () => void packAction(tier, "download"));
          actions.append(dl);
        }
        const imp = h("button", { type: "button", disabled: busy }, L("从文件夹导入…", "Import folder…"));
        imp.addEventListener("click", () => void packAction(tier, "import", L(`已导入 ${tier}p 视频包`, `Imported ${tier}p video pack`)));
        actions.append(imp);
        if (pack.installed) {
          const del = h("button", { type: "button", disabled: busy }, L("删除", "Delete"));
          del.addEventListener("click", () => void packAction(tier, "delete", L(`已删除 ${tier}p 视频包`, `Deleted ${tier}p video pack`)));
          actions.append(del);
        }
      }
      const badge = h(`span.video-pack-status${pack.download?.state === "failed" ? ".bad" : ""}`, {}, packStatusText(pack, tier));
      rows.push(h("div.setting-row.video-pack-row", {}, h("div.video-pack-info", {}, h("h2", {}, title), badge), actions));
    }
    videoBox.replaceChildren(...rows);
  }

  window.addEventListener("agentnave:preferences", ({ detail }) => {
    if (["static", "video-272", "video-544"].includes(detail?.character_style)) {
      styleChoice = detail.character_style;
      ensureInstalledStyle(false);
      renderVideoPacks();
    }
  });
  // A chosen folder holds s/, m/ and h/ directly, or one level further down.
  async function pack(files) {
    const found = new Map();
    for (const file of files) {
      const parts = file.webkitRelativePath.split("/");
      const frame = parts.slice(-2).join("/").replace(/\.webp$/i, "");
      if (FRAMES.includes(frame) && /\.webp$/i.test(file.name)) found.set(frame, file);
    }
    const missing = FRAMES.filter((frame) => !found.has(frame));
    if (missing.length)
      throw new Error(L(`缺少 ${missing.length} 张图：${missing.slice(0, 4).map((f) => `${f}.webp`).join("、")}${missing.length > 4 ? " 等" : ""}`, `Missing ${missing.length} images: ${missing.slice(0, 4).map((f) => `${f}.webp`).join(", ")}${missing.length > 4 ? ", etc." : ""}`));
    const chunks = [];
    for (const frame of FRAMES) {
      // Decode each frame once, so a broken or wrongly sized image never replaces a working set.
      const edge = { s: 266, m: 320, h: 96 }[frame[0]];
      const bitmap = await createImageBitmap(found.get(frame)).catch(() => null);
      const fits = bitmap && bitmap.width === edge && bitmap.height === edge;
      bitmap?.close();
      if (!fits) throw new Error(L(`${frame}.webp 必须是能打开的 ${edge}×${edge} WebP 图片`, `${frame}.webp must be a valid ${edge}×${edge} WebP image`));
      const bytes = new Uint8Array(await found.get(frame).arrayBuffer());
      const head = new Uint8Array(4);
      new DataView(head.buffer).setUint32(0, bytes.length);
      chunks.push(head, bytes);
    }
    return new Blob(chunks);
  }
  async function send(family, body) {
    const response = await fetch(`/app/characters/${family}`, {
      method: "POST", headers: { "X-AgentNave-Action": "1" }, body,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || L("保存小人失败", "Failed to save character"));
    replaced = data;
    await art.charactersChanged();
  }
  function editor(family, label) {
    const row = h("div.avatar-editor");
    const face = h("span.char-preview");
    const input = h("input", { type: "file", webkitdirectory: "", multiple: true, hidden: true, "aria-label": L(`选择${label}小人文件夹`, `Choose ${label} character folder`) });
    const upload = h("button", { type: "button" }, L("更换小人", "Replace character"));
    const reset = h("button", { type: "button" }, L("恢复默认", "Restore default"));
    function update() {
      face.replaceChildren(art.figure(art.character("", family), "m", true));
      upload.disabled = busy;
      reset.disabled = busy || !replaced[family];
    }
    async function run(action, done) {
      busy = true; refreshers.forEach((f) => f());
      try { await action(); status.textContent = L(`${done}，办公室和菜单栏随后更新`, `${done}. Office and menu bar will update shortly.`); }
      catch (error) { status.textContent = error.message; }
      finally { busy = false; input.value = ""; refreshers.forEach((f) => f()); }
    }
    upload.addEventListener("click", () => input.click());
    input.addEventListener("change", () => { if (input.files.length) run(async () => send(family, await pack(input.files)), L(`${label} 的小人已更换`, `Replaced character for ${label}`)); });
    reset.addEventListener("click", () => run(() => send(family, new Blob([])), L(`${label} 已恢复默认小人`, `Restored default character for ${label}`)));
    refreshers.push(update);
    update();
    row.append(face, h("div.avatar-side", {},
      h("div.avatar-name", {}, Avatars.create("family", family, ""), h("span.avatar-label", {}, label)),
      h("div.avatar-actions", {}, upload, reset)), input);
    return row;
  }
  function copyBox(codeText, btnLabel) {
    const btn = h("button.char-guide-copy", { type: "button" }, btnLabel);
    btn.addEventListener("click", () => {
      navigator.clipboard?.writeText?.(codeText)?.catch?.(() => {});
      btn.textContent = L("已复制", "Copied");
      setTimeout(() => { btn.textContent = btnLabel; }, 1500);
    });
    return h("div.char-guide-code-wrap", {}, h("div.char-guide-code-bar", {}, btn), h("pre.char-guide-code", {}, codeText));
  }

  function buildGuideDialog() {
    const closeBtn = h("button.primary", { type: "button" }, L("关闭", "Close"));
    const dialog = h("dialog.char-guide-dialog", {},
      h("h2.upd-title", {}, L("自制与更换 CSS 小人指南", "DIY & replace CSS characters guide")),
      h("div.char-guide-body", {},
        h("section.char-guide-card", {},
          h("h3", {}, L("1. 目录结构与 27 张 WebP 规格", "1. Folder structure & 27-WebP specification")),
          h("p", {}, L("点击某个模型家族的「更换小人」，选择包含 s/、m/、h/ 三个子文件夹的目录，共 27 张真透明底正方形 WebP（单张 ≤ 1 MB）：", "Click Replace character on a model family and choose a folder with s/, m/ and h/ subfolders containing 27 transparent square WebP images (≤ 1 MB each):")),
          h("ul", {},
            h("li", {}, L("s/：266×266，10 张（idle、blink、working、speaking、review、done、sleep、sad、walk-1、walk-2）", "s/: 266×266, 10 frames (idle, blink, working, speaking, review, done, sleep, sad, walk-1, walk-2)")),
            h("li", {}, L("m/：320×320，10 张（同上 10 个状态）", "m/: 320×320, 10 frames (same 10 states)")),
            h("li", {}, L("h/：96×96，7 张（前 7 个状态，不含 sad / walk-1 / walk-2，按同一头部方框裁切）", "h/: 96×96, 7 frames (first 7 states without sad / walk, cropped to the same head box)"))),
          h("p.char-guide-note", {}, L("仅在「显示方式」选为「CSS 小人」时显示自定义立绘；切换为 272p / 544p 视频小人时统一播放内置视频包。", "Custom sets appear when Display mode is set to CSS; 272p / 544p video modes always play the built-in video packs."))),
        h("section.char-guide-card", {},
          h("h3", {}, L("2. AI 生图规范与提示词模板", "2. AI image generation rules & prompt template")),
          h("ul", {},
            h("li", {}, L("真透明底 PNG：直接输出带 Alpha 通道的正方形 PNG（建议 1024×1024），人物之外完全透明。", "True transparent PNG: output square RGBA PNGs (1024×1024 recommended) with full transparency outside the character.")),
            h("li", {}, L("舞台对齐：头顶（发顶，不含高出的角或耳朵）约在画面高度 12% 处，鞋底约在 97.8% 处。", "Stage alignment: top of hair at ~12% height, shoe soles at ~97.8% height.")),
            h("li", {}, L("姿态要求：idle / blink / sleep 双手揣兜或缩袖不露手；working / speaking / review / done / sad 动作与小道具抬高到桌面高度以上；walk-1 / walk-2 正面迈步且鞋底与坐姿同高。", "Pose rules: idle / blink / sleep hide hands in pockets or sleeves; working / speaking / review / done / sad raise hands and props above desk height; walk-1 / walk-2 step forward with soles aligned to the seated pose."))),
          copyBox(PROMPT_TEMPLATE, L("复制提示词", "Copy prompt"))),
        h("section.char-guide-card", {},
          h("h3", {}, L("3. 一键裁切导出 27 张 WebP 脚本", "3. One-click Python script to export 27 WebPs")),
          h("p", {}, L("将 10 张透明底 PNG 命名为 idle.png … walk-2.png 放入同一目录后，运行以下脚本即可导出 s/、m/、h/ 共 27 张 WebP：", "Name your 10 transparent PNGs idle.png … walk-2.png in one folder, then run this script to export all 27 s/m/h WebP files:")),
          copyBox(CROP_SCRIPT, L("复制裁切脚本", "Copy script")))),
      h("div.char-guide-foot", {}, closeBtn));
    closeBtn.addEventListener("click", () => dialog.close());
    dialog.addEventListener("click", (e) => { if (e.target === dialog) dialog.close(); });
    return dialog;
  }
  const guideDialog = buildGuideDialog();
  const guideBtn = h("button.char-guide-btn", { type: "button" }, L("自制与更换指南", "DIY & replace guide"));
  guideBtn.addEventListener("click", () => guideDialog.showModal?.());
  const grid = h("div.avatar-grid");
  root.append(status, h("h2", {}, L("小人动画", "Character animation")), videoBox, h("div.avatar-section-head", {}, h("h2", {}, L("CSS 小人（按模型家族更换）", "CSS characters (per model family)")), guideBtn), grid, guideDialog);
  renderVideoPacks();
  void loadPacks();
  fetch("/app/preferences", { cache: "no-store" }).then((r) => (r.ok ? r.json() : {})).then((data) => {
    const saved = data?.effective?.general?.character_style || data?.preferences?.general?.character_style;
    if (["static", "video-272", "video-544"].includes(saved)) {
      styleChoice = saved;
      ensureInstalledStyle(true);
      renderVideoPacks();
    }
  }).catch(() => {});
  fetch("/app/characters", { cache: "no-store" }).then((r) => (r.ok ? r.json() : {})).catch(() => ({})).then((data) => {
    replaced = data;
    for (const [id, [label]] of Object.entries(Avatars.families)) grid.append(editor(id, label));
  });
})();
