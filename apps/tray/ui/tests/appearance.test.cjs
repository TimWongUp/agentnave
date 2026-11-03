const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function appearance({ saved = {}, legacy = null, systemReduce = false, failWrite = false } = {}) {
  const listeners = {}, requests = [], buttons = [];
  const root = { dataset: { theme: saved.theme || "system", motion: saved.motion || "system" } };
  const document = { documentElement: root, querySelectorAll: () => buttons, addEventListener() {} };
  const context = {
    document, window: {}, location: { protocol: "agentnave:" }, queueMicrotask,
    matchMedia: query => ({ matches: query.includes("reduced-motion") ? systemReduce : false, addEventListener() {} }),
    addEventListener: (name, fn) => { listeners[name] = fn; },
    localStorage: { getItem: () => legacy, removeItem: () => { legacy = null; } },
    async fetch(path, options) {
      requests.push({ path, options });
      if (options?.method === "POST") {
        if (failWrite) return { ok: false, status: 500 };
        Object.assign(saved, JSON.parse(options.body).general);
      }
      return { ok: true, json: async () => ({ preferences: { general: { ...saved } }, error: null }) };
    },
  };
  vm.runInNewContext(readFileSync(`${__dirname}/../appearance.js`, "utf8"), context);
  function h(tag, attributes, ...children) {
    const node = { tag, children, attributes, dataset: { themeChoice: attributes["data-theme-choice"] },
      listeners: {}, setAttribute(key, value) { this.attributes[key] = value; },
      addEventListener(name, fn) { this.listeners[name] = fn; } };
    if (tag === "button") buttons.push(node);
    return node;
  }
  return { api: context.window.ANAppearance, root, listeners, requests, saved, buttons, h, legacy: () => legacy };
}

test("legacy theme migrates once into preferences and is then removed", async () => {
  const app = appearance({ legacy: "light" });
  await app.listeners.DOMContentLoaded();
  assert.equal(app.saved.theme, "light");
  assert.equal(app.root.dataset.theme, "light");
  assert.equal(app.legacy(), null);
  await app.listeners.focus();
  assert.equal(app.requests.filter(r => r.options?.method === "POST").length, 1);
});

test("saved preferences take precedence and failed migrations remain retryable", async () => {
  const saved = appearance({ saved: { theme: "dark" }, legacy: "light" });
  await saved.listeners.DOMContentLoaded();
  assert.equal(saved.root.dataset.theme, "dark");
  assert.equal(saved.requests.length, 1);
  const failed = appearance({ legacy: "light", failWrite: true });
  await failed.listeners.DOMContentLoaded();
  assert.equal(failed.legacy(), "light");
  assert.equal(failed.root.dataset.theme, "dark");
});

test("all motion choices override or follow the system and native broadcasts reach open pages", () => {
  const app = appearance({ systemReduce: true });
  assert.equal(app.api.reducedMotion.matches, true);
  app.listeners["agentnave:preferences"]({ detail: { theme: "light", motion: "full" } });
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
