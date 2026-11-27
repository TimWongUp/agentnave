"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const UI = path.join(__dirname, "..");
const read = (name) => fs.readFileSync(path.join(UI, name), "utf8");

test("theme.css owns unified motion tokens and deduplicated keyframes across tray stylesheets", () => {
  const theme = read("theme.css");
  for (const token of [
    "--dur-press:", "--dur-hover:", "--dur-out:", "--dur-in:",
    "--dur-pop-out:", "--dur-pop:", "--dur-view-out:", "--dur-view:",
    "--dur-slide-out:", "--dur-slide:", "--shift-sm:", "--shift-md:",
    "--scale-pop:", "--scale-card-press:", "--ease-out:", "--ease-in:", "--ease-inout:",
  ]) {
    assert.ok(theme.includes(token), `theme.css must define ${token}`);
  }
  assert.match(theme, /@keyframes an-spin\b/);
  assert.match(theme, /@keyframes an-pulse\b/);
  assert.match(theme, /@keyframes an-drip\b/);
  assert.match(theme, /@keyframes an-pulse-dot\b/);
  assert.match(theme, /animation-duration:\s*2\.4s\s*!important/);

  for (const file of ["manage.css", "panel.css", "chat.css", "hosts.css", "usage.css", "onboarding.css", "settings.css"]) {
    const css = read(file);
    assert.doesNotMatch(css, /@keyframes\s+(drip|pulse|bar-update-spin|spin|u-spin|ob-spin|pulse-dot)\b/, `${file} must reuse shared keyframes from theme.css`);
  }
  const tipCss = read("settings-layout.css");
  assert.doesNotMatch(tipCss, /\.setting-tip:not\(:hover\):not\(:focus-visible\)::after\s*\{\s*display:\s*none/, "tooltip must not kill transitions with display: none");
});

test("ANAppearance splits uiReducedMotion from battery_still while keeping reducedMotion unchanged", () => {
  const listeners = [];
  const root = { dataset: { theme: "dark", motion: "system", onBattery: "true", batteryStill: "true" } };
  const context = vm.createContext({
    document: {
      documentElement: root,
      hidden: false,
      querySelectorAll: () => [],
      addEventListener() {},
    },
    location: { protocol: "http:", hostname: "127.0.0.1" },
    matchMedia: () => ({ matches: false, addEventListener: (_, fn) => listeners.push(fn) }),
    addEventListener() {},
    queueMicrotask: (fn) => fn(),
  });
  context.window = context;
  vm.runInContext(read("appearance.js"), context);

  assert.equal(context.ANAppearance.reducedMotion.matches, true, "battery_still still pauses heavy character animations");
  assert.equal(context.ANAppearance.uiReducedMotion.matches, false, "battery_still does not disable lightweight UI motion");
  assert.equal(context.ANMotion.scrollBehavior(), "smooth");

  root.dataset.motion = "reduce";
  assert.equal(context.ANAppearance.uiReducedMotion.matches, true);
  assert.equal(context.ANMotion.scrollBehavior(), "auto");

  root.dataset.motion = "full";
  assert.equal(context.ANAppearance.uiReducedMotion.matches, false);
});

test("laptop.js respects reducedMotion and skips per-frame width/height layout writes", () => {
  let rafCalls = 0;
  let mood = "idle";
  const styleWrites = [];
  const lid = {
    style: new Proxy({}, {
      set(target, prop, value) {
        styleWrites.push(String(prop));
        target[prop] = value;
        return true;
      },
    }),
  };
  const makeEl = (tag) => ({
    tagName: tag.toUpperCase(),
    className: "",
    style: {},
    clientWidth: 100,
    innerHTML: "",
    children: [],
    append(...nodes) { this.children.push(...nodes); },
    setAttribute() {},
    closest: (sel) => (sel === "[data-mood]" ? { dataset: { mood } } : null),
  });
  const context = vm.createContext({
    document: {
      documentElement: {},
      createElement: makeEl,
      createElementNS: (_, tag) => makeEl(tag),
      querySelectorAll: () => [],
    },
    ResizeObserver: class { observe() {} },
    MutationObserver: class { observe() {} },
    requestAnimationFrame: () => { rafCalls += 1; return rafCalls; },
    performance: { now: () => 0 },
    ANAppearance: { reducedMotion: { matches: true } },
  });
  context.window = context;
  vm.runInContext(read("laptop.js"), context);

  const el = context.Laptop.create(lid);
  el.laptop.sync();
  styleWrites.length = 0;
  mood = "working";
  el.laptop.sync();
  assert.equal(rafCalls, 0, "reducedMotion snaps laptop lid directly without rAF loop");
  assert.equal(styleWrites.includes("width"), false, "draw does not rewrite lid width once cached");
  assert.equal(styleWrites.includes("height"), false, "draw does not rewrite lid height once cached");
  assert.ok(styleWrites.includes("transform"), "draw updates lid transform");
});

test("foldRoom animates compositor-only transform and opacity without height/margin/padding", () => {
  let recordedFrames = null;
  const context = vm.createContext({
    AN: { isHidden: () => false },
    ResizeObserver: class { observe() {} },
    reduceMotion: { matches: false },
    el: { floor: {} },
    window: { addEventListener() {} },
    document: { documentElement: { classList: { contains: () => false } } },
    Element: class { animate() {} },
    getComputedStyle: () => ({}),
    setTimeout() {},
    FOLD_MS: 500, WALK_MS: 900, HOLD_MS: 500,
  });
  vm.runInContext(read("chat-office.js"), context);
  context.foldRoom({
    offsetHeight: 120,
    animate(frames) {
      recordedFrames = frames;
      return { finished: Promise.resolve(), cancel() {} };
    },
  });
  assert.ok(Array.isArray(recordedFrames) && recordedFrames.length === 2);
  for (const frame of recordedFrames) {
    assert.ok("transform" in frame && "opacity" in frame);
    for (const banned of ["height", "marginTop", "paddingTop", "paddingBottom", "borderTopWidth", "borderBottomWidth"]) {
      assert.equal(banned in frame, false, `foldRoom must not animate layout property ${banned}`);
    }
  }
});

test("usage-chart drawDonut wraps arcs in rotated <g> and drawCursor uses transform instead of style.left", () => {
  const mkNode = (tag) => {
    const attrs = {};
    const node = {
      tagName: tag,
      attributes: attrs,
      children: [],
      style: {
        vars: {},
        setProperty(k, v) { this.vars[k] = v; },
      },
      classList: { toggle() {} },
      setAttribute(k, v) { attrs[k] = String(v); },
      getAttribute(k) { return attrs[k]; },
      removeAttribute(k) { delete attrs[k]; },
      append(...xs) { this.children.push(...xs.flat().filter(Boolean)); },
      replaceChildren(...xs) { this.children = xs.flat().filter(Boolean); },
    };
    return node;
  };
  const context = vm.createContext({
    AN: {
      h: (sel, attrs = {}, ...kids) => {
        const n = mkNode(sel);
        Object.entries(attrs || {}).forEach(([k, v]) => v != null && n.setAttribute(k, v));
        n.append(...kids);
        return n;
      },
      L: (zh) => zh,
    },
    document: { createElementNS: (_, tag) => mkNode(tag) },
  });
  context.window = context;
  vm.runInContext(read("usage-chart.js"), context);

  const box = mkNode("div");
  const { arcs } = context.UsageChart.drawDonut(box, [
    { id: "claude", name: "Claude", value: 60, provider: "claude", shade: 0, ci: 0 },
    { id: "codex", name: "Codex", value: 40, provider: "codex", shade: 0, ci: 1 },
  ], 100, (v) => `$${v}`);
  const svg = box.children[0];
  assert.equal(svg.children[0].tagName, "g");
  assert.equal(svg.children[0].getAttribute("transform"), "rotate(-90 74 74)");
  assert.equal(arcs.length, 2);
  assert.equal(arcs[0].getAttribute("transform"), undefined, "arc leaves rotation to parent <g> so CSS transform stays safe");
  assert.ok(arcs[0].style.vars["--dx"] && arcs[0].style.vars["--dy"]);

  const tip = mkNode("div");
  tip.hidden = true;
  const geo = {
    x: (i) => 100 + i * 200,
    y: () => 100,
    Tpad: 16, base: 200, totals: [10, 20], W: 600, band: 40, bars: false,
    crossLayer: mkNode("g"), bandLayer: mkNode("g"), tip,
  };
  context.UsageChart.drawCursor(geo, {
    plot: mkNode("div"), cursor: 1,
    buckets: [{ has: true, label: "Day 1" }, { has: true, label: "Day 2" }],
    items: [{ id: "claude", name: "Claude", values: [10, 20], provider: "claude", shade: 0, ci: 0 }],
    grain: "day", mode: "total", pinned: new Set(), hover: null, dim: "provider",
    fmt: (v) => `$${v}`, hitExact: () => "70%", hitOfBucket: () => 0.7,
  });
  assert.equal(tip.hidden, false);
  assert.equal(tip.style.left, undefined, "tooltip must not set layout property style.left");
  assert.match(tip.style.transform, /translate3d\(/, "tooltip must position with GPU translate3d");

  // Wide model tooltip near the midline must clamp strictly inside [0, W - tipW] with no overflow.
  tip.offsetWidth = 260;
  const narrowGeo = { ...geo, W: 380, x: () => 195 };
  context.UsageChart.drawCursor(narrowGeo, {
    plot: mkNode("div"), cursor: 1,
    buckets: [{ has: true, label: "Day 1" }, { has: true, label: "Day 2" }],
    items: [{ id: "m", name: "claude-opus-5-5-very-long-model", values: [10, 20], provider: "claude", shade: 0, ci: 0 }],
    grain: "day", mode: "total", pinned: new Set(), hover: null, dim: "model",
    fmt: (v) => `$${v}`, hitExact: () => "70%", hitOfBucket: () => 0.7,
  });
  const m = /translate3d\(([-\d.]+)px/.exec(tip.style.transform);
  assert.ok(m, "clamped tooltip transform must be numeric translate3d");
  const left = Number(m[1]);
  assert.ok(left >= 0 && left + 260 <= 380, `wide tooltip left=${left} must stay within [0, 120]`);
});

test("ANMotion primitives cover flipY, dialogEnter, reveal, batch syncThumbs, and reduced-motion degradation", () => {
  const ops = [];
  const docEl = {
    attrs: {},
    setAttribute(k, v) { this.attrs[k] = String(v); },
    removeAttribute(k) { delete this.attrs[k]; },
    getAttribute(k) { return this.attrs[k] ?? null; },
    dataset: {},
  };
  let roDisconnected = 0;
  const rafs = [];
  const context = vm.createContext({
    AN: {
      reducedMotion: { matches: false, addEventListener() {} },
      isHidden: () => false,
    },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    addEventListener() {},
    getComputedStyle: () => ({ getPropertyValue: (k) => (k === "--dur-in" ? "180ms" : k === "--dur-pop" ? "0.1s" : "") }),
    document: {
      documentElement: docEl,
      visibilityState: "visible",
      hasFocus: () => true,
      addEventListener() {},
      querySelectorAll: () => [],
    },
    requestAnimationFrame: (fn) => { rafs.push(fn); return rafs.length; },
    cancelAnimationFrame() {},
    ResizeObserver: class {
      observe() {}
      disconnect() { roDisconnected += 1; }
    },
  });
  context.window = context;
  context.globalThis = context;
  vm.runInContext(read("appearance.js"), context);
  const M = context.ANMotion;

  assert.equal(M.ms("--dur-in", 99), 180);
  assert.equal(M.ms("--dur-pop", 99), 100);
  assert.equal(M.ms("--missing", 77), 77);

  // flipY animates moved elements via translate3d and skips unmoved ones.
  const calls = [];
  const a = { offsetParent: {}, offsetTop: 10, isConnected: true, animate: (f, o) => calls.push({ el: "a", f, o }) };
  const b = { offsetParent: {}, offsetTop: 60, isConnected: true, animate: (f, o) => calls.push({ el: "b", f, o }) };
  M.flipY([a, b], () => { a.offsetTop = 60; b.offsetTop = 60; }, { duration: 150 });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].el, "a");
  assert.equal(calls[0].f[0].transform, "translate3d(0, -50px, 0)");

  // dialogEnter & reveal invoke WAAPI when visible/focused, and skip translate in reduced motion.
  const dlgCalls = [];
  M.dialogEnter({ animate: (f, o) => dlgCalls.push({ f, o }) });
  assert.equal(dlgCalls.length, 1);
  assert.ok(dlgCalls[0].f[0].transform);
  context.ANAppearance.applyPreferences({ motion: "reduce" });
  const revCalls = [];
  M.reveal({ animate: (f, o) => revCalls.push({ f, o }) }, { dy: 6 });
  assert.equal(revCalls.length, 1);
  assert.equal(revCalls[0].f[0].transform, undefined, "reduced motion must omit transform");
  context.ANAppearance.applyPreferences({ motion: "full" });

  // syncThumbs performs all reads before all writes (zero interleaved layout thrash) and destroy disconnects RO.
  const mkSeg = (name, x, w) => {
    const btn = {
      get offsetLeft() { ops.push(`read:${name}`); return x; },
      offsetWidth: w,
      offsetHeight: 24,
      offsetTop: 2,
    };
    const box = {
      offsetWidth: 200,
      children: [btn],
      attrs: {},
      querySelector: () => btn,
      setAttribute(k, v) { ops.push(`write:${name}`); this.attrs[k] = v; },
      removeAttribute(k) { ops.push(`write:${name}`); delete this.attrs[k]; },
      style: { setProperty(k, v) { ops.push(`write:${name}:${k}=${v}`); } },
    };
    return M.attachThumb(box, ".active");
  };
  const t1 = mkSeg("s1", 4, 50);
  const t2 = mkSeg("s2", 60, 70);
  M.syncThumbs([t1, t2], false);
  const lastRead = ops.map((x, i) => (x.startsWith("read:") ? i : -1)).filter((i) => i >= 0).at(-1);
  const firstWrite = ops.findIndex((x) => x.startsWith("write:"));
  assert.ok(lastRead < firstWrite, `all reads must finish before writes, got: ${ops.join(", ")}`);
  t1.destroy();
  t2.destroy();
  assert.equal(roDisconnected, 2);
});

test("update-dialog reuses download progress bar and creates .upd-detail when downloaded transitions from 0 to >0", () => {
  const mkEl = (sel, attrs = {}, ...kids) => {
    const parts = sel.split(".");
    const tag = parts[0] || "div";
    const classes = new Set(parts.slice(1));
    const el = {
      tagName: tag.toUpperCase(),
      classes,
      classList: { add: (c) => classes.add(c), contains: (c) => classes.has(c) },
      attrs: { ...attrs },
      style: {},
      children: [],
      parentNode: null,
      open: false,
      textContent: "",
      addEventListener() {},
      showModal() { this.open = true; },
      close() { this.open = false; },
      focus() {},
      append(...xs) {
        for (const x of xs.flat()) {
          if (!x) continue;
          if (typeof x === "string") this.textContent += x;
          else { x.parentNode = this; this.children.push(x); }
        }
      },
      replaceChildren(...xs) { this.children = []; this.textContent = ""; this.append(...xs); },
      insertBefore(node, ref) {
        node.parentNode = this;
        const idx = this.children.indexOf(ref);
        if (idx >= 0) this.children.splice(idx, 0, node);
        else this.children.push(node);
      },
      querySelector(q) {
        const match = (n) => {
          if (q === ".upd-bar i") return n.tagName === "I" && n.parentNode?.classes.has("upd-bar");
          if (q === ".upd-detail") return n.classes.has("upd-detail");
          if (q === ".primary") return n.classes.has("primary");
          return false;
        };
        const walk = (n) => {
          for (const c of n.children) {
            if (match(c)) return c;
            const hit = walk(c);
            if (hit) return hit;
          }
          return null;
        };
        return walk(this);
      },
    };
    el.append(...kids);
    return el;
  };
  const bodyNode = mkEl("body");
  let pollIdx = 0;
  const polls = [
    { update: { state: "installing", version: "1.18.0", step: "download", downloaded: 0, total: 10485760 }, elapsed: 0 },
    { update: { state: "installing", version: "1.18.0", step: "download", downloaded: 5242880, total: 10485760 }, elapsed: 1 },
    { update: { state: "installing", version: "1.18.0", step: "download", downloaded: 10485760, total: 10485760 }, elapsed: 2 },
    { update: { state: "failed", version: "1.18.0", step: "download", error: "stop-loop" }, elapsed: 3 },
  ];
  let barBeforeSecondPoll = null;
  let barAt100 = null;
  let detailAt100 = null;
  let rowHadTold = false;
  const context = vm.createContext({
    AN: {
      h: mkEl,
      L: (zh) => zh,
      load: async () => {
        if (pollIdx === 1) barBeforeSecondPoll = bodyNode.children[0]?.querySelector(".upd-bar i");
        if (pollIdx === 3) {
          const dlg = bodyNode.children[0];
          barAt100 = dlg?.querySelector(".upd-bar i");
          detailAt100 = dlg?.querySelector(".upd-detail");
          rowHadTold = Boolean(barAt100?.parentNode?.parentNode?.classes.has("told"));
        }
        return polls[pollIdx++] || { update: { state: "idle" } };
      },
      post: async () => ({}),
    },
    document: { body: bodyNode },
    addEventListener() {},
    dispatchEvent() {},
    CustomEvent: class { constructor(t, o) { this.type = t; this.detail = o?.detail; } },
    setTimeout: (fn) => { fn(); return 0; },
  });
  context.window = context;
  context.globalThis = context;
  vm.runInContext(read("update-dialog.js"), context);

  // Trigger download flow: 0 -> 50% -> 100%
  return context.AN.updateFlow().then(() => {
    assert.ok(barBeforeSecondPoll, "initial downloaded=0 poll must render .upd-bar i");
    assert.equal(barAt100, barBeforeSecondPoll, "subsequent download polls must reuse the exact .upd-bar i node");
    assert.equal(barAt100.style.transform, "scaleX(1.0000)");
    assert.ok(detailAt100, ".upd-detail must be inserted once downloaded > 0");
    assert.match(detailAt100.textContent, /100%（10\.0 \/ 10\.0 MB）/);
    assert.equal(rowHadTold, true, "download row must gain .told when detail is inserted");
  });
});

test("chat.css keeps .jump visibility inherited and hides .jump[hidden] in term-mode", () => {
  const chatCss = read("chat.css");
  assert.match(chatCss, /\.jump\s*\{[^}]*visibility:\s*inherit;/, ".jump must inherit visibility so .conv.folded hides it");
  assert.match(chatCss, /\.conv\.term-mode\s+:is\(\.tl,\s*\.jump,\s*\.voice-channel\)\s*\{\s*display:\s*none\s*!important;\s*\}/);
});
