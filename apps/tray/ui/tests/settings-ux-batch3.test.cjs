const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function makeNode(tag = "div", cls = "") {
  return {
    tagName: tag.toUpperCase(),
    className: cls,
    children: [],
    attrs: {},
    listeners: {},
    dataset: {},
    textContent: "",
    value: "",
    checked: false,
    disabled: false,
    hidden: false,
    open: false,
    focused: false,
    classList: {
      toggle(name, on) {
        const list = new Set(this._owner.className.split(/\s+/).filter(Boolean));
        if (on) list.add(name);
        else list.delete(name);
        this._owner.className = [...list].join(" ");
      },
      _owner: null,
    },
    setAttribute(k, v) {
      this.attrs[k] = String(v);
      if (k === "disabled") this.disabled = true;
      if (k === "hidden") this.hidden = true;
    },
    removeAttribute(k) {
      delete this.attrs[k];
    },
    addEventListener(type, fn) {
      (this.listeners[type] ||= []).push(fn);
    },
    append(...kids) {
      for (const k of kids.flat(Infinity)) {
        if (k == null || k === false) continue;
        if (typeof k === "string") this.textContent += k;
        else this.children.push(k);
      }
    },
    replaceChildren(...kids) {
      this.children = [];
      this.textContent = "";
      this.append(...kids);
    },
    focus() { this.focused = true; },
    showModal() { this.open = true; },
    close() { this.open = false; },
    findAll(pred, out = []) {
      if (pred(this)) out.push(this);
      for (const c of this.children) if (c && typeof c === "object" && c.findAll) c.findAll(pred, out);
      return out;
    },
    allText() {
      return this.textContent + this.children.map((c) => (c && typeof c === "object" && c.allText ? c.allText() : String(c || ""))).join(" ");
    },
  };
}

function h(spec, props = {}, ...kids) {
  const [tag, ...cls] = spec.split(".");
  const el = makeNode(tag || "div", cls.join(" "));
  el.classList._owner = el;
  for (const [k, v] of Object.entries(props || {})) {
    if (v == null || v === false) continue;
    if (k === "checked") el.checked = Boolean(v);
    else if (k === "disabled") el.disabled = Boolean(v);
    else if (k === "hidden") el.hidden = Boolean(v);
    else el.setAttribute(k, v === true ? "" : v);
  }
  el.append(...kids);
  return el;
}

test("#07 alerts-settings hides test button and subordinate rows when notify is off", async () => {
  const root = h("div"), notice = h("div");
  let alerts = { notify: "off", scope: "all", threshold: 10, problems: false };
  const context = vm.createContext({
    AN: {
      h,
      $: (id) => (id === "alerts-settings" ? root : id === "notice" ? notice : null),
      L: (zh) => zh,
      load: async () => ({ preferences: { alerts: { ...alerts } }, effective: { alerts: { ...alerts } } }),
      post: async (_path, body) => {
        alerts = { ...alerts, ...body.alerts };
        return { preferences: { alerts: { ...alerts } }, effective: { alerts: { ...alerts } } };
      },
      settings: { settingTip: (t) => h("span.setting-tip", { "aria-label": t }, "?") },
    },
    document: { hidden: false, addEventListener() {} },
    addEventListener() {},
    window: {},
  });
  vm.runInContext(readFileSync(`${__dirname}/../alerts-settings.js`, "utf8"), context);
  await context.window.AgentNaveAlertsSettings.load();
  assert.equal(root.children.length, 1, "only notify method row shown when off");
  assert.equal(root.findAll((n) => n.tagName === "BUTTON" && n.textContent === "发一条测试通知").length, 0);

  // Switch to system notification -> test button and 3 subordinate rows appear.
  const sysBtn = root.findAll((n) => n.tagName === "BUTTON" && n.textContent === "系统通知")[0];
  sysBtn.listeners.click[0]();
  await new Promise(setImmediate);
  assert.equal(root.children.length, 4);
  assert.equal(root.findAll((n) => n.tagName === "BUTTON" && n.textContent === "发一条测试通知").length, 1);
});

test("#18-A & #08 avatar-settings removes 小人 wording, shows video mode note, shows pack delete size, and confirms custom CSS restore", async () => {
  const avatarSrc = readFileSync(`${__dirname}/../avatar-settings.js`, "utf8");
  const generalSrc = readFileSync(`${__dirname}/../general-settings.js`, "utf8");
  const settingsHtml = readFileSync(`${__dirname}/../settings.html`, "utf8");
  assert.doesNotMatch(avatarSrc, /小人/);
  assert.doesNotMatch(generalSrc, /小人/);
  assert.doesNotMatch(settingsHtml, /小人/);

  const root = h("div"), notice = h("p");
  const posts = [];
  const packs = {
    "272": { installed: false, bytes: 0, download: null },
    "544": { installed: true, stamp: "s544", bytes: 42 * 1048576, download: null },
  };
  let replaced = { claude: true };
  const context = vm.createContext({
    AN: {
      h,
      L: (zh) => zh,
      post: async (path, body) => {
        posts.push({ path, body });
        return { packs };
      },
      art: {
        STATES: ["idle", "blink", "working", "speaking", "review", "done", "sleep"],
        EXTRA: ["sad", "walk-1", "walk-2"],
        figure: () => h("span"),
        character: () => "claude",
        videoPacksChanged: async () => {},
        charactersChanged: async () => {},
      },
    },
    Avatars: { families: { claude: ["Claude"] }, create: () => h("span") },
    Blob: class {},
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    document: {
      documentElement: { dataset: { characterStyle: "static" } },
      getElementById: (id) => (id === "avatar-settings" ? root : id === "notice" ? notice : null),
    },
    window: { addEventListener() {}, dispatchEvent() {} },
    fetch: async (url, opts = {}) => {
      if (opts.method === "POST") {
        posts.push({ path: url, method: "POST" });
        if (url === "/app/characters/claude") replaced = { claude: false };
        return { ok: true, json: async () => replaced };
      }
      if (url === "/app/video-packs") return { ok: true, json: async () => ({ packs }) };
      if (url === "/app/preferences") return { ok: true, json: async () => ({ effective: { general: { character_style: "video-544" } } }) };
      if (url === "/app/characters") return { ok: true, json: async () => replaced };
      return { ok: true, json: async () => ({}) };
    },
    clearTimeout() {},
    setTimeout: () => 1,
  });
  vm.runInContext(avatarSrc, context);
  await new Promise(setImmediate);

  // Video mode shows inline note under custom CSS heading and pack size on Delete button.
  const modeNote = root.findAll((n) => n.className.includes("char-mode-note"))[0];
  assert.ok(modeNote);
  assert.equal(modeNote.hidden, false);
  assert.match(modeNote.textContent, /视频模式.*CSS 形象/);
  const delBtn = root.findAll((n) => n.tagName === "BUTTON" && n.textContent.startsWith("删除"))[0];
  assert.equal(delBtn.textContent, "删除（42.0 MB）");

  // Switching to CSS mode hides the video mode note.
  const cssRadio = root.findAll((n) => n.attrs.role === "radio" && n.textContent === "CSS 形象")[0];
  await cssRadio.listeners.click[0]();
  await new Promise(setImmediate);
  assert.equal(modeNote.hidden, true);
  assert.equal(notice.textContent, "已切换为 CSS 形象");

  // Restoring default custom CSS character requires inline confirmation with Keep focused.
  const restoreBtn = root.findAll((n) => n.tagName === "BUTTON" && n.textContent === "恢复默认")[0];
  restoreBtn.listeners.click[0]();
  const confirmBox = root.findAll((n) => n.className.includes("disable-confirm") && !n.hidden)[0];
  assert.ok(confirmBox);
  const [keepBtn, confirmRestoreBtn] = confirmBox.children[1].children;
  assert.equal(keepBtn.focused, true);
  keepBtn.listeners.click[0]();
  assert.equal(posts.some((p) => p.path === "/app/characters/claude"), false);

  restoreBtn.listeners.click[0]();
  const confirmBox2 = root.findAll((n) => n.className.includes("disable-confirm") && !n.hidden)[0];
  await confirmBox2.children[1].children[1].listeners.click[0]();
  await new Promise(setImmediate);
  assert.equal(posts.some((p) => p.path === "/app/characters/claude"), true);
});

test("C5 hosts.js sets descriptive title and aria-label on #hosts-badge", async () => {
  const nodes = new Map();
  const getEl = (id) => nodes.get(id) || nodes.set(id, h("div")).get(id);
  const hostsSrc = readFileSync(`${__dirname}/../hosts.js`, "utf8");
  let hostList = [
    { id: "a", boss: "a", label: "A", status: "needs_attention", reload: { state: "required" }, fault: false, skill: { state: "app" }, root: { path: "/tmp/a", source: "default" } },
    { id: "b", boss: "b", label: "B", status: "connected", reload: { state: "ok" }, fault: true, skill: { state: "app" }, root: { path: "/tmp/b", source: "default" } },
  ];
  const hostsPayload = () => ({ service: { installed: true }, backups: { config: 0, skill: 0, dir: "" }, hosts: hostList });
  const context = vm.createContext({
    AN: {
      h,
      $: getEl,
      L: (zh) => zh,
      load: async (path) => {
        if (path === "/app/hosts") return hostsPayload();
        if (path === "/app/status") return { version: "1.0.0", service: { state: "done" } };
        return { document: { version: 1 }, bosses: [] };
      },
      post: async () => hostsPayload(),
      settings: {},
      hostTile: () => h("span"),
    },
    document: { getElementById: getEl, hidden: false, addEventListener() {}, querySelector: () => null },
    window: { addEventListener() {}, dispatchEvent() {} },
    localStorage: { getItem: () => null, setItem() {} },
    CustomEvent: class { constructor(t, o) { this.type = t; this.detail = o?.detail; } },
    addEventListener() {},
    setTimeout: () => 1,
    clearTimeout() {},
    setInterval: () => 1,
    clearInterval() {},
  });
  vm.runInContext(readFileSync(`${__dirname}/../host-copies.js`, "utf8"), context);
  vm.runInContext(hostsSrc, context);
  await new Promise(setImmediate);
  await new Promise(setImmediate);

  const badge = getEl("hosts-badge");
  assert.equal(badge.hidden, false);
  assert.match(badge.attrs.title, /需要处理/);
  assert.match(badge.attrs.title, /待生效/);
  assert.match(badge.attrs.title, /连接中断/);
  assert.equal(badge.attrs["aria-label"], badge.attrs.title);

  hostList = [{ id: "a", boss: "a", label: "A", status: "connected", reload: { state: "ok" }, fault: false, skill: { state: "app" }, root: { path: "/tmp/a", source: "default" } }];
  getEl("refresh-hosts").listeners.click[0]();
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.equal(badge.hidden, true);
  assert.equal(badge.attrs.title, undefined);
  assert.equal(badge.attrs["aria-label"], undefined);
});

test("#14 settings-layout.css avoids darkening active segment buttons on hover", () => {
  const css = readFileSync(`${__dirname}/../settings-layout.css`, "utf8");
  assert.match(css, /\.segment-btn:not\(\.active\):hover:enabled/);
  assert.doesNotMatch(css, /\.segment-btn:hover:enabled/);
});
