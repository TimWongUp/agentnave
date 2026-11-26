const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { common } = require("./helpers/common.cjs");
const { dom, flush } = require("./helpers/voice.cjs");

const source = (name) => readFileSync(`${__dirname}/../${name}`, "utf8");
const text = (node) => (typeof node === "string" ? node : node.children?.length ? node.children.filter(Boolean).map(text).join("|") : node.textContent || "");
const find = (node, match) => (!node || typeof node === "string" ? []
  : [...(match(node) ? [node] : []), ...(node.children || []).flatMap((child) => find(child, match))]);

function clock() {
  const timers = new Map();
  let now = 0, id = 0;
  return {
    setTimeout: (fn, delay) => { timers.set(++id, { fn, at: now + (delay || 0) }); return id; },
    clearTimeout: (timer) => timers.delete(timer),
    tick(ms) {
      const end = now + ms;
      for (;;) {
        const [next] = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at);
        if (!next) break;
        timers.delete(next[0]);
        now = next[1].at;
        next[1].fn();
      }
      now = end;
    },
  };
}

// ---------------------------------------------------------------- section loader
function loader() {
  const { h } = dom(), time = clock(), observers = new Set();
  const classes = () => {
    const set = new Set();
    return { add: (name) => set.add(name), remove: (name) => set.delete(name), contains: (name) => set.has(name) };
  };
  const pane = Object.assign(h("section"), { classList: classes() });
  const box = { childElementCount: 0, textContent: "" };
  const notice = { classList: classes() };
  const elements = { "pane-voice": pane, "voice-settings": box, notice };
  const AN = { h: (...args) => Object.assign(h(...args), { classList: classes() }), L: (zh) => zh, art: { url: (path) => path } };
  const context = vm.createContext({
    AN, window: {}, document: { getElementById: (id) => elements[id] || null },
    setTimeout: time.setTimeout, clearTimeout: time.clearTimeout,
    MutationObserver: class { constructor(fn) { this.fn = fn; } observe() { observers.add(this); } disconnect() { observers.delete(this); } },
  });
  vm.runInContext(source("pane-loader.js"), context);
  return {
    pane, time,
    show: () => context.window.AgentNavePaneLoader.show("voice"),
    mounted: () => pane.children.filter((child) => !child.removed).length,
    loading: () => pane.classList.contains("is-loading"),
    arrive() { box.childElementCount = 1; for (const observer of observers) observer.fn(); },
  };
}

test("a section that answers at once never shows the loader", () => {
  const app = loader();
  app.show();
  assert.equal(app.loading(), false);
  app.time.tick(100);
  app.arrive();
  app.time.tick(5000);
  assert.equal(app.mounted(), 0);
  assert.equal(app.loading(), false);
});

test("a slow section shows the loader after a short wait and keeps it briefly", () => {
  const app = loader();
  app.show();
  app.show();
  app.time.tick(149);
  assert.equal(app.mounted(), 0);
  app.time.tick(1);
  assert.equal(app.mounted(), 1, "two calls during the wait mount one loader");
  assert.equal(app.loading(), true);
  app.time.tick(50);
  app.arrive();
  app.time.tick(349);
  assert.equal(app.loading(), true, "it stays 400ms once shown");
  app.time.tick(1 + 260);
  assert.equal(app.loading(), false);
  assert.equal(app.mounted(), 0);
});

test("a section that never answers is uncovered in the end", () => {
  const app = loader();
  app.show();
  app.time.tick(150 + 59999);
  assert.equal(app.loading(), true);
  app.time.tick(1 + 260);
  assert.equal(app.loading(), false);
});

// ---------------------------------------------------------------- install timeout copy
test("the install timeout points at the settings section by its real name", async () => {
  const context = vm.createContext({
    document: { documentElement: { lang: "zh", dataset: {}, classList: { toggle() {} } }, hidden: false, querySelectorAll: () => [], querySelector: () => null, addEventListener() {} },
    window: { ANAppearance: { applyTheme() {}, reducedMotion: { matches: true } }, addEventListener() {}, dispatchEvent() {} },
    CustomEvent: class {}, localStorage: { getItem() {}, setItem() {} }, setInterval() {}, setTimeout: (fn) => queueMicrotask(fn),
    fetch: async () => ({ ok: true, json: async () => ({ ok: true, service: { state: "installing" } }) }),
  });
  vm.runInContext(source("common.js"), context);
  const label = source("settings.html").match(/id="nav-about"[^>]*>([^<]+)</)[1];
  await assert.rejects(context.window.AN.installService(), (error) => error.message.includes(`“设置 → ${label}”`));
});

// ---------------------------------------------------------------- usage range switch
async function usage() {
  const { h, document } = dom(), time = clock(), requests = [];
  const byId = new Map();
  document.getElementById = (id) => {
    if (!byId.has(id)) byId.set(id, h("div"));
    return byId.get(id);
  };
  document.querySelectorAll = () => [];
  const AN = {
    ...common(), h, L: (zh) => zh, relTime: () => "刚刚", isHidden: () => false, onVisibility() {}, fullModelNames: () => false,
    hostTile: () => h("span"), cliName: (value) => value, modelName: (value) => value, post: async () => ({}),
    load: (path) => new Promise((resolve, reject) => requests.push({ path, resolve, reject })),
  };
  const context = vm.createContext({
    AN, document, window: { addEventListener() {} }, Intl, queueMicrotask,
    localStorage: { getItem: () => JSON.stringify({ view: "projects" }), setItem() {} },
    setTimeout: time.setTimeout, clearTimeout: time.clearTimeout,
  });
  vm.runInContext(source("usage.js"), context);
  const body = document.getElementById("u-body");
  const answer = async (index, name) => {
    const range = requests[index].path.split("=")[1];
    requests[index].resolve({ range, updated_at: 1, projects: [{ key: name, label: name, total: {} }] });
    await flush();
  };
  return {
    requests, answer, body,
    async fail(index) { requests[index].reject(new Error()); await flush(); },
    shown: () => text(body),
    busy: () => body.attributes["aria-busy"],
    label: () => document.getElementById("u-updated").textContent,
    range(value) { document.getElementById("u-range").listeners.click({ target: { closest: () => ({ dataset: { range: value } }) } }); },
  };
}

test("switching the usage range keeps the last numbers until the new ones arrive", async () => {
  const app = await usage();
  assert.equal(app.shown(), "", "the first load starts empty");
  assert.equal(app.busy(), "false");
  await app.answer(0, "week");
  assert.match(app.shown(), /week/);
  const cards = app.body.children;

  app.range("30d");
  assert.equal(app.body.children, cards, "nothing is redrawn while waiting");
  assert.equal(app.busy(), "true");
  assert.equal(app.label(), "读取中…");

  app.range("today");
  await app.answer(2, "day");
  assert.match(app.shown(), /day/);
  assert.equal(app.busy(), "false");
  await app.answer(1, "month");
  assert.match(app.shown(), /day/, "a late answer for a range left behind is ignored");
});

test("a range that cannot be read does not leave another range's numbers up", async () => {
  const app = await usage();
  await app.answer(0, "week");
  app.range("30d");
  await app.fail(1);
  assert.doesNotMatch(app.shown(), /week/);
  assert.match(app.shown(), /暂时读不到用量/);
  assert.equal(app.busy(), "false");
});

test("a first load that fails says so", async () => {
  const app = await usage();
  await app.fail(0);
  assert.match(app.shown(), /暂时读不到用量/);
});

// ---------------------------------------------------------------- menu bar panel
async function panel({ voice = { enabled: true, engine: "elevenlabs", keys: { elevenlabs: true } }, english = false } = {}) {
  const surface = dom(), { h, nodes, document, elements } = surface, events = {};
  let status = {}, held = null;
  const reply = (value) => structuredClone(value);
  const L = (zh, en) => (english ? en : zh);
  const AN = {
    ...common(english), h, L, $: (id) => document.getElementById(id), href: (path) => path,
    art: { character: () => "default", head: () => h("span.head") }, themeSwitch: () => h("span.theme"),
    load: async (path) => {
      if (path === "/app/voice") return voice;
      if (path === "/app/voice/player") return null;
      if (path === "/app/stage") return { managers: [], counts: { running: 0 }, empty: null };
      const now = reply(status);
      if (held) await held;
      return now;
    },
    setText: (el, value) => { el.textContent = value; },
    setAttr: (el, key, value) => (value == null ? el.removeAttribute(key) : el.setAttribute(key, value)),
    setClass: (el, value) => { el.className = value; }, reconcile: (el, children) => { el.children = children; },
    stickyOrder: () => [], activities: () => [], urgent: () => false, poll: (fn) => { queueMicrotask(fn); return fn; },
  };
  const context = vm.createContext({
    AN, document, URLSearchParams, location: { search: "" }, structuredClone,
    addEventListener: (name, fn) => { events[name] = fn; }, setTimeout() {}, clearTimeout() {},
    fetch: async () => ({ ok: true, json: async () => ({}) }),
  });
  for (const file of ["voice-menu.js", "panel.js"]) vm.runInContext(source(file), context);
  await flush(); await flush();
  const notices = () => elements.get("notices")?.children || [];
  const links = () => notices().flatMap((card) => find(card, (node) => node.tag === "a.notice-action"));
  return {
    nodes, context,
    async feed(next) { status = next; await context.refresh(); },
    // A status read that was already on its way when something else happened.
    async inFlight(during) {
      let release;
      held = new Promise((resolve) => { release = resolve; });
      const reading = context.refresh();
      during();
      held = null;
      release();
      await reading;
    },
    titles: () => notices().map((card) => find(card, (node) => node.tag === "strong")[0].textContent),
    actions: () => links().map((link) => link.textContent),
    retry: () => links().find((link) => link.attributes.href === "/app/upgrade-service").listeners.click(),
    listen: async (snapshot) => { events["agentnave:voice-player"]({ detail: AN.voiceState({ jobs: [], state: "idle", ...snapshot }) }); await flush(); },
  };
}

const failedService = (error = "uv missing") => ({ service: { state: "failed", error } });

test("an update stopped by the service upgrade shows only the service card", async () => {
  const app = await panel();
  await app.feed({ ...failedService(), update: { state: "failed", step: "service", error: "本机服务升级失败，App 未更新" } });
  assert.deepEqual(app.titles(), ["升级本机服务失败"]);
  await app.feed({ ...failedService(), update: { state: "failed", step: "download", error: "下载失败" } });
  assert.deepEqual(app.titles(), ["更新失败", "升级本机服务失败"], "another cause keeps its own card");
  await app.feed({ update: { state: "failed", step: "service", error: "本机服务升级失败，App 未更新" } });
  assert.deepEqual(app.titles(), ["更新失败"], "without a service card the update card stays");
});

test("the command to paste appears only after a retry ends in the same error", async () => {
  const app = await panel();
  await app.feed(failedService());
  assert.deepEqual(app.actions(), ["重试"]);
  await app.feed(failedService());
  assert.deepEqual(app.actions(), ["重试"]);

  await app.inFlight(() => app.retry());
  assert.deepEqual(app.actions(), ["重试"], "a read from before the retry is not its outcome");
  await app.feed({ service: { state: "upgrading", error: null } });
  await app.feed(failedService("network down"));
  assert.deepEqual(app.actions(), ["重试"], "a different error is a first failure");

  app.retry();
  // A retry that fails at once leaves the status exactly as it was.
  await app.feed(failedService("network down"));
  assert.deepEqual(app.actions(), ["复制命令", "重试"]);
  await app.feed(failedService("disk full"));
  assert.deepEqual(app.actions(), ["重试"]);
  app.retry();
  await app.feed({ service: { state: "upgrading", error: null } });
  await app.feed(failedService("disk full"));
  assert.deepEqual(app.actions(), ["复制命令", "重试"]);

  await app.feed({ service: { state: "done", error: null } });
  await app.feed(failedService("disk full"));
  assert.deepEqual(app.actions(), ["重试"], "a success starts over");
});

for (const [english, tip] of [[false, "请先在 设置 → 语音 中配置密钥"], [true, "Configure an API key in Settings → Voice first"]]) {
  test(`a Listen button disabled for want of a key says why (${english ? "en" : "zh"})`, async () => {
    const app = await panel({ english });
    app.context.drawListen(app.context.listenButton(), { group: "g", room: "r" });
    const button = app.nodes.filter((node) => node.tag === "button.voice-listen").at(-1);
    const draw = () => app.context.drawListen(button);

    await app.listen({ enabled: true, ready: false });
    draw();
    assert.equal(button.disabled, true);
    assert.equal(button.attributes.title, tip);

    await app.listen({ enabled: true, ready: true });
    draw();
    assert.equal(button.disabled, false);
    assert.equal(button.attributes.title, undefined);

    await app.listen({ enabled: true, ready: false, error_source: "load" });
    draw();
    assert.equal(button.attributes.title, undefined, "a failed settings read is not a missing key");
  });
}
