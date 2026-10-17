"use strict";
// Shared by the tray and capability-scoped browser dashboard. All images stay local.
(function () {
  const base = new URL(".", document.currentScript.src);
  const families = {
    claude: ["Claude", "avatar-claude.png"], gpt: ["OpenAI", "avatar-gpt.svg"],
    gemini: ["Gemini", "avatar-gemini.png"], grok: ["Grok", "avatar-grok.png"],
    deepseek: ["DeepSeek", "avatar-deepseek.ico"], glm: ["GLM", "avatar-glm.svg"],
    qwen: ["Qwen", "avatar-qwen.png"], kimi: ["Kimi", "avatar-kimi.png"],
    minimax: ["MiniMax", "avatar-minimax.ico"],
  };
  // Transparent symbol assets: source dimensions and visible bounds. Keep the original
  // official files intact while giving each symbol the same centered 82% footprint.
  const symbolBounds = {
    "avatar-gemini.png": [512, 512, 21, 21, 470, 470],
    "avatar-qwen.png": [80, 80, 4, 3, 72, 74],
    "avatar-deepseek.ico": [225, 225, 0, 31, 224, 165],
  };
  const maxPixels = 16_000_000;
  const decodeEdge = 2048;
  let overrides = {}, loaded = false, revision = 0;
  const key = (kind, provider, model = "") => JSON.stringify([kind, provider || "", model || ""]);
  function family(provider, model) {
    // Model identity never inherits the CLI brand, including for managers.
    const name = String(model || "").toLowerCase().split("/").pop();
    return Object.keys(families).find((id) => name.includes(id)) ||
      (/^(o[134](?:-|$)|chatgpt|codex)/.test(name) ? "gpt" : null) ||
      (/^(opus|sonnet|haiku)(?:-|$)/.test(name) ? "claude" : null);
  }
  function source(kind, provider, model) {
    const id = kind === "family" ? provider : family(provider, model);
    return overrides[key(kind, provider, model)] || overrides[key("family", id)] ||
      (families[id] ? new URL(families[id][1], base).href : null);
  }
  function paint(node) {
    const [kind, provider, model] = JSON.parse(node.dataset.avatar);
    const src = source(kind, provider, model);
    node.replaceChildren();
    // A clipped square owns the shape; transparent logos must not acquire a white tile.
    node.style.position = "relative";
    node.style.overflow = "hidden";
    node.style.background = src ? "transparent" : "hsl(210 18% 38%)";
    node.style.boxShadow = "none";
    if (src) {
      const img = document.createElement("img");
      img.alt = "";
      img.src = src;
      img.style.cssText = "position:absolute;inset:0;width:100%;height:100%;max-width:none;max-height:none;object-fit:cover;object-position:50% 50%;display:block";
      const bounds = Object.entries(symbolBounds).find(([file]) => src === new URL(file, base).href)?.[1];
      if (bounds) {
        const [width, height, x, y, visibleWidth, visibleHeight] = bounds;
        const extent = Math.max(visibleWidth, visibleHeight) / 0.82;
        img.style.width = `${width / extent * 100}%`;
        img.style.height = `${height / extent * 100}%`;
        img.style.left = `${50 - (x + visibleWidth / 2) / extent * 100}%`;
        img.style.top = `${50 - (y + visibleHeight / 2) / extent * 100}%`;
      }
      img.onerror = () => { img.remove(); node.textContent = String(model || "?").slice(0, 1).toUpperCase(); };
      node.append(img);
    } else node.textContent = String(model || "?").slice(0, 1).toUpperCase();
  }
  function refresh() {
    document.querySelectorAll("[data-avatar]").forEach(paint);
    window.dispatchEvent(new Event("agentnave:avatars"));
  }
  function create(kind, provider, model = "", cls = "") {
    const node = document.createElement("span");
    node.className = "avatar " + cls;
    node.setAttribute("aria-hidden", "true");
    node.dataset.avatar = key(kind, provider, model);
    paint(node);
    return node;
  }
  async function load() {
    const stamp = ++revision;
    const response = await fetch(new URL("avatar-settings", base), { cache: "no-store" });
    if (!response.ok) throw new Error("无法读取头像设置");
    const data = await response.json();
    if (stamp !== revision) return;
    const changed = JSON.stringify(overrides) !== JSON.stringify(data);
    overrides = data; loaded = true;
    if (changed) refresh();
  }
  async function save(kind, provider, model, image) {
    if (!loaded) await load();
    ++revision;
    const response = await fetch(new URL("app/avatars", base), {
      method: "POST", headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ key: key(kind, provider, model), image }),
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "保存头像失败");
    ++revision; overrides = data; refresh();
  }
  const u16be = (bytes, at) => (bytes[at] << 8) | bytes[at + 1];
  const u16le = (bytes, at) => bytes[at] | (bytes[at + 1] << 8);
  const u24le = (bytes, at) => bytes[at] | (bytes[at + 1] << 8) | (bytes[at + 2] << 16);
  function ascii(bytes, at, text) {
    return [...text].every((ch, index) => bytes[at + index] === ch.charCodeAt(0));
  }
  function pngSize(bytes) {
    if (bytes.length < 24 || bytes[0] !== 0x89 || !ascii(bytes, 1, "PNG\r\n\x1a\n") ||
        !ascii(bytes, 12, "IHDR")) return null;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return {width: view.getUint32(16), height: view.getUint32(20)};
  }
  function jpegSize(bytes) {
    if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8) return null;
    let offset = 2;
    while (offset + 3 < bytes.length) {
      if (bytes[offset] !== 0xff) { offset++; continue; }
      while (bytes[offset] === 0xff) offset++;
      const marker = bytes[offset++];
      if (marker === 0x01 || marker === 0xd8 || marker === 0xd9 || (marker >= 0xd0 && marker <= 0xd7))
        continue;
      if (offset + 2 > bytes.length) return null;
      const length = u16be(bytes, offset);
      if (length < 2 || offset + length > bytes.length) return null;
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
        if (length < 7) return null;
        return {width: u16be(bytes, offset + 5), height: u16be(bytes, offset + 3)};
      }
      offset += length;
    }
    return null;
  }
  function webpSize(bytes) {
    if (bytes.length < 30 || !ascii(bytes, 0, "RIFF") || !ascii(bytes, 8, "WEBP")) return null;
    if (ascii(bytes, 12, "VP8X"))
      return {width: 1 + u24le(bytes, 24), height: 1 + u24le(bytes, 27)};
    if (ascii(bytes, 12, "VP8 ") && bytes[23] === 0x9d && bytes[24] === 0x01 && bytes[25] === 0x2a)
      return {width: u16le(bytes, 26) & 0x3fff, height: u16le(bytes, 28) & 0x3fff};
    if (ascii(bytes, 12, "VP8L") && bytes[20] === 0x2f)
      return {
        width: 1 + (((bytes[22] & 0x3f) << 8) | bytes[21]),
        height: 1 + (((bytes[24] & 0x0f) << 10) | (bytes[23] << 2) | ((bytes[22] & 0xc0) >> 6)),
      };
    return null;
  }
  function imageSize(bytes) {
    return pngSize(bytes) || jpegSize(bytes) || webpSize(bytes);
  }
  async function upload(kind, provider, model, file) {
    if (!file || !["image/png", "image/jpeg", "image/webp"].includes(file.type))
      throw new Error("请选择 PNG、JPEG 或 WebP 图片");
    if (file.size > 5 * 1024 * 1024) throw new Error("图片不能超过 5 MB");
    const size = imageSize(new Uint8Array(await file.arrayBuffer()));
    if (!size || !size.width || !size.height) throw new Error("无法读取图片尺寸");
    if (size.width * size.height > maxPixels) throw new Error("图片不能超过 1600 万像素");
    const scale = Math.min(1, decodeEdge / Math.max(size.width, size.height));
    const bitmap = await createImageBitmap(file, {
      resizeWidth: Math.max(1, Math.round(size.width * scale)),
      resizeHeight: Math.max(1, Math.round(size.height * scale)),
      resizeQuality: "high",
    });
    try {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 128;
      const size = Math.min(bitmap.width, bitmap.height);
      canvas.getContext("2d").drawImage(bitmap, (bitmap.width-size)/2, (bitmap.height-size)/2, size, size, 0, 0, 128, 128);
      await save(kind, provider, model, canvas.toDataURL("image/png"));
    } finally { bitmap.close(); }
  }
  window.Avatars = { create, family, families, key, load, save, upload,
    custom: (kind, provider, model) => Boolean(overrides[key(kind, provider, model)]) };
  load().catch(() => {});
  setInterval(() => { if (!document.hidden) load().catch(() => {}); }, 5000);
})();
