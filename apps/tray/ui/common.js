"use strict";
// Helpers shared by the panel and the chat page. Classic script, one global: AN.
// Every node is built with createElement/textContent.
{

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
  // WebKit's own menu (Reload, Back, Inspect…) offers nothing on these pages. Text entry keeps it
  // for copy and paste, and so does selected text under the pointer; workspaces and rooms open
  // their own menus. A menu opened from the keyboard carries no pointer position, so a selection
  // anywhere keeps it there.
  const NON_TEXT_INPUTS = new Set(["checkbox", "radio", "button", "submit", "reset", "range", "color", "file", "image", "hidden"]);
  function overSelection(x, y) {
    const selection = getSelection();
    if (!String(selection)) return false;
    if (!x && !y) return true;
    for (let i = 0; i < selection.rangeCount; i++) {
      for (const r of selection.getRangeAt(i).getClientRects()) {
        if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) return true;
      }
    }
    return false;
  }
  document.addEventListener("contextmenu", (e) => {
    const field = e.target.closest?.("input, textarea");
    if (e.target.isContentEditable || (field && !NON_TEXT_INPUTS.has(field.type)) || overSelection(e.clientX, e.clientY)) return;
    e.preventDefault();
  });
  // Bilingual text: the page's <html lang> carries the App's interface language. The App sets
  // it when serving the page, including the system language, before any text is built.
  const root = document.documentElement;
  const translations = new Map(), localizedTables = [];
  const L = (zh, en) => {
    if (typeof zh === "string" && typeof en === "string") translations.set(zh, en);
    return root.lang.startsWith("en") ? en : zh;
  };
  const labels = [...document.querySelectorAll("[data-en]")].map(node => {
    const text = [...(node.childNodes || [])].find(n => n.nodeType === 3 && n.nodeValue.trim());
    return { node, text, zh: text?.nodeValue || node.textContent, en: node.dataset.en };
  });
  const attributes = ["title", "label"].flatMap(key => [...document.querySelectorAll(`[data-en-${key}]`)].map(node => ({
    node, name: key === "label" ? "aria-label" : "title", zh: node.getAttribute(key === "label" ? "aria-label" : "title"), en: node.dataset[key === "label" ? "enLabel" : "enTitle"],
  })));
  function setLanguage(language) {
    const english = String(language).startsWith("en");
    const previous = root.lang.startsWith("en");
    root.lang = language;
    for (const label of labels) {
      const text = english ? label.en : label.zh;
      if (label.text) label.text.nodeValue = text;
      else if (!label.node.children.length) label.node.textContent = text;
      if (label.node.tagName === "TITLE") document.title = text;
    }
    for (const attr of attributes) attr.node.setAttribute(attr.name, english ? attr.en : attr.zh);
    if (english !== previous) {
      const lookup = english ? translations : new Map([...translations].map(([zh, en]) => [en, zh]));
      for (const table of localizedTables) for (const entry of Object.values(table)) if (lookup.has(entry[0])) entry[0] = lookup.get(entry[0]);
      for (const node of document.querySelectorAll("*")) {
        // Conversation text and names belong to the workspace, not the interface language.
        if (node.closest?.("#tl-inner, .zone, .floor-head, .voice-queue-line, .voice-album-title, .voice-album-who, .voice-queue-who, .voice-queue-album, .gi-label, .gi-prev, .dr-title, .dr-sub, textarea, input")) continue;
        for (const text of node.childNodes || []) if (text.nodeType === 3 && lookup.has(text.nodeValue)) text.nodeValue = lookup.get(text.nodeValue);
        for (const name of ["title", "aria-label", "placeholder"]) {
          const value = node.getAttribute(name);
          if (lookup.has(value)) node.setAttribute(name, lookup.get(value));
        }
      }
    }
  }
  setLanguage(root.lang);

  // The App serves its pages over agentnave: on macOS and Linux, and over http://agentnave.localhost
  // on Windows, whose WebView2 has no custom schemes. Both are the App; a browser preview is neither.
  const inApp = () => location.protocol === "agentnave:" || location.hostname === "agentnave.localhost";

  async function load(path) {
    const response = await fetch(path, { cache: "no-store" });
    if (!response.ok) throw Object.assign(new Error(String(response.status)), { status: response.status });
    const data = await response.json();
    return root.lang.startsWith("en") ? englishLabels(data) : data;
  }
  // The tray names desktop hosts in Chinese ("Claude 桌面"); the English interface shows "Claude Desktop".
  function englishLabels(value) {
    if (Array.isArray(value)) value.forEach(englishLabels);
    else if (value && typeof value === "object") {
      for (const [key, item] of Object.entries(value)) {
        const name = (text) => typeof text === "string" ? text.replace(/ 桌面$/, " Desktop") : text;
        if (key === "label" || key === "previous_label") value[key] = name(item);
        else if (key === "labels" && Array.isArray(item)) value[key] = item.map(name);
        else englishLabels(item);
      }
    }
    return value;
  }
  const href = (path) => path;

  async function post(path, body) {
    const response = await fetch(path, {
      method: "POST",
      headers: {
        "X-AgentNave-Action": "1",
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body || {}),
    });
    if (!response.ok)
      throw Object.assign(new Error((await response.json().catch(() => ({}))).error || String(response.status)), {
        status: response.status,
      });
    return response.json();
  }

  // The install runs for about a minute: POST returns at once, progress is /app/status.service.state.
  async function installService() {
    const started = await post("/app/service/install", {});
    if (!started.ok) throw new Error(started.message || L("已有安装正在进行", "An installation is already in progress"));
    for (let i = 0; i < 300; i++) {
      await new Promise((resolve) => setTimeout(resolve, 800));
      // A closed window stays loaded; check again once it is shown, the install keeps going.
      if (document.hidden) await new Promise((resolve) => document.addEventListener("visibilitychange", resolve, { once: true }));
      const state = (await load("/app/status")).service;
      if (state?.state === "failed")
        throw new Error(state.error || L("安装失败，请重试。", "Installation failed, please try again."));
      if (state?.state === "done") return;
    }
    throw new Error(L("安装超时，请稍后到“设置 → 版本与更新”查看。", "Installation timed out. Check Settings → About later."));
  }

  // ---- vocabulary ---------------------------------------------------------------------
  // [label, tone]; tone selects one of --run --review --bad --done --idle.
  const MEMBER = {
    running: [L("进行中", "Running"), "run"], review: [L("待审核", "Review"), "review"], succeeded: [L("已回复", "Replied"), "done"],
    failed: [L("失败", "Failed"), "bad"], blocked: [L("受阻", "Blocked"), "bad"], cancelled: [L("已取消", "Cancelled"), "idle"],
    interrupted: [L("已中断", "Interrupted"), "bad"], pending: [L("等待中", "Pending"), "idle"],
  };
  const GROUP = { running: [L("运行中", "Running"), "run"], review: [L("待审核", "Review"), "review"], failed: [L("有失败", "Failed"), "bad"], idle: [L("空闲", "Idle"), "idle"] };
  localizedTables.push(MEMBER, GROUP);
  const pick = (table, status) => table[status] || [status || "—", "idle"];
  const urgent = (status) => status === "running" || status === "review";
  // What a desk is doing, in one word and tone. A desk in a meeting and a desk chatting privately are
  // different threads; both can run at once. `room` is the meeting room the desk sits in, or null.
  function deskState(d, room) {
    const privRun = d.private.running > 0;
    const inReview = d.meeting.status === "review";
    if (d.status === "running") {
      if (room) return { word: `${L("会议中", "In meeting")}${inReview ? L(" · 待审核", " · Review") : ""} · ${room.title}`, tone: inReview && !privRun ? "review" : "run", working: !inReview || privRun };
      if (privRun) return { word: L("私聊处理中", "In private chat"), tone: "run", working: true };
      return { word: L("待审核", "Review"), tone: "review", working: false };
    }
    if (d.status === "pending") return { word: L("等待中", "Pending"), tone: "idle", working: false };
    if (d.status === "failed") return { word: L("上次失败", "Last failed"), tone: "bad", working: false };
    return { word: L("空闲", "Idle"), tone: "idle", working: false };
  }

  function avatar(provider, model, cls) {
    return Avatars.create("model", provider, model, cls);
  }
  const colorClass = (n) => "c" + (Math.abs(Number(n) || 0) % 8);

  // ---- time ---------------------------------------------------------------------------
  const at = (iso) => {
    const t = Date.parse(iso || "");
    return Number.isNaN(t) ? 0 : t;
  };
  function relTime(iso) {
    const s = Math.max(0, (Date.now() - at(iso)) / 1000);
    if (s < 45) return L("刚刚", "just now");
    if (s < 3600) return L(`${Math.round(s / 60)} 分钟前`, `${Math.round(s / 60)}m ago`);
    if (s < 86400) return L(`${Math.floor(s / 3600)} 小时前`, `${Math.floor(s / 3600)}h ago`);
    const d = new Date(at(iso));
    return `${d.getMonth() + 1}/${d.getDate()}`;
  }
  // Formatters are built once per interface language; a language switch picks another one.
  const formatters = new Map();
  const formatter = (kind, options) => {
    const locale = L("zh-CN", "en-US"), key = `${kind}|${locale}`;
    if (!formatters.has(key)) formatters.set(key, new Intl.DateTimeFormat(locale, options));
    return formatters.get(key);
  };
  const clock = (iso) => formatter("clock", { hour: "2-digit", minute: "2-digit", hour12: false }).format(at(iso));
  const fullTime = (iso) => formatter("full", { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric", hour12: false }).format(at(iso));
  function dayLabel(iso) {
    const d = new Date(at(iso));
    const today = new Date();
    const diff = Math.round((new Date(today.getFullYear(), today.getMonth(), today.getDate()) - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 864e5);
    return diff === 0 ? L("今天", "Today") : diff === 1 ? L("昨天", "Yesterday") : L(`${d.getMonth() + 1} 月 ${d.getDate()} 日`, d.toLocaleDateString("en-US", { month: "short", day: "numeric" }));
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
  // A window the App hid (data-window-hidden) is as good as a hidden page; the App also announces the change
  // with an "agentnave:visibility" event, and the attribute is watched as well.
  const isHidden = () => document.hidden || root.dataset?.windowHidden === "true";
  const onVisibility = (fn) => {
    document.addEventListener("visibilitychange", fn);
    window.addEventListener("agentnave:visibility", fn);
    if (typeof MutationObserver === "function") new MutationObserver(fn).observe(root, { attributes: true, attributeFilter: ["data-window-hidden"] });
  };
  // 1.5 s polling that pauses while hidden, resumes with one poll when shown, and never overlaps itself.
  // An unfocused window (see syncBlurred) polls less often.
  function poll(tick, every = 1500) {
    let timer = null, busy = false;
    async function run() {
      clearTimeout(timer);
      if (isHidden() || busy) return;
      busy = true;
      try { await tick(); } finally {
        busy = false;
        if (!isHidden()) timer = setTimeout(run, blurred ? Math.max(every, BLURRED_POLL) : every);
      }
    }
    onVisibility(run);
    window.addEventListener("focus", run);
    window.addEventListener("agentnave:focus", run);
    run();
    return run;
  }
  // Markdown punctuation dropped; blank runs collapsed (used for one-line previews).
  const plain = (text) =>
    String(text || "").split("\n").filter((l) => !/^\s*```/.test(l))
      .map((l) => l.replace(/^\s{0,3}(#{1,6}|>)\s*/, "").replace(/(\*\*|__|`)/g, "").trimEnd())
      .join("\n").replace(/\n{3,}/g, "\n\n").trim();

  // Appearance is shared with settings and the panel through App preferences.
  const applyTheme = window.ANAppearance.applyTheme;
  const themeSwitch = () => window.ANAppearance.themeSwitch(h, L);

  // Which system the App runs on; the bar draws the window buttons that system would.
  function shellOS() {
    const ua = navigator.userAgent;
    if (/Windows/i.test(ua)) return "windows";
    if (/Mac OS X|Macintosh/i.test(ua)) return "macos";
    return "linux";
  }

  // The caption buttons Windows would draw at the right end of its title bar: minimize, maximize
  // or restore, close. They are the title bar's buttons rather than page buttons, so hovering one
  // never starts a window drag, and the close button turns red under the pointer like the system's.
  function captionButtons(topBar, act) {
    const glyph = (d, cls) => {
      const ns = "http://www.w3.org/2000/svg";
      const svg = document.createElementNS(ns, "svg");
      svg.setAttribute("viewBox", "0 0 10 10");
      svg.setAttribute("aria-hidden", "true");
      if (cls) svg.setAttribute("class", cls);
      const path = document.createElementNS(ns, "path");
      path.setAttribute("d", d);
      svg.append(path);
      return svg;
    };
    const button = (cls, title, ...kids) =>
      h(`button.win-btn.${cls}`, { type: "button", title, "aria-label": title }, ...kids);
    // Each glyph fills its 10-unit box (the bar is 48px, half again the system's 32px title bar):
    // a dash as wide as the system's, a square the same measure, and a cross that reads at arm's
    // length instead of the specks a 10px box draws at this height.
    const minimize = button("win-min", L("最小化", "Minimize"), glyph("M1 5h8"));
    const maximize = button("win-max", L("最大化", "Maximize"),
      glyph("M1.5 1.5h7v7h-7z", "maximize"), glyph("M1.5 3.5h5v5h-5zM3.5 3.5V1.5h5v5H6.5", "restore"));
    const close = button("win-close", L("关闭", "Close"), glyph("M1.5 1.5l7 7M8.5 1.5l-7 7"));
    minimize.addEventListener("click", () => act("/app/window/minimize"));
    maximize.addEventListener("click", () => act("/app/window/zoom"));
    close.addEventListener("click", () => act("/app/window/close"));
    // The maximize button reads as restore whenever the window is maximized, however it got there.
    let timer = 0;
    const syncMaximized = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        fetch("/app/window/state").then((r) => (r.ok ? r.json() : null))
          .then((state) => { if (state) document.documentElement.classList.toggle("maximized", !!state.maximized); })
          .catch(() => {});
      }, 120);
    };
    addEventListener("resize", syncMaximized);
    syncMaximized();
    (topBar.querySelector(".bar-end") || topBar).append(h("div.win-ctl", {}, minimize, maximize, close));
  }

  if (root.dataset.embedded !== "true") document.querySelector("header.bar")?.append(h("div.bar-end", {}, themeSwitch()));
  // The top bar is the window's title bar, but only when the page is the App's own (see inApp).
  // Each system keeps its own window buttons: macOS draws traffic lights at the left of the bar,
  // Windows has no title bar to put them in, so the bar carries Windows' caption buttons at the
  // right. Either way the bar drags the window and zooms it on double-click.
  const bar = document.querySelector("header.bar");
  if (bar && inApp() && root.dataset.embedded !== "true") {
    const shell = shellOS();
    document.documentElement.classList.add("titlebar", `os-${shell}`);
    const blank = (e) => e.button === 0 && !e.target.closest("a, button, input, select, label, textarea");
    const act = (path) => fetch(path, { method: "POST", headers: { "X-AgentNave-Action": "1" } }).catch(() => {});
    bar.addEventListener("mousedown", (e) => { if (blank(e) && e.detail === 1) act("/app/window/drag"); });
    bar.addEventListener("dblclick", (e) => { if (blank(e)) act("/app/window/zoom"); });
    if (shell === "windows") captionButtons(bar, act);
  }
  // Painted art pauses with the window: a hidden window runs no animation at all.
  const syncPaused = () => root.classList?.toggle("paused", isHidden());
  onVisibility(syncPaused);
  syncPaused();
  // A page that sets data-idle-blur on <html> goes quiet while its window is behind others: figures
  // and decorations stand still (html.blurred) and polling slows down. The App also reports the
  // window's focus as data-window-blurred, for when the page gets no blur events. Coming back
  // refreshes at once.
  const BLURRED_POLL = 5000;
  let blurred = false;
  function syncBlurred() {
    const next = root.dataset?.idleBlur !== undefined && (root.dataset.windowBlurred === "true" || !document.hasFocus());
    if (next === blurred) return;
    blurred = next;
    root.classList?.toggle("blurred", next);
    window.dispatchEvent(new Event(next ? "agentnave:blur" : "agentnave:focus"));
  }
  // Focus moving into an embedded frame blurs this window too; ask again once it has settled.
  const checkBlurred = () => setTimeout(syncBlurred, 0);
  window.addEventListener("blur", checkBlurred);
  window.addEventListener("focus", checkBlurred);
  if (typeof MutationObserver === "function") {
    new MutationObserver(syncBlurred).observe(root, { attributes: true, attributeFilter: ["data-window-blurred"] });
  }
  syncBlurred();

  // ---- painted office art (ui/art, served at /art/) -----------------------------------------
  // Characters follow the model family, like the avatars; a model without a character of its own is
  // the default newcomer. The CLI brand is never used to guess a model, except DeepSeek Harness:
  // its boss and desks wear DeepSeek's character when their model is unknown or has none.
  const FAMILY_ART = { claude: "claude", gpt: "codex", deepseek: "deepseek", gemini: "gemini", glm: "glm", grok: "grok", qwen: "qwen",
    kimi: "kimi", minimax: "minimax", hunyuan: "hunyuan", doubao: "doubao", muse: "muse" };
  const STATES = ["idle", "blink", "working", "speaking", "review", "done", "sleep"];
  // Optional frames: a character without them keeps the older look (grey idle, gliding seat).
  const EXTRA = ["sad", "walk-1", "walk-2"];
  const missing = new Set();
  // The App supplies its release version.
  const artVersion = typeof AGENTNAVE_ART_VERSION === "undefined" ? "preview" : AGENTNAVE_ART_VERSION;
  const artUrl = (path) => href(`/art/${path}?v=${encodeURIComponent(artVersion)}`);
  // A family's replacement set (Settings → 头像) wins over the painted one: "@family@version".
  let replaced = {}, version = 0;
  const CHARACTERS_KEY = "agentnave.characters";
  const loadCharacters = () => fetch(href("/app/characters"), { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : {})).then((data) => {
      if (JSON.stringify(data || {}) !== JSON.stringify(replaced)) { replaced = data || {}; version++; }
    }).catch(() => {});
  loadCharacters();
  // The window that saved a set tells the others through storage.
  window.addEventListener("storage", (e) => { if (e.key === CHARACTERS_KEY) loadCharacters(); });
  const charactersChanged = () => {
    try { localStorage.setItem(CHARACTERS_KEY, String(Date.now())); } catch { /* storage may be unavailable */ }
    return loadCharacters();
  };
  const character = (provider, model) => {
    const family = window.Avatars?.family(provider, model) || (provider === "deepseek" ? "deepseek" : null);
    return replaced[family] ? `@${family}@${replaced[family]}` : FAMILY_ART[family] || "default";
  };
  // Built-in characters may carry moving parts as layers of their own (<part>-<state>.webp): an
  // ahoge (tuft), cat ears (ear-l, ear-r), lop ears (lop-l, lop-r), bat wings (wing-l, wing-r), a whale tail, a two-piece devil tail, a hairpin pendant or a starburst halo. parts.json gives each frame's pivot at the part's root,
  // in percent of the frame, also as --ox/--oy for masks that must stay centred on it.
  const parts = fetch(artUrl("parts.json")).then((r) => (r.ok ? r.json() : {})).catch(() => ({}));
  // Some characters give off little sprites instead (notes, bubbles, leaves): emit.json names them and,
  // per frame, where they start in percent of the frame. The office mood sets how often; asleep, failed,
  // walking, hidden or with reduced motion nothing comes out.
  const emits = fetch(artUrl("emit.json")).then((r) => (r.ok ? r.json() : {})).catch(() => ({}));
  const EMIT_EVERY = { idle: 2600, working: 700, speaking: 1000, review: 1800, done: 350 };
  const emitters = new Set();
  const still = window.ANAppearance.reducedMotion;
  // The 120 ms tick runs only while something can come out of it: an emitter exists and the window is shown,
  // in front and moving.
  let emitTimer = null;
  function syncEmitTimer() {
    const wanted = emitters.size > 0 && !isHidden() && !blurred && !still.matches;
    if (wanted && !emitTimer) emitTimer = setInterval(emitTick, 120);
    else if (!wanted && emitTimer) { clearInterval(emitTimer); emitTimer = null; }
  }
  onVisibility(syncEmitTimer);
  window.addEventListener("agentnave:blur", syncEmitTimer);
  window.addEventListener("agentnave:focus", syncEmitTimer);
  still.addEventListener?.("change", syncEmitTimer);
  function emitTick() {
    if (isHidden() || blurred || still.matches) return syncEmitTimer();
    const now = performance.now();
    for (const e of emitters) {
      if (!e.fx.isConnected) { if (now - e.born > 5000) emitters.delete(e); continue; }
      if (e.fig.classList.contains("video") && !e.fig.classList.contains("walk")) {
        if (e.fx.childElementCount) e.fx.replaceChildren();
        continue;
      }
      const mood = e.fig.closest("[data-mood]")?.dataset.mood || "idle";
      const every = !e.fig.classList.contains("walk") && EMIT_EVERY[mood];
      if (!every || now < e.next) continue;
      e.next = now + every * (0.7 + 0.6 * Math.random());
      const [x, y] = e.spec.origin[mood] || e.spec.origin.idle;
      const sprite = e.spec.sprites[Math.floor(Math.random() * e.spec.sprites.length)];
      const img = h(`img.fx-${sprite.replace(/-\d+\.\w+$/, "")}`, { src: artUrl(`m/${e.name}/${sprite}`), alt: "", decoding: "async", draggable: "false",
        style: `left:${x}%;top:${y}%;--dx:${(Math.random() - 0.5).toFixed(2)};--life:${Math.round(every < 1000 ? 1700 : 2300)}ms` });
      img.addEventListener("animationend", () => img.remove(), { once: true });
      e.fx.append(img);
    }
    syncEmitTimer();
  }
  const frameUrl = (name, size, state, staticOnly = false) => {
    if (!name.startsWith("@") || (!staticOnly && activeVideoPack())) return artUrl(`${size}/${videoChar(name)}/${state}.webp`);
    const [, family, version] = name.split("@");
    return href(`/character/${family}/${size}/${state}.webp?v=${version}`);
  };
  let videoPacks = {}, videoPackKey = "", videoHttp = "", videoExt = "mov";
  const VIDEO_PACKS_KEY = "agentnave.video-packs";
  function activeVideoPack() {
    const style = root.dataset.characterStyle || window.ANAppearance?.preferences?.().character_style || "static";
    const tier = style === "video-272" ? "272" : style === "video-544" ? "544" : "";
    const pack = tier && videoPacks[tier];
    return pack?.installed && pack.stamp ? { tier, stamp: pack.stamp } : null;
  }
  const videoEnabled = (name) => Boolean(VIDEO[videoChar(name)] && activeVideoPack());
  const videoPackUrl = (path) => {
    const pack = activeVideoPack();
    if (!pack) return artUrl(`video/${path}`);
    const query = `${pack.tier}/${path}?v=${encodeURIComponent(pack.stamp)}`;
    return videoHttp && path.endsWith(`.${videoExt}`) ? `${videoHttp}/${query}` : href(`/art/video/${query}`);
  };
  function syncVideoPackFigures() {
    const pack = activeVideoPack();
    const nextKey = pack ? `${pack.tier}:${pack.stamp}:${videoExt}` : "";
    if (nextKey === videoPackKey) return;
    videoPackKey = nextKey;
    version++;
    for (const el of document.querySelectorAll?.(".head[data-char]") || []) {
      el.style.backgroundImage = `url("${frameUrl(el.dataset.char, "h", el.dataset.state || "idle")}")`;
    }
    for (const fig of document.querySelectorAll?.(".fig[data-char]") || []) {
      if (fig.dataset.staticOnly === "true") continue;
      const name = fig.dataset.char, vchar = videoChar(name), size = fig.dataset.size || "s";
      if (name.startsWith("@")) {
        for (const s of [...STATES, ...EXTRA]) {
          const img = fig.querySelector(`img.s-${s}`);
          if (img) img.src = frameUrl(name, size, s);
        }
      }
      releaseFigure(fig);
      if (videoEnabled(name)) {
        const state = fig.dataset.video || "idle";
        let poster = fig.querySelector("img.poster");
        if (!poster) {
          poster = h("img.poster", { src: videoPackUrl(`${vchar}/${state}.webp`), alt: "", decoding: "async", draggable: "false" });
          fig.querySelector(".fig-b")?.append(poster);
        } else poster.src = videoPackUrl(`${vchar}/${state}.webp`);
        fig.classList.add("video");
        scheduleVideo(fig);
        removeParts(fig);
      } else {
        fig.classList.remove("video", "video-on");
        addParts(fig);
      }
    }
  }
  const loadVideoPacks = () => fetch(href("/app/video-packs"), { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : {})).then((data) => {
      videoPacks = data?.packs || {};
      videoHttp = typeof data?.http === "string" ? data.http : ""; videoExt = data?.ext === "webm" ? "webm" : "mov";
      syncVideoPackFigures();
    }).catch(() => {});
  loadVideoPacks();
  window.addEventListener("storage", (e) => { if (e.key === VIDEO_PACKS_KEY) loadVideoPacks(); });
  window.addEventListener("agentnave:preferences", syncVideoPackFigures);
  const videoPacksChanged = () => {
    try { localStorage.setItem(VIDEO_PACKS_KEY, String(Date.now())); } catch { /* storage may be unavailable */ }
    window.AgentNaveSettingsFrame?.changed?.();
    return loadVideoPacks();
  };
  // Each part carries its frame's s-<state> class, so it shows with that frame. A figure remembers its
  // part nodes (fig.partNodes) so a switch to video can drop them and a switch back can build them again.
  function addParts(fig, body = fig.querySelector(".fig-b"), name = fig.dataset.char, size = fig.dataset.size || "s") {
    if (name.startsWith("@") || !body || fig.partNodes) return;
    const nodes = fig.partNodes = [];
    parts.then((all) => {
      if (fig.partNodes !== nodes) return;
      for (const [part, frames] of Object.entries(all[name] || {})) {
        // A "<part>-tip" bends about its own pivot inside a box that swings about its "<part>"'s pivot.
        const root = part.endsWith("-tip") ? all[name][part.slice(0, -4)] : null;
        for (const [state, [x, y]] of Object.entries(frames)) {
          const img = h(`img.part.${part}.s-${state}`, { src: artUrl(`${size}/${name}/${part}-${state}.webp`), alt: "",
            decoding: "async", draggable: "false", style: `transform-origin:${x}% ${y}%;--ox:${x}%;--oy:${y}%` });
          const pivot = root?.[state];
          const node = pivot ? h(`span.chain.${part.slice(0, -4)}-chain`, { style: `transform-origin:${pivot[0]}% ${pivot[1]}%` }, img)
            : part === "halo" ? h("span.halo-fan", { style: `--ox:${x}%;--oy:${y}%` }, img) : img;
          nodes.push(node);
          body.append(node);
        }
      }
    });
  }
  function removeParts(fig) {
    for (const node of fig.partNodes || []) node.remove();
    fig.partNodes = null;
  }
  // One <img> per state; CSS shows the one matching the nearest mood class (.working, .review ...).
  // A missing optional frame marks the figure .no-<state> so CSS falls back.
  function figure(name, size = "s", staticOnly = false) {
    const fig = h("span.fig", { "data-char": name, "data-size": size, "aria-hidden": "true" });
    if (staticOnly) fig.dataset.staticOnly = "true";
    const vchar = videoChar(name), useVideo = !staticOnly && videoEnabled(name);
    if (useVideo) {
      fig.classList.add("video");
    }
    // Each figure keeps its own rhythm, so a row of desks never blinks or breathes in step.
    const r = Math.random;
    fig.style.cssText = `--phase:${(-7 * r()).toFixed(2)}s;--cycle:${(6 + 3 * r()).toFixed(2)}s;--breath:${(2.8 + r()).toFixed(2)}s`;
    const fit = VIDEO_FIT[vchar];
    if (fit) fig.style.cssText += `;--vs:${fit[0]};--vo:${fit[1]}%${fit[2] == null ? "" : `;--vx:${fit[2]}%`}`;
    const imgs = [...STATES, ...EXTRA].map((s) => {
      const path = frameUrl(name, size, s, staticOnly);
      if (missing.has(path)) { fig.classList.add(`no-${s}`); return null; }
      const img = h(`img.s-${s}`, { src: path, alt: "", decoding: "async", draggable: "false" });
      if (EXTRA.includes(s)) img.addEventListener("error", () => { missing.add(path); fig.classList.add(`no-${s}`); img.remove(); }, { once: true });
      return img;
    });
    // A video character shows its clip's own first frame until the clip plays, so the swap is seamless.
    if (useVideo) imgs.push(h("img.poster", { src: videoPackUrl(`${vchar}/idle.webp`), alt: "", decoding: "async", draggable: "false" }));
    const body = h("span.fig-b", {}, imgs.filter(Boolean));
    fig.append(h("span.fig-a", {}, body));
    if (!name.startsWith("@")) emits.then((all) => {
      if (!all[name]) return;
      const fx = h("span.fx");
      fig.append(fx);
      emitters.add({ name, fig, fx, spec: all[name], born: performance.now(), next: 0 });
      syncEmitTimer();
    });
    // A video character shows none of them; they are built when it falls back to CSS.
    if (!useVideo) addParts(fig, body, name, size);
    return fig;
  }
  // A character with video loops plays one <video> for its nearest mood; its images stay as the
  // fallback until the first loop plays, and for walking.
  const VIDEO_STATES = ["idle", "working", "speaking", "review", "done", "sleep", "sad"];
  const VIDEO = Object.fromEntries(["claude", "codex", "deepseek", "gemini", "glm", "grok", "qwen", "kimi", "minimax",
    "hunyuan", "doubao", "muse", "default"].map((name) => [name, VIDEO_STATES]));
  function videoChar(name) {
    if (!name?.startsWith("@")) return name;
    const family = name.split("@")[1];
    return (typeof FAMILY_ART === "object" && FAMILY_ART[family]) || family;
  }
  // The clips frame each character a little differently; this scale and its origin (from the top) put every head
  // top (hair, not horns, ears or headdresses) at 12% and every pair of shoes at 97.8% of the figure.
  const VIDEO_FIT = { claude: [0.986, 83.5], codex: [1.092, 95.4], deepseek: [1.065, 94.5], gemini: [1.212, 34.9, 33.5], glm: [1.03, 97.8],
    grok: [1.017, 153], qwen: [0.975, 90], kimi: [1.041, 75.1], minimax: [0.942, 76.8], hunyuan: [1.058, 110.5], doubao: [0.95, 84.5],
    muse: [0.946, 82], default: [0.964, 79] };
  const videoAssetUrl = (path) => (typeof videoPackUrl === "function" ? videoPackUrl(path) : artUrl(`video/${path}`));
  // These moods loop their main clip and now and then slip in another one from the same pose.
  const EXTRAS = { idle: ["slack-1", "slack-2"], sleep: ["sleep-1"] };
  // Chance per finished loop of slipping one in; idle splits it evenly between its two clips.
  const EXTRA_CHANCE = { idle: 0.3, sleep: 0.2 };
  const nextClip = (state) => {
    const extras = EXTRAS[state];
    return extras && Math.random() < EXTRA_CHANCE[state] ? extras[Math.floor(Math.random() * extras.length)] : state;
  };
  const videoPlayers = new Map();
  const pageMediaHidden = (fig) => document.hidden || root.dataset.artPaused === "true"
    || root.dataset.windowHidden === "true" || Boolean(fig.closest("[hidden], [inert]"));
  // A window behind others (html.blurred) freezes its players on the current frame; they are let go if it stays so.
  const windowBlurred = () => Boolean(root.classList?.contains("blurred"));
  const BLUR_RELEASE_MS = 45000, OFFSCREEN_RELEASE_MS = 10000;
  const videoVisible = (fig) => fig.isConnected && !pageMediaHidden(fig) && !windowBlurred()
    && !still.matches && !fig.classList.contains("offscreen") && !fig.classList.contains("walk");
  const currentPlayback = (rec, seq) => rec.seq === seq && rec.running && videoVisible(rec.fig);
  function cancelVideoCallbacks(slot) {
    const video = slot.video;
    if (slot.frame != null) video.cancelVideoFrameCallback(slot.frame);
    if (slot.raf != null) cancelAnimationFrame(slot.raf);
    slot.frame = slot.raf = null;
    video.onloadeddata = video.onerror = video.onended = video.onplaying = video.onseeked = null;
    slot.ready = null;
  }
  function unloadVideo(slot, remove = false) {
    slot.token++;
    cancelVideoCallbacks(slot);
    slot.video.pause();
    slot.video.removeAttribute("src");
    slot.video.load();
    slot.requested = false;
    slot.clip = null;
    slot.loaded = false;
    slot.video.classList.add("next");
    if (remove) slot.video.remove();
  }
  // A spare is dropped with its element, so a long-lived player never accumulates source changes.
  function dropSlot(rec, slot) {
    unloadVideo(slot, true);
    rec.slots = rec.slots.filter((s) => s !== slot);
  }
  const dropSpares = (rec) => { for (const s of [...rec.slots]) if (s !== rec.active) dropSlot(rec, s); };
  // A released player leaves the frame it showed as a small canvas, so the figure does not jump to its
  // poster. A canvas that lost its transparency or came out blank is not used; the poster stays instead.
  function clearHold(rec) {
    if (!rec.hold) return;
    rec.hold.remove();
    rec.hold = null;
    rec.fig.classList.remove("held");
  }
  function captureFrame(rec) {
    const video = rec.active?.video;
    if (!video || !rec.fig.classList.contains("video-on") || video.readyState < 2 || !video.videoWidth) return null;
    try {
      const scale = Math.min(1, Math.max(64, Math.ceil(video.clientWidth * (window.devicePixelRatio || 1))) / video.videoWidth);
      const canvas = document.createElement("canvas");
      canvas.width = Math.round(video.videoWidth * scale);
      canvas.height = Math.round(video.videoHeight * scale);
      const g = canvas.getContext("2d");
      g.drawImage(video, 0, 0, canvas.width, canvas.height);
      const { data } = g.getImageData(0, 0, canvas.width, canvas.height);
      const alpha = (x, y) => data[(y * canvas.width + x) * 4 + 3];
      const corners = [alpha(0, 0), alpha(canvas.width - 1, 0), alpha(0, canvas.height - 1), alpha(canvas.width - 1, canvas.height - 1)];
      let drawn = false;
      for (let i = 3; i < data.length && !drawn; i += 4 * 16) drawn = data[i] > 0;
      if (!drawn || corners.some(Boolean)) return null;
      canvas.className = "hold";
      return canvas;
    } catch { return null; }
  }
  function releaseBlurred(rec) {
    const canvas = rec.fig.classList.contains("offscreen") || pageMediaHidden(rec.fig) ? null : captureFrame(rec);
    pauseVideos(rec, true);
    if (!canvas) return;
    rec.hold = canvas;
    rec.fig.classList.add("held");
    rec.fig.querySelector(".fig-b").append(canvas);
  }
  function pauseVideos(rec, release = false) {
    if (rec.offscreenTimer) { clearTimeout(rec.offscreenTimer); rec.offscreenTimer = null; }
    cancelSettle(rec);
    rec.seq++;
    rec.running = false;
    rec.incoming = rec.next = null;
    rec.waiting = false;
    for (const slot of rec.slots) {
      if (!release && slot === rec.active) { cancelVideoCallbacks(slot); slot.video.pause(); }
      else unloadVideo(slot, true);
    }
    rec.slots = release || !rec.active ? [] : [rec.active];
    if (release) { rec.active = null; rec.failed = false; rec.fig.classList.remove("video-on"); clearHold(rec); }
  }
  function releaseFigure(fig) {
    dirtyVideos.delete(fig);
    const rec = videoPlayers.get(fig);
    if (!rec) return;
    pauseVideos(rec, true);
    if (onScreen) onScreen.unobserve(fig);
    videoPlayers.delete(fig);
  }
  function failVideo(rec, seq) {
    if (!currentPlayback(rec, seq)) return;
    pauseVideos(rec, true);
    rec.failed = true;
  }
  function videoSlot(rec, clip) {
    let slot = rec.slots.find((s) => s.clip === clip);
    if (slot) return slot;
    dropSpares(rec);
    const video = document.createElement("video");
    video.className = "next";
    video.muted = true;
    video.playsInline = true;
    video.preload = "auto";
    // No cast or picture-in-picture, so the page does not watch for playback targets.
    video.disableRemotePlayback = true;
    video.disablePictureInPicture = true;
    slot = { video, token: 0, frame: null, raf: null };
    rec.slots.push(slot);
    slot.clip = clip;
    slot.state = rec.state;
    slot.video.dataset.state = rec.state;
    slot.video.loop = !EXTRAS[rec.state];
    return slot;
  }
  function prepareClip(rec, clip, ready) {
    const slot = videoSlot(rec, clip), seq = rec.seq, token = slot.token;
    slot.ready = ready;
    if (slot.loaded) { ready(slot); return slot; }
    if (slot.requested) return slot;
    slot.requested = true;
    const valid = () => currentPlayback(rec, seq) && slot.token === token;
    slot.video.onloadeddata = () => {
      if (!valid()) return;
      slot.loaded = true;
      slot.video.onloadeddata = null;
      slot.ready?.(slot);
    };
    slot.video.onerror = () => {
      if (!valid()) return;
      failVideo(rec, seq);
    };
    slot.video.src = videoAssetUrl(`${videoChar(rec.fig.dataset.char)}/${clip}.${typeof videoExt === "string" ? videoExt : "mov"}`);
    if (rec.active) rec.active.video.before(slot.video);
    else rec.fig.querySelector(".fig-b").append(slot.video);
    return slot;
  }
  // Main loops reuse the player. Only an actual clip change needs the second slot;
  // after its first painted frame the former player becomes the reusable spare.
  function prepareNext(rec) {
    rec.next = null;
    const clip = nextClip(rec.state);
    if (clip === rec.active.clip) {
      if (clip === rec.state) dropSpares(rec);
      rec.next = rec.active;
      return;
    }
    prepareClip(rec, clip, (slot) => {
      rec.next = slot;
      if (rec.waiting) runClip(rec, slot);
    });
  }
  // After a swap, the next clip is prepared and the old player is torn down only once the new player has
  // presented a frame of its own while visible. Doing it in the swap's own task blanks that frame on screen,
  // and every desk that swaps in the same moment goes blank together.
  function cancelSettle(rec) {
    const job = rec.settle;
    if (!job) return;
    rec.settle = null;
    if (job.frame != null) job.slot.video.cancelVideoFrameCallback(job.frame);
    if (job.raf != null) cancelAnimationFrame(job.raf);
  }
  function settleVideo(rec) {
    const job = rec.settle;
    if (!job) return;
    cancelSettle(rec);
    if (rec.active !== job.slot) return;
    if (job.slot.video.classList.contains("over")) {
      job.slot.video.classList.remove("over");
      for (const s of rec.slots) if (s !== rec.active) s.video.classList.add("next");
    }
    if (!job.slot.video.loop) prepareNext(rec);
    else dropSpares(rec);
  }
  function settleAfterFrame(rec, slot) {
    const job = { slot, frame: null, raf: null };
    rec.settle = job;
    const run = () => settleVideo(rec);
    if (slot.video.requestVideoFrameCallback) job.frame = slot.video.requestVideoFrameCallback(run);
    else job.raf = requestAnimationFrame(() => { job.raf = requestAnimationFrame(run); });
  }
  const overlapSwap = () => typeof videoExt === "string" && videoExt === "webm";
  function finishVideoFrame(rec, slot, seq) {
    if (!currentPlayback(rec, seq) || rec.incoming !== slot) return;
    // WebM (Chromium) blanks the frame in which a hidden player appears and the old one hides at once. There the
    // new player shows above the old one, which stays until the new one has presented a frame of its own; the
    // clips meet on the same pose, so the overlap is not visible. Elsewhere the old player hides and the new one
    // shows in this one task, so both are never on screen together.
    const overlap = overlapSwap();
    if (rec.active && rec.active !== slot) {
      cancelVideoCallbacks(rec.active);
      rec.active.video.pause();
      if (!overlap) rec.active.video.classList.add("next");
    }
    slot.video.classList.remove("next");
    if (overlap) slot.video.classList.add("over");
    clearHold(rec);
    rec.active = slot;
    rec.incoming = null;
    rec.fig.classList.add("video-on");
    rec.fig.querySelector(".fx")?.replaceChildren();
    settleAfterFrame(rec, slot);
  }
  // Figures of one character play the same clips, so starting them together keeps them in lockstep for good
  // (a mood change restarts every desk at once too). A random wait before a clip starts spreads them out;
  // a resume after hiding or blur is not delayed, since the players keep the phase they stopped at.
  const FIRST_START_MS = 2000, CHANGE_START_MS = 300;
  function startStaggered(rec, slot) {
    const seq = rec.seq, scale = typeof videoStartJitter === "number" ? videoStartJitter : 1;
    const wait = Math.random() * scale * (rec.active ? CHANGE_START_MS : FIRST_START_MS);
    if (wait < 1) { runClip(rec, slot); return; }
    setTimeout(() => { if (rec.seq === seq) runClip(rec, slot); }, wait);
  }
  function runClip(rec, slot) {
    const seq = rec.seq;
    if (!currentPlayback(rec, seq)) return;
    rec.waiting = false;
    const video = slot.video;
    cancelVideoCallbacks(slot);
    video.onerror = () => failVideo(rec, seq);
    video.onended = () => {
      if (!currentPlayback(rec, seq)) return;
      settleVideo(rec);
      if (rec.next === slot) {
        video.currentTime = 0;
        video.play().catch(() => {});
        prepareNext(rec);
      } else if (rec.next) runClip(rec, rec.next);
      else rec.waiting = true;
    };
    if (rec.active === slot) {
      if (!video.loop) prepareNext(rec);
      if (video.ended) video.currentTime = 0;
      video.play().catch(() => {});
      return;
    }
    // A clip change starts hidden: the new player first returns to its first frame if it was played before,
    // then plays until that frame is presented, and only then does finishVideoFrame swap the two.
    rec.incoming = slot;
    const painted = () => {
      slot.frame = null;
      slot.raf = requestAnimationFrame(() => {
        slot.raf = null;
        finishVideoFrame(rec, slot, seq);
      });
    };
    const start = () => {
      if (video.requestVideoFrameCallback) slot.frame = video.requestVideoFrameCallback(painted);
      else video.onplaying = () => { video.onplaying = null; slot.raf = requestAnimationFrame(painted); };
      video.play().catch(() => {});
    };
    if (video.ended || video.currentTime > 0) {
      video.onseeked = () => { video.onseeked = null; start(); };
      video.currentTime = 0;
    } else start();
  }
  // Geometry is one gate; hidden documents, covered offices, and retained settings
  // documents also stop media explicitly rather than relying on CSS animations.
  const onScreen = typeof IntersectionObserver === "function" && new IntersectionObserver((entries) => {
    for (const { target: fig, isIntersecting } of entries) {
      fig.classList.toggle("offscreen", !isIntersecting);
      scheduleVideo(fig);
    }
  });
  function syncVideo(fig) {
    if (!fig.isConnected || !fig.classList.contains("video")) { releaseFigure(fig); return; }
    const name = videoChar(fig.dataset.char);
    let rec = videoPlayers.get(fig);
    if (!rec) {
      rec = { fig, seq: 0, slots: [], active: null, running: false, state: null, offscreenTimer: null };
      videoPlayers.set(fig, rec);
      if (onScreen) { fig.classList.add("offscreen"); onScreen.observe(fig); }
    }
    const hiddenMedia = still.matches || pageMediaHidden(fig);
    if (hiddenMedia) { rec.failed = false; clearHold(rec); }
    if (hiddenMedia && rec.slots.length) pauseVideos(rec, true);
    if (fig.classList.contains("warping")) {
      if (!videoVisible(fig) && rec.running) pauseVideos(rec);
      return;
    }
    const mood = fig.closest("[data-mood]")?.dataset.mood || "idle";
    const wanted = mood === "failed" ? "sad" : mood;
    const state = VIDEO[name].includes(wanted) ? wanted : "idle";
    if (rec.state !== state) {
      // Only a player that keeps running may hold its old frame while the new clip loads.
      pauseVideos(rec, !videoVisible(fig));
      rec.state = state;
      rec.failed = false;
      fig.dataset.video = state;
      const poster = fig.querySelector("img.poster");
      if (poster) poster.src = videoAssetUrl(`${name}/${state}.webp`);
    }
    if (hiddenMedia) { if (rec.slots.length) pauseVideos(rec, true); return; }
    if (!videoVisible(fig)) {
      if (rec.running) pauseVideos(rec);
      const offscreen = fig.classList.contains("offscreen");
      if ((offscreen || windowBlurred()) && rec.slots.length) {
        const delay = offscreen ? OFFSCREEN_RELEASE_MS : BLUR_RELEASE_MS;
        if (rec.offscreenTimer && delay < rec.releaseDelay) { clearTimeout(rec.offscreenTimer); rec.offscreenTimer = null; }
        if (!rec.offscreenTimer) {
          rec.releaseDelay = delay;
          rec.offscreenTimer = setTimeout(() => {
            rec.offscreenTimer = null;
            if (!fig.isConnected || pageMediaHidden(fig) || fig.classList.contains("offscreen")) pauseVideos(rec, true);
            else if (windowBlurred()) releaseBlurred(rec);
          }, delay);
        }
      }
      return;
    }
    if (rec.offscreenTimer) { clearTimeout(rec.offscreenTimer); rec.offscreenTimer = null; }
    if (rec.running || rec.failed) return;
    rec.running = true;
    if (rec.active?.state === state) runClip(rec, rec.active);
    else prepareClip(rec, state, (slot) => startStaggered(rec, slot));
  }
  // Every character teleports the same way: a small crouch and a springy hop that shrinks her away, or a
  // drop that grows and settles her. It moves the figure's body, so the desk in front still covers it.
  const WARP = {
    out: { duration: 720, easing: "linear", frames: [
      { transform: "translateY(0) scale(1, 1)", opacity: 1, easing: "cubic-bezier(.3, 0, .5, 1)" },
      { transform: "translateY(1.5%) scale(1.04, 0.95)", opacity: 1, offset: 0.2, easing: "cubic-bezier(.2, .7, .4, 1)" },
      { transform: "translateY(-7%) scale(0.97, 1.05)", opacity: 1, offset: 0.48, easing: "cubic-bezier(.5, 0, .75, .4)" },
      { transform: "translateY(-13%) scale(0.02)", opacity: 0 },
    ] },
    in: { duration: 520, easing: "ease-in-out", frames: [
      { transform: "translateY(-14%) scale(0)", opacity: 0 },
      { transform: "translateY(-6%) scale(1.06)", opacity: 1, offset: 0.7 },
      { transform: "translateY(0) scale(1)", opacity: 1 },
    ] },
  };
  const canWarp = (name) => videoEnabled(name);
  // Resolves when the move ends. An exit keeps her shrunk a moment longer, until the figure itself is hidden.
  async function warp(fig, kind) {
    const { duration, easing, frames } = WARP[kind];
    fig.classList.add("warping");
    const body = fig.querySelector(".fig-b");
    const run = body.animate(frames.map((f) => ({ ...f, transformOrigin: "50% 40%" })), { duration, easing, fill: "forwards" });
    await run.finished.catch(() => {});
    fig.classList.remove("warping");
    if (kind === "out") setTimeout(() => run.cancel(), 400);
    else run.cancel();
  }
  let videoSync = 0;
  const dirtyVideos = new Set();
  function scheduleVideo(fig) {
    if (!fig.isConnected) { releaseFigure(fig); dirtyVideos.delete(fig); return; }
    if (videoPlayers.has(fig) && !videoVisible(fig)) { syncVideo(fig); return; }
    dirtyVideos.add(fig);
    videoSync ||= requestAnimationFrame(() => {
      videoSync = 0;
      const figures = [...dirtyVideos];
      dirtyVideos.clear();
      for (const node of figures) syncVideo(node);
    });
  }
  function scheduleVideoTree(node) {
    if (node.nodeType !== 1) return;
    if (node.matches(".fig.video")) scheduleVideo(node);
    for (const fig of node.querySelectorAll(".fig.video")) scheduleVideo(fig);
  }
  // Hidden documents can stop receiving rAF entirely: pausing must happen in the
  // visibility event itself, not in a frame that may never be delivered.
  const syncAllVideos = () => { for (const fig of videoPlayers.keys()) syncVideo(fig); };
  if (typeof MutationObserver === "function") new MutationObserver((records) => {
    for (const m of records) {
      if (m.type === "childList") {
        for (const node of [...m.addedNodes, ...m.removedNodes]) scheduleVideoTree(node);
      } else if (m.attributeName === "class") {
        if (m.target.matches(".fig.video") && ["walk", "warping"].some((name) =>
          (m.oldValue || "").split(/\s+/).includes(name) !== m.target.classList.contains(name))) scheduleVideo(m.target);
      } else if (m.oldValue !== m.target.getAttribute(m.attributeName)) {
        if (m.target === root && m.attributeName === "data-window-hidden") syncPaused();
        scheduleVideoTree(m.target);
      }
    }
  }).observe(root, { subtree: true, childList: true, attributes: true, attributeOldValue: true,
    attributeFilter: ["data-mood", "hidden", "inert", "class", "data-motion", "data-art-paused", "data-window-hidden", "data-on-battery", "data-battery-still"] });
  document.addEventListener("visibilitychange", syncAllVideos);
  window.addEventListener("agentnave:blur", syncAllVideos);
  window.addEventListener("agentnave:focus", syncAllVideos);
  window.addEventListener("pagehide", () => { for (const fig of [...videoPlayers.keys()]) releaseFigure(fig); });
  window.addEventListener("pageshow", () => scheduleVideoTree(root));
  window.matchMedia?.("(prefers-reduced-motion: reduce)")?.addEventListener?.("change", syncAllVideos);
  const head = (name, state = "idle") => h("span.head", { "data-char": name, "data-state": state, "aria-hidden": "true", style: `background-image:url("${frameUrl(name, "h", state)}")` });
  // A CLI's or host app's own mark (LobeHub icons, art/cli); models never borrow it. The value is its tile, if any;
  // a desktop app's mark takes the colours of its app icon.
  const CLI_MARK = { claude: "", "claude-desktop": "claude-tile", codex: "", "codex-desktop": "", codebuddy: "", workbuddy: "", antigravity: "", deepseek: "", devin: "", grok: "mono", pi: "mono", hermes: "", zcode: "mono" };
  function cliMark(provider) {
    if (provider && window.Avatars?.custom("host", provider, "")) return Avatars.create("host", provider, "");
    if (!Object.hasOwn(CLI_MARK, provider)) return null;
    return h(`span.avatar.cli-mark${CLI_MARK[provider] ? `.${CLI_MARK[provider]}` : ""}`, { "aria-hidden": "true" },
      h("img", { src: artUrl(`cli/${provider}.svg`), alt: "", draggable: "false" }));
  }
  // The laptop lid shows the CLI the desk runs in; the figure at it is the model.
  function lid(provider) {
    const mark = cliMark(provider);
    if (!mark) return h("img", { src: artUrl("logo-default.svg"), alt: "" });
    // Codex bakes a plain white tile into its logo; the other baked tiles are part of the picture
    if (provider === "codex" && mark.matches(".cli-mark")) mark.firstChild.src = artUrl("cli/lid/codex.svg");
    return mark;
  }
  // ---- CLIs and desks, shared by the office and the menu bar panel ---------------------------
  const CLI = { codex: "Codex", claude: "Claude Code", codebuddy: "CodeBuddy", grok: "Grok", antigravity: "Antigravity", pi: "Pi", deepseek: "DeepSeek Harness", devin: "Devin", zcode: "ZCode" };
  const cliName = (provider) => CLI[provider] || provider || "";
  // The boss tile and CLI badges show the CLI's mark, or its initial. A boss in a desktop app keeps the
  // claude or codex host and passes its app, claude-desktop or codex-desktop, for the mark.
  const hostTile = (host, label) => cliMark(host) || h("span.host-letter", {}, (label || cliName(host) || "?").slice(0, 1));
  // Model names drop the routing prefix ("opencode-go/deepseek-v4.1-flash" -> "deepseek-v4.1-flash")
  // unless the user chose full names in Settings; shared by every window through localStorage.
  const MODEL_NAMES_KEY = "agentnave.model-names";
  const fullModelNames = () => {
    try { return localStorage.getItem(MODEL_NAMES_KEY) === "full"; } catch { return false; }
  };
  // Names already drawn follow the choice at once: here by this event, in other windows by the storage event.
  const announceModelNames = (full) => window.dispatchEvent(new CustomEvent("agentnave:model-names", { detail: { full: Boolean(full) } }));
  const setFullModelNames = (full) => {
    try { localStorage.setItem(MODEL_NAMES_KEY, full ? "full" : "short"); } catch { /* storage may be unavailable */ }
    announceModelNames(full);
  };
  window.addEventListener("storage", (event) => { if (event.key === MODEL_NAMES_KEY) announceModelNames(event.newValue === "full"); });
  const modelName = (model) => (model && !fullModelNames() ? model.split("/").pop() || model : model);
  // Member names come from the service, as `identity` on a desk, seat, speaker or addressee; a view
  // it has not rebuilt yet has none and reads as unknown. These only pick from what it produced.
  // Antigravity has no effort option: each model it lists ends in its level, as in gemini-4-argon-high.
  // Other CLIs' names keep their ending, since a -max there is part of the model's name.
  const deskParts = (who) => {
    const id = who.identity;
    if (!id) return { name: L("未记录身份", "Identity not recorded"), effort: who.effort || "" };
    const name = id.model_source === "unknown"
      ? L("未记录模型", "Model not recorded")
      : (fullModelNames() && id.model) || id.model_short;
    const level = id.provider === "antigravity" ? name.match(/-(low|medium|high)$/) : null;
    const bare = level ? name.slice(0, level.index) : name;
    return { name: id.numbered ? `${bare} #${id.ordinal}` : bare, effort: who.effort || level?.[1] || "" };
  };
  const deskName = (who) => {
    const { name, effort } = deskParts(who);
    return effort ? `${name} · ${effort}` : name;
  };
  const localizedHandle = (identity, handle) => identity?.model_source === "unknown"
    ? handle?.replace(/^@未记录模型(?=·|$)/, `@${L("未记录模型", "Model not recorded")}`) : handle;
  const handleOf = (who) => localizedHandle(who.identity, fullModelNames() ? who.identity?.handle : who.identity?.handle_short) || deskName(who);
  const fullHandle = (who) => localizedHandle(who.identity, who.identity?.handle) || L("未记录身份，重连宿主后更新", "Identity not recorded; reconnect the host to update");
  const cliOf = (who) => who.identity?.cli || cliName(who.provider);
  // A workspace is named by its first dispatch's title; where it runs is the boss's CLI and project.
  const chatName = (g) => g.title || g.project || g.label;
  // The title a reader sees: a workspace waiting for a new boss says so until one takes it over.
  const shownName = (g) => (g.handover === "pending" ? L(`「待接手」${chatName(g)}`, `[Handover] ${chatName(g)}`) : chatName(g));
  const chatPlace = (g) => [g.label, g.project !== chatName(g) && g.project].filter(Boolean).join(" · ");
  // Cut long labels to n characters (by code point) with an ellipsis.
  const clip = (text, n) => { const chars = [...text]; return chars.length > n ? `${chars.slice(0, n - 1).join("")}…` : text; };
  // A desk's live activities: private chats and meeting seats can run at the same time.
  function activities(d) {
    if (Array.isArray(d.activities)) return d.activities;
    const out = [];
    if (d.meeting?.room) out.push({ kind: "meeting", room: d.meeting.room, seat: d.meeting.seat, status: d.meeting.status });
    if (d.private?.running > 0) out.push({ kind: "private", status: "running" });
    return out;
  }
  // A desk rests once it has had no work for two hours.
  const SLEEP_AFTER = 2 * 3600e3;
  const dozing = (d) => Date.now() - at(d.last_active) > SLEEP_AFTER;
  // A reviewed answer waits only briefly on the boss: it reads as work going on, not a state of its own.
  const ongoing = (status) => (status === "review" ? "running" : status);

  const art = { url: artUrl, character, figure, canWarp, warp, head, lid, STATES, EXTRA, charactersChanged, loadVideoPacks, videoPacksChanged,
    // Bumped whenever a set is replaced or restored: views fold it into their render caches.
    version: () => version };

  window.AN = { h, $, L, setLanguage, load, post, installService, href, shellOS, MEMBER, GROUP, pick, urgent, deskState, avatar, colorClass, at, relTime, clock, fullTime, dayLabel, setText, setAttr, setClass, reconcile, stickyOrder, isHidden, onVisibility, poll, plain, themeSwitch, applyTheme, reducedMotion: window.ANAppearance.reducedMotion, art, cliName, hostTile, deskName, deskParts, handleOf, fullHandle, cliOf, chatName, shownName, chatPlace, activities, dozing, SLEEP_AFTER, ongoing, clip, modelName, fullModelNames, setFullModelNames };
}
