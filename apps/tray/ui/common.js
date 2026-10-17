"use strict";
// Helpers shared by the panel and the chat page. Classic script, one global: AN.
// Every node is built with createElement/textContent.
(function () {
  const mock = () => (window.AgentNaveMock && window.AgentNaveMock.enabled() ? window.AgentNaveMock : null);

  // h("a.row.tone-run", {href}, child, [children]) -> element. "style" goes through CSSOM.
  function h(tag, props, ...kids) {
    const [name, ...classes] = tag.split(".");
    const node = document.createElement(name || "div");
    if (classes.length) node.className = classes.join(" ");
    for (const [key, value] of Object.entries(props || {})) {
      if (value == null || value === false) continue;
      if (key === "style") node.style.cssText = value;
      else node.setAttribute(key, value === true ? "" : value);
    }
    node.append(...kids.flat(Infinity).filter((kid) => kid != null && kid !== false));
    return node;
  }
  const $ = (id) => document.getElementById(id);

  async function load(path) {
    const m = mock();
    if (m) return m.fetch(path);
    const response = await fetch(path, { cache: "no-store" });
    if (!response.ok) throw Object.assign(new Error(String(response.status)), { status: response.status });
    return response.json();
  }
  // Real routes in production; mapped to the prototype's files under the mock.
  const href = (path) => (mock() ? mock().href(path) : path);

  // ---- vocabulary ---------------------------------------------------------------------
  // [label, tone]; tone selects one of --run --review --bad --done --idle.
  const MEMBER = {
    running: ["进行中", "run"], review: ["待审核", "review"], succeeded: ["已回复", "done"],
    failed: ["失败", "bad"], blocked: ["受阻", "bad"], cancelled: ["已取消", "idle"],
    interrupted: ["已中断", "bad"], pending: ["等待中", "idle"],
  };
  const GROUP = { running: ["运行中", "run"], review: ["待审核", "review"], failed: ["有失败", "bad"], idle: ["空闲", "idle"] };
  const CONV = {
    idle: ["未开始", "idle"], running: ["进行中", "run"], review: ["待审核", "review"],
    succeeded: ["已回复", "done"], finished: ["已结束", "done"], failed: ["失败", "bad"],
    blocked: ["受阻", "bad"], cancelled: ["已取消", "idle"], interrupted: ["已中断", "bad"],
  };
  const pick = (table, status) => table[status] || [status || "—", "idle"];
  const urgent = (status) => status === "running" || status === "review";

  // ---- avatars (sprite shared with the workbench) -------------------------------------
  const hueOf = (text) => [...String(text || "?")].reduce((hue, ch) => (hue * 31 + ch.charCodeAt(0)) % 360, 0);
  function avatar(provider, model, cls) {
    const key = `${model || ""} ${provider || ""}`.toLowerCase();
    const known =
      ["claude", "gpt", "gemini", "grok", "deepseek"].find((name) => key.includes(name)) ||
      { codex: "gpt", antigravity: "gemini" }[provider];
    if (known) return h("span.avatar.av-" + known + (cls ? "." + cls : ""), { "aria-hidden": "true" });
    return h("span.avatar.av-plain" + (cls ? "." + cls : ""), { "aria-hidden": "true", style: `--hue:${hueOf(provider)}` }, String(provider || "?").slice(0, 1).toUpperCase());
  }
  // A manager is its host CLI: known hosts reuse the portrait of the same-named model family.
  function hostAvatar(host, label, cls) {
    const known = { claude: "claude", codex: "gpt", grok: "grok", antigravity: "gemini" }[host];
    if (known) return h("span.avatar.av-" + known + (cls ? "." + cls : ""), { "aria-hidden": "true" });
    if (!host) return h("span.avatar.av-host" + (cls ? "." + cls : ""), { "aria-hidden": "true" });
    return h("span.avatar.av-plain" + (cls ? "." + cls : ""), { "aria-hidden": "true", style: `--hue:${hueOf(host)}` }, String(label || host).slice(0, 1).toUpperCase());
  }
  const colorClass = (n) => "c" + (Math.abs(Number(n) || 0) % 8);

  // ---- time ---------------------------------------------------------------------------
  const at = (iso) => {
    const t = Date.parse(iso || "");
    return Number.isNaN(t) ? 0 : t;
  };
  function relTime(iso) {
    const s = Math.max(0, (Date.now() - at(iso)) / 1000);
    if (s < 45) return "刚刚";
    if (s < 3600) return `${Math.round(s / 60)} 分钟前`;
    if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
    const d = new Date(at(iso));
    return `${d.getMonth() + 1}/${d.getDate()}`;
  }
  const clock = (iso) => new Date(at(iso)).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  const fullTime = (iso) => new Date(at(iso)).toLocaleString("zh-CN", { hour12: false });
  function dayLabel(iso) {
    const d = new Date(at(iso));
    const today = new Date();
    const diff = Math.round((new Date(today.getFullYear(), today.getMonth(), today.getDate()) - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 864e5);
    return diff === 0 ? "今天" : diff === 1 ? "昨天" : `${d.getMonth() + 1} 月 ${d.getDate()} 日`;
  }

  // ---- in-place DOM updates -----------------------------------------------------------
  const setText = (node, text) => { if (node.textContent !== text) node.textContent = text; };
  const setAttr = (node, name, value) => {
    if (value == null) { if (node.hasAttribute(name)) node.removeAttribute(name); }
    else if (node.getAttribute(name) !== value) node.setAttribute(name, value);
  };
  const setClass = (node, className) => { if (node.className !== className) node.className = className; };
  // Make parent's children equal `nodes`, touching the DOM only where the order differs.
  function reconcile(parent, nodes) {
    nodes.forEach((node, i) => {
      if (parent.children[i] !== node) parent.insertBefore(node, parent.children[i] || null);
    });
    while (parent.children.length > nodes.length) parent.lastElementChild.remove();
  }
  // Stable order: ids keep their slot; new ids go first; rows only move when `rank` changes
  // (running/review before the rest). Server order is used only to seed new ids.
  function stickyOrder(previous, items) {
    const slot = new Map(previous.map((id, i) => [id, i]));
    const fresh = items.filter((x) => !slot.has(x.id));
    const kept = items.filter((x) => slot.has(x.id)).sort((a, b) => slot.get(a.id) - slot.get(b.id));
    return [...(previous.length ? fresh : []), ...kept, ...(previous.length ? [] : fresh)]
      .sort((a, b) => a.rank - b.rank) // Array.prototype.sort is stable
      .map((x) => x.id);
  }
  // 1.5 s polling that pauses while hidden and never overlaps itself.
  function poll(tick, every = 1500) {
    let timer = null, busy = false;
    async function run() {
      clearTimeout(timer);
      if (document.hidden || busy) return;
      busy = true;
      try { await tick(); } finally {
        busy = false;
        if (!document.hidden) timer = setTimeout(run, every);
      }
    }
    document.addEventListener("visibilitychange", run);
    window.addEventListener("focus", run);
    run();
    return run;
  }
  // Markdown punctuation dropped; blank runs collapsed (used for one-line previews).
  const plain = (text) =>
    String(text || "").split("\n").filter((l) => !/^\s*```/.test(l))
      .map((l) => l.replace(/^\s{0,3}(#{1,6}|>)\s*/, "").replace(/(\*\*|__|`)/g, "").trimEnd())
      .join("\n").replace(/\n{3,}/g, "\n\n").trim();

  document.addEventListener("click", (event) => {
    const link = event.target.closest("a[href]");
    if (mock() && link?.getAttribute("href").startsWith("/app/")) event.preventDefault();
  });

  window.AN = { h, $, load, href, MEMBER, GROUP, CONV, pick, urgent, avatar, hostAvatar, colorClass, at, relTime, clock, fullTime, dayLabel, setText, setAttr, setClass, reconcile, stickyOrder, poll, plain };
})();
