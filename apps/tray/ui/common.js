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
  const pick = (table, status) => table[status] || [status || "—", "idle"];
  const urgent = (status) => status === "running" || status === "review";
  // What a desk is doing, in one word and tone. A desk in a meeting and a desk chatting privately are
  // different threads; both can run at once. `room` is the meeting room the desk sits in, or null.
  function deskState(d, room) {
    const privRun = d.private.running > 0;
    const inReview = d.meeting.status === "review";
    if (d.status === "running") {
      if (room) return { word: `会议中${inReview ? " · 待审核" : ""} · ${room.title}`, tone: inReview && !privRun ? "review" : "run", working: !inReview || privRun };
      if (privRun) return { word: "私聊处理中", tone: "run", working: true };
      return { word: "待审核", tone: "review", working: false };
    }
    if (d.status === "pending") return { word: "等待中", tone: "idle", working: false };
    if (d.status === "failed") return { word: "上次失败", tone: "bad", working: false };
    return { word: "空闲", tone: "idle", working: false };
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

  // ---- appearance: system / light / dark, shared by every window through localStorage ----------
  const THEME_KEY = "agentnave.theme";
  const lightQuery = matchMedia("(prefers-color-scheme: light)");
  const themeChoice = () => {
    try { return localStorage.getItem(THEME_KEY) || "system"; } catch { return "system"; }
  };
  function applyTheme() {
    const choice = themeChoice();
    document.documentElement.dataset.theme = choice === "system" ? (lightQuery.matches ? "light" : "dark") : choice;
    document.querySelectorAll("[data-theme-choice]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.themeChoice === choice)));
  }
  function themeSwitch() {
    const labels = { system: ["◐", "跟随系统"], light: ["☀", "浅色"], dark: ["☾", "深色"] };
    const group = h("div.theme", { role: "group", "aria-label": "外观" },
      ...Object.entries(labels).map(([choice, [icon, title]]) => {
        const b = h("button", { type: "button", title, "aria-label": title, "data-theme-choice": choice }, icon);
        b.addEventListener("click", () => {
          try { localStorage.setItem(THEME_KEY, choice); } catch { /* storage may be unavailable */ }
          applyTheme();
        });
        return b;
      }));
    queueMicrotask(applyTheme);
    return group;
  }
  lightQuery.addEventListener("change", applyTheme);
  addEventListener("storage", (e) => e.key === THEME_KEY && applyTheme());
  applyTheme();
  document.querySelector("header.bar")?.append(h("div.bar-end", {}, themeSwitch()));
  // On macOS the window has no title bar of its own (only the agentnave: scheme is the Mac App):
  // the top bar leaves room for the window buttons, drags the window, and zooms on double-click.
  const bar = document.querySelector("header.bar");
  if (bar && location.protocol === "agentnave:") {
    document.documentElement.classList.add("titlebar");
    const blank = (e) => e.button === 0 && !e.target.closest("a, button, input, select, label, textarea");
    const act = (path) => fetch(path, { method: "POST", headers: { "X-AgentNave-Action": "1" } }).catch(() => {});
    bar.addEventListener("mousedown", (e) => { if (blank(e) && e.detail === 1) act("/app/window/drag"); });
    bar.addEventListener("dblclick", (e) => { if (blank(e)) act("/app/window/zoom"); });
  }
  // Painted art pauses with the window: a hidden window runs no animation at all.
  const syncPaused = () => document.documentElement.classList.toggle("paused", document.hidden);
  document.addEventListener("visibilitychange", syncPaused);
  syncPaused();

  // ---- painted office art (ui/art, served at /art/) -----------------------------------------
  // Characters follow the model family, like the avatars; a model without a character of its own is
  // the default newcomer. The CLI brand is never used to guess a model.
  const FAMILY_ART = { claude: "claude", gpt: "codex", deepseek: "deepseek", gemini: "gemini", glm: "glm", grok: "grok", qwen: "qwen" };
  const STATES = ["idle", "blink", "working", "speaking", "review", "done", "sleep"];
  // Optional frames: a character without them keeps the older look (grey idle, gliding seat).
  const EXTRA = ["sad", "walk-1", "walk-2"];
  const missing = new Set();
  // The App supplies its release version; direct mock pages use a stable preview key.
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
    const family = window.Avatars?.family(provider, model);
    return replaced[family] ? `@${family}@${replaced[family]}` : FAMILY_ART[family] || "default";
  };
  // Built-in characters may carry moving parts as layers of their own (<part>-<state>.webp): an
  // ahoge (tuft), cat ears (ear-l, ear-r), bat wings (wing-l, wing-r), a hairpin pendant or a starburst halo. parts.json gives each frame's pivot at the part's root,
  // in percent of the frame, also as --ox/--oy for masks that must stay centred on it.
  const parts = fetch(artUrl("parts.json")).then((r) => (r.ok ? r.json() : {})).catch(() => ({}));
  const frameUrl = (name, size, state) => {
    if (!name.startsWith("@")) return artUrl(`${size}/${name}/${state}.webp`);
    const [, family, version] = name.split("@");
    return href(`/character/${family}/${size}/${state}.webp?v=${version}`);
  };
  // One <img> per state; CSS shows the one matching the nearest mood class (.working, .review ...).
  // A missing optional frame marks the figure .no-<state> so CSS falls back.
  function figure(name, size = "s") {
    const fig = h("span.fig", { "data-char": name, "aria-hidden": "true" });
    // Each figure keeps its own rhythm, so a row of desks never blinks or breathes in step.
    const r = Math.random;
    fig.style.cssText = `--phase:${(-7 * r()).toFixed(2)}s;--cycle:${(6 + 3 * r()).toFixed(2)}s;--breath:${(2.8 + r()).toFixed(2)}s`;
    const imgs = [...STATES, ...EXTRA].map((s) => {
      const path = frameUrl(name, size, s);
      if (missing.has(path)) { fig.classList.add(`no-${s}`); return null; }
      const img = h(`img.s-${s}`, { src: path, alt: "", decoding: "async", draggable: "false" });
      if (EXTRA.includes(s)) img.addEventListener("error", () => { missing.add(path); fig.classList.add(`no-${s}`); img.remove(); }, { once: true });
      return img;
    });
    const body = h("span.fig-b", {}, imgs.filter(Boolean));
    fig.append(h("span.fig-a", {}, body));
    // Each part carries its frame's s-<state> class, so it shows with that frame.
    if (!name.startsWith("@")) parts.then((all) => {
      for (const [part, frames] of Object.entries(all[name] || {})) {
        for (const [state, [x, y]] of Object.entries(frames)) {
          body.append(h(`img.part.${part}.s-${state}`, { src: artUrl(`${size}/${name}/${part}-${state}.webp`), alt: "",
            decoding: "async", draggable: "false", style: `transform-origin:${x}% ${y}%;--ox:${x}%;--oy:${y}%` }));
        }
      }
    });
    return fig;
  }
  const head = (name, state = "idle") => h("span.head", { "aria-hidden": "true", style: `background-image:url("${frameUrl(name, "h", state)}")` });
  // A CLI's own mark (LobeHub icons, art/cli); models never borrow it. Mono marks sit on a dark tile.
  const CLI_MARK = { claude: "", codex: "", codebuddy: "", antigravity: "", deepseek: "", grok: "mono", pi: "mono" };
  function cliMark(provider) {
    if (provider && window.Avatars?.custom("host", provider, "")) return Avatars.create("host", provider, "");
    if (!Object.hasOwn(CLI_MARK, provider)) return null;
    return h(`span.avatar.cli-mark${CLI_MARK[provider] ? ".mono" : ""}`, { "aria-hidden": "true" },
      h("img", { src: artUrl(`cli/${provider}.svg`), alt: "", draggable: "false" }));
  }
  // The laptop lid shows the CLI the desk runs in; the figure at it is the model.
  const lid = (provider) => cliMark(provider) || h("img", { src: artUrl("logo-default.svg"), alt: "" });
  // ---- CLIs and desks, shared by the office and the menu bar panel ---------------------------
  const CLI = { codex: "Codex", claude: "Claude Code", codebuddy: "CodeBuddy", grok: "Grok", antigravity: "Antigravity", pi: "Pi", deepseek: "DeepSeek Harness" };
  const cliName = (provider) => CLI[provider] || provider || "";
  // The boss tile and CLI badges show the CLI's mark, or its initial.
  const hostTile = (host, label) => cliMark(host) || h("span.host-letter", {}, (label || cliName(host) || "?").slice(0, 1));
  // Model names drop the routing prefix ("opencode-go/deepseek-v4.1-flash" -> "deepseek-v4.1-flash")
  // unless the user chose full names in Settings; shared by every window through localStorage.
  const MODEL_NAMES_KEY = "agentnave.model-names";
  const fullModelNames = () => {
    try { return localStorage.getItem(MODEL_NAMES_KEY) === "full"; } catch { return false; }
  };
  const setFullModelNames = (full) => {
    try { localStorage.setItem(MODEL_NAMES_KEY, full ? "full" : "short"); } catch { /* storage may be unavailable */ }
  };
  const modelName = (model) => (model && !fullModelNames() ? model.split("/").pop() || model : model);
  const deskName = (d) => modelName(d.display_model || d.model) || "CLI 默认模型";
  // A workspace is named by its first dispatch's title; where it runs is the boss's CLI and project.
  const chatName = (g) => g.title || g.project || g.label;
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
  // An idle desk rests once it has had no work for half an hour.
  const dozing = (d) => Date.now() - at(d.last_active) > 30 * 60e3;
  // A reviewed answer waits only briefly on the boss: it reads as work going on, not a state of its own.
  const ongoing = (status) => (status === "review" ? "running" : status);

  const art = { url: artUrl, character, figure, head, lid, STATES, EXTRA, charactersChanged,
    // Bumped whenever a set is replaced or restored: views fold it into their render caches.
    version: () => version };

  window.AN = { h, $, load, href, MEMBER, GROUP, pick, urgent, deskState, avatar, colorClass, at, relTime, clock, fullTime, dayLabel, setText, setAttr, setClass, reconcile, stickyOrder, poll, plain, themeSwitch, applyTheme, art, cliName, hostTile, deskName, chatName, chatPlace, activities, dozing, ongoing, clip, modelName, fullModelNames, setFullModelNames };
})();

(function () {
  const { h, load, href, setText } = AN;
  const mock = () =>
    window.AgentNaveMock && window.AgentNaveMock.enabled()
      ? window.AgentNaveMock
      : null;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  async function post(path, body) {
    const m = mock();
    if (m && m.post) return m.post(path, body || {});
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
    if (!started.ok) throw new Error(started.message || "已有安装正在进行");
    for (let i = 0; i < 300; i++) {
      await sleep(800);
      // A closed window stays loaded; check again once it is shown, the install keeps going.
      if (document.hidden) await new Promise((resolve) => document.addEventListener("visibilitychange", resolve, { once: true }));
      const state = (await load("/app/status")).service;
      if (state?.state === "failed")
        throw new Error(state.error || "安装失败，请重试。");
      if (state?.state === "done") return;
    }
    throw new Error("安装超时，请稍后到“设置 → 版本与更新”查看。");
  }

  // ---- 开始使用 -----------------------------------------------------------------------------
  const TITLES = {
    service: "安装本机服务",
    host: "接入一个宿主",
    members: "设置组员",
    permissions: "选择组员权限",
    try: "在宿主里派一次任务",
  };
  const FALLBACK = {
    service: "宿主要通过它才能启动 AgentNave",
    host: "在设置里一键接入已检测到的宿主",
    members: "选择哪些 CLI 与模型可以当组员",
    permissions: "决定派出去的 CLI 能不能直接跑命令",
    try: "在宿主里说：用 agentnave 让几位组员各答一遍",
  };
  const SAMPLE = "用 agentnave 让两位组员各自回答一遍：这个仓库是做什么的？";
  // Every CLI with an approval step gets the same tier; later ones per CLI in Settings.
  const PERMISSION_CLIS = ["antigravity", "claude", "codebuddy", "codex", "deepseek", "grok"];
  const setTiers = (tier) =>
    post("/app/permissions", {
      version: 1,
      clis: Object.fromEntries(PERMISSION_CLIS.map((cli) => [cli, tier])),
    });
  // First pending step -> its primary action, then any alternatives. `to` is a real route; `run`
  // is an in-place action.
  const ACTION = {
    service: {
      label: "安装本机服务",
      busy: "安装中，约需一分钟…",
      run: installService,
    },
    host: { label: "去接入宿主", to: "/settings/#hosts" },
    members: { label: "去设置组员", to: "/settings/#members" },
    permissions: [
      { label: "用推荐的「自动」", busy: "保存中…", run: () => setTiers("auto") },
      { label: "保持各 CLI 原生", busy: "保存中…", run: () => setTiers("native"), alt: true },
    ],
    try: { label: "复制示例指令", copy: SAMPLE },
  };

  function onboarding({ variant = "compact", onChange } = {}) {
    const root = h("section.ob.ob-" + variant, {
      "aria-labelledby": "ob-title",
    });
    root.hidden = true;
    const ctrl = { el: root, data: null, visible: false };
    let busy = null,
      error = null,
      sawIncomplete = false,
      celebrating = false,
      lastKey = "";

    const allDone = (d) => d.steps.every((s) => s.done);
    const shown = () =>
      !!ctrl.data &&
      !ctrl.data.dismissed &&
      (celebrating || !allDone(ctrl.data));

    function render() {
      const was = ctrl.visible;
      ctrl.visible = shown();
      if (was !== ctrl.visible) onChange && onChange(ctrl.visible);
      root.hidden = !ctrl.visible;
      if (!ctrl.visible) {
        lastKey = "";
        return;
      }
      const key = JSON.stringify([
        ctrl.data,
        busy,
        error,
        celebrating,
        variant,
      ]);
      if (key === lastKey) return;
      lastKey = key;
      const { steps } = ctrl.data;
      const done = steps.filter((s) => s.done).length;
      const current = steps.find((s) => !s.done);
      const dismiss = h(
        "button.ob-x",
        {
          type: "button",
          "aria-label": "关闭“开始使用”",
          title: "关闭后不再显示",
        },
        "×",
      );
      dismiss.addEventListener("click", onDismiss);
      const head = h(
        "header.ob-head",
        {},
        h(
          "h2.ob-title",
          { id: "ob-title" },
          celebrating ? "都准备好了" : "开始使用",
        ),
        h(
          "span.ob-count",
          { "aria-label": `已完成 ${done} 步，共 ${steps.length} 步` },
          `${done}/${steps.length}`,
        ),
        dismiss,
      );
      const bar = h(
        "div.ob-bar",
        { "aria-hidden": "true" },
        steps.map((s) => h("i" + (s.done ? ".on" : ""))),
      );
      const kids = [head, bar];
      if (celebrating) {
        kids.push(
          h("p.ob-lead", {}, "接入和设置都完成了，这张卡片几秒后自动收起。"),
        );
      } else {
        if (variant === "full")
          kids.push(
            h(
              "p.ob-lead",
              {},
              "完成这几步，办公室里就会出现 boss、工位和会议室。",
            ),
          );
        kids.push(
          h(
            "ol.ob-steps",
            {},
            steps.map((s) => stepNode(s, s === current)),
          ),
        );
      }
      root.replaceChildren(...kids);
    }

    function stepNode(step, isCurrent) {
      const cls = step.done ? ".done" : isCurrent ? ".current" : "";
      const name = TITLES[step.id] || step.id;
      const detail = step.detail || (step.done ? "" : FALLBACK[step.id]);
      const kids = [
        h("span.ob-ind", { "aria-hidden": "true" }),
        h(
          "div.ob-body",
          {},
          h(
            "b.ob-name",
            {},
            name,
            h(
              "span.sr-only",
              {},
              step.done
                ? "（已完成）"
                : isCurrent
                  ? "（当前步骤）"
                  : "（未完成）",
            ),
          ),
          detail && h("span.ob-detail", {}, detail),
        ),
      ];
      const li = h("li.ob-step" + cls, {}, ...kids);
      if (isCurrent) {
        const body = li.querySelector(".ob-body");
        const acts = [].concat(ACTION[step.id] || []);
        if (acts.length)
          body.append(h("div.ob-act", {}, acts.map((act) => actionNode(step.id, act))));
        if (error && error.step === step.id)
          body.append(h("p.ob-err", { role: "alert" }, error.message));
      }
      return li;
    }

    function actionNode(id, act) {
      if (act.to)
        return h(
          "a.ob-btn",
          { href: href(act.to) },
          act.label,
          h("span", { "aria-hidden": "true" }, " →"),
        );
      const pending = busy === `${id}/${act.label}`;
      const btn = h(
        "button.ob-btn" + (act.alt ? ".ob-alt" : "") + (pending ? ".pending" : ""),
        { type: "button", disabled: !!busy },
        pending ? [h("i.ob-spin"), act.busy] : error && error.act === act.label && act.run ? "重试" : act.label,
      );
      btn.addEventListener("click", async () => {
        if (act.copy) {
          try {
            await navigator.clipboard.writeText(act.copy);
          } catch {
            setText(btn, "复制失败");
            setTimeout(() => setText(btn, act.label), 2000);
            return;
          }
          setText(btn, "已复制");
          setTimeout(() => setText(btn, act.label), 2000);
          return;
        }
        busy = `${id}/${act.label}`;
        error = null;
        render();
        try {
          await act.run();
        } catch (e) {
          error = { step: id, act: act.label, message: `${e.message || "操作失败"}` };
        }
        busy = null;
        await refresh();
      });
      return btn;
    }

    async function onDismiss() {
      try {
        await post("/app/onboarding/dismiss", {});
        ctrl.data = { ...ctrl.data, dismissed: true };
      } catch {
        error = {
          step: (ctrl.data.steps.find((s) => !s.done) || {}).id,
          message: "没能关闭，请稍后再试。",
        };
      }
      render();
    }

    async function refresh() {
      if (busy) return;
      try {
        ctrl.data = await load("/app/onboarding");
      } catch {
        return; // keep what is shown; the next poll retries
      }
      if (!allDone(ctrl.data)) sawIncomplete = true;
      else if (sawIncomplete && !celebrating && !ctrl.data.dismissed) {
        celebrating = true;
        setTimeout(() => {
          celebrating = false;
          sawIncomplete = false;
          render();
        }, 4000);
      }
      render();
    }
    ctrl.refresh = refresh;
    ctrl.setVariant = (next) => {
      if (next === variant) return;
      variant = next;
      root.className = "ob ob-" + variant;
      lastKey = "";
      render();
    };
    ctrl.start = () => AN.poll(refresh, 15000); // not the 1.5 s loop: the server reads host configs
    return ctrl;
  }

  Object.assign(AN, { post, installService, onboarding });
})();

// Updating, as one flow everywhere (header button, settings, the menu's "检查更新"): check again,
// show what changed since this version, and only on "立即更新" upgrade the service, install the
// App and restart. The release notes are plain text set with textContent, never as HTML.
(function () {
  const { h, load, post } = AN;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  let dialog = null;
  let running = false;

  // A small subset of the release notes' Markdown: "## " headings, "- " bullets, paragraphs and
  // `code`.
  function inline(text) {
    return text.split(/(`[^`]+`)/).filter(Boolean).map((part) =>
      part.startsWith("`") && part.endsWith("`") ? h("code", {}, part.slice(1, -1)) : part.replace(/\*\*/g, ""));
  }
  function notesNode(text) {
    const out = [];
    let list = null;
    for (const raw of text.split("\n")) {
      const line = raw.trim();
      if (!line) { list = null; continue; }
      if (line.startsWith("#")) {
        list = null;
        out.push(h("h4.upd-h", {}, ...inline(line.replace(/^#+\s*/, ""))));
      } else if (/^[-*] /.test(line)) {
        if (!list) out.push((list = h("ul.upd-list")));
        list.append(h("li", {}, ...inline(line.slice(2))));
      } else {
        list = null;
        out.push(h("p.upd-p", {}, ...inline(line)));
      }
    }
    return out;
  }

  function show(title, body, buttons) {
    if (!dialog) {
      dialog = h("dialog.upd");
      dialog.addEventListener("cancel", (e) => { if (running) e.preventDefault(); });
      document.body.append(dialog);
    }
    const row = h("div.upd-row", {}, ...buttons.map(([label, run, primary]) => {
      const b = h(`button${primary ? ".primary" : ""}`, { type: "button" }, label);
      b.addEventListener("click", run);
      return b;
    }));
    dialog.replaceChildren(h("h2.upd-title", {}, title), h("div.upd-body", {}, ...body), row);
    if (!dialog.open) dialog.showModal();
    dialog.querySelector(".primary")?.focus();
  }
  const close = () => dialog?.close();
  const done = (status) => window.dispatchEvent(new CustomEvent("agentnave:status", { detail: status }));

  const STEPS = [["download", "下载新版 App"], ["service", "升级本机服务（约一分钟）"], ["app", "安装 App 并重启"]];
  function progress(step, version) {
    const at = STEPS.findIndex(([key]) => key === step);
    show(`正在更新到 v${version}`, [
      h("ol.upd-steps", {}, ...STEPS.map(([, label], i) =>
        h(`li.${i < at ? "done" : i === at ? "now" : "todo"}`, {}, label))),
      h("p.upd-note", {}, "完成后 App 会自动重启。正在运行的任务不受影响。"),
    ], []);
  }

  async function apply(version, serviceOnly) {
    running = true;
    try {
      if (serviceOnly) show("正在升级本机服务", [h("p.upd-p", {}, "约需一分钟，正在运行的任务不受影响。")], []);
      else progress("download", version);
      let status = await post("/app/update/apply");
      // Another check or update holding the lock leaves this one unstarted. A fast end (failed or,
      // for the service, done) still counts as started; the loop below reports it.
      const started = serviceOnly
        ? ["upgrading", "installing", "failed", "done"].includes(status.service?.state)
        : ["installing", "failed"].includes(status.update?.state);
      if (!started) throw new Error("另一项检查或更新正在进行，请稍后重试。");
      for (;;) {
        done(status);
        const update = status.update;
        const service = status.service;
        if (update?.state === "failed") throw new Error(update.error || "更新失败");
        if (serviceOnly) {
          if (service?.state === "failed") throw new Error(service.error || "升级本机服务失败");
          if (!["upgrading", "installing"].includes(service?.state)) break;
        } else if (update?.state === "installing") {
          progress(update.step, version);
        } else if (update?.state !== "installing" && status.version === version) {
          break;
        }
        await sleep(1000);
        status = await load("/app/status");
      }
      running = false;
      // The App restarts before this on a real update; only the preview's mock gets here.
      show(serviceOnly ? "本机服务已升级" : `已更新到 v${version}`, [h("p.upd-p", {}, "已切换到新版本，无需重连。")], [["好", close, true]]);
    } catch (error) {
      running = false;
      show("更新没有完成", [h("p.upd-p.upd-error", {}, error.message)], [["关闭", close], ["重新检查", () => run(), true]]);
    }
  }

  async function run() {
    if (running) return;
    show("检查更新", [h("p.upd-p", {}, "正在检查最新版本…")], [["取消", close]]);
    let status;
    try {
      status = await post("/app/update/check");
    } catch (error) {
      return show("检查更新失败", [h("p.upd-p.upd-error", {}, error.message)], [["关闭", close], ["重试", () => run(), true]]);
    }
    done(status);
    const update = status.update;
    const service = status.service;
    if (update?.state === "failed")
      return show("检查更新失败", [h("p.upd-p.upd-error", {}, update.error || "请检查网络后重试")], [["关闭", close], ["重试", () => run(), true]]);
    if (update?.state === "available") {
      const items = update.notes?.items || [];
      const link = update.notes?.link;
      const notes = [
        ...(items.length
          ? items.map((item) => h("section.upd-ver", {}, h("h3.upd-v", {}, `v${item.version}`), ...notesNode(item.notes)))
          : [h("p.upd-p", {}, "这个版本没有附带更新说明。")]),
        // With items, the link covers the releases older than those the manifest carries.
        ...(link ? [h("a.upd-link", { href: link, target: "_blank", rel: "noopener" }, items.length ? "更早版本的更新说明见 GitHub" : "在 GitHub 查看更新说明")] : []),
      ];
      return show(`有新版本 v${update.version}`, [
        h("p.upd-sub", {}, `当前 v${status.version}`),
        h("div.upd-notes", {}, ...notes),
        h("p.upd-note", {}, "更新时先升级本机服务，再安装 App 并自动重启；服务升级失败就不装 App。"),
      ], [["稍后", close], ["立即更新", () => apply(update.version, false), true]]);
    }
    if (service?.state === "failed")
      return show("升级本机服务失败", [h("p.upd-p.upd-error", {}, service.error || "升级本机服务失败")],
        [["关闭", close], ["重试升级", () => apply(status.version, true), true]]);
    if (service?.state === "outdated")
      return show("升级本机服务", [h("p.upd-p", {}, `本机服务 v${service.version} 比 App v${status.version} 旧。升级约需一分钟，正在运行的任务不受影响。`)],
        [["稍后", close], ["升级", () => apply(status.version, true), true]]);
    show("已是最新版本", [h("p.upd-p", {}, `App 与本机服务都是 v${status.version}。`)], [["好", close, true]]);
  }
  AN.updateFlow = run;
  // The menu's "检查更新" and the panel open the main window with ?check-update.
  addEventListener("DOMContentLoaded", () => {
    const url = new URL(location.href);
    if (!url.searchParams.has("check-update")) return;
    url.searchParams.delete("check-update");
    history.replaceState(null, "", url);
    run();
  });
})();

// The window header's update button: shown only when the App or the local service can be updated,
// or for a while after an update; a click runs the update flow. The menu bar panel is too narrow
// for its dialog and opens the main window's instead.
(function () {
  const { h, load } = AN;
  const end = document.querySelector("header.bar .bar-end, header.top .top-end");
  if (!end) return;
  const button = h("button.bar-update", { type: "button", hidden: true });
  end.prepend(button);
  const short = end.matches(".top-end");
  let status = null;
  function render() {
    const update = status?.update?.state;
    const service = status?.service?.state;
    const busy = update === "installing" || ["upgrading", "installing"].includes(service);
    const found = update === "available" || service === "outdated";
    const updated = !busy && !found && status?.updated;
    button.hidden = !busy && !found && !updated;
    button.disabled = busy;
    button.classList.toggle("updated", Boolean(updated));
    button.textContent = busy ? (short ? "更新中…" : "正在更新…")
      : updated ? `已更新到 v${status.updated}`
      : update === "available" ? `↑ ${short ? "" : "更新到 "}v${status.update.version}`
      : short ? "↑ 升级服务" : "↑ 升级本机服务";
    button.title = updated ? "点此查看是否还有更新" : "先看看更新了什么，确认后再更新";
  }
  async function refresh() {
    if (document.hidden) return;
    try { status = await load("/app/status"); } catch { return; }
    render();
  }
  button.addEventListener("click", () => {
    if (short) location.href = "/app/update";
    else AN.updateFlow();
  });
  // The settings page and the update flow read the status themselves; reuse what they got.
  addEventListener("agentnave:status", ({ detail }) => { status = detail; render(); });
  document.addEventListener("visibilitychange", refresh);
  setInterval(refresh, 60000);
  // After every deferred script, so the preview's mock backend is in place too.
  addEventListener("DOMContentLoaded", refresh);
})();

// The brand's N lets its dot drip when the pointer comes over it, as the menu bar icon does on a
// click; once started it plays to the end.
for (const brand of document.querySelectorAll(".brand")) {
  brand.addEventListener("mouseenter", () => {
    const mark = brand.querySelector(".mark");
    if (!mark || mark.classList.contains("drip") || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    mark.classList.add("drip");
    mark.querySelector(".drop").addEventListener("animationend", () => mark.classList.remove("drip"), { once: true });
  });
}
