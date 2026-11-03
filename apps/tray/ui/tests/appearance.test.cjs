const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function appearance({ saved = {}, legacy = null, systemReduce = false, failWrite = false, transport } = {}) {
  const listeners = {}, requests = [], buttons = [], nodes = new Map();
  const on = (name, fn) => { (listeners[name] ||= []).push(fn); };
  const emit = async (name, event = {}) => { for (const fn of listeners[name] || []) await fn(event); };
  const root = { dataset: { theme: saved.theme || "system", motion: saved.motion || "system" } };
  const document = { documentElement: root, querySelectorAll: () => buttons, getElementById: id => nodes.get(id), addEventListener: on };
  const context = {
    document, window: { addEventListener: on, dispatchEvent(event) { void emit(event.type, event); } },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } }, location: { protocol: "agentnave:" }, queueMicrotask,
    matchMedia: query => ({ matches: query.includes("reduced-motion") ? systemReduce : false, addEventListener() {} }),
    addEventListener: on,
    localStorage: { getItem: () => legacy, removeItem: () => { legacy = null; } },
    async fetch(path, options) {
      requests.push({ path, options });
      if (transport) return transport(path, options);
      if (options?.method === "POST") {
        if (failWrite) return { ok: false, status: 500 };
        if (!options.headers["X-AgentNave-Migrate-Theme"] || !saved.theme) Object.assign(saved, JSON.parse(options.body).general);
      }
      return { ok: true, json: async () => ({ preferences: { general: { ...saved } }, error: null }) };
    },
  };
  vm.runInNewContext(readFileSync(`${__dirname}/../appearance.js`, "utf8"), context);
  function h(tag, attributes, ...children) {
    const node = { tag, children, attributes, classList: { toggle() {} },
      append(...items) { this.children.push(...items); }, replaceChildren(...items) { this.children = items; }, dataset: { themeChoice: attributes["data-theme-choice"] },
      listeners: {}, setAttribute(key, value) { this.attributes[key] = value; },
      addEventListener(name, fn) { this.listeners[name] = fn; } };
    if (tag === "button") buttons.push(node);
    return node;
  }
  function settings() {
    nodes.set("general-prefs", h("div", {}));
    nodes.set("pane-general", { hidden: false });
    context.AN = { h, $: id => nodes.get(id), L: (zh, en) => en,
      load: async path => (await context.fetch(path)).json(),
      post: async (path, body) => (await context.fetch(path, { method: "POST", headers: {}, body: JSON.stringify(body) })).json() };
    vm.runInNewContext(readFileSync(`${__dirname}/../general-settings.js`, "utf8"), context);
    return nodes.get("general-prefs");
  }
  return { api: context.window.ANAppearance, root, emit, requests, saved, buttons, h, settings, legacy: () => legacy };

}

test("legacy theme migrates once into preferences and is then removed", async () => {
  const app = appearance({ legacy: "light" });
  await app.emit("DOMContentLoaded");
  assert.equal(app.saved.theme, "light");
  assert.equal(app.root.dataset.theme, "light");
  assert.equal(app.legacy(), null);
  await app.emit("focus");
  assert.equal(app.requests.filter(r => r.options?.method === "POST").length, 1);
});

test("saved preferences take precedence and failed migrations remain retryable", async () => {
  const saved = appearance({ saved: { theme: "dark" }, legacy: "light" });
  await saved.emit("DOMContentLoaded");
  assert.equal(saved.root.dataset.theme, "dark");
  assert.equal(saved.requests.length, 1);
  const failed = appearance({ legacy: "light", failWrite: true });
  await failed.emit("DOMContentLoaded");
  assert.equal(failed.legacy(), "light");
  assert.equal(failed.root.dataset.theme, "dark");
});

test("all motion choices override or follow the system and native broadcasts reach open pages", async () => {
  const app = appearance({ systemReduce: true });
  assert.equal(app.api.reducedMotion.matches, true);
  await app.emit("agentnave:preferences", { detail: { theme: "light", motion: "full" } });
  assert.equal(app.api.reducedMotion.matches, false);
  assert.equal(app.root.dataset.theme, "light");
  app.api.applyPreferences({ motion: "reduce" });
  assert.equal(app.api.reducedMotion.matches, true);
  const animated = appearance({ systemReduce: false });
  assert.equal(animated.api.reducedMotion.matches, false);
  animated.api.applyPreferences({ motion: "reduce" });
  assert.equal(animated.api.reducedMotion.matches, true);
});

test("the panel and header switches persist preferences and keep the choice on failure", async () => {
  const app = appearance();
  app.api.themeSwitch(app.h, (zh, en) => en);
  await app.buttons[1].listeners.click();
  assert.equal(app.saved.theme, "light");
  assert.deepEqual(JSON.parse(app.requests[0].options.body), { general: { theme: "light" } });
  const failed = appearance({ saved: { theme: "dark" }, failWrite: true });
  failed.api.themeSwitch(failed.h, (zh, en) => en);
  await failed.buttons[1].listeners.click();
  assert.equal(failed.root.dataset.theme, "dark");
});

const flush = () => new Promise(resolve => setImmediate(resolve));

test("settings follow a shortcut and let the previous theme be selected again", async () => {
  const app = appearance({ saved: { theme: "dark" } });
  const settings = app.settings();
  await flush();
  app.api.themeSwitch(app.h, (zh, en) => en);
  await app.buttons.find(button => button.attributes["data-theme-choice"] === "light").listeners.click();
  await flush();
  let choices = settings.children[0].children[1].children;
  assert.equal(choices[1].attributes["aria-checked"], "true");
  await choices[2].listeners.click();
  await flush();
  assert.equal(app.saved.theme, "dark");
  app.saved.theme = "light";
  await app.emit("focus");
  choices = settings.children[0].children[1].children;
  assert.equal(choices[1].attributes["aria-checked"], "true");
});

test("a stale GET cannot overwrite migration broadcast from another window", async () => {
  let release, began;
  const started = new Promise(resolve => { began = resolve; });
  const saved = {};
  const old = appearance({ transport: async () => {
    began();
    return new Promise(resolve => { release = () => resolve({ ok: true, json: async () => ({ preferences: { general: {} }, effective: { general: { theme: "system" } } }) }); });
  } });
  const pending = old.emit("DOMContentLoaded");
  await started;
  const migrating = appearance({ saved, legacy: "light" });
  await migrating.emit("DOMContentLoaded");
  await old.emit("agentnave:preferences", { detail: saved });
  release();
  await pending;
  assert.equal(old.root.dataset.theme, "light");
});

test("concurrent legacy candidates only fill an unset theme and converge on server choice", async () => {
  const saved = {};
  let gets = 0, release;
  const both = new Promise(resolve => { release = resolve; });
  const transport = async (path, options) => {
    if (options?.method === "POST") {
      if (!options.headers["X-AgentNave-Migrate-Theme"] || !saved.theme) Object.assign(saved, JSON.parse(options.body).general);
      return { ok: true, json: async () => ({ preferences: { general: { ...saved } } }) };
    }
    if (++gets === 2) release();
    await both;
    return { ok: true, json: async () => ({ preferences: { general: {} }, effective: { general: { theme: "system" } } }) };
  };
  const a = appearance({ legacy: "light", transport });
  const b = appearance({ legacy: "dark", transport });
  await Promise.all([a.emit("DOMContentLoaded"), b.emit("DOMContentLoaded")]);
  assert.equal(saved.theme, "light");
  assert.equal(a.root.dataset.theme, "light");
  assert.equal(b.root.dataset.theme, "light");
});

test("settings.html scripts load in declared order and language switch respects unsaved changes", async () => {
  const html = readFileSync(`${__dirname}/../settings.html`, "utf8");
  const scripts = [...html.matchAll(/<script src="([^"]+)" defer><\/script>/g)].map(m => m[1]);
  assert.ok(scripts.indexOf("./advanced.js") < scripts.indexOf("./settings.js"));

  const elements = new Map();
  function makeNode(tag = "div", id = "") {
    const node = {
      tagName: tag.toUpperCase(), id, className: "", textContent: "", value: "", hidden: false, disabled: false,
      dataset: {}, style: { setProperty() {} }, attributes: {}, listeners: {}, children: [], options: [],
      classList: { toggle() {}, add() {}, remove() {}, contains: () => false },
      setAttribute(k, v) { this.attributes[k] = String(v); },
      removeAttribute(k) { delete this.attributes[k]; },
      getAttribute(k) { return this.attributes[k] ?? null; },
      addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); },
      prepend(...kids) { this.children.unshift(...kids.flat().filter(Boolean)); },
      append(...kids) { this.children.push(...kids.flat().filter(Boolean)); if (this.tagName === "SELECT") this.options = this.children; },
      appendChild(child) { if (child) this.children.push(child); return child; },
      replaceChildren(...kids) { this.children = kids.flat().filter(Boolean); },
      querySelectorAll(sel) {
        if (sel === "button") return this.children.filter(c => c.tagName === "BUTTON");
        return [];
      },
      querySelector() { return null; },
      matches() { return false; },
      closest() { return null; },
      focus() {},
      get childElementCount() { return this.children.length; },
    };
    if (id) elements.set(id, node);
    return node;
  }
  const getEl = id => elements.get(id) || makeNode("div", id);
  ["preset-off", "preset-split", "preset-parallel", "preset-custom"].forEach(id => {
    const b = makeNode("button", id);
    b.dataset.preset = id.replace("preset-", "");
    getEl("orchestration-presets").children.push(b);
  });

  const posts = [];
  let reloaded = 0;
  const root = makeNode("html");
  root.lang = "zh-CN";
  root.dataset = { theme: "system", motion: "system", lang: "system" };
  const document = {
    documentElement: root, hidden: false, visibilityState: "visible", activeElement: null,
    currentScript: { src: "http://127.0.0.1/avatars.js" },
    getElementById: getEl,
    createElement: tag => makeNode(tag),
    createTextNode: text => ({ nodeType: 3, textContent: String(text) }),
    createDocumentFragment: () => makeNode("fragment"),
    querySelector: () => makeNode("header"),
    querySelectorAll: () => [],
    addEventListener() {},
  };
  const context = {
    window: null, document, navigator: { language: "zh-CN" },
    location: { protocol: "http:", hostname: "127.0.0.1", search: "", hash: "#general", href: "http://127.0.0.1/settings/#general", reload() { reloaded += 1; } },
    history: { replaceState(_s, _t, url) { context.location.hash = url; } },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    addEventListener() {}, CustomEvent: class { constructor(t, o) { this.type = t; this.detail = o?.detail; } },
    Event: class { constructor(t) { this.type = t; } },
    URL, URLSearchParams, CSS: { escape: s => s }, queueMicrotask,
    setTimeout: () => 1, clearTimeout() {}, setInterval: () => 1, clearInterval() {},
    structuredClone,
    async fetch(url, opts) {
      const path = String(url);
      if (opts?.method === "POST") posts.push({ path, body: opts.body ? JSON.parse(opts.body) : null });
      if (path.includes("avatar-settings") || path.includes("/app/characters")) return { ok: true, json: async () => ({}) };
      if (path.startsWith("/settings/members")) return { ok: true, json: async () => ({ document: { version: 1, clis: {}, modes: {} }, detected: {}, review: { seats: {} } }) };
      if (path === "/app/orchestration") return { ok: true, json: async () => ({ document: { version: 1, preset: "split" }, bosses: [], presets: { split: "rule" }, files: [] }) };
      if (path === "/app/preferences") return { ok: true, json: async () => ({ preferences: { general: { language: "system" } }, effective: { general: { language: "system" } } }) };
      if (path === "/app/settings") return { ok: true, json: async () => ({ settings: { max_running: 8 } }) };
      if (path === "/app/alerts") return { ok: true, json: async () => ({ alerts: {}, sound_choices: [] }) };
      if (path === "/app/hosts") return { ok: true, json: async () => ({ service: { installed: true, command: "agentnave-mcp" }, hosts: [], backups: {} }) };
      if (path === "/app/skills") return { ok: true, json: async () => ({ skills: [] }) };
      if (path === "/app/storage") return { ok: true, json: async () => ({ total_bytes: 0, workspaces: 0 }) };
      return { ok: true, json: async () => ({}) };
    },
  };
  context.window = context;
  context.window.dispatchEvent = () => {};

  const mapPath = src => src === "/avatars.js" ? `${__dirname}/../avatars/avatars.js`
    : src.startsWith("/chat/") ? `${__dirname}/../${src.slice(6)}`
    : `${__dirname}/../${src.slice(2)}`;
  for (const src of scripts) {
    vm.runInNewContext(readFileSync(mapPath(src), "utf8"), context, { filename: src });
  }

  // Trigger an unsaved edit in Advanced (custom text edited).
  context.setTab("advanced");
  await new Promise(r => setImmediate(r));
  const customText = getEl("custom-text");
  customText.value = "my custom rule";
  customText.listeners.input[0]();
  context.setTab("general");

  // Click "English" in #general-language while Advanced has unsaved edits.
  const langRow = getEl("general-language").children[0];
  const langButtons = langRow.children[1].children;
  const enBtn = langButtons.find(b => b.children.includes("English"));
  await enBtn.listeners.click[0]();

  // Language was not saved or reloaded yet; confirmation is shown on the Advanced pane.
  assert.equal(reloaded, 0);
  assert.equal(posts.filter(p => p.path === "/app/preferences").length, 0);
  const confirmBox = getEl("advanced-leave-confirm");
  assert.equal(confirmBox.hidden, false);

  // Clicking "继续编辑" cancels the switch and keeps the draft.
  const [stayBtn, discardBtn] = confirmBox.children[1].children;
  stayBtn.listeners.click[0]();
  assert.equal(confirmBox.hidden, true);
  assert.equal(reloaded, 0);
  assert.equal(posts.filter(p => p.path === "/app/preferences").length, 0);
  assert.equal(customText.value, "my custom rule");

  // Trigger language switch again and confirm "放弃修改并离开".
  await enBtn.listeners.click[0]();
  const [, confirmDiscardBtn] = confirmBox.children[1].children;
  confirmDiscardBtn.listeners.click[0]();
  await new Promise(r => setImmediate(r));
  assert.equal(posts.filter(p => p.path === "/app/preferences").length, 1);
  assert.deepEqual(posts.find(p => p.path === "/app/preferences").body, { general: { language: "en" } });
  assert.equal(reloaded, 1);
});
