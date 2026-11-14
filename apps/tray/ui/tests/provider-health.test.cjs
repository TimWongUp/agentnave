const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

test("warnings group models and disappear after recovery", async () => {
  const until = new Date(Date.now() + 3600000).toISOString();
  let providers = { antigravity: { models: { a: { code: "quota", until }, b: { code: "quota", until } } }, claude: { default: { code: "auth", until: null } } };
  const list = {};
  const close = { dataset: {}, addEventListener(type, handler) { this.click = handler; } };
  const box = { hidden: true, dataset: {}, querySelector: (selector) => (selector === ".provider-health-lines" ? list : close) };
  const stored = new Map();
  const localStorage = { getItem: (key) => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value) };
  const context = vm.createContext({ AN: {}, localStorage, fetch: async () => ({ ok: true, json: async () => ({ providers }) }), document: { createElement: () => ({}) } });
  vm.runInContext(readFileSync(`${__dirname}/../provider-health.js`, "utf8"), context);
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, false);
  assert.match(list.textContent, /^⚠ Antigravity CLI 额度用完，2 个模型暂停派活.*；另有 1 项$/);
  assert.match(list.title, /重新登录后点“立即恢复”/);
  // Closed, it stays closed until the warning changes.
  close.click();
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, true);
  delete providers.claude;
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, false);
  providers = {};
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, true);
});
