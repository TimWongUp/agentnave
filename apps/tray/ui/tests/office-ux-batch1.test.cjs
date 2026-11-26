const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const source = (name) => readFileSync(`${__dirname}/../${name}`, "utf8");
const tick = () => new Promise((resolve) => setImmediate(resolve));

// A small DOM: nodes keep their class names, children and listeners, and record who took the focus.
function dom() {
  const focused = [];
  const find = (node, match) => {
    for (const child of node.children || []) {
      if (child && typeof child === "object") {
        if (match(child)) return child;
        const deeper = find(child, match);
        if (deeper) return deeper;
      }
    }
    return null;
  };
  const text = (node) => (typeof node === "string" ? node : (node.children || []).map(text).join(""));
  const h = (tag, attrs = {}, ...children) => {
    const [name, ...cls] = tag.split(".");
    const node = {
      tag, name, cls, attrs, children: children.flat(Infinity).filter((x) => x != null && x !== false && x !== ""),
      listeners: {}, disabled: false, hidden: Boolean(attrs.hidden), style: {}, dataset: {}, value: attrs.value ?? "",
      addEventListener(event, fn) { (this.listeners[event] ||= []).push(fn); },
      replaceChildren(...items) { this.children = items.flat().filter(Boolean); },
      append(...items) { this.children.push(...items.flat().filter(Boolean)); },
      querySelector(selector) {
        const wanted = selector.startsWith(".") ? (n) => n.cls?.includes(selector.slice(1)) : (n) => n.name === selector;
        return find(this, wanted);
      },
      getBoundingClientRect: () => ({ width: 100, height: 100, left: 0, top: 0, bottom: 20 }),
      focus() { focused.push(this); },
      select() {},
      classList: { toggle() {} },
      setAttribute(key, value) { this.attrs[key] = value; },
      removeAttribute(key) { delete this.attrs[key]; },
      showModal() { this.open = true; },
      close() { this.open = false; },
    };
    Object.defineProperty(node, "textContent", { get: () => text(node), set: (value) => { node.children = [value]; } });
    return node;
  };
  return { h, focused };
}

function loadActions({ post, load, groupsData = [] } = {}) {
  const { h, focused } = dom();
  const body = {};
  const context = vm.createContext({
    AN: {
      h, $: () => null, L: (zh) => zh, at: Date.parse, urgent: (s) => s === "running" || s === "review",
      load: load || (async () => ({})), post: post || (async () => ({})), chatName: (g) => g.title || g.group,
    },
    document: { body: { append(...nodes) { for (const node of nodes) body[node.tag] = node; } }, addEventListener() {} },
    window: { addEventListener() {} },
    state: { groupsData, order: [] },
    refresh: async () => {}, renderList() {}, go() {}, openScope() {},
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(`${source("actions.js")}\nglobalThis.Actions = Actions;`, context);
  const { Actions } = context;
  const dialog = () => body["dialog.act"];
  const event = { clientX: 1, clientY: 1, preventDefault() {}, stopPropagation() {} };
  return {
    Actions, focused, dialog, event,
    row: () => dialog().children.at(-1),
    error: () => dialog().children.at(-2),
    // right-click an agent row, then choose the menu entry
    choose(label) {
      Actions.workspaceMenu("g", event);
      body["div.ctx"].children.find((item) => item.textContent === label).listeners.click[0]();
    },
  };
}

const cancel = { label: "取消" };

test("a dialog with a danger button focuses Cancel unless a button asks for the focus", async () => {
  const app = loadActions();
  const first = app.Actions.ask({ title: "删除？", buttons: [cancel, { label: "删除", danger: true, primary: true, value: "delete" }] });
  assert.equal(app.focused.at(-1), app.row().children[0], "Enter on the open dialog must not delete");
  app.row().children[0].listeners.click[0]();
  assert.equal(await first, null);

  const second = app.Actions.ask({ title: "删除？", buttons: [cancel, { label: "删除", danger: true, value: "delete", focus: true }] });
  assert.equal(app.focused.at(-1), app.row().children[1]);
  app.row().children[0].listeners.click[0]();
  await second;

  const plain = app.Actions.ask({ title: "交接", buttons: [cancel, { label: "交接", primary: true, value: "ok" }] });
  assert.equal(app.focused.at(-1), app.row().children[1], "without a danger button the primary one keeps the focus");
  app.row().children[0].listeners.click[0]();
  await plain;
});

test("renaming checks the width while typing and holds every button while it saves", async () => {
  let finish;
  const posts = [];
  const app = loadActions({
    groupsData: [{ group: "g", title: "旧名", title_id: "t1", conversations: 1 }],
    post: (path, body) => { posts.push([path, body]); return new Promise((resolve) => { finish = resolve; }); },
  });
  app.choose("改名…");
  const [, save] = app.row().children;
  const field = app.dialog().children.find((node) => node.name === "input");
  assert.equal(app.dialog().children.some((node) => node.textContent?.includes("最多 24 个汉字")), false, "no static rule sentence");

  field.value = "长".repeat(25);
  field.listeners.input[0]();
  assert.equal(app.error().hidden, false);
  assert.match(app.error().textContent, /最多 24 个汉字/);
  assert.equal(save.disabled, true);
  field.value = "长".repeat(24);
  field.listeners.input[0]();
  assert.equal(app.error().hidden, true);
  assert.equal(save.disabled, false, "24 wide characters still fit");

  field.value = "新名字";
  field.listeners.input[0]();
  const running = save.listeners.click[0]();
  await tick();
  assert.deepEqual(app.row().children.map((b) => b.disabled), [true, true], "Cancel and Save wait");
  assert.equal(save.textContent, "保存中…");
  save.listeners.click[0]();
  assert.equal(posts.length, 1, "a second click while saving sends nothing");

  finish({ status: "renamed" });
  await running;
  assert.equal(app.dialog().open, false);
  assert.equal(save.textContent, "保存", "the label comes back");
});

test("cleaning up keeps Next off while the chosen days match nothing, and says only what the title does not", async () => {
  const updated = new Date(Date.now() - 45 * 864e5).toISOString();
  const app = loadActions({
    groupsData: [{ group: "g1", title: "旧 agent" }],
    load: async () => ({ items: [{ id: "c1", group: "g1", title: "旧", deletable: true, updated }], workspaces: [] }),
  });
  const running = app.Actions.cleanup();
  await tick();
  const [note, days, count] = app.dialog().children.slice(1, 4);
  assert.equal(note.textContent, "进行中和待审核的会保留。");
  const next = () => app.row().children.at(-1);
  const pickDays = (label) => days.children.find((b) => b.textContent === label).listeners.click[0]();
  assert.match(count.textContent, /有 1 个 agent 超过 30 天/);
  assert.equal(next().disabled, false);
  pickDays("90 天");
  assert.match(count.textContent, /没有超过 90 天/);
  assert.equal(next().disabled, true);
  pickDays("7 天");
  assert.equal(next().disabled, false, "a longer match brings Next back");
  app.row().children[0].listeners.click[0]();
  await running;
});

test("deleting an agent whose conversations are running says why they stay", async () => {
  const app = loadActions({
    groupsData: [{ group: "g", title: "忙碌", title_id: "t1", conversations: 1, status: "running" }],
    load: async () => ({ items: [], workspaces: [] }),
  });
  app.choose("删除 agent…");
  await tick();
  await tick();
  assert.match(app.dialog().children[1].textContent, /运行|待审核/);
  assert.doesNotMatch(app.dialog().children[1].textContent, /没有可删除/);
  app.row().children[0].listeners.click[0]();
  await tick();
});

test("the usage line names the cache figure and always explains itself", async () => {
  const { h } = dom();
  let lang = "zh";
  const data = (workers) => ({ has_data: true, total: { cost_usd: 0.05, total_tokens: 286000, cache_hit_rate: 0.74 }, boss: { cost_usd: 0.05, total_tokens: 286000 }, workers });
  let next = data({ calls: 0 });
  const context = vm.createContext({
    window: {}, Intl,
    document: { documentElement: { get lang() { return lang; } }, createElementNS: () => h("svg") },
    AN: { h, L: (zh, en) => (lang === "zh" ? zh : en), load: async () => next },
  });
  vm.runInContext(source("chat-usage.js"), context);
  const root = context.window.ChatUsage.mount();
  context.window.ChatUsage.show("g1");
  await tick();
  assert.match(root.textContent, /命中 74%/);
  assert.equal(root.title, "花费 · token · 缓存命中率", "boss alone still gets the explanation");

  lang = "en";
  next = data({ calls: 2, cost_usd: 0.01, total_tokens: 1000 });
  context.window.ChatUsage.show("g2");
  await tick();
  assert.match(root.textContent, /74% hit/);
  assert.match(root.title, /cost · tokens · cache hit rate\nBoss /);
});

test("a manual isolated cleanup reports how many directories this run removed", async () => {
  const { h } = dom();
  const report = (trigger, outcomes) => ({ report: { at: "2026-11-26T00:00:00Z", trigger, entries: outcomes.map((outcome) => ({ outcome })) }, recent: [], running: false });
  const context = vm.createContext({
    AN: { h, L: (zh) => zh, relTime: () => "刚刚", load: async () => report("auto", []), post: async () => report("manual", ["removed", "collapsed", "kept"]) },
    window: { addEventListener() {} },
  });
  vm.runInContext(`${source("chat-isolated-cleanup.js")}\nglobalThis.Cleanup = ChatIsolatedCleanup;`, context);
  await tick();
  const last = () => context.Cleanup.panel("ws").querySelector(".iso-clean-last").textContent;
  assert.equal(last(), "上次清理 刚刚");
  context.Cleanup.panel("ws").querySelector("button").listeners.click[0]();
  await tick();
  assert.equal(last(), "上次清理 刚刚 · 本次清理 2 个");
});

test("between 761px and 1100px the sidebar button lays the full list over the 64px rail and back", () => {
  const { h } = dom();
  const chat = source("chat.js");
  const slice = chat.slice(chat.indexOf("// The workspace column folds away"), chat.indexOf("// The conversation button belongs"));
  const doc = {};
  const side = h("aside");
  const toggle = h("button");
  const app = { dataset: {}, querySelector: () => side, removeAttribute() {} };
  const state = { sideCollapsed: true, sidePeek: false };
  const rail = { matches: true };
  const peeks = [];
  const context = vm.createContext({
    AN: { L: (zh) => zh }, rail, narrow: { matches: false }, state, el: { app, sideToggle: toggle },
    setAttr: (node, key, value) => { node.attrs[key] = value; },
    setPeek: (name, on) => { assert.equal(name, "side"); state.sidePeek = on; peeks.push(on); },
    slideIn() {}, store() {}, SIDE_COLLAPSED_KEY: "k",
    document: { addEventListener: (name, fn) => { (doc[name] ||= []).push(fn); } },
  });
  toggle.contains = (node) => node === toggle;
  side.contains = (node) => node === side;
  vm.runInContext(slice, context);

  assert.equal(app.dataset.side, "on", "the rail stays even though the wide layout saved a fold");
  assert.equal(toggle.attrs["aria-expanded"], "false");
  toggle.listeners.click[0]();
  assert.deepEqual(peeks, [true]);
  assert.equal(toggle.attrs["aria-expanded"], "true");
  assert.equal(toggle.attrs.title, "隐藏项目列表");
  assert.equal(state.sideCollapsed, true, "the saved fold is untouched");
  doc.pointerdown[0]({ target: side });
  assert.deepEqual(peeks, [true], "a click inside the list keeps it");
  doc.pointerdown[0]({ target: {} });
  assert.deepEqual(peeks, [true, false]);
  assert.equal(toggle.attrs["aria-expanded"], "false");
  toggle.listeners.click[0]();
  doc.keydown[0]({ key: "Escape" });
  assert.deepEqual(peeks, [true, false, true, false]);
  toggle.listeners.click[0]();
  side.listeners.click[0]({ target: { closest: (selector) => (selector === ".gi" ? {} : null) } });
  assert.equal(state.sidePeek, false, "choosing an agent closes the list");

  rail.matches = false;
  toggle.listeners.click[0]();
  assert.equal(state.sideCollapsed, false, "wide windows still fold and unfold");
  assert.equal(app.dataset.side, "on");
});
