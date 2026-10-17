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
  const hosts = { claude: "claude", codex: "gpt", grok: "grok", antigravity: "gemini" };
  let overrides = {}, loaded = false, revision = 0;
  const key = (kind, provider, model = "") => JSON.stringify([kind, provider || "", model || ""]);
  function family(provider, model) {
    // Resolve the model first: a Pi/OpenCode model must not inherit its CLI's brand.
    const name = String(model || "").toLowerCase().split("/").pop();
    return Object.keys(families).find((id) => name.includes(id)) ||
      (/^(o[134](?:-|$)|chatgpt|codex)/.test(name) ? "gpt" : null) || hosts[provider];
  }
  function source(kind, provider, model) {
    const id = kind === "family" ? provider : family(provider, kind === "host" ? "" : model);
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
      img.onerror = () => { img.remove(); node.textContent = String(provider || "?").slice(0, 1).toUpperCase(); };
      node.append(img);
    } else node.textContent = String(provider || "?").slice(0, 1).toUpperCase();
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
  async function upload(kind, provider, model, file) {
    if (!file || !["image/png", "image/jpeg", "image/webp"].includes(file.type))
      throw new Error("请选择 PNG、JPEG 或 WebP 图片");
    if (file.size > 5 * 1024 * 1024) throw new Error("图片不能超过 5 MB");
    const bitmap = await createImageBitmap(file);
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
