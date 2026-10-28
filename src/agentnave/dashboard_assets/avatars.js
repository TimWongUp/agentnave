"use strict";
// Shared by the tray and capability-scoped browser dashboard. All images stay local.
(function () {
  const base = new URL(".", document.currentScript.src);
  // Official LobeHub icons, fixed. Mono marks are dark ink, shown light on a dark tile.
  const families = {
    claude: ["Claude", "avatar-claude.svg"], gpt: ["OpenAI", "avatar-gpt.svg", "mono"],
    gemini: ["Gemini", "avatar-gemini.svg"], grok: ["Grok", "avatar-grok.svg", "mono"],
    deepseek: ["DeepSeek", "avatar-deepseek.svg"], glm: ["GLM", "avatar-glm.svg", "mono"],
    qwen: ["Qwen", "avatar-qwen.svg"], kimi: ["Kimi", "avatar-kimi.svg", "mono"],
    minimax: ["MiniMax", "avatar-minimax.svg"], hunyuan: ["Hunyuan", "avatar-hunyuan.svg"],
    doubao: ["Doubao", "avatar-doubao.svg"], muse: ["Muse", "avatar-muse.svg"],
  };
  let overrides = {}, revision = 0;
  const key = (kind, provider, model = "") => JSON.stringify([kind, provider || "", model || ""]);
  function family(provider, model) {
    // Model identity never inherits the CLI brand, including for managers.
    const name = String(model || "").toLowerCase().split("/").pop();
    return Object.keys(families).find((id) => name.includes(id)) ||
      (/^(o[134](?:-|$)|chatgpt|codex)/.test(name) ? "gpt" : null) ||
      (/^(opus|sonnet|haiku)(?:-|$)/.test(name) ? "claude" : null) ||
      (/^hy\d/.test(name) ? "hunyuan" : null) || (/^seed(?:-|$)/.test(name) ? "doubao" : null);
  }
  // Only a CLI mark saved by an earlier version can still be replaced; model logos are fixed.
  function source(kind, provider, model) {
    const id = kind === "family" ? provider : family(provider, model);
    if (kind === "host") return overrides[key(kind, provider, model)] || null;
    return families[id] ? [new URL(families[id][1], base).href, families[id][2]] : null;
  }
  function paint(node) {
    const [kind, provider, model] = JSON.parse(node.dataset.avatar);
    const found = source(kind, provider, model);
    const [src, mono] = Array.isArray(found) ? found : [found];
    node.replaceChildren();
    // A clipped square owns the shape; transparent logos must not acquire a white tile.
    node.style.position = "relative";
    node.style.overflow = "hidden";
    node.style.background = mono ? "#111" : src ? "transparent" : "hsl(210 18% 38%)";
    node.style.boxShadow = "none";
    if (src) {
      const img = document.createElement("img");
      img.alt = "";
      img.src = src;
      const inset = kind === "host" ? "0" : "12%";
      img.style.cssText = `position:absolute;inset:${inset};width:calc(100% - 2 * ${inset});height:calc(100% - 2 * ${inset});max-width:none;max-height:none;object-fit:contain;display:block${mono ? ";filter:invert(1)" : ""}`;
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
    overrides = data;
    if (changed) refresh();
  }
  window.Avatars = { create, family, families, key, load,
    custom: (kind, provider, model) => Boolean(overrides[key(kind, provider, model)]) };
  load().catch(() => {});
  setInterval(() => { if (!document.hidden) load().catch(() => {}); }, 5000);
})();
