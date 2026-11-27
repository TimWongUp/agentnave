const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { common } = require("./helpers/common.cjs");
const { flush } = require("./helpers/voice.cjs");

const source = name => readFileSync(`${__dirname}/../${name}`, "utf8");
const text = node => (typeof node === "string" ? node
  : [node.textContent || "", ...(node.children || []).filter(Boolean).map(text)].filter(Boolean).join("|"));
const walk = (node, fn) => {
  if (!node || typeof node === "string") return;
  fn(node);
  for (const child of node.children || []) walk(child, fn);
};
const findAll = (root, match) => {
  const out = [];
  walk(root, n => { if (match(n)) out.push(n); });
  return out;
};
const hasClass = (node, cls) => Boolean(node?.classSet?.has(cls) || String(node?.className || "").split(/\s+/).includes(cls));

function richDom(lang = "zh-CN") {
  const nodes = [];
  const makeNode = (tag, attributes = {}, children = []) => {
    const classSet = new Set(tag.split(".").slice(1));
    const node = {
      tag, tagName: tag.split(".")[0].toUpperCase(),
      id: attributes.id || "",
      classSet,
      get className() { return [...classSet].join(" "); },
      set className(v) { classSet.clear(); for (const c of String(v || "").split(/\s+/).filter(Boolean)) classSet.add(c); },
      classList: {
        add: (...cs) => cs.forEach(c => classSet.add(c)),
        remove: (...cs) => cs.forEach(c => classSet.delete(c)),
        toggle: (c, force) => {
          const on = force === undefined ? !classSet.has(c) : Boolean(force);
          if (on) classSet.add(c); else classSet.delete(c);
          return on;
        },
        contains: c => classSet.has(c),
      },
      style: {},
      attributes: { ...attributes },
      dataset: {},
      children: [],
      listeners: {},
      parentNode: null,
      hidden: Boolean(attributes.hidden),
      disabled: Boolean(attributes.disabled),
      value: attributes.value ?? "",
      clientWidth: 720,
      offsetWidth: 180,
      textContent: "",
      setAttribute(k, v) {
        this.attributes[k] = String(v);
        if (k === "id") this.id = String(v);
        if (k === "class") this.className = String(v);
        if (k.startsWith("data-")) {
          const camel = k.slice(5).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
          this.dataset[camel] = String(v);
        }
      },
      removeAttribute(k) { delete this.attributes[k]; },
      getAttribute(k) { return this.attributes[k]; },
      append(...kids) {
        for (const k of kids.flat(Infinity)) {
          if (k == null || k === false) continue;
          if (typeof k === "string") this.textContent += k;
          else { k.parentNode = this; this.children.push(k); }
        }
      },
      prepend(...kids) {
        const objs = kids.flat(Infinity).filter(k => k && typeof k !== "string");
        for (const k of objs) k.parentNode = this;
        this.children.unshift(...objs);
      },
      replaceChildren(...kids) {
        this.children = [];
        this.textContent = "";
        for (const k of kids) {
          if (k === null || k === undefined) this.textContent += String(k);
          else this.append(k);
        }
      },
      insertBefore(nodeToInsert, ref) {
        const idx = this.children.indexOf(ref);
        const cur = this.children.indexOf(nodeToInsert);
        if (cur >= 0) this.children.splice(cur, 1);
        const at = idx >= 0 ? this.children.indexOf(ref) : this.children.length;
        this.children.splice(at, 0, nodeToInsert);
        nodeToInsert.parentNode = this;
      },
      remove() { this.removed = true; },
      addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
      dispatch(name, event = {}) {
        let cur = this, stopped = false;
        const ev = {
          target: this, pointerType: "mouse", clientX: 200,
          stopPropagation() { stopped = true; },
          preventDefault() {},
          ...event,
        };
        while (cur) {
          for (const fn of cur.listeners[name] || []) fn(ev);
          if (stopped) break;
          cur = cur.parentNode;
        }
      },
      closest(sel) {
        const parts = sel.split(",").map(s => s.trim());
        let cur = this;
        while (cur) {
          if (parts.some(p => matchesSelector(cur, p))) return cur;
          cur = cur.parentNode;
        }
        return null;
      },
      matches(sel) { return sel.split(",").some(p => matchesSelector(this, p.trim())); },
      contains(item) { return this === item || this.children.some(c => c?.contains?.(item)); },
      querySelector(sel) { return findAll(this, n => matchesSelector(n, sel))[0] || null; },
      querySelectorAll(sel) { return findAll(this, n => matchesSelector(n, sel)); },
      getBoundingClientRect: () => ({ left: 0, right: 720, top: 0, bottom: 40, width: 720, height: 40 }),
      focus() {},
    };
    for (const [k, v] of Object.entries(attributes)) {
      if (k.startsWith("data-")) {
        const camel = k.slice(5).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
        node.dataset[camel] = String(v);
      }
    }
    node.append(...children);
    nodes.push(node);
    return node;
  };
  const matchesSimple = (node, sel) => {
    if (!node || typeof node === "string") return false;
    const attrM = /\[([^\]=]+)(?:="([^"]*)")?\]/.exec(sel);
    let rest = sel;
    if (attrM) {
      const val = node.attributes?.[attrM[1]];
      if (attrM[2] === undefined ? val == null : val !== attrM[2]) return false;
      rest = sel.replace(attrM[0], "");
    }
    if (rest.includes(":not(:disabled)")) {
      if (node.disabled) return false;
      rest = rest.replace(":not(:disabled)", "");
    }
    if (!rest) return true;
    if (rest.startsWith("#")) return node.id === rest.slice(1);
    if (rest.startsWith(".")) return rest.slice(1).split(".").every(c => hasClass(node, c));
    const [tag, ...classes] = rest.split(".");
    if (tag && node.tagName !== tag.toUpperCase()) return false;
    return classes.every(c => hasClass(node, c));
  };
  const matchesSelector = (node, sel) => sel.split(",").some(group => {
    const parts = group.trim().split(/\s+/);
    if (!matchesSimple(node, parts.at(-1))) return false;
    let cur = node.parentNode;
    for (let i = parts.length - 2; i >= 0; i--) {
      while (cur && !matchesSimple(cur, parts[i])) cur = cur.parentNode;
      if (!cur) return false;
      cur = cur.parentNode;
    }
    return true;
  });
  const h = (tag, props = {}, ...kids) => makeNode(tag, props, kids);

  const elements = new Map();
  const makeSeg = (id, attr, values) => {
    const seg = h("div.u-seg", { id });
    for (const v of values) seg.append(h("button", { type: "button", [`data-${attr}`]: v }, v));
    elements.set(id, seg);
    return seg;
  };
  makeSeg("u-view", "view", ["overall", "projects"]);
  makeSeg("u-range", "range", ["24h", "today", "7d", "30d", "custom"]);
  makeSeg("u-dim", "dim", ["provider", "family", "model"]);
  makeSeg("u-cache", "cache", ["with", "net"]);
  makeSeg("u-metric", "metric", ["cost", "tokens"]);
  for (const id of ["u-app", "u-subbar", "u-pop", "u-body", "u-updated", "u-refresh", "u-foot"]) {
    const el = h("div", { id });
    if (id === "u-pop" || id === "u-foot") el.hidden = true;
    elements.set(id, el);
  }
  const docListeners = {};
  const document = {
    hidden: false,
    documentElement: { lang, style: { setProperty() {} } },
    getElementById: id => elements.get(id) || null,
    querySelector: sel => findAll(elements.get("u-app"), n => matchesSelector(n, sel))[0]
      || findAll(elements.get("u-body"), n => matchesSelector(n, sel))[0] || null,
    querySelectorAll: () => [],
    createElementNS: (_ns, tag) => makeNode(tag, {}, []),
    addEventListener: (name, fn) => { (docListeners[name] ||= []).push(fn); },
    docListeners,
  };
  return { h, document, elements, matchesSelector };
}

function mountUsage({ savedPrefs = {}, lang = "zh-CN" } = {}) {
  const { h, document, elements } = richDom(lang);
  const requests = [], storage = new Map(), winListeners = {};
  if (Object.keys(savedPrefs).length) storage.set("agentnave.usage", JSON.stringify(savedPrefs));
  let fullModels = false, currentLang = lang;
  const baseCommon = common(lang.startsWith("en"));
  const AN = {
    ...baseCommon,
    h,
    L: (zh, en) => (currentLang.startsWith("en") ? en : zh),
    setLanguage: next => { currentLang = next; document.documentElement.lang = next; },
    relTime: () => "刚刚",
    isHidden: () => false,
    onVisibility() {},
    fullModelNames: () => fullModels,
    cliName: p => ({ claude: "Claude Code", codex: "Codex", antigravity: "Antigravity", grok: "Grok", deepseek: "DeepSeek Harness", codebuddy: "CodeBuddy", hermes: "Hermes" }[p] || p),
    hostTile: p => h("span.u-tile-stub", { "data-p": p }),
    modelName: m => (fullModels ? m : m.split("/").pop() || m),
    post: async () => ({}),
    load: path => new Promise((resolve, reject) => requests.push({ path, resolve, reject })),
  };
  const context = vm.createContext({
    AN, document, Intl, queueMicrotask, Math, Date, Number, String, Array, Object, Set, Map, JSON,
    window: {
      addEventListener: (name, fn) => { (winListeners[name] ||= []).push(fn); },
      parent: {},
    },
    localStorage: {
      getItem: k => storage.get(k) ?? null,
      setItem: (k, v) => storage.set(k, v),
    },
    setTimeout: () => 1,
    clearTimeout() {},
  });
  context.window.window = context.window;
  for (const file of ["usage-picker.js", "usage-chart.js", "usage.js"]) vm.runInContext(source(file), context);

  const answer = async (idx, payload) => {
    requests[idx].resolve(payload);
    await flush();
    await flush();
  };
  const body = elements.get("u-body");
  return {
    AN, document, elements, body, requests, storage, winListeners,
    answer,
    setFullModels: v => { fullModels = v; for (const fn of winListeners["agentnave:model-names"] || []) fn(); },
    setLangMsg: next => {
      for (const fn of winListeners.message || []) {
        fn({ source: context.window.parent, data: { type: "agentnave:settings", action: "language", language: next } });
      }
    },
    clickSeg(id, attr, val) {
      const btn = elements.get(id).children.find(b => b.getAttribute(`data-${attr}`) === val);
      btn.dispatch("click");
    },
    clickRefresh: () => elements.get("u-refresh").dispatch("click"),
    pressEscape: () => { for (const fn of document.docListeners.keydown || []) fn({ key: "Escape" }); },
    find: (sel, root = body) => root.querySelectorAll(sel),
  };
}

function samplePayload(overrides = {}) {
  const baseMs = new Date(2026, 10, 20, 0, 0, 0, 0).getTime();
  const providersList = ["claude", "codex", "antigravity", "grok", "deepseek", "codebuddy", "hermes"];
  const daily = Array.from({ length: 7 }, (_, i) => {
    const start_ms = baseMs + i * 86400000;
    const by_provider = {};
    const by_model = {};
    providersList.forEach((p, pi) => {
      const cost = (7 - pi) * (i + 1) * 0.5;
      const tok = (7 - pi) * (i + 1) * 100000;
      const net = Math.round(tok * 0.35);
      by_provider[p] = { cost_usd: cost, total_tokens: tok, net_tokens: net };
      by_model[`${p}::org/${p}-model`] = { provider: p, model: `org/${p}-model`, cost_usd: cost, total_tokens: tok, net_tokens: net };
    });
    return {
      date: `2026-11-${20 + i}`,
      start_ms,
      end_ms: start_ms + 86400000,
      future: false,
      cost_usd: 14 * (i + 1),
      unpriced_tokens: 0,
      total_tokens: 2800000 * (i + 1),
      net_tokens: 980000 * (i + 1),
      fresh_input: 500000 * (i + 1),
      cache_read: 1820000 * (i + 1),
      cache_write: 300000 * (i + 1),
      output: 180000 * (i + 1),
      cache_hit_rate: 0.7854,
      by_provider,
      by_model,
    };
  });
  const providers = providersList.map((p, pi) => {
    const cost_usd = (7 - pi) * 14;
    const total_tokens = (7 - pi) * 2800000;
    const net_tokens = Math.round(total_tokens * 0.35);
    return {
      provider: p,
      cost_usd,
      prev_cost_usd: cost_usd * 0.9,
      unpriced_tokens: 0,
      total_tokens,
      prev_total_tokens: total_tokens * 0.85,
      net_tokens,
      prev_net_tokens: net_tokens * 0.85,
      fresh_input: Math.round(total_tokens * 0.2),
      cache_read: Math.round(total_tokens * 0.65),
      cache_write: Math.round(total_tokens * 0.1),
      output: Math.round(total_tokens * 0.05),
      cache_hit_rate: 0.8512 - pi * 0.03,
      calls: (7 - pi) * 20,
      models: [{
        key: `${p}::org/${p}-model`,
        provider: p,
        model: `org/${p}-model`,
        priced: p !== "hermes",
        cost_usd: p === "hermes" ? 0 : cost_usd,
        prev_cost_usd: p === "hermes" ? 0 : cost_usd * 0.9,
        total_tokens,
        prev_total_tokens: total_tokens * 0.85,
        net_tokens,
        prev_net_tokens: net_tokens * 0.85,
        cache_hit_rate: 0.8512 - pi * 0.03,
        calls: (7 - pi) * 20,
      }],
    };
  });
  return {
    range: "7d",
    granularity: "day",
    available_granularities: ["day", "week"],
    now_ms: baseMs + 6 * 86400000 + 18 * 3600000,
    min_ts_ms: baseMs - 40 * 86400000,
    updated_at: baseMs + 6 * 86400000 + 18 * 3600000,
    scanning: false,
    uncovered_installed: ["devin"],
    overall: {
      cost_usd: 392,
      unpriced_tokens: 2800000,
      unpriced_calls: 20,
      total_tokens: 78400000,
      net_tokens: 27440000,
      fresh_input: 14000000,
      cache_read: 50960000,
      cache_write: 8400000,
      output: 5040000,
      cache_hit_rate: 0.8642,
      total_calls: 560,
    },
    prev_overall: {
      cost_usd: 350,
      total_tokens: 70000000,
      net_tokens: 24500000,
      fresh_input: 12000000,
      cache_read: 45500000,
      cache_write: 7500000,
      output: 5000000,
      cache_hit_rate: 0.841,
      total_calls: 500,
    },
    daily,
    providers,
    projects: [
      {
        key: "agentnave", label: "agentnave", path: "~/Code/agentnave",
        total: { cost_usd: 220, total_tokens: 44000000, net_tokens: 15400000, cache_hit_rate: 0.88 },
        boss: { cost_usd: 90, total_tokens: 18000000, net_tokens: 6000000, cache_hit_rate: 0.90 },
        workers: { cost_usd: 80, total_tokens: 16000000, net_tokens: 5800000, cache_hit_rate: 0.87 },
        native: { cost_usd: 50, total_tokens: 10000000, net_tokens: 3600000, cache_hit_rate: 0.83 },
        by_provider: [
          { provider: "claude", cost_usd: 150, total_tokens: 30000000, net_tokens: 10500000, boss_tokens: 12000000, worker_tokens: 10000000, native_tokens: 8000000, boss_cost: 60, worker_cost: 50, native_cost: 40 },
        ],
      },
    ],
    ...overrides,
  };
}

test("common.js CLI map covers all new providers cleanly", () => {
  const api = common();
  const context = vm.createContext({
    document: { documentElement: { lang: "zh", dataset: {}, classList: { toggle() {} } }, hidden: false, querySelectorAll: () => [], querySelector: () => null, addEventListener() {} },
    window: { ANAppearance: { applyTheme() {}, reducedMotion: { matches: true } }, addEventListener() {}, dispatchEvent() {} },
    CustomEvent: class {}, localStorage: { getItem() {}, setItem() {} }, setInterval() {}, setTimeout() {},
    fetch: async () => ({ ok: false, json: async () => ({}) }),
  });
  vm.runInContext(source("common.js"), context);
  const cliName = context.window.AN.cliName;
  assert.equal(cliName("claude-desktop"), "Claude Desktop");
  assert.equal(cliName("workbuddy"), "WorkBuddy");
  assert.equal(cliName("hermes"), "Hermes");
  assert.equal(cliName("opencode"), "OpenCode");
  assert.equal(cliName("kimi"), "Kimi Code");
  assert.equal(cliName("minimax"), "MiniMax Code");
  assert.equal(cliName("devin"), "Devin");
  assert.ok(api.L);
});

test("usage HTML, CSS and JS contain zero inline style attributes", () => {
  for (const f of ["usage.html", "usage-picker.js", "usage-chart.js", "usage.js"]) {
    const content = source(f);
    assert.equal(/\bstyle\s*=/.test(content), false, `${f} must not contain inline style=`);
    assert.equal(/\.cssText\b/.test(content), false, `${f} must not assign style.cssText`);
  }
});

test("multi-pinning highlights multiple items, draws >Top5 series, rescales series Y-axis, and handles preview/clear", async () => {
  const app = mountUsage();
  await app.answer(0, samplePayload());

  // Initially Top 5 are drawn in total mode (hermes is #7, outside Top 5)
  assert.equal(app.find(".u-series").length, 5);
  assert.equal(app.find(".u-legend")[0].textContent, "");
  assert.equal(app.find(".u-series").some(g => g.getAttribute("data-id") === "hermes"), false);

  // Pin claude and hermes (#7) simultaneously via legend chips
  const chips = app.find(".u-chip");
  const claudeChip = chips.find(c => c.getAttribute("data-id") === "claude");
  const hermesChip = chips.find(c => c.getAttribute("data-id") === "hermes");
  const codexChip = chips.find(c => c.getAttribute("data-id") === "codex");

  claudeChip.dispatch("click");
  hermesChip.dispatch("click");

  assert.equal(claudeChip.getAttribute("aria-pressed"), "true");
  assert.equal(hermesChip.getAttribute("aria-pressed"), "true");
  assert.equal(codexChip.getAttribute("aria-pressed"), "false");
  assert.ok(hasClass(claudeChip, "is-hi"));
  assert.ok(hasClass(hermesChip, "is-hi"));
  assert.ok(hasClass(codexChip, "is-lo"));

  // hermes (#7) is now force-drawn on the SVG chart!
  assert.ok(app.find(".u-series").some(g => g.getAttribute("data-id") === "hermes"));

  // Donut center summarizes the 2 selected items
  const center = app.elements.get("u-body").querySelector("#u-donut-center");
  assert.match(text(center), /已选 2 项/);

  // Clear button appears when >= 2 pinned
  const clearBtn = app.find(".u-clear-pins")[0];
  assert.equal(clearBtn.hidden, false);

  // Hovering codex previews codex without unpinning claude/hermes
  codexChip.dispatch("pointerover");
  assert.ok(hasClass(codexChip, "is-preview"));
  assert.ok(hasClass(claudeChip, "is-hi"));
  assert.ok(hasClass(hermesChip, "is-hi"));
  codexChip.dispatch("pointerleave");

  // Switch to series mode: Y-axis rescales to pinned items (unpin claude, keep hermes which is small)
  const modeBtns = app.find(".u-trend-tools .u-seg")[0].children;
  modeBtns[1].dispatch("click"); // "仅各系列"
  await flush();
  const ticksWithBoth = app.find(".u-axis").map(t => t.textContent).filter(s => s.startsWith("$"));
  assert.ok(ticksWithBoth.length > 0);
  app.find(".u-chip").find(c => c.getAttribute("data-id") === "claude").dispatch("click"); // unpin claude, leaving only hermes
  const ticksHermesOnly = app.find(".u-axis").map(t => t.textContent).filter(s => s.startsWith("$"));
  assert.notEqual(ticksHermesOnly.at(-1), ticksWithBoth.at(-1), "series Y-axis rescales to pinned set");

  // Escape clears pins
  app.pressEscape();
  const hermesAfter = app.find(".u-chip").find(c => c.getAttribute("data-id") === "hermes");
  assert.equal(hermesAfter.getAttribute("aria-pressed"), "false");
  assert.equal(app.find(".u-clear-pins")[0].hidden, true);
});

test("ranking sort, $/1M fixed denominator, provider twist vs pin, and model dimension", async () => {
  const app = mountUsage();
  await app.answer(0, samplePayload());

  // Clicking the row or its twist arrow toggles model sub-rows without pinning
  const claudeChip = () => app.find(".u-chip").find(c => c.getAttribute("data-id") === "claude");
  const claudeRow = () => app.find(".u-rrow.item").find(r => r.getAttribute("data-id") === "claude");
  claudeRow().querySelector(".u-twist").dispatch("click");
  assert.equal(app.find(".u-rrow.sub").length, 1);
  assert.equal(claudeRow().getAttribute("aria-expanded"), "true");
  assert.equal(claudeRow().getAttribute("aria-pressed"), undefined, "ranking row is expandable, not pinnable");
  assert.equal(claudeChip().getAttribute("aria-pressed"), "false", "twist does not pin row");

  // Clicking the row itself collapses, then Enter re-expands; neither pins
  claudeRow().dispatch("click");
  assert.equal(app.find(".u-rrow.sub").length, 0);
  assert.equal(claudeRow().getAttribute("aria-expanded"), "false");
  assert.equal(claudeChip().getAttribute("aria-pressed"), "false", "row click does not pin");
  claudeRow().dispatch("keydown", { key: "Enter" });
  assert.equal(app.find(".u-rrow.sub").length, 1);
  assert.equal(claudeChip().getAttribute("aria-pressed"), "false", "row keydown does not pin");

  // Clicking a donut arc does not pin either (pinning is legend chips only)
  const claudeArc = app.find(".u-arc").find(a => a.getAttribute("data-id") === "claude");
  claudeArc.dispatch("click");
  assert.equal(claudeChip().getAttribute("aria-pressed"), "false", "donut click does not pin");

  // Sort by $/1M
  const priceSort = app.find(".u-rrow.head button").find(b => b.getAttribute("data-sort") === "price");
  priceSort.dispatch("click");
  const priceSortAfter = app.find(".u-rrow.head button").find(b => b.getAttribute("data-sort") === "price");
  assert.ok(hasClass(priceSortAfter, "on"));

  // Full model names applies inside tool sub-rows; model dimension merges by base model
  app.setFullModels(true);
  assert.match(text(app.find(".u-rrow.sub")[0]), /org\/claude-model/);

  // Switch to model dimension -> base model name shown
  app.clickSeg("u-dim", "dim", "model");
  assert.match(text(app.find(".u-rrow.item")[0]), /claude-model/);

  // Switch to tokens + net cache cuts cache_read in composition
  app.clickSeg("u-metric", "metric", "tokens");
  assert.equal(app.elements.get("u-cache").hidden, false);
  app.clickSeg("u-cache", "cache", "net");
  assert.ok(app.find(".u-comp-item.is-cut").length === 1);
  assert.match(text(app.find(".u-comp")[0]), /净 Token/);
});

test("custom time picker handles presets, N hours, hour validation, and never persists custom in localStorage", async () => {
  const app = mountUsage({ savedPrefs: { range: "custom", metric: "tokens", cache: "net" } });
  // Stored "custom" must be ignored on startup, falling back to default 7d
  assert.match(app.requests[0].path, /\/app\/usage\?range=7d$/);
  await app.answer(0, samplePayload());

  const customBtn = app.elements.get("u-range").children.find(b => b.getAttribute("data-range") === "custom");
  customBtn.dispatch("click");
  const pop = app.elements.get("u-pop");
  assert.equal(pop.hidden, false);

  // Test N hours invalid then valid (default 48)
  const nInput = pop.querySelector("#u-nhours");
  nInput.value = "999";
  pop.querySelector("#u-napply").dispatch("click");
  assert.ok(hasClass(nInput, "bad"));
  assert.equal(pop.hidden, false);

  nInput.value = "";
  pop.querySelector("#u-napply").dispatch("click");
  assert.equal(pop.hidden, true);
  assert.match(app.requests[1].path, /^\/app\/usage\?range=custom&from=\d+&to=\d+$/);
  const urlParams = new URLSearchParams(app.requests[1].path.split("?")[1]);
  assert.equal(Number(urlParams.get("to")) - Number(urlParams.get("from")), 48 * 3600000);
  assert.equal(customBtn.textContent, "近 48 小时");

  // Saved localStorage never contains "custom"
  const saved = JSON.parse(app.storage.get("agentnave.usage"));
  assert.notEqual(saved.range, "custom");

  // Reopen & test same-day endHour <= startHour validation
  customBtn.dispatch("click");
  const days = pop.querySelectorAll(".u-day").filter(d => !d.disabled);
  days.at(-1).dispatch("click");
  days.at(-1).dispatch("click");
  const t0 = pop.querySelector("#u-t0"), t1 = pop.querySelector("#u-t1");
  t0.value = "15";
  t1.value = "15";
  pop.querySelector("#u-apply").dispatch("click");
  assert.ok(hasClass(t1, "bad"));
  assert.equal(pop.hidden, false);

  t1.value = "18";
  pop.querySelector("#u-apply").dispatch("click");
  assert.equal(pop.hidden, true);
  assert.equal(app.requests.length, 3);

  // Quick preset "近 90 天" requests range=90d and labels custom button
  customBtn.dispatch("click");
  pop.querySelectorAll("button").find(b => b.getAttribute("data-quick") === "3").dispatch("click");
  assert.equal(app.requests[3].path, "/app/usage?range=90d");
  assert.equal(customBtn.textContent, "近 90 天");
});

test("single-day drilldown, granularity switch, projects native group, and bilingual switch", async () => {
  const app = mountUsage();
  await app.answer(0, samplePayload());

  // Granularity switch sends &granularity=week
  const grainSeg = app.find(".u-trend-tools .u-seg")[1];
  grainSeg.children[1].dispatch("click");
  assert.equal(app.requests[1].path, "/app/usage?range=7d&granularity=week");
  await app.answer(1, samplePayload({ granularity: "day" }));

  // Clicking plot drills into that day's hourly view
  const plot = app.find(".u-plot")[0];
  plot.dispatch("click", { clientX: 100 });
  assert.match(app.requests[2].path, /^\/app\/usage\?range=custom&from=\d+&to=\d+&granularity=hour$/);
  await app.answer(2, samplePayload({ range: "custom", granularity: "hour", available_granularities: ["hour"] }));

  const backBtn = app.find("#u-back")[0];
  assert.equal(backBtn.hidden, false);
  backBtn.dispatch("click");
  assert.equal(app.requests[3].path, "/app/usage?range=7d&granularity=week");
  await app.answer(3, samplePayload());

  // Projects view renders Boss, Employees, and Native
  app.clickSeg("u-view", "view", "projects");
  assert.match(text(app.body), /agentnave/);
  assert.match(text(app.body), /Boss/);
  assert.match(text(app.body), /员工/);
  assert.match(text(app.body), /原生/);

  // Language switch updates to English
  app.setLangMsg("en");
  assert.match(text(app.body), /Employees/);
  assert.match(text(app.body), /Native/);
});

test("model legend labels empty models, merges duplicate names across tools, and caps long lists with toggle", async () => {
  const app = mountUsage({ savedPrefs: { dim: "model" } });
  const base = samplePayload();
  const manyModels = Array.from({ length: 13 }, (_, idx) => ({
    key: `claude::m-${idx}`,
    provider: "claude",
    model: idx === 0 ? "" : idx === 1 ? "shared-model-high" : `m-${idx}`,
    priced: true,
    cost_usd: 100 - idx * 5,
    total_tokens: (100 - idx * 5) * 10000,
    net_tokens: (100 - idx * 5) * 4000,
    cache_hit_rate: 0.9,
    calls: 10,
  }));
  manyModels.push({
    key: "claude-desktop::shared-model-medium",
    provider: "claude-desktop",
    model: "shared-model-medium",
    priced: true,
    cost_usd: 94,
    total_tokens: 940000,
    net_tokens: 376000,
    cache_hit_rate: 0.9,
    calls: 10,
  });
  await app.answer(0, {
    ...base,
    daily: base.daily.map(d => ({ ...d, by_model: {} })),
    providers: [{ ...base.providers[0], models: manyModels }],
  });

  // shared-model-high + shared-model-medium merge into 1 base model -> 13 unique models total; capped at 10 + "其余 3 项"
  assert.equal(app.find(".u-chip").length, 10);
  const moreBtn = app.find(".u-more-chips")[0];
  assert.ok(moreBtn);
  assert.match(text(moreBtn), /其余 3 项/);
  assert.equal(app.find(".u-chip").filter(c => text(c).includes("shared-model")).length, 1);
  // Empty model name gets localized fallback
  assert.ok(app.find(".u-chip").some(c => text(c).includes("未知模型")));

  // Clicking a hidden model row (#12) in the ranking table expands its tool breakdown without pinning
  const tailRow = app.find(".u-rrow.item").find(r => r.getAttribute("data-id") === "m-12");
  tailRow.dispatch("click");
  assert.equal(app.find(".u-rrow.sub.sub-tool").length, 1);
  assert.equal(app.find(".u-chip").length, 10);

  // Expanding shows all 13 chips and flips button label to "收起"; pinning via legend chip keeps it surfaced on collapse
  app.find(".u-more-chips")[0].dispatch("click");
  assert.equal(app.find(".u-chip").length, 13);
  assert.equal(text(app.find(".u-more-chips")[0]), "收起");
  app.find(".u-chip").find(c => c.getAttribute("data-id") === "m-12").dispatch("click");
  app.find(".u-more-chips")[0].dispatch("click");
  assert.equal(app.find(".u-chip").length, 11);
  assert.ok(app.find(".u-chip").some(c => c.getAttribute("data-id") === "m-12" && c.getAttribute("aria-pressed") === "true"));
});

test("three dimensions: tools keep split, families merge cross-tool, models merge by base model with tool drilldown", async () => {
  const app = mountUsage();
  const base = samplePayload();
  const payload = {
    ...base,
    daily: base.daily.map((d, idx) => ({
      ...d,
      by_provider: {
        "claude-desktop": { cost_usd: 20, total_tokens: 200000, net_tokens: 80000 },
        claude: { cost_usd: 10, total_tokens: 100000, net_tokens: 40000 },
        antigravity: { cost_usd: 15, total_tokens: 150000, net_tokens: 60000 },
        workbuddy: { cost_usd: 5, total_tokens: 50000, net_tokens: 20000 },
      },
      by_model: {
        "claude-desktop::claude-opus-5-5": { provider: "claude-desktop", model: "claude-opus-5-5", cost_usd: 20, total_tokens: 200000, net_tokens: 80000 },
        "claude::claude-opus-5-5": { provider: "claude", model: "claude-opus-5-5", cost_usd: 10, total_tokens: 100000, net_tokens: 40000 },
        "antigravity::gemini-4-neon-high": { provider: "antigravity", model: "gemini-4-neon-high", cost_usd: idx % 2 ? 10 : 5, total_tokens: 100000, net_tokens: 40000 },
        "antigravity::gemini-4-neon-xhigh": { provider: "antigravity", model: "gemini-4-neon-xhigh", cost_usd: idx % 2 ? 5 : 10, total_tokens: 50000, net_tokens: 20000 },
        "workbuddy::opencode-go/GLM-5.3": { provider: "workbuddy", model: "opencode-go/GLM-5.3", cost_usd: 5, total_tokens: 50000, net_tokens: 20000 },
      },
    })),
    prev_by_model: [
      { provider: "antigravity", model: "gemini-4-neon-medium", cost_usd: 50, total_tokens: 500000, net_tokens: 200000 },
      { provider: "claude", model: "claude-opus-5-5", cost_usd: 100, total_tokens: 1000000, net_tokens: 400000 },
    ],
    providers: [
      {
        provider: "claude-desktop", cost_usd: 140, total_tokens: 1400000, net_tokens: 560000, cache_hit_rate: 0.98, calls: 70,
        models: [{ key: "claude-desktop::claude-opus-5-5", provider: "claude-desktop", model: "claude-opus-5-5", priced: true, cost_usd: 140, total_tokens: 1400000, net_tokens: 560000, cache_known_prompt: 1000, cache_known_read: 980, cache_hit_rate: 0.98, calls: 70 }],
      },
      {
        provider: "claude", cost_usd: 70, total_tokens: 700000, net_tokens: 280000, cache_hit_rate: 0.96, calls: 35,
        models: [{ key: "claude::claude-opus-5-5", provider: "claude", model: "claude-opus-5-5", priced: true, cost_usd: 70, total_tokens: 700000, net_tokens: 280000, cache_known_prompt: 1000, cache_known_read: 960, cache_hit_rate: 0.96, calls: 35 }],
      },
      {
        provider: "antigravity", cost_usd: 105, total_tokens: 1050000, net_tokens: 420000, cache_hit_rate: 0.90, calls: 50,
        models: [
          { key: "antigravity::gemini-4-neon-high", provider: "antigravity", model: "gemini-4-neon-high", priced: true, cost_usd: 60, total_tokens: 600000, net_tokens: 240000, cache_known_prompt: 600, cache_known_read: 540, cache_hit_rate: 0.90, calls: 30 },
          { key: "antigravity::gemini-4-neon-xhigh", provider: "antigravity", model: "gemini-4-neon-xhigh", priced: true, cost_usd: 45, total_tokens: 450000, net_tokens: 180000, cache_known_prompt: 400, cache_known_read: 360, cache_hit_rate: 0.90, calls: 20 },
        ],
      },
      {
        provider: "workbuddy", cost_usd: 35, total_tokens: 350000, net_tokens: 140000, cache_hit_rate: 0.95, calls: 20,
        models: [{ key: "workbuddy::opencode-go/GLM-5.3", provider: "workbuddy", model: "opencode-go/GLM-5.3", priced: true, cost_usd: 35, total_tokens: 350000, net_tokens: 140000, cache_known_prompt: 500, cache_known_read: 475, cache_hit_rate: 0.95, calls: 20 }],
      },
    ],
  };
  await app.answer(0, payload);

  // 1. Tools view keeps Claude Desktop and Claude Code separate
  assert.equal(app.find(".u-rrow.item").length, 4);
  assert.match(text(app.find(".u-rrow.head")[0]), /工具/);

  // 2. Family view merges Claude Desktop + Claude Code -> Claude ($210), Gemini ($105), GLM ($35)
  app.clickSeg("u-dim", "dim", "family");
  assert.match(text(app.find(".u-rrow.head")[0]), /模型系列/);
  const famRows = app.find(".u-rrow.item");
  assert.equal(famRows.length, 3);
  assert.equal(famRows[0].getAttribute("data-id"), "claude");
  assert.match(text(famRows[0]), /Claude.*\$210\.00/);
  assert.equal(famRows[1].getAttribute("data-id"), "gemini");
  assert.match(text(famRows[1]), /Gemini.*\$105\.00.*110%.*↑/); // vs prev $50 gemini-4-neon-medium!
  // Expanding Claude family lists merged base model claude-opus-5-5
  famRows[0].querySelector(".u-twist").dispatch("click");
  assert.equal(app.find(".u-rrow.sub").length, 1);
  assert.match(text(app.find(".u-rrow.sub")[0]), /claude-opus-5-5.*\$210\.00/);

  // 3. Model view merges across tools + effort variants into 3 base models
  app.clickSeg("u-dim", "dim", "model");
  const modRows = app.find(".u-rrow.item");
  assert.equal(modRows.length, 3);
  assert.equal(modRows[0].getAttribute("data-id"), "claude-opus-5-5");
  assert.match(text(modRows[0]), /claude-opus-5-5.*Claude · 2 工具.*\$210\.00/);
  assert.equal(modRows[1].getAttribute("data-id"), "gemini-4-neon");
  assert.match(text(modRows[1]), /gemini-4-neon.*\$105\.00.*110%.*↑/);
  assert.equal(modRows[2].getAttribute("data-id"), "glm-5.3");
  // Expanding claude-opus-5-5 shows both tools (Claude Desktop + Claude Code)
  modRows[0].querySelector(".u-twist").dispatch("click");
  const toolSubs = app.find(".u-rrow.sub.sub-tool");
  assert.equal(toolSubs.length, 2);
  assert.match(text(toolSubs[0]), /\$140\.00/);
  assert.match(text(toolSubs[1]), /\$70\.00/);
});
