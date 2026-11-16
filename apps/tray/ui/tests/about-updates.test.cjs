const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

test("automatic update choice defaults to four hours and keeps saved choices", async () => {
  for (const [updates, expected] of [[undefined, 4], [{ check_hours: 24 }, 24], [{ auto_check: false }, 0]]) {
    const buttons = [];
    const root = { replaceChildren() {} };
    function h(tag, attrs) {
      const node = { attrs, classList: { toggle() {} }, append() {}, addEventListener() {}, setAttribute(k, v) { this.attrs[k] = v; } };
      if (tag === "button.segment-btn") buttons.push(node);
      return node;
    }
    const window = {};
    const context = { window, document: { getElementById: id => id === "update-auto-check" ? root : null }, AN: { h, L: zh => zh, load: async () => ({ preferences: { updates } }) } };
    vm.runInNewContext(readFileSync(`${__dirname}/../about-settings.js`, "utf8"), context);
    assert.equal(await window.AgentNaveAboutSettings.interval(), expected);
    assert.deepEqual(buttons.map(b => b.attrs["aria-checked"]), [0, 4, 12, 24].map(n => String(n === expected)));
  }
});

test("header skips hidden window polling and refreshes when shown", async () => {
  let hidden = true, visibility, tick, reads = 0, button;
  const received = [];
  const end = { prepend(node) { button = node; }, matches: () => false };
  const document = { hidden: false, querySelector: () => end, querySelectorAll: () => [], addEventListener() {} };
  const window = { dispatchEvent(event) { received.push(event.detail); } };
  const AN = {
    h: () => ({ textContent: "", dataset: {}, setAttribute() {}, addEventListener() {} }),
    L: zh => zh,
    load: async () => { reads++; return { update: { state: "available", version: "2.0" } }; },
    isHidden: () => hidden,
    onVisibility: fn => { visibility = fn; },
  };
  const context = { AN, document, window, CustomEvent: class { constructor(type, { detail }) { this.detail = detail; } },
    addEventListener: (event, fn) => { if (event === "DOMContentLoaded") tick = fn; }, setTimeout() {} };
  vm.runInNewContext(readFileSync(`${__dirname}/../common-status.js`, "utf8"), context);
  await tick();
  assert.equal(reads, 0);
  hidden = false;
  await visibility();
  assert.equal(reads, 1);
  assert.equal(button.dataset.state, "available");
  assert.equal(received[0].update.version, "2.0");
});

test("a refresh answered after a newer status does not write its older answer back", async () => {
  let visibility, release, button;
  const received = [];
  const end = { prepend(node) { button = node; }, matches: () => false };
  const document = { hidden: false, querySelector: () => end, querySelectorAll: () => [], addEventListener() {} };
  const window = { dispatchEvent(event) { received.push(event.detail); } };
  const AN = {
    h: () => ({ textContent: "", dataset: {}, title: "", setAttribute() {}, addEventListener() {} }),
    L: zh => zh,
    load: () => new Promise(resolve => { release = resolve; }),
    isHidden: () => false,
    onVisibility: fn => { visibility = fn; },
  };
  let statusListener;
  const context = { AN, document, window, CustomEvent: class { constructor(type, { detail }) { this.detail = detail; } },
    addEventListener: (event, fn) => { if (event === "agentnave:status") statusListener = fn; }, setTimeout() {} };
  vm.runInNewContext(readFileSync(`${__dirname}/../common-status.js`, "utf8"), context);
  const pending = visibility();
  statusListener({ detail: { update: { state: "none" }, version: "2.0", updated: "2.0" } });
  assert.equal(button.dataset.state, "updated");
  release({ update: { state: "available", version: "3.0" }, version: "1.0" });
  await pending;
  assert.equal(button.dataset.state, "updated");
  assert.equal(received.length, 0);
});

test("opening the about page checks only once the chosen hours are stale", async () => {
  const minute = 60 * 1000;
  const cases = [[true, 4 * 60 * minute - minute, 0], [true, 4 * 60 * minute, 1], [false, 9 * minute, 0], [false, 10 * minute, 1]];
  for (const [answered, ago, expected] of cases) {
    let checks = 0;
    const nodes = new Map();
    const element = () => ({ textContent: "", hidden: false, disabled: false, className: "", dataset: {}, addEventListener() {}, classList: { toggle() {} }, setAttribute() {} });
    const status = { version: "2.0.0", update: { state: "none" }, service: null, last_checked: new Date(Date.now() - ago).toISOString(), check_answered: answered };
    const S = {
      $: id => { if (!nodes.has(id)) nodes.set(id, element()); return nodes.get(id); },
      L: zh => zh,
      apiGetStatus: async () => status,
      apiCheckUpdate: async () => { checks++; return { ...status, update: { state: "none" } }; },
      bindAdvanced() {},
      showNotice() {},
    };
    const window = { AgentNaveAboutSettings: { interval: async () => 4 }, dispatchEvent() {}, addEventListener() {} };
    vm.runInNewContext(readFileSync(`${__dirname}/../update-settings.js`, "utf8"), {
      window, AN: { settings: S }, document: { hidden: false, addEventListener() {} },
      CustomEvent: class { constructor(type, { detail }) { this.detail = detail; } },
      setInterval: () => 0, clearInterval() {}, setTimeout,
    });
    await S.loadStatus();
    await S.checkIfStale();
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.equal(checks, expected, `answered=${answered} ago=${ago}`);
  }
});

test("the update flow checks again on every click and keeps an offline service upgrade", async () => {
  async function click(status, answer = status) {
    const posts = [], titles = [];
    const node = () => ({ open: false, addEventListener() {}, replaceChildren() {}, showModal() { this.open = true; }, querySelector: () => null, append() {} });
    const AN = {
      h: (tag, attrs, ...children) => { if (tag === "h2.upd-title") titles.push(children[0]); return node(); },
      L: zh => zh,
      load: async () => status,
      post: async path => { posts.push(path); return answer; },
    };
    const window = { dispatchEvent() {} };
    const context = { AN, window, document: { body: node() }, CustomEvent: class { constructor(type, { detail }) { this.detail = detail; } },
      addEventListener() {}, setTimeout };
    vm.runInNewContext(readFileSync(`${__dirname}/../update-dialog.js`, "utf8"), context);
    await AN.updateFlow();
    return { posts, title: titles.at(-1) };
  }
  const found = { version: "1.0", update: { state: "available", version: "2.0", notes: { items: [] } }, service: null };
  assert.deepEqual(await click(found), { posts: ["/app/update/check?quiet=1"], title: "有新版本 v2.0" });
  // A quiet check that gets no answer leaves the status as it was; the service upgrade needs no network.
  const outdated = { version: "1.0", update: { state: "none" }, service: { state: "outdated", version: "0.9" } };
  assert.deepEqual(await click(outdated), { posts: ["/app/update/check?quiet=1"], title: "升级本机服务" });
  const current = { version: "1.0", update: { state: "none" }, service: null };
  assert.deepEqual(await click(current), { posts: ["/app/update/check"], title: "已是最新版本" });
});
