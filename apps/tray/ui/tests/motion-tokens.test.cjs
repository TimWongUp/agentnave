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
});
