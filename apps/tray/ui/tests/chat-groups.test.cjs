const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const context = vm.createContext({ window: {}, Date });
vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
const { other, handover, caps, capSections } = context.window.ChatGroups;
// Results come from another realm; copy them so deepEqual compares plain values.
const plain = (value) => JSON.parse(JSON.stringify(value));
const arrange = (...args) => plain(context.window.ChatGroups.arrange(...args));
const cap = (...args) => plain(capSections(...args));
const env = { urgent: status => status === "running" || status === "review", L: zh => zh };
const must = g => env.urgent(g.status);

const ws = (group, project, updated, status = "idle", extra = {}) => ({
  group, status, updated, label: "Codex", boss_key: "codex|Codex",
  project_key: project, project_label: project === other ? null : project.split("/").pop(),
  project_path: project === other ? null : project, ...extra,
});

test("projects follow their latest activity, work going on leads inside a group, other comes last", () => {
  const sections = arrange([
    ws("a", other, "2026-11-12T12:00:00Z"),
    ws("b", "/w/old", "2026-11-10T00:00:00Z", "running"),
    ws("c", "/w/new", "2026-11-12T00:00:00Z"),
    ws("d", "/w/old", "2026-11-11T00:00:00Z"),
    ws("e", "/w/old", "2026-11-09T00:00:00Z", "review"),
  ], env);
  assert.deepEqual(sections.map(s => s.key), ["/w/new", "/w/old", other]);
  assert.deepEqual(sections[1].items.map(g => g.group), ["b", "e", "d"]);
  assert.equal(sections[2].title, "其他");
});

test("projects that share a name show where they are", () => {
  const sections = arrange([
    ws("a", "/w/app", "2026-11-12T00:00:00Z"),
    ws("b", "/x/app", "2026-11-11T00:00:00Z"),
    ws("c", "/w/lib", "2026-11-10T00:00:00Z"),
  ], env);
  assert.deepEqual(sections.map(s => s.hint), ["/w/app", "/x/app", ""]);
});

test("manual projects appear even when empty, share name disambiguation, and merge with active projects", () => {
  const projects = [
    { key: "/w/solo", label: "solo", path: "/w/solo", added_at: Date.parse("2026-11-11T12:00:00Z") },
    { key: "/w/app", label: "app", path: "/w/app", added_at: Date.parse("2026-11-01T00:00:00Z") },
    { key: "/y/app", label: "app", path: "/y/app", added_at: Date.parse("2026-11-10T00:00:00Z") },
    { key: other, label: "ignored", path: "" },
  ];
  const sections = arrange([
    ws("a", "/w/app", "2026-11-12T00:00:00Z"),
    ws("b", "/x/app", "2026-11-11T00:00:00Z"),
  ], { ...env, projects });
  assert.deepEqual(
    sections.map(s => [s.key, s.manual, s.items.map(g => g.group), s.hint]),
    [
      ["/w/app", true, ["a"], "/w/app"],
      ["/w/solo", true, [], ""],
      ["/x/app", false, ["b"], "/x/app"],
      ["/y/app", true, [], "/y/app"],
    ],
  );
});

test("a long list keeps its first groups and first entries, and counts each group whole", () => {
  const groups = [];
  for (let p = 0; p < 12; p++)
    for (let i = 0; i < 7; i++)
      groups.push(ws(`g${p}-${i}`, `/w/p${p}`, `2026-11-${String(20 - i).padStart(2, "0")}T00:00:00Z`));
  const sections = arrange(groups, env);
  const capped = cap(sections, { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.equal(capped.sections.length, caps.groups);
  assert.equal(capped.hiddenGroups, 2);
  assert.equal(capped.sections[0].items.length, 7); // the header keeps counting the whole group
  assert.deepEqual(capped.sections[0].shown.map(g => g.group), ["g0-0", "g0-1", "g0-2", "g0-3", "g0-4"]);
  assert.equal(capped.sections[0].hidden, 2);
  // One more ask opens five more entries, and a group that fits hides nothing.
  const wider = cap(sections, { groupLimit: caps.groups, itemLimit: caps.items + caps.itemStep, must });
  assert.equal(wider.sections[0].shown.length, 7);
  assert.equal(wider.sections[0].hidden, 0);
});

test("a group past the limit still shows what the cap may not hide", () => {
  const deep = (statuses) => statuses.map((status, i) => ws(`deep${i}`, "/w/deep", `2026-11-0${8 - i}T00:00:00Z`, status));
  // Exactly the cap in front of it, so the deep group is the only one past the limit.
  const ahead = Array.from({ length: caps.groups }, (_, p) => ws(`a${p}`, `/w/p${p}`, `2026-11-${String(20 - p).padStart(2, "0")}T00:00:00Z`));
  const idle = ["idle", "idle", "idle", "idle", "idle", "idle", "idle", "idle"];

  // Work going on in a group past the limit keeps the group, at its own place in the order.
  const running = cap(arrange([...ahead, ...deep([...idle.slice(1), "running"])], env),
    { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.deepEqual(running.sections.at(-1).key, "/w/deep");
  assert.deepEqual(running.sections.at(-1).shown.map(g => g.group), ["deep7", "deep0", "deep1", "deep2", "deep3"]);
  assert.equal(running.hiddenGroups, 0);

  // The agent already open comes along even when it sorts past the group's first entries.
  const opened = cap(arrange([...ahead, ...deep(idle)], env),
    { groupLimit: caps.groups, itemLimit: caps.items, must: g => g.group === "deep7" });
  assert.deepEqual(opened.sections.at(-1).shown.map(g => g.group), ["deep0", "deep1", "deep2", "deep3", "deep4", "deep7"]);
  assert.equal(opened.sections.at(-1).hidden, 2);

  // Nothing in it to keep: the group waits behind the "more" button.
  const hidden = cap(arrange([...ahead, ...deep(idle)], env), { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.equal(hidden.sections.length, caps.groups);
  assert.equal(hidden.hiddenGroups, 1);
});

test("pending handovers move into a pinned top section without counting against the group cap", () => {
  assert.equal(arrange([ws("a", "/w/app", "2026-11-12T00:00:00Z")], env).some(s => s.key === handover), false);

  const byProject = arrange([
    ws("a", "/w/app", "2026-11-10T00:00:00Z", "idle", { handover: "pending" }),
    ws("b", "/w/app", "2026-11-12T00:00:00Z"),
    ws("c", "/w/solo", "2026-11-13T00:00:00Z", "running", { handover: "pending" }),
  ], env);
  assert.deepEqual(byProject.map(s => [s.key, s.title, s.items.map(g => g.group)]), [
    [handover, "待接手", ["c", "a"]],
    ["/w/app", "app", ["b"]],
  ]);

  const ahead = Array.from({ length: caps.groups }, (_, p) => ws(`p${p}`, `/w/p${p}`, `2026-11-${String(20 - p).padStart(2, "0")}T00:00:00Z`));
  const pending = Array.from({ length: caps.items + 2 }, (_, i) => ws(`h${i}`, "/w/h", `2026-11-0${9 - i}T00:00:00Z`, "idle", { handover: "pending" }));
  const capped = cap(arrange([...ahead, ...pending], env), {
    groupLimit: caps.groups, itemLimit: caps.items, must: g => g.handover === "pending" || must(g),
  });
  assert.equal(capped.sections.length, caps.groups + 1);
  assert.equal(capped.hiddenGroups, 0);
  assert.equal(capped.sections[0].shown.length, pending.length);
});

test("search supports multi-term AND, case-insensitivity, aliases, paths, preview, group ID, pending terminals, and empty manual projects", () => {
  const { arrangeFlat } = context.window.ChatGroups;
  const projects = [{ key: "/w/solo", label: "solo", path: "/w/solo", added_at: 100 }];
  const sidebar = { project_aliases: { "/w/app": "核心服务" } };
  const groups = [
    ws("ws-alpha", "/w/app", "2026-11-12T10:00:00Z", "idle", { display_title: "Fix Login Bug", preview: "check oauth token", label: "Claude Code", host: "claude" }),
    ws("term-7", "/w/beta", "2026-11-12T11:00:00Z", "running", { display_title: "新 agent", preview: "beta", label: "Codex", host: "codex" }),
    ws("ws-other", other, "2026-11-12T09:00:00Z", "idle", { title: "Quick Chat", preview: "misc" }),
  ];

  // Multi-word AND across alias + preview + host
  assert.deepEqual(
    arrange(groups, { ...env, projects, sidebar, query: "核心 oauth CLAUDE" }).map(s => s.items.map(i => i.group)),
    [["ws-alpha"]],
  );
  // Matches group ID and synthetic term-* pending items
  assert.deepEqual(
    plain(arrangeFlat(groups, { ...env, projects, sidebar, query: "term-7 codex" })).map(g => g.group),
    ["term-7"],
  );
  // Matches empty manual project by name/path
  assert.deepEqual(
    arrange(groups, { ...env, projects, sidebar, query: "SOLO" }).map(s => [s.key, s.items.length]),
    [["/w/solo", 0]],
  );
  // No matches returns empty list
  assert.deepEqual(arrange(groups, { ...env, projects, sidebar, query: "nonexistent xyz" }), []);
});

test("sorting, filtering, pinning, aliases, flat view, and folded status dot priority", () => {
  const { arrangeFlat, foldedTone } = context.window.ChatGroups;
  const projects = [{ key: "/w/empty", label: "empty", path: "/w/empty", added_at: Date.parse("2026-11-15T00:00:00Z") }];
  const sidebar = {
    project_aliases: { "/w/z": "AlphaAlias", "/w/a": "AlphaAlias" },
    pinned_projects: ["/w/b"],
    pinned_groups: ["g2"],
    project_order: ["/w/a", "/w/z", "/w/empty"],
  };
  const groups = [
    ws("g1", "/w/a", "2026-11-12T01:00:00Z", "running", { created: "2026-11-10T00:00:00Z" }),
    ws("g2", "/w/a", "2026-11-10T00:00:00Z", "idle", { created: "2026-11-12T05:00:00Z" }),
    ws("g3", "/w/z", "2026-11-11T00:00:00Z", "failed", { created: "2026-11-11T00:00:00Z" }),
    ws("g4", "/w/b", "2026-11-09T00:00:00Z", "idle", { live: true, created: "2026-11-09T00:00:00Z" }),
    ws("g5", other, "2026-11-13T00:00:00Z", "idle", { handover: "pending", created: "2026-11-08T00:00:00Z" }),
  ];

  // Pinned project leads regular sections, other stays last, aliases disambiguate duplicate titles
  const manualSorted = arrange(groups, {
    ...env, projects, sidebar, prefs: { projectSort: "manual", itemSort: "created", urgentFirst: false, hideEmpty: true },
  });
  assert.deepEqual(manualSorted.map(s => [s.key, s.title, s.hint, Boolean(s.pinnedProject)]), [
    [handover, "待接手", "", false],
    ["/w/b", "b", "", true],
    ["/w/a", "AlphaAlias", "/w/a", false],
    ["/w/z", "AlphaAlias", "/w/z", false],
  ]);
  // Pinned agent g2 leads /w/a, and created sort orders properly
  assert.deepEqual(manualSorted[2].items.map(g => g.group), ["g2", "g1"]);

  // Name sort orders by title/alias A-Z
  const nameSorted = arrange(groups, {
    ...env, projects, sidebar: { project_aliases: { "/w/z": "Beta", "/w/a": "Alpha" } }, prefs: { projectSort: "name", hideEmpty: true },
  });
  assert.deepEqual(nameSorted.map(s => s.key), [handover, "/w/a", "/w/b", "/w/z"]);

  // Active filter keeps running, review, pending handover, and live items only
  const activeFlat = plain(arrangeFlat(groups, { ...env, projects, sidebar, prefs: { view: "flat", filter: "active" } }));
  assert.deepEqual(activeFlat.map(g => g.group), ["g5", "g1", "g4"]);

  // Flat view pins pending handover first, then pinned_groups, then urgentFirst + updated
  const allFlat = plain(arrangeFlat(groups, { ...env, projects, sidebar, prefs: { view: "flat" } }));
  assert.deepEqual(allFlat.map(g => g.group), ["g5", "g2", "g1", "g3", "g4"]);

  // Folded tone priority: pending/review > failed > running/live
  assert.equal(foldedTone([ws("1", "/p", "2026-11-10T00:00:00Z", "running"), ws("2", "/p", "2026-11-10T00:00:00Z", "failed"), ws("3", "/p", "2026-11-10T00:00:00Z", "review")]), "review");
  assert.equal(foldedTone([ws("1", "/p", "2026-11-10T00:00:00Z", "idle", { handover: "pending" }), ws("2", "/p", "2026-11-10T00:00:00Z", "failed")]), "review");
  assert.equal(foldedTone([ws("1", "/p", "2026-11-10T00:00:00Z", "running"), ws("2", "/p", "2026-11-10T00:00:00Z", "failed")]), "bad");
  assert.equal(foldedTone([ws("1", "/p", "2026-11-10T00:00:00Z", "idle", { live: true })]), "run");
  assert.equal(foldedTone([ws("1", "/p", "2026-11-10T00:00:00Z", "idle")]), "");
});

test("ChatSidebar manages search keyboard navigation, Cmd/Ctrl+F, '/', and view preferences persistence", () => {
  const storage = new Map();
  const docListeners = {};
  const winListeners = {};
  const went = [];
  let renders = 0;
  const makeEl = (tag = "div") => ({
    tag, hidden: false, value: "", textContent: "", attributes: {}, children: [], listeners: {},
    classList: { set: new Set(), toggle(n, on) { if (on) this.set.add(n); else this.set.delete(n); }, contains(n) { return this.set.has(n); }, add(n) { this.set.add(n); }, remove(n) { this.set.delete(n); } },
    setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
    getAttribute(k) { return this.attributes[k] ?? null; },
    hasAttribute(k) { return k in this.attributes; },
    removeAttribute(k) { delete this.attributes[k]; },
    addEventListener(n, fn) { (this.listeners[n] ||= []).push(fn); },
    replaceChildren(...items) { this.children = items.flat(); },
    append(...items) { this.children.push(...items.flat()); },
    querySelector: () => null,
    querySelectorAll: () => [],
    getBoundingClientRect: () => ({ left: 10, bottom: 30, width: 20, height: 20 }),
    focus() { this.focused = true; },
    blur() { this.focused = false; },
    select() { this.selected = true; },
    scrollIntoView() {},
    closest: () => null,
    contains: () => false,
  });
  const h = (tag, attrs = {}, ...kids) => Object.assign(makeEl(tag), {
    attributes: { ...attrs }, children: kids.flat(Infinity).filter(x => x != null && x !== false),
  });
  const els = new Map([
    ["side-view-btn", makeEl("button")],
    ["side-more-btn", makeEl("button")],
    ["side-search-input", makeEl("input")],
    ["side-search-clear", makeEl("button")],
  ]);
  const itemA = makeEl("a"), itemB = makeEl("a");
  const sbContext = vm.createContext({
    AN: {
      h, $: id => els.get(id) || null, L: zh => zh,
      setAttr: (n, k, v) => n.setAttribute(k, v),
      urgent: s => s === "running",
    },
    ChatGroups: context.window.ChatGroups,
    localStorage: {
      getItem: k => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, v),
      removeItem: k => storage.delete(k),
    },
    document: {
      readyState: "complete",
      activeElement: null,
      body: makeEl("body"),
      addEventListener(n, fn) { (docListeners[n] ||= []).push(fn); },
      querySelector: () => null,
    },
    window: { addEventListener(n, fn) { (winListeners[n] ||= []).push(fn); } },
    state: { order: ["g1", "g2"], items: new Map([["g1", { el: itemA }], ["g2", { el: itemB }]]), heads: new Map(), sideCollapsed: false },
    narrow: { matches: false },
    renderList: () => { renders++; },
    go: path => went.push(path),
    innerWidth: 1200, innerHeight: 800,
  });
  sbContext.window.ChatGroups = context.window.ChatGroups;
  vm.runInContext(readFileSync(`${__dirname}/../chat-sidebar.js`, "utf8"), sbContext);
  const { ChatSidebar } = sbContext.window;
  const input = els.get("side-search-input");
  const viewBtn = els.get("side-view-btn");

  assert.equal(viewBtn.attributes["data-active"], undefined);
  ChatSidebar.setPrefs({ view: "flat" });
  assert.equal(viewBtn.attributes["data-active"], "true");
  assert.equal(JSON.parse(storage.get("agentnave.office.sidebarView")).view, "flat");
  assert.equal(renders, 1);

  // '/' focuses search when not in input or terminal; ignored in input or iframe
  let prevented = false;
  docListeners.keydown.forEach(fn => fn({ key: "/", target: makeEl("div"), preventDefault() { prevented = true; } }));
  assert.equal(prevented, true);
  assert.equal(input.focused, true);

  input.focused = false; prevented = false;
  docListeners.keydown.forEach(fn => fn({ key: "/", target: { tagName: "INPUT", type: "text" }, preventDefault() { prevented = true; } }));
  assert.equal(prevented, false);

  sbContext.document.activeElement = { tagName: "IFRAME" };
  docListeners.keydown.forEach(fn => fn({ key: "f", metaKey: true, target: makeEl("div"), preventDefault() { prevented = true; } }));
  assert.equal(prevented, false, "Cmd+F ignores when terminal iframe is focused");

  sbContext.document.activeElement = null;
  docListeners.keydown.forEach(fn => fn({ key: "f", metaKey: true, target: makeEl("div"), preventDefault() { prevented = true; } }));
  assert.equal(prevented, true);
  assert.equal(input.focused, true);

  // Search input ArrowDown / ArrowUp / Enter / Escape
  input.value = "foo";
  input.listeners.input[0]();
  assert.equal(itemA.classList.contains("kbd-active"), true);
  input.listeners.keydown[0]({ key: "ArrowDown", preventDefault() {} });
  assert.equal(itemB.classList.contains("kbd-active"), true);
  input.listeners.keydown[0]({ key: "Enter", preventDefault() {} });
  assert.deepEqual(went, ["/chat/g2"]);
  input.listeners.keydown[0]({ key: "Escape", preventDefault() {}, stopPropagation() {} });
  assert.equal(ChatSidebar.query(), "");
  assert.equal(els.get("side-search-clear").hidden, true);
});

test("ChatSidebar new-chat flow picks project then CLI and launches terminal", async () => {
  const els = new Map();
  const bodyChildren = [];
  const docListeners = {};
  const makeEl = (tag) => {
    const classes = new Set();
    const attrs = {};
    const listeners = {};
    const node = {
      tag,
      hidden: false,
      style: {},
      children: [],
      listeners,
      classList: {
        add: (...c) => c.forEach(x => classes.add(x)),
        remove: (...c) => c.forEach(x => classes.delete(x)),
        toggle: (c, on) => (on === undefined ? (classes.has(c) ? classes.delete(c) : classes.add(c)) : (on ? classes.add(c) : classes.delete(c))),
        contains: (c) => classes.has(c),
      },
      setAttribute(k, v) { attrs[k] = String(v); },
      removeAttribute(k) { delete attrs[k]; },
      getAttribute(k) { return attrs[k] ?? null; },
      addEventListener(type, fn) { (listeners[type] ||= []).push(fn); },
      focus() {},
      contains(target) { return target === node || node.children.some(c => c?.contains?.(target)); },
      getBoundingClientRect: () => ({ left: 120, top: 12, bottom: 38, width: 200, height: 120 }),
      replaceChildren(...items) { node.children = items.filter(Boolean); },
      querySelectorAll(sel) {
        const out = [];
        const walk = (n) => {
          for (const c of n.children || []) {
            if (typeof c === "object" && c) {
              if (sel === "button" && String(c.tag || "").startsWith("button")) out.push(c);
              walk(c);
            }
          }
        };
        walk(node);
        return out;
      },
      querySelector(sel) { return node.querySelectorAll(sel)[0] || null; },
    };
    return node;
  };
  for (const id of ["side-view-btn", "side-more-btn", "new-chat", "side-search-input", "side-search-clear"]) {
    els.set(id, makeEl("button"));
  }
  const started = [];
  const sbContext = vm.createContext({
    window: {},
    innerWidth: 1200,
    innerHeight: 800,
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    AN: {
      $: id => els.get(id) || null,
      h: (tag, attrs = {}, ...kids) => {
        const n = makeEl(tag);
        for (const [k, v] of Object.entries(attrs || {})) {
          if (k === "hidden") n.hidden = Boolean(v);
          else n.setAttribute(k, v);
        }
        n.children = kids.filter(Boolean);
        return n;
      },
      L: zh => zh,
      setAttr: (el, k, v) => (v == null ? el.removeAttribute(k) : el.setAttribute(k, v)),
      urgent: env.urgent,
      hostTile: (p) => makeEl(`span.tile-${p}`),
      cliName: (p) => ({ codex: "Codex", claude: "Claude Code" }[p] || p),
    },
    ChatGroups: context.window.ChatGroups,
    ChatTerminal: {
      clis: async () => [
        { provider: "codex", accounts: [] },
        { provider: "claude", accounts: [{ id: "work", label: "工作" }] },
      ],
      start: async (provider, cwd, account) => { started.push({ provider, cwd, account }); },
    },
    state: {
      groupsData: [ws("g1", "/w/alpha", "2026-11-12T10:00:00Z")],
      projects: [{ key: "/w/beta", label: "beta", path: "/w/beta", added_at: Date.parse("2026-11-11T00:00:00Z") }],
      sidebar: { project_aliases: { "/w/alpha": "Alpha App" }, pinned_projects: ["/w/beta"], pinned_groups: [], project_order: [] },
      order: [],
      items: new Map(),
      heads: new Map(),
    },
    document: {
      readyState: "complete",
      body: { append: (...nodes) => bodyChildren.push(...nodes) },
      addEventListener: (t, fn) => { (docListeners[t] ||= []).push(fn); },
      querySelector: () => null,
      activeElement: null,
    },
    addEventListener() {},
    Date,
  });
  sbContext.window = sbContext;
  vm.runInContext(readFileSync(`${__dirname}/../chat-sidebar.js`, "utf8"), sbContext);

  const btn = els.get("new-chat");
  btn.listeners.click[0]({ stopPropagation() {} });
  const pop = bodyChildren.find(n => n.tag === "div.side-popover");
  assert.ok(pop && !pop.hidden);

  // Step 1: pinned project beta first, then aliased alpha, then choose other folder.
  const step1Buttons = pop.querySelectorAll("button");
  assert.equal(step1Buttons.length, 3);
  const textOf = (node) => (node.children || []).map(c => typeof c === "string" ? c : textOf(c)).join("");
  assert.match(textOf(step1Buttons[0]), /beta/);
  assert.match(textOf(step1Buttons[1]), /Alpha App/);
  assert.match(textOf(step1Buttons[2]), /选择其他文件夹/);

  // Click Alpha App -> Step 2 shows back button + Codex + Claude + Claude (工作)
  step1Buttons[1].listeners.click[0]();
  await new Promise(r => setImmediate(r));
  const step2Buttons = pop.querySelectorAll("button");
  assert.equal(step2Buttons.length, 4);
  assert.match(textOf(step2Buttons[0]), /Alpha App/);
  assert.match(textOf(step2Buttons[1]), /Codex/);
  assert.match(textOf(step2Buttons[3]), /工作/);

  // Back returns to Step 1
  step2Buttons[0].listeners.click[0]();
  assert.equal(pop.querySelectorAll("button").length, 3);

  // Pick Alpha App again and launch Claude (工作)
  pop.querySelectorAll("button")[1].listeners.click[0]();
  await new Promise(r => setImmediate(r));
  pop.querySelectorAll("button")[3].listeners.click[0]();
  assert.equal(pop.hidden, true);
  assert.deepEqual(started, [{ provider: "claude", cwd: "/w/alpha", account: "work" }]);
});

test("ChatSidebar preserves popover focus on setPrefs, hides fold actions in flat view, and handles drop target, edge scroll, and closeNewChat", () => {
  const storage = new Map();
  const bodyChildren = [];
  const menuCalls = [];
  const makeEl = (tag = "div") => {
    const node = {
      tag, hidden: false, value: "", textContent: "", attributes: {}, children: [], style: {}, listeners: {},
      parentElement: null, scrollTop: 0, scrollHeight: 800, clientHeight: 300,
      classList: { set: new Set(), toggle(n, on) { if (on) this.set.add(n); else this.set.delete(n); }, contains(n) { return this.set.has(n); }, add(n) { this.set.add(n); }, remove(n) { this.set.delete(n); } },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = String(v); },
      getAttribute(k) { return this.attributes[k] ?? null; },
      addEventListener(n, fn) { (this.listeners[n] ||= []).push(fn); },
      replaceChildren(...items) { this.children = items.flat().filter(Boolean); },
      append(...items) { this.children.push(...items.flat().filter(Boolean)); },
      querySelectorAll(sel) {
        const out = [];
        const walk = (cur) => {
          for (const c of cur.children || []) {
            if (typeof c !== "object" || !c) continue;
            if ((sel === "button" && String(c.tag || "").startsWith("button")) || (sel === "[data-drop]" && "data-drop" in (c.attributes || {}))) {
              out.push(c);
            }
            walk(c);
          }
        };
        walk(node);
        return out;
      },
      querySelector(sel) { return node.querySelectorAll(sel)[0] || null; },
      getBoundingClientRect: () => ({ left: 10, top: 10, bottom: 30, width: 200, height: 20 }),
      focus() { sbContext.document.activeElement = node; node.focused = true; },
      closest: () => null,
      contains(target) { return target === node || (node.children || []).some(c => c?.contains?.(target)); },
    };
    return node;
  };
  const h = (tag, attrs = {}, ...kids) => {
    const n = makeEl(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === "hidden") n.hidden = Boolean(v);
      else n.setAttribute(k, v);
    }
    n.children = kids.flat(Infinity).filter(x => x != null && x !== false);
    return n;
  };
  const wrap = makeEl("div.gl-wrap");
  wrap.getBoundingClientRect = () => ({ left: 0, top: 0, bottom: 300, width: 240, height: 300 });
  const list = makeEl("div#list");
  list.parentElement = wrap;
  list.closest = (sel) => (sel === ".gl-wrap" ? wrap : null);
  const els = new Map([
    ["side-view-btn", makeEl("button")],
    ["side-more-btn", makeEl("button")],
    ["new-chat", makeEl("button")],
    ["side-search-input", makeEl("input")],
    ["side-search-clear", makeEl("button")],
    ["list", list],
  ]);
  const sbContext = vm.createContext({
    AN: {
      h, $: id => els.get(id) || null, L: zh => zh,
      setAttr: (n, k, v) => n.setAttribute(k, v),
      urgent: s => s === "running",
    },
    ChatGroups: context.window.ChatGroups,
    Actions: {
      openMenu: (_e, items) => menuCalls.push(items.map(i => i.label)),
      reorderProjects: () => {},
    },
    localStorage: {
      getItem: k => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, v),
      removeItem: k => storage.delete(k),
    },
    document: {
      readyState: "complete",
      activeElement: null,
      body: { append: (...nodes) => bodyChildren.push(...nodes) },
      addEventListener() {},
      querySelector: () => null,
    },
    window: {},
    addEventListener() {},
    state: {
      groupsData: [
        ws("ga", "/w/a", "2026-11-12T12:00:00Z"),
        ws("gb", "/w/b", "2026-11-12T11:00:00Z"),
        ws("gc", "/w/c", "2026-11-12T10:00:00Z"),
        ws("go", other, "2026-11-12T09:00:00Z"),
      ],
      projects: [],
      sidebar: { project_aliases: {}, pinned_projects: [], pinned_groups: [], project_order: [] },
      order: [],
      items: new Map(),
      heads: new Map(),
      moreNodes: new Map(),
    },
    renderList() {},
    innerWidth: 1200,
    innerHeight: 800,
  });
  sbContext.window = sbContext;
  vm.runInContext(readFileSync(`${__dirname}/../chat-sidebar.js`, "utf8"), sbContext);
  const { ChatSidebar } = sbContext.window;
  assert.equal(typeof ChatSidebar.closeNewChat, "function");

  // M5: popover keeps focus on the active option after setPrefs re-renders it
  els.get("side-view-btn").listeners.click[0]({ stopPropagation() {} });
  const pop = bodyChildren.find(n => n.tag === "div.side-popover");
  const createdBtn = pop.querySelectorAll("button").find(b => b.getAttribute("data-pop-key") === "itemSort:created");
  createdBtn.focus();
  createdBtn.listeners.click[0]();
  const refocused = pop.querySelectorAll("button").find(b => b.getAttribute("data-pop-key") === "itemSort:created");
  assert.equal(refocused.focused, true);
  assert.equal(sbContext.document.activeElement, refocused);

  // L1: more menu hides collapse/expand in flat view
  els.get("side-more-btn").listeners.click[0]({ stopPropagation() {} });
  assert.deepEqual(plain(menuCalls.at(-1)), ["全部折叠", "全部展开", "清理…"]);
  ChatSidebar.setPrefs({ view: "flat" });
  els.get("side-more-btn").listeners.click[0]({ stopPropagation() {} });
  assert.deepEqual(plain(menuCalls.at(-1)), ["清理…"]);
  ChatSidebar.setPrefs({ view: "project" });

  // M3 & M4: drop indicator targets next draggable header or last visible element, clears on drop, and edge-scrolls
  const makeHead = (key, extra = {}) => ({
    el: makeEl("button.gl-grp"),
    section: { key, other: false, pinned: false, pinnedProject: false, ...extra },
  });
  const recA = makeHead("/w/a"), recB = makeHead("/w/b"), recC = makeHead("/w/c"), recOther = makeHead(other, { other: true });
  const itemA1 = makeEl("a.gi"), itemB1 = makeEl("a.gi"), itemC1 = makeEl("a.gi");
  for (const [k, rec] of [["project:/w/a", recA], ["project:/w/b", recB], ["project:/w/c", recC], ["project:other", recOther]]) {
    sbContext.state.heads.set(k, rec);
    ChatSidebar.attachDrag(rec);
  }
  list.children = [recA.el, itemA1, recB.el, itemB1, recC.el, itemC1, recOther.el];
  for (const node of list.children) node.parentElement = list;

  recA.el.listeners.dragstart[0]({ target: recA.el, dataTransfer: { setData() {} } });
  recB.el.listeners.dragover[0]({ clientY: 25, preventDefault() {}, dataTransfer: {} });
  assert.equal(recC.el.getAttribute("data-drop"), "before");
  assert.equal(recB.el.getAttribute("data-drop"), null);

  recC.el.listeners.dragover[0]({ clientY: 25, preventDefault() {}, dataTransfer: {} });
  assert.equal(itemC1.getAttribute("data-drop"), "after");
  assert.equal(recC.el.getAttribute("data-drop"), null);

  wrap.listeners.dragover[0]({ clientY: 290 });
  assert.ok(wrap.scrollTop > 0);
  const scrolled = wrap.scrollTop;
  wrap.listeners.dragover[0]({ clientY: 5 });
  assert.ok(wrap.scrollTop < scrolled);

  recC.el.listeners.drop[0]({ clientY: 25, preventDefault() {} });
  assert.equal(itemC1.getAttribute("data-drop"), null);
  assert.equal(recC.el.getAttribute("data-drop"), null);
  assert.equal(recA.el.classList.contains("dragging"), false);
});
