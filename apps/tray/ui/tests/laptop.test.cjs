const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

test("a hidden laptop does not rebuild SVG, and redraws when it regains size", () => {
  let mutations, resize, writes = 0, width = 0, counter = 0;
  const frames = new Map(), laptops = [];
  const node = tag => ({
    nodeType: 1, className: "", style: {}, setAttribute() {}, append() {},
    matches: () => false, querySelector: () => null,
    closest: () => ({ dataset: { mood: "idle" } }),
    get clientWidth() { return width; },
    set innerHTML(value) {
      assert.ok(value.includes("<path")); writes++;
      mutations([{ type: "childList", addedNodes: [node("path")], removedNodes: [] }]);
    },
  });
  const context = vm.createContext({
    window: {}, document: { documentElement: {}, createElementNS: (_ns, tag) => node(tag),
      createElement: tag => { const el = node(tag); laptops.push(el); return el; }, querySelectorAll: () => laptops },
    ResizeObserver: class { constructor(fn) { resize = fn; } observe() {} },
    MutationObserver: class { constructor(fn) { mutations = fn; } observe() {} },
    performance: { now: () => 0 },
    requestAnimationFrame: fn => { frames.set(++counter, fn); return counter; },
  });
  vm.runInContext(readFileSync(`${__dirname}/../laptop.js`, "utf8"), context);
  const el = context.window.Laptop.create();
  for (let n = 0; n < 12; n++) {
    el.laptop.sync();
    const pending = [...frames.values()]; frames.clear(); pending.forEach(fn => fn(500));
  }
  assert.equal(writes, 0);
  assert.equal(frames.size, 0);
  width = 120; resize();
  assert.equal(writes, 1);
  assert.equal(frames.size, 0);
  width = 0; resize();
  assert.equal(writes, 1);
});
