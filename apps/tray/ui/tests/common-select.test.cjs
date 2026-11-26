const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

// Just enough DOM for the list a <select> opens.
function element(tag, props = {}) {
  const classes = new Set();
  return Object.assign({
    tagName: tag.toUpperCase(), children: [], parent: null, attributes: {}, style: {}, listeners: {}, textContent: "",
    offsetHeight: 100, offsetWidth: 120,
    classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)), contains: (name) => classes.has(name) },
    setAttribute(key, value) { this.attributes[key] = String(value); },
    getAttribute(key) { return this.attributes[key] ?? null; },
    removeAttribute(key) { delete this.attributes[key]; },
    append(...kids) { for (const kid of kids) { kid.parent = this; this.children.push(kid); } },
    remove() { this.parent?.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; },
    contains(node) { return node === this || this.children.some((child) => child.contains(node)); },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    scrollIntoView() {},
    focus() {},
  }, props);
}
const option = (text, props) => element("option", { textContent: text, ...props });
const optgroup = (label, children, props) => element("optgroup", { label, children, ...props });

function page() {
  const listeners = {}, timers = new Map();
  let clock = 0, timerId = 0;
  const body = element("body");
  class HTMLSelectElement {}
  const document = { body, createElement: (tag) => element(tag), addEventListener: (name, fn) => { listeners[name] = fn; } };
  const context = vm.createContext({
    document, HTMLSelectElement, innerHeight: 800, innerWidth: 1000, addEventListener() {},
    Event: class { constructor(type) { this.type = type; } },
    setTimeout: (fn, delay) => { timers.set(++timerId, { fn, at: clock + delay }); return timerId; },
    clearTimeout: (id) => timers.delete(id),
  });
  vm.runInContext(readFileSync(`${__dirname}/../common-select.js`, "utf8"), context);

  function select(children, { selectedIndex = 0, dialog = null } = {}) {
    const node = element("select", {
      children, selectedIndex, dialog, disabled: false, multiple: false, size: 0, events: [],
      closest(selector) { return selector === "dialog[open]" ? this.dialog : null; },
      getBoundingClientRect: () => ({ left: 200, top: 300, bottom: 330, width: 180 }),
      dispatchEvent(event) { this.events.push(event.type); },
    });
    Object.defineProperty(node, "options", { get: () => node.children.flatMap((c) => (c.tagName === "OPTGROUP" ? c.children : [c])) });
    return Object.setPrototypeOf(node, HTMLSelectElement.prototype);
  }
  const open = (target) => listeners.mousedown({ target, button: 0, preventDefault() {} });
  function key(target, value, extra = {}) {
    let prevented = false;
    listeners.keydown({ target, key: value, preventDefault() { prevented = true; }, ...extra });
    return prevented;
  }
  const tick = (ms) => {
    clock += ms;
    for (const [id, timer] of timers) if (timer.at <= clock) { timers.delete(id); timer.fn(); }
  };
  const menuIn = (parent) => parent.children.find((child) => child.className === "select-menu");
  return { body, select, open, key, tick, menuIn };
}

const items = (menu) => menu.children.flatMap((child) => (child.attributes.role === "group" ? child.children.slice(1) : [child]));
const highlighted = (select, menu) => items(menu).find((item) => item.id === select.attributes["aria-activedescendant"])?.textContent;

test("a list opened inside a modal dialog is mounted in it and placed from the box", () => {
  const app = page();
  const dialog = element("dialog");
  const inside = app.select([option("one"), option("two")], { dialog });
  app.open(inside);
  const menu = app.menuIn(dialog);
  assert.ok(menu, "the dialog holds the list");
  assert.equal(app.menuIn(app.body), undefined);
  assert.deepEqual([menu.style.left, menu.style.top, menu.style.minWidth], ["200px", "334px", "180px"]);

  const outside = app.select([option("one")]);
  app.open(outside);
  assert.ok(app.menuIn(app.body));
  assert.equal(app.menuIn(dialog), undefined, "opening another box closes the first list");
});

test("each optgroup keeps its heading and the keys move over options only", () => {
  const app = page();
  const select = app.select([
    option("Default"),
    optgroup("Recommended", [option("alpha"), option("beta", { disabled: true })]),
    optgroup("Off", [option("gamma")], { disabled: true }),
    optgroup("Available", [option("delta")]),
  ]);
  app.open(select);
  const menu = app.menuIn(app.body);
  const groups = menu.children.filter((child) => child.attributes.role === "group");
  assert.deepEqual(groups.map((group) => [group.children[0].className, group.children[0].textContent, group.attributes["aria-label"]]),
    [["select-group-label", "Recommended", "Recommended"], ["select-group-label", "Off", "Off"], ["select-group-label", "Available", "Available"]]);
  assert.equal(groups[0].children[0].attributes.role, undefined, "a heading is not an option");
  assert.deepEqual(items(menu).map((item) => item.textContent), ["Default", "alpha", "beta", "gamma", "delta"]);

  const walk = (...keys) => keys.map((value) => { app.key(select, value); return highlighted(select, menu); });
  assert.deepEqual(walk("ArrowDown", "ArrowDown", "ArrowDown", "ArrowUp"), ["alpha", "delta", "delta", "alpha"]);
  assert.deepEqual(walk("End", "Home"), ["delta", "Default"]);
  app.key(select, "End");
  app.key(select, "Enter");
  assert.equal(select.selectedIndex, 4);
  assert.deepEqual(select.events, ["input", "change"]);
});

test("the open list is tied to its box with stable ids that are removed on close", () => {
  const app = page();
  const first = app.select([option("one"), option("two")], { selectedIndex: 1 });
  const second = app.select([option("one")]);
  app.open(first);
  const menu = app.menuIn(app.body), id = menu.id;
  assert.equal(menu.attributes.role, "listbox");
  assert.ok(id);
  assert.equal(first.attributes["aria-controls"], id);
  assert.equal(first.attributes["aria-activedescendant"], items(menu)[1].id);
  assert.equal(new Set(items(menu).map((item) => item.id)).size, 2);
  items(menu)[0].listeners.mousemove();
  assert.equal(first.attributes["aria-activedescendant"], items(menu)[0].id);

  for (const close of [() => app.key(first, "Escape"), () => app.key(first, "Tab"), () => items(app.menuIn(app.body))[0].listeners.click()]) {
    if (!app.menuIn(app.body)) app.open(first);
    close();
    for (const name of ["aria-controls", "aria-activedescendant", "aria-expanded"]) assert.equal(first.attributes[name], undefined, name);
  }
  app.open(first);
  assert.equal(app.menuIn(app.body).id, id, "the same box keeps its list id");
  app.open(second);
  assert.notEqual(app.menuIn(app.body).id, id);
});

test("typed letters move the highlight without choosing", () => {
  const app = page();
  const select = app.select(["Default", "gpt mini", "gpt max", "grok", "Gemini", "haiku"].map((text) =>
    option(text, { disabled: text === "grok" })));
  app.open(select);
  const menu = app.menuIn(app.body);
  const type = (text) => { for (const letter of text) assert.equal(app.key(select, letter), true, "the box never sees the letter"); return highlighted(select, menu); };

  assert.equal(type("g"), "gpt mini");
  assert.equal(type("g"), "gpt max");
  assert.equal(type("g"), "Gemini", "the disabled option is passed over, case ignored");
  assert.equal(type("g"), "gpt mini", "and it wraps");
  app.tick(600);
  assert.equal(type("gpt ma"), "gpt max", "a space inside the prefix belongs to it");
  app.tick(600);
  assert.equal(type("h"), "haiku", "a pause starts a new prefix");
  assert.equal(type("zz"), "haiku", "nothing matching leaves the highlight");
  assert.equal(select.selectedIndex, 0);
  assert.deepEqual(select.events, []);

  app.tick(600);
  app.key(select, " ");
  assert.equal(select.selectedIndex, 5, "a space on its own chooses");
  assert.equal(app.menuIn(app.body), undefined);

  app.open(select);
  assert.equal(app.key(select, "c", { metaKey: true }), true);
  assert.equal(highlighted(select, app.menuIn(app.body)), "haiku", "a key combination is not typing");
});
