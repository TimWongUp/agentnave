const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

// Exercise the browser-facing player with controllable media readiness and frame delivery.
function player({ delayed = false, noFrameCallback = false } = {}) {
  const reveals = [], violations = [], seekWaiters = [];
  let manualSeek = false;
  const videos = [], frames = new Map(), paints = new Map(), timers = new Map(), observers = [], observed = new Set(), srcs = [], loadWaiters = [];
  let counter = 0;
  class Node {
    constructor(tag = "span", classes = "") {
      this.nodeType = 1; this.tagName = tag.toUpperCase(); this.dataset = {}; this.children = []; this.parent = null;
      this.className = classes; this.hidden = false; this.inert = false;
      this.classList = {
        contains: name => this.className.split(/\s+/).includes(name),
        add: name => { if (!this.classList.contains(name)) this.className += ` ${name}`; },
        remove: name => { this.className = this.className.split(/\s+/).filter(x => x !== name).join(" "); },
        toggle: (name, on) => on ? this.classList.add(name) : this.classList.remove(name),
      };
    }
    get isConnected() { return this === root || !!this.parent?.isConnected; }
    matches(selector) {
      return selector.split(",").some(s => {
        s = s.trim();
        if (s === "[hidden]") return this.hidden;
        if (s === "[inert]") return this.inert;
        const [tag, ...classes] = s.split(".");
        return (!tag || this.tagName === tag.toUpperCase()) && classes.every(c => this.classList.contains(c));
      });
    }
    closest(selector) {
      if (selector === "[data-mood]" && this.dataset.mood) return this;
      if (selector !== "[data-mood]" && this.matches(selector)) return this;
      return this.parent?.closest(selector) || null;
    }
    querySelectorAll(selector) { return this.children.flatMap(c => [c, ...c.querySelectorAll(selector)]).filter(c => c.matches(selector)); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    detach() { if (this.parent) this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; }
    append(child) { child.detach(); child.parent = this; this.children.push(child); }
    before(child) { child.detach(); child.parent = this.parent; this.parent.children.splice(this.parent.children.indexOf(this), 0, child); }
    remove() { this.detach(); }
    replaceChildren() { for (const c of [...this.children]) c.remove(); }
  }
  // Playing media is only fresh once a frame of the current position was presented; a seek makes it stale again.
  class Video extends Node {
    constructor() {
      super("video"); this.paused = true; this.ended = false; this.plays = 0; this.loads = 0; this.seeking = false; this.fresh = false; this.removed = false; videos.push(this);
      if (noFrameCallback) this.requestVideoFrameCallback = undefined;
      const { add, remove } = this.classList;
      this.classList.add = name => { add(name); audit(this, name, false); };
      this.classList.remove = name => { const was = this.classList.contains(name); remove(name); if (was) audit(this, name, true); };
    }
    set src(value) {
      this.source = value; srcs.push(value);
      const loaded = () => { if (this.source === value) this.onloadeddata?.(); };
      if (delayed) loadWaiters.push(loaded); else queueMicrotask(loaded);
    }
    get src() { return this.source; }
    set currentTime(value) {
      this.time = value; this.ended = false; this.seeking = true; this.fresh = false;
      const done = () => { this.seeking = false; this.onseeked?.(); };
      if (manualSeek) seekWaiters.push(done); else queueMicrotask(done);
    }
    get currentTime() { return this.time || 0; }
    play() { this.paused = false; this.ended = false; this.plays++; if (noFrameCallback && !this.seeking) this.fresh = true; this.onplaying?.(); return Promise.resolve(); }
    pause() { this.paused = true; }
    load() { this.loads++; }
    removeAttribute(name) { if (name === "src") this.source = ""; }
    remove() { this.removed = true; this.detach(); }
    requestVideoFrameCallback(cb) { const id = ++counter; paints.set(id, () => { if (!this.seeking) this.fresh = true; cb(); }); return id; }
    cancelVideoFrameCallback(id) { paints.delete(id); }
    end() { this.ended = true; this.paused = true; this.onended?.(); }
  }
  // At no point may two players be on screen, and a player may only appear once it shows its first frame.
  function audit(video, name, removed) {
    if (name !== "next") return;
    const shown = videos.filter(v => v.isConnected && !v.classList.contains("next"));
    if (shown.length > 1) violations.push(`${shown.length} players visible`);
    if (removed) {
      reveals.push({ seeking: video.seeking, fresh: video.fresh });
      if (video.seeking || !video.fresh) violations.push(`player shown ${video.seeking ? "while seeking" : "before its first frame"}`);
      // The frame that swaps players must stay light: tearing a player down or starting the next clip's
      // fetch in that same task blanks the new frame on screen, on every desk that swaps at once.
      const before = [videos.reduce((n, v) => n + v.loads, 0), srcs.length];
      queueMicrotask(() => {
        if (videos.reduce((n, v) => n + v.loads, 0) !== before[0] || srcs.length !== before[1]) violations.push("teardown or clip preparation in the swap task");
      });
    }
  }
  const root = new Node("html"), host = new Node("div"), fig = new Node("span", "fig video"), body = new Node("span", "fig-b");
  const poster = new Node("img", "poster");
  root.append(host); host.append(fig); fig.append(body); body.append(poster);
  host.dataset.mood = "idle"; fig.dataset.char = "codex";
  const events = {}, still = { matches: false }, math = Object.create(Math);
  math.random = () => 0.99;
  const document = { hidden: false, createElement: () => new Video(), addEventListener: (n, f) => { events[n] = f; } };
  const ctx = vm.createContext({
    root, document, still, Math: math, artUrl: s => s,
    requestAnimationFrame: fn => { const id = ++counter; frames.set(id, fn); return id; }, cancelAnimationFrame: id => frames.delete(id),
    setTimeout: fn => { const id = ++counter; timers.set(id, fn); return id; }, clearTimeout: id => timers.delete(id),
    MutationObserver: class { constructor(fn) { observers.push(fn); } observe() {} },
    IntersectionObserver: class {
      constructor(fn) { events.intersect = fn; }
      observe(node) { observed.add(node); }
      unobserve(node) { observed.delete(node); }
    },
    window: { addEventListener: (n, f) => { events[n] = f; }, matchMedia: () => ({ addEventListener() {} }) },
  });
  const source = readFileSync(`${__dirname}/../common.js`, "utf8");
  vm.runInContext(source.slice(source.indexOf("  const VIDEO_STATES"), source.indexOf("  // Every character teleports"))
    + source.slice(source.indexOf("  let videoSync"), source.indexOf("  const head ="))
    + "\nglobalThis.api = { syncVideo, videoPlayers };", ctx);
  async function flush() {
    for (let i = 0; i < 12; i++) {
      await Promise.resolve();
      const media = [...paints]; paints.clear(); media.forEach(([, f]) => f());
      const rafs = [...frames]; frames.clear(); rafs.forEach(([, f]) => f());
    }
  }
  const sync = () => ctx.api.syncVideo(fig);
  const visible = async () => { sync(); events.intersect([{ target: fig, isIntersecting: true }]); await flush(); };
  const resolve = () => loadWaiters.splice(0).forEach(f => f());
  return { root, host, fig, body, poster, document, still, math, videos, frames, timers, observed, srcs, events, observers,
    reveals, violations, completeSeeks: () => seekWaiters.splice(0).forEach(f => f()), holdSeeks: () => { manualSeek = true; },
    sync, syncFigure: ctx.api.syncVideo, visible, resolve, flush, rec: () => ctx.api.videoPlayers.get(fig) };
}

test("same idle clip repeats without creating or reloading a player", async () => {
  const app = player(); await app.visible();
  assert.equal(app.videos.length, 1);
  for (let n = 0; n < 20; n++) { app.rec().active.video.end(); await app.flush(); }
  assert.equal(app.videos.length, 1);
  assert.equal(app.srcs.length, 1);
  assert.match(app.srcs[0], /codex\/idle\.mov$/);
  assert.equal(app.videos[0].loads, 0);
  assert.equal(app.videos[0].plays, 21);
});

test("extra clips use a bounded spare and retain the old image until the first frame", async () => {
  const app = player(); app.math.random = () => 0; await app.visible();
  assert.equal(app.videos.length, 2);
  const main = app.rec().active;
  main.video.end();
  assert.equal(app.rec().active, main);
  await app.flush();
  assert.equal(app.rec().active.clip, "slack-1");
  app.math.random = () => 0.99;
  app.rec().active.video.end(); await app.flush();
  app.rec().active.video.end(); await app.flush();
  assert.equal(app.rec().active.clip, "idle");
  assert.equal(app.videos.length, 2);
  assert.equal(app.srcs.length, 2);
  assert.equal(app.rec().slots.length, 1);
  assert.equal(app.videos.filter(v => v.removed).length, 1);
});

// idle -> slack-1, slack-1 played twice, then back to idle (a reused, ended player) and out to slack-1 again.
async function roundTrips(app) {
  const clip = () => app.rec().active.clip, end = async () => { app.rec().active.video.end(); await app.flush(); };
  app.math.random = () => 0; await app.visible();
  await end(); assert.equal(clip(), "slack-1");
  app.math.random = () => 0.99; await end(); assert.equal(clip(), "slack-1");
  app.math.random = () => 0; await end(); assert.equal(clip(), "idle");
  await end(); assert.equal(clip(), "slack-1");
  assert.equal(app.videos.length, 2);
  assert.ok(app.reveals.length >= 4);
  assert.deepEqual(app.violations, []);
}

test("clip changes never show two players or an old frame, across round trips and repeats", async () => {
  await roundTrips(player());
});

test("clip changes stay clean when the browser has no frame callback", async () => {
  const app = player({ noFrameCallback: true });
  await roundTrips(app);
  assert.equal(app.videos.every(v => !v.requestVideoFrameCallback), true);
});

test("a mood change swaps to its looping clip without tearing the old player down in the same task", async () => {
  const app = player(); await app.visible();
  const old = app.rec().active.video;
  app.host.dataset.mood = "working"; app.sync(); await app.flush();
  assert.equal(app.rec().active.state, "working");
  assert.equal(old.classList.contains("next"), true);
  assert.equal(old.loads > 0, true);
  assert.deepEqual(app.violations, []);
});

test("a reused player stays hidden until it is back at its first frame", async () => {
  const app = player(); app.math.random = () => 0; await app.visible();
  app.rec().active.video.end(); await app.flush();
  app.math.random = () => 0.99; app.rec().active.video.end(); await app.flush();
  const old = app.rec().active.video, spare = app.rec().slots.find(s => s !== app.rec().active).video;
  app.holdSeeks(); old.end(); await app.flush();
  assert.equal(old.classList.contains("next"), false);
  assert.equal(spare.classList.contains("next"), true);
  assert.equal(spare.seeking, true);
  app.completeSeeks(); await app.flush();
  assert.equal(spare.classList.contains("next"), false);
  assert.equal(old.classList.contains("next"), true);
  assert.deepEqual(app.violations, []);
});

test("a seek finishing after a state change or hiding cannot start or show the old clip", async () => {
  const app = player(); app.math.random = () => 0; await app.visible();
  app.rec().active.video.end(); await app.flush();
  app.math.random = () => 0.99; app.rec().active.video.end(); await app.flush();
  const spare = app.rec().slots.find(s => s !== app.rec().active).video;
  app.holdSeeks(); app.rec().active.video.end(); await app.flush();
  const plays = spare.plays;
  app.host.dataset.mood = "working"; app.sync();
  app.completeSeeks(); await app.flush();
  assert.equal(spare.plays, plays);
  assert.equal(spare.classList.contains("next"), true);
  assert.equal(app.rec().active.state, "working");
  assert.equal(app.videos.filter(v => v.isConnected && !v.classList.contains("next")).length, 1);
  app.host.inert = true; app.sync();
  app.host.inert = false; app.sync(); await app.flush(); app.completeSeeks(); await app.flush();
  assert.equal(app.videos.filter(v => v.isConnected && !v.classList.contains("next")).length, 1);
  assert.equal(app.rec().active.video.paused, false);
  assert.deepEqual(app.violations, []);
});

test("players read their clip by URL, never through a blob, and cannot cast or pop out", async () => {
  const app = player(); app.math.random = () => 0; await app.visible();
  app.rec().active.video.end(); await app.flush();
  assert.equal(app.srcs.length, 2);
  for (const src of app.srcs) assert.match(src, /^video\/codex\/[a-z0-9-]+\.mov$/);
  for (const video of app.videos) {
    assert.equal(video.disableRemotePlayback, true);
    assert.equal(video.disablePictureInPicture, true);
  }
});

test("a long run of extra clips keeps at most two live players, and every dropped one lets go of its source", async () => {
  const app = player(); await app.visible();
  const end = async () => { app.rec().active.video.end(); await app.flush(); };
  for (let n = 0; n < 40; n++) {
    app.math.random = () => (n % 3 === 0 ? 0 : 0.99);
    await end();
    assert.ok(app.rec().slots.length <= 2);
    assert.ok(app.body.querySelectorAll("video").length <= 2);
  }
  const dropped = app.videos.filter(v => v.removed);
  assert.ok(dropped.length > 5);
  assert.ok(dropped.every(v => v.src === "" && v.paused));
  assert.equal(app.videos.length - dropped.length, app.rec().slots.length);
  assert.deepEqual(app.violations, []);
});

test("late loads of superseded clips cannot attach or start players", async () => {
  const app = player({ delayed: true }); await app.visible();
  app.host.dataset.mood = "working"; app.sync();
  app.host.dataset.mood = "idle"; app.sync();
  app.resolve(); await app.flush();
  assert.equal(app.videos.filter(v => v.plays).length, 1);
  assert.equal(app.body.querySelectorAll("video").length, 1);
  assert.equal(app.rec().active.state, "idle");
  assert.equal(app.videos.filter(v => v.removed).length, 2);
});

test("a pending load cannot start while hidden, and resumes when visible", async () => {
  const app = player({ delayed: true }); await app.visible();
  app.document.hidden = true; app.events.visibilitychange(); await app.flush();
  app.resolve(); await app.flush();
  assert.equal(app.videos.reduce((n, v) => n + v.plays, 0), 0);
  assert.equal(app.body.querySelectorAll("video").length, 0);
  app.document.hidden = false; app.events.visibilitychange(); await app.flush();
  app.resolve(); await app.flush();
  assert.equal(app.rec().active.video.paused, false);
});

test("hiding releases active players immediately without waiting for a background animation frame", async () => {
  const app = player(); await app.visible();
  app.events.intersect([{ target: app.fig, isIntersecting: false }]);
  assert.equal(app.rec().active.video.paused, true);
  assert.equal(app.timers.size, 1);
  app.root.dataset.windowHidden = "true"; app.sync();
  assert.equal(app.rec().active, null);
  assert.equal(app.timers.size, 0);
  assert.equal(app.body.querySelectorAll("video").length, 0);
  assert.equal(app.frames.size, 0);
  await app.flush();
  assert.equal(app.videos[0].removed, true);
  assert.equal(app.videos[0].src, "");
});

test("figures of the same character play from their own players and release independently", async () => {
  const app = player(); await app.visible();
  const Node = app.fig.constructor, second = new Node("span", "fig video"), body = new Node("span", "fig-b");
  second.dataset.char = "codex"; second.append(body); app.host.append(second);
  app.syncFigure(second); app.events.intersect([{ target: second, isIntersecting: true }]); await app.flush();
  assert.equal(app.videos.length, 2);
  assert.equal(app.srcs.length, 2);
  app.fig.remove(); app.sync(); await app.flush();
  assert.equal(app.videos[0].src, "");
  assert.equal(body.querySelector("video").paused, false);
  second.remove(); app.syncFigure(second); await app.flush();
  assert.equal(app.videos[1].src, "");
});

test("covered offices and reduced motion stop media, then restore the current mood", async () => {
  const app = player(); await app.visible();
  app.host.inert = true; app.sync();
  assert.equal(app.rec().active, null);
  assert.equal(app.body.querySelectorAll("video").length, 0);
  app.host.inert = false; app.sync(); await app.flush();
  assert.equal(app.rec().active.video.paused, false);
  app.still.matches = true; app.sync(); await app.flush();
  assert.equal(app.body.querySelectorAll("video").length, 0);
  assert.equal(app.fig.classList.contains("video-on"), false);
  app.host.dataset.mood = "working"; app.sync();
  assert.match(app.poster.src, /working.webp/);
  app.still.matches = false; app.sync(); await app.flush();
  assert.equal(app.rec().active.state, "working");
});

test("removing a figure releases sources and observers without reacting to unrelated DOM", async () => {
  const app = player(); await app.visible();
  app.observers[0]([{ type: "childList", addedNodes: [], removedNodes: [app.poster] }]);
  assert.equal(app.frames.size, 0);
  app.fig.remove();
  app.observers[0]([{ type: "childList", addedNodes: [], removedNodes: [app.fig] }]);
  await app.flush();
  assert.equal(app.observed.has(app.fig), false);
  assert.equal(app.rec(), undefined);
  assert.equal(app.videos[0].src, "");
  assert.equal(app.videos[0].removed, true);
});

test("a playback error releases media and leaves a static poster", async () => {
  const app = player(); await app.visible();
  app.rec().active.video.onerror(); await app.flush();
  assert.equal(app.body.querySelectorAll("video").length, 0);
  assert.equal(app.fig.classList.contains("video-on"), false);
  assert.equal(app.videos[0].removed, true);
  app.sync(); await app.flush();
  assert.equal(app.srcs.length, 1);
});

test("video characters clear hidden particles instead of continuing to emit", () => {
  const source = readFileSync(`${__dirname}/../common.js`, "utf8");
  const start = source.indexOf("  setInterval(() => {\n    if (document.hidden || blurred || still.matches) return;");
  let emitted = 0;
  const fx = { isConnected: true, childElementCount: 1, replaceChildren() { this.childElementCount = 0; }, append() { emitted++; } };
  const fig = { classList: { contains: name => name === "video" }, closest: () => ({ dataset: { mood: "idle" } }) };
  vm.runInNewContext(source.slice(start, source.indexOf("  const frameUrl", start)), {
    document: { hidden: false }, blurred: false, still: { matches: false }, performance: { now: () => 10000 },
    emitters: new Set([{ fig, fx, name: "muse", born: 0, next: 0, spec: { origin: { idle: [50, 50] }, sprites: ["note-1.webp"] } }]),
    EMIT_EVERY: { idle: 2600 }, h: () => ({ addEventListener() {} }), artUrl: s => s,
    setInterval: fn => fn(),
  });
  assert.equal(emitted, 0);
  assert.equal(fx.childElementCount, 0);
});

test("an unfocused window emits no particles", () => {
  const source = readFileSync(`${__dirname}/../common.js`, "utf8");
  const start = source.indexOf("  setInterval(() => {\n    if (document.hidden || blurred || still.matches) return;");
  const emitted = blurred => {
    let count = 0;
    const fx = { isConnected: true, childElementCount: 0, replaceChildren() {}, append() { count++; } };
    const fig = { classList: { contains: () => false }, closest: () => ({ dataset: { mood: "idle" } }) };
    vm.runInNewContext(source.slice(start, source.indexOf("  const frameUrl", start)), {
      document: { hidden: false }, blurred, still: { matches: false }, performance: { now: () => 10000 },
      emitters: new Set([{ fig, fx, name: "muse", born: 0, next: 0, spec: { origin: { idle: [50, 50] }, sprites: ["note-1.webp"] } }]),
      EMIT_EVERY: { idle: 2600 }, h: () => ({ addEventListener() {} }), artUrl: s => s, Math,
      setInterval: fn => fn(),
    });
    return count;
  };
  assert.equal(emitted(false), 1);
  assert.equal(emitted(true), 0);
});

test("video packs stay off in static mode and switch figures in place across 272p, 544p and static", async () => {
  const source = readFileSync(`${__dirname}/../common.js`, "utf8");
  const start = source.indexOf("  const FAMILY_ART =");
  const end = source.indexOf("  const head =");
  const fetches = [], made = [], listeners = {};
  let packs = {
    "272": { installed: true, stamp: "s272", bytes: 1000, download: null },
    "544": { installed: true, stamp: "s544", bytes: 2000, download: null },
  };
  const figs = [];
  class Elem {
    constructor(tag = "span", cls = "") {
      this.nodeType = 1; this.tagName = tag.toUpperCase(); this.className = cls;
      this.dataset = {}; this.style = { cssText: "" }; this.children = []; this.parent = null;
      this.classList = {
        contains: n => this.className.split(/\s+/).includes(n),
        add: (...ns) => ns.forEach(n => { if (!this.classList.contains(n)) this.className = `${this.className} ${n}`.trim(); }),
        remove: (...ns) => { this.className = this.className.split(/\s+/).filter(x => !ns.includes(x)).join(" "); },
        toggle: (n, on) => on ? this.classList.add(n) : this.classList.remove(n),
      };
    }
    get isConnected() { return true; }
    matches(sel) { return sel === ".fig.video" && this.classList.contains("fig") && this.classList.contains("video"); }
    closest(sel) { return sel === "[data-mood]" ? { dataset: { mood: "idle" } } : null; }
    querySelector(sel) {
      if (sel === ".fig-b") return this.children[0]?.children?.[0] || null;
      if (sel === "img.poster") return this.querySelector(".fig-b")?.children.find(c => c.classList?.contains("poster")) || null;
      return null;
    }
    querySelectorAll() { return []; }
    append(...kids) { kids.flat().filter(Boolean).forEach(k => { k.parent = this; this.children.push(k); }); }
    before(child) { child.parent = this.parent; this.parent.children.unshift(child); }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter(c => c !== this); }
    addEventListener() {}
    set src(v) { this.source = v; if (this.tagName === "VIDEO" && v) fetches.push(v); }
    get src() { return this.source; }
    removeAttribute(n) { if (n === "src") this.src = ""; }
    play() { this.paused = false; this.onplaying?.(); return Promise.resolve(); }
    pause() { this.paused = true; }
    load() {}
  }
  const root = new Elem("html");
  const h = (tag, props = {}, ...kids) => {
    const [name, ...cls] = tag.split(".");
    const el = new Elem(name || "div", cls.join(" "));
    if (props["data-char"]) el.dataset.char = props["data-char"];
    if (props.src) el.src = props.src;
    if (props.style) el.style.cssText = props.style;
    el.append(...kids);
    return el;
  };
  const ctx = vm.createContext({
    root, h, href: s => s, still: { matches: false }, Math, performance: { now: () => 0 },
    setInterval: () => 1, setTimeout: () => 1,
    requestAnimationFrame: fn => { queueMicrotask(fn); return 1; }, cancelAnimationFrame() {},
    MutationObserver: class { observe() {} }, IntersectionObserver: undefined,
    localStorage: { setItem() {}, getItem: () => null },
    document: {
      hidden: false,
      createElement: tag => { const e = new Elem(tag); made.push(e); return e; },
      querySelectorAll: sel => sel === ".fig[data-char]" ? figs : [],
      addEventListener() {},
    },
    window: {
      ANAppearance: {
        reducedMotion: { matches: false },
        preferences: () => ({ character_style: root.dataset.characterStyle || "static" }),
      },
      addEventListener: (n, fn) => { (listeners[n] ||= []).push(fn); },
      matchMedia: () => ({ addEventListener() {} }),
    },
    fetch: async url => {
      const path = String(url);
      fetches.push(path);
      if (path.startsWith("/app/video-packs")) return { ok: true, json: async () => ({ packs }) };
      if (path.startsWith("/app/characters") || path.includes(".json")) return { ok: true, json: async () => ({}) };
      return { ok: true };
    },
  });
  vm.runInContext(`${source.slice(start, end)}\nglobalThis.api = { figure, canWarp, loadVideoPacks };`, ctx);
  await new Promise(r => setImmediate(r));

  const gemini = ctx.api.figure("gemini");
  const customGpt = ctx.api.figure("@gpt@c1");
  const previewGpt = ctx.api.figure("@gpt@c1", "m", true);
  figs.push(gemini, customGpt, previewGpt);
  assert.match(gemini.style.cssText, /--vs:1\.212;--vo:34\.9%;--vx:33\.5%/);
  assert.match(customGpt.style.cssText, /--vs:1\.092;--vo:95\.4%/);
  assert.equal(gemini.classList.contains("video"), false);
  assert.equal(customGpt.classList.contains("video"), false);
  assert.equal(ctx.api.canWarp("gemini"), false);
  assert.equal(ctx.api.canWarp("@gpt@c1"), false);
  assert.equal(fetches.some(u => u.includes("/art/video/")), false);

  root.dataset.characterStyle = "video-272";
  listeners["agentnave:preferences"].forEach(fn => fn());
  await new Promise(r => setImmediate(r));
  assert.equal(gemini.classList.contains("video"), true);
  assert.equal(customGpt.classList.contains("video"), true);
  assert.equal(previewGpt.classList.contains("video"), false);
  assert.equal(ctx.api.canWarp("gemini"), true);
  assert.equal(ctx.api.canWarp("@gpt@c1"), true);
  assert.equal(gemini.querySelector("img.poster").src, "/art/video/272/gemini/idle.webp?v=s272");
  assert.equal(customGpt.querySelector("img.poster").src, "/art/video/272/codex/idle.webp?v=s272");
  assert.ok(fetches.includes("/art/video/272/gemini/idle.mov?v=s272"));
  assert.ok(fetches.includes("/art/video/272/codex/idle.mov?v=s272"));

  root.dataset.characterStyle = "video-544";
  listeners["agentnave:preferences"].forEach(fn => fn());
  await new Promise(r => setImmediate(r));
  assert.equal(gemini.querySelector("img.poster").src, "/art/video/544/gemini/idle.webp?v=s544");
  assert.equal(customGpt.querySelector("img.poster").src, "/art/video/544/codex/idle.webp?v=s544");
  assert.ok(fetches.includes("/art/video/544/gemini/idle.mov?v=s544"));
  assert.ok(fetches.includes("/art/video/544/codex/idle.mov?v=s544"));
  assert.equal(made.filter(v => v.src).length, 2);
  assert.ok(made.filter(v => v.src).every(v => v.src.includes("/544/")));

  root.dataset.characterStyle = "static";
  listeners["agentnave:preferences"].forEach(fn => fn());
  await new Promise(r => setImmediate(r));
  assert.equal(gemini.classList.contains("video"), false);
  assert.equal(customGpt.classList.contains("video"), false);
  assert.equal(ctx.api.canWarp("gemini"), false);
  assert.equal(ctx.api.canWarp("@gpt@c1"), false);
  assert.equal(made.filter(v => v.src).length, 0);
});

test("avatar settings disables uninstalled video tiers, falls back to CSS on delete, and embeds the DIY CSS guide", async () => {
  const source = readFileSync(`${__dirname}/../avatar-settings.js`, "utf8");
  const listeners = {}, posts = [], copied = [];
  class Elem {
    constructor(tag = "div", cls = "") {
      this.tagName = tag.toUpperCase();
      this.className = cls;
      this.children = [];
      this.attrs = {};
      this.events = {};
      this.textContent = "";
      this.disabled = false;
      this.open = false;
    }
    setAttribute(k, v) {
      this.attrs[k] = String(v);
      if (k === "disabled") this.disabled = true;
    }
    append(...kids) {
      for (const k of kids.flat(Infinity)) {
        if (k == null || k === false) continue;
        if (typeof k === "string") this.textContent += k;
        else this.children.push(k);
      }
    }
    replaceChildren(...kids) { this.children = []; this.textContent = ""; this.append(...kids); }
    addEventListener(type, fn) { (this.events[type] ||= []).push(fn); }
    click(target = this) { if (!this.disabled) (this.events.click || []).forEach(fn => fn({ target })); }
    showModal() { this.open = true; }
    close() { this.open = false; }
    findAll(pred, out = []) {
      if (pred(this)) out.push(this);
      for (const c of this.children) if (c instanceof Elem) c.findAll(pred, out);
      return out;
    }
    allText() {
      return this.textContent + this.children.map(c => (c instanceof Elem ? c.allText() : "")).join(" ");
    }
  }
  const h = (spec, props = {}, ...kids) => {
    const [tag, ...cls] = spec.split(".");
    const el = new Elem(tag || "div", cls.join(" "));
    for (const [k, v] of Object.entries(props || {})) {
      if (v != null && v !== false) el.setAttribute(k, v === true ? "" : v);
    }
    el.append(...kids);
    return el;
  };
  const root = new Elem("div");
  const packs = {
    "272": { installed: false, stamp: null, bytes: 0, download: null },
    "544": { installed: true, stamp: "s544", bytes: 42 * 1048576, download: null },
  };
  const ctx = vm.createContext({
    AN: {
      h,
      L: (zh) => zh,
      post: async (path, body) => {
        posts.push({ path, body });
        if (path === "/app/video-packs/272/import") {
          packs["272"] = { installed: true, stamp: "s272", bytes: 15 * 1048576, download: null };
          return { packs };
        }
        if (path === "/app/video-packs/544/delete") {
          packs["544"] = { installed: false, stamp: null, bytes: 0, download: null };
          return { packs };
        }
        return {};
      },
      art: {
        STATES: ["idle", "blink", "working", "speaking", "review", "done", "sleep"],
        EXTRA: ["sad", "walk-1", "walk-2"],
        figure: (_name, _size, staticOnly) => {
          const el = new Elem("span");
          if (staticOnly) el.setAttribute("data-static-only", "true");
          return el;
        },
        character: () => "claude",
        videoPacksChanged: async () => {},
        charactersChanged: async () => {},
      },
    },
    Avatars: { families: { claude: ["Claude"] }, create: () => new Elem("span") },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    navigator: { clipboard: { writeText: async text => { copied.push(text); } } },
    document: {
      documentElement: { dataset: { characterStyle: "static" } },
      getElementById: id => (id === "avatar-settings" ? root : null),
    },
    window: {
      addEventListener: (t, fn) => { (listeners[t] ||= []).push(fn); },
      dispatchEvent: ev => { (listeners[ev.type] || []).forEach(fn => fn(ev)); },
    },
    fetch: async url => {
      if (url === "/app/video-packs") return { ok: true, json: async () => ({ packs }) };
      return { ok: true, json: async () => ({}) };
    },
    clearTimeout() {},
    setTimeout: () => 1,
  });
  vm.runInContext(source, ctx);
  await new Promise(r => setImmediate(r));

  const status = root.children[0];
  const tip = root.findAll(el => el.className.includes("setting-tip"))[0];
  assert.equal(tip.textContent, "?");
  assert.match(tip.attrs["aria-label"], /150–300 MB/);
  const radios = () => root.findAll(el => el.attrs.role === "radio");
  assert.equal(radios()[0].textContent, "CSS 小人");
  assert.equal(radios()[0].disabled, false);
  assert.equal(radios()[1].disabled, true);
  assert.equal(radios()[1].attrs.title, "请先安装 272p 视频包");
  assert.equal(radios()[2].disabled, false);
  const badges = () => root.findAll(el => el.className.includes("video-pack-status")).map(el => el.textContent);
  assert.deepEqual(badges(), ["未安装 · 180.4 MB", "已安装 · 42.0 MB"]);

  radios()[2].click();
  await new Promise(r => setImmediate(r));
  assert.equal(status.textContent, "已切换为 544p 视频小人");
  assert.equal(radios()[2].attrs["aria-checked"], "true");

  // Deleting the active 544p pack falls back to CSS ("static") and disables the 544p button.
  const del544 = root.findAll(el => el.tagName === "BUTTON" && el.textContent === "删除")[0];
  del544.click();
  await new Promise(r => setImmediate(r));
  assert.equal(radios()[0].attrs["aria-checked"], "true");
  assert.equal(radios()[2].disabled, true);
  assert.ok(posts.some(p => p.path === "/app/preferences" && p.body?.general?.character_style === "static"));

  // Late preferences broadcast for an uninstalled tier stays on CSS ("static").
  ctx.window.dispatchEvent({ type: "agentnave:preferences", detail: { character_style: "video-544" } });
  assert.equal(radios()[0].attrs["aria-checked"], "true");
  assert.equal(radios()[2].attrs["aria-checked"], "false");

  // Importing 272p enables the 272p radio option.
  const importBtns = root.findAll(el => el.tagName === "BUTTON" && el.textContent === "从文件夹导入…");
  importBtns[0].click();
  await new Promise(r => setImmediate(r));
  assert.equal(radios()[1].disabled, false);
  radios()[1].click();
  await new Promise(r => setImmediate(r));
  assert.equal(status.textContent, "已切换为 272p 视频小人");

  radios()[0].click();
  await new Promise(r => setImmediate(r));
  assert.equal(status.textContent, "已切换为 CSS 小人");

  // Preview in the family editor always uses staticOnly.
  assert.equal(root.findAll(el => el.attrs["data-static-only"] === "true").length, 1);

  const guideBtn = root.findAll(el => el.className.includes("char-guide-btn"))[0];
  const dialog = root.findAll(el => el.tagName === "DIALOG")[0];
  assert.equal(guideBtn.textContent, "自制与更换指南");
  assert.equal(dialog.open, false);
  guideBtn.click();
  assert.equal(dialog.open, true);
  const text = dialog.allText();
  assert.match(text, /27 张真透明底正方形 WebP/);
  assert.match(text, /build_css_character\.py/);
  assert.equal(dialog.findAll(el => el.tagName === "A").length, 0);
  const copyBtn = dialog.findAll(el => el.className.includes("char-guide-copy"))[0];
  copyBtn.click();
  assert.equal(copyBtn.textContent, "已复制");
  assert.equal(copied.length, 1);
  dialog.click(dialog);
  assert.equal(dialog.open, false);
});
