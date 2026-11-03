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
