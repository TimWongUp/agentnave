const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

test("warnings group models and disappear after recovery", async () => {
  const until = new Date(Date.now() + 3600000).toISOString();
  let providers = { antigravity: { models: { a: { code: "quota", until }, b: { code: "quota", until } } }, claude: { default: { code: "auth", until: null } } };
  const box = { hidden: true, replaceChildren(...children) { this.children = children; } };
  const context = vm.createContext({ AN: {}, fetch: async () => ({ ok: true, json: async () => ({ providers }) }), document: { createElement: () => ({}) } });
  vm.runInContext(readFileSync(`${__dirname}/../provider-health.js`, "utf8"), context);
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, false);
  assert.equal(box.children.length, 2);
  assert.match(box.children[0].textContent, /Antigravity CLI 额度用完，2 个模型暂停派活/);
  assert.match(box.children[1].textContent, /重新登录后点“立即恢复”/);
  providers = {};
  await context.AN.providerHealth.refresh(box);
  assert.equal(box.hidden, true);
});
