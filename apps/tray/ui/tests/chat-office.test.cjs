const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { common, identities } = require("./helpers/common.cjs");

test("typing status measures the current activity and formats minutes", () => {
  const source = readFileSync(`${__dirname}/../chat-messages.js`, "utf8");
  const start = source.indexOf("const STALE_TAIL_MS");
  const end = source.indexOf("const SOURCE_STAMP", start);
  const context = vm.createContext({ L: zh => zh, relTime: () => "", Date });
  vm.runInContext(source.slice(start, end), context);
  const now = Date.now();
  assert.equal(context.typingStatus({ doing: "thinking", doing_since: new Date(now - 23000).toISOString() }), "思考中 · 23s");
  assert.equal(context.typingStatus({ doing: "tool:read_file", doing_since: new Date(now - 65000).toISOString() }), "read_file · 1m05s");
  assert.equal(context.typingStatus({ doing: "thinking", doing_since: "bad" }), "思考中");
  assert.equal(context.typingStatus({ doing: "tool:" }), "使用工具中");
});

function office(character = (_host, model) => model || "default", terminal = undefined) {
  const node = () => ({
    dataset: {}, classList: { toggle() {} }, children: [],
    replaceChildren(...children) { this.children = children; },
    insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
    remove() {},
  });
  const context = vm.createContext({
    AN: { at: Date.parse }, window: { addEventListener() {} }, ResizeObserver: class { observe() {} },
    ART: { character, figure: node,
      head: name => ({ name }), lid: host => ({ host }), url: path => path },
    el: { boss: node() }, state: { groupsData: [], drawer: {} },
    L: zh => zh, cliName: host => host,
    setText: (element, text) => { element.textContent = text; },
    setAttr: (element, key, value) => { element[key] = value; },
    ChatTerminal: terminal,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  const rec = Object.fromEntries(["lid", "scene", "desk", "chair", "open", "closed", "portrait", "cli", "st", "state"].map(key => [key, node()]));
  rec.laptop = { laptop: { dress(set) { rec.laptop.set = set; } } };
  return { rec, boss: context.el.boss, update: (c, n = { running: 0, review: 0 }) => context.updateBoss(rec, c, n) };
}

test("a boss whose terminal CLI runs shows as working", () => {
  let open = true;
  let act = "working";
  const { rec, boss, update } = office(undefined, {
    running: group => open && group === "ws",
    bossActivity: group => (open && group === "ws" ? act : null),
  });
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.equal(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "工作中");
  act = "idle";
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.notEqual(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "空闲");
  act = "unknown";
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.notEqual(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "终端运行中");
  open = false;
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.notEqual(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "空闲");
});

test("boss office status follows the ADR-0128 §4 priority table", () => {
  let activity = "blocked";
  const { rec, boss, update } = office(undefined, {
    running: () => activity !== null,
    bossActivity: () => activity,
  });
  const base = { group: "ws", host: "codex", label: "Codex CLI", messages: [] };

  // 1. Blocked confirmation outranks running tasks and meetings.
  update(base, { running: 2, review: 1 });
  assert.equal(boss.dataset.mood, "review");
  assert.equal(rec.st.textContent, "等待确认");

  // 2. Waiting on tasks outranks review and working terminal.
  activity = "working";
  update(base, { running: 2, review: 1 });
  assert.equal(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "等 2 个回复");

  // 3. Review meeting outranks working terminal.
  update(base, { running: 0, review: 1 });
  assert.equal(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "会议进行中");

  // 4. Working terminal.
  update(base, { running: 0, review: 0 });
  assert.equal(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "工作中");

  // 5. Idle terminal.
  activity = "idle";
  update(base, { running: 0, review: 0 });
  assert.equal(rec.st.textContent, "空闲");

  // 6. Live terminal without an explicit signal.
  activity = "unknown";
  update(base, { running: 0, review: 0 });
  assert.equal(rec.st.textContent, "终端运行中");

  // 7. No App terminal.
  activity = null;
  update(base, { running: 0, review: 0 });
  assert.equal(rec.st.textContent, "空闲");
});

const speech = (provider, model, time) => ({ kind: "prompt", time, speaker: { role: "manager", provider, model } });
const old = speech("claude", "claude-opus-5-5", "2026-11-04T13:30:00Z");
const handover = { kind: "handover", time: "2026-11-04T13:37:42Z", speaker: { role: "system" } };
const claimed = () => ({ host: "antigravity", app: "antigravity", label: "Antigravity", handover: "changed", previous_host: "claude", messages: [old, handover] });

test("office resizing ignores height feedback and still lays out changed desks at the same floor width", () => {
  let resize;
  let deskWidth = 160;
  const frames = [];
  const writes = [];
  const style = new Proxy({}, {
    set(target, key, value) { writes.push([key, value]); target[key] = value; return true; },
  });
  const context = vm.createContext({
    AN: {}, window: { addEventListener() {} },
    ResizeObserver: class { constructor(callback) { resize = callback; } observe() {} },
    requestAnimationFrame: callback => frames.push(callback),
    getComputedStyle: () => ({ getPropertyValue: () => " 3", columnGap: "10px" }),
    el: { floor: {}, desks: { clientWidth: 920, querySelector: selector => selector === ".desk" && deskWidth ? { clientWidth: deskWidth + 16 } : null } },
    state: { bossRec: { scene: { style } }, roomRecs: new Map() },
    FIG: 100,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  resize([{ contentRect: { width: 800, height: 700 } }]);
  assert.equal(writes.length, 0, "resize delivery must not change the observed floor's height");
  frames.splice(0).forEach(callback => callback());
  assert.equal(style.width, "208px");
  resize([{ contentRect: { width: 800, height: 710 } }]);
  assert.equal(frames.length, 0, "the height change caused by layout must not schedule another layout");
  context.layoutRooms();
  assert.equal(writes.length, 1, "unchanged scene sizes must not mutate the DOM again");
  deskWidth = 180;
  context.layoutRooms();
  assert.equal(style.width, "234px", "new desk data still lays out without a floor resize");
  deskWidth = 140;
  resize([{ contentRect: { width: 720, height: 710 } }]);
  frames.splice(0).forEach(callback => callback());
  assert.equal(style.width, "182px");
  deskWidth = 0;
  context.layoutRooms();
  assert.equal(style.width, "367px", "with no desk open boss takes the size one desk would have");

  const chatSource = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = chatSource.indexOf("function renderAll(");
  const end = chatSource.indexOf("for (const link of", start);
  const blanks = [];
  const drawer = { hidden: true };
  let measuredWithDrawerHidden = null;
  const chatContext = vm.createContext({
    AN: {}, state: { loaded: "ws-a", selected: "ws-b", groupsData: [], auto: false },
    narrow: { matches: false }, el: { banner: {}, app: { dataset: {} } },
    ChatTerminal: { refresh: async () => {}, pending: () => [], follow: (group) => group, pendingId: () => null, displayState: () => null, column() {}, readMessages() {} },
    contextOf: c => c, followAlias() {}, counts: () => ({}),
    renderHead() {}, renderConv() { drawer.hidden = false; }, renderSideToggle() {}, renderTicker() {},
    renderOffice() { measuredWithDrawerHidden = drawer.hidden; },
    showBlank: kind => blanks.push(kind), renderList() {}, ingest() {},
    ART: { version: () => 1 }, document: { documentElement: { lang: "zh" } }, tickOf: () => 1, timeDriven: () => false,
    load: async path => path === "/app/chats" ? { groups: [{ group: "ws-b" }] } : { group: "ws-b", rooms: [], desks: [], messages: [] },
  });
  vm.runInContext(chatSource.slice(start, end), chatContext);
  return chatContext.refresh().then(() => {
    assert.deepEqual(blanks, [null], "switching between loaded workspaces must not flash loading blank state");
    assert.equal(measuredWithDrawerHidden, false, "renderAll must restore the conversation drawer before measuring office layout");
  });
});

test("takeover changes the furniture and laptop immediately, then changes the person on first dispatch", () => {
  const app = office();
  app.update({ host: "claude", app: "claude-cli", label: "Claude Code", handover: "pending", messages: [old, handover] });
  assert.equal(app.rec.char, old.speaker.model);
  assert.equal(app.rec.bossSet, "claude");
  const c = claimed();
  app.update(c);
  assert.equal(app.rec.char, old.speaker.model);
  assert.equal(app.rec.portrait.hidden, true);
  assert.equal(app.rec.cli.textContent, "Antigravity");
  assert.equal(app.rec.desk.src, "office/boss-desk-agy.webp");
  assert.equal(app.rec.chair.src, "office/boss-chair-agy.webp");
  assert.equal(app.rec.laptop.set, "agy");
  assert.equal(app.rec.lid.children[0].host, "antigravity");
  c.messages.push(speech("antigravity", "claude-opus-5-5-medium", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.char, "claude-opus-5-5-medium");
  assert.equal(app.rec.portrait.hidden, false);
  assert.equal(app.rec.portrait.children[0].name, old.speaker.model);
});

test("an old boss speaking after release cannot take the new boss's model or erase its own portrait", () => {
  const app = office(), c = claimed();
  c.messages.push(speech("claude", old.speaker.model, "2026-11-04T13:38:00Z"));
  app.update(c);
  assert.equal(app.rec.portrait.hidden, true);
  c.messages.push(speech("antigravity", "", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.char, "default");
  assert.equal(app.rec.portrait.children[0].name, old.speaker.model);
});

test("a boss that has not dispatched wears its default model's figure and names no model until one is reported", () => {
  const app = office(), c = { host: "codex", app: "codex", label: "Codex CLI", boss_art_model: "gpt-6.1-sol", messages: [] };
  app.update(c);
  assert.equal(app.rec.char, "gpt-6.1-sol");
  assert.equal(app.rec.state.textContent, "");
  c.messages.push(speech("codex", "gpt-6.1-luna", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.char, "gpt-6.1-luna");
  assert.equal(app.rec.state.textContent, "gpt-6.1-luna");
});

test("a default model never dresses a new boss that has yet to speak after a handover", () => {
  const app = office(), c = { ...claimed(), boss_art_model: "gemini-4-argon" };
  app.update(c);
  assert.equal(app.rec.char, old.speaker.model);
});

test("same-host takeover does not carry the old model into an unknown new boss", () => {
  const app = office(), c = { ...claimed(), host: "claude", app: "claude-cli" };
  c.messages.push(speech("claude", "", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.char, "default");
  assert.equal(app.rec.portrait.children[0].name, old.speaker.model);
});

test("a former boss with no workspace activity has no portrait before or after the new boss dispatches", () => {
  const app = office(), c = { ...claimed(), messages: [handover] };
  app.update(c);
  assert.equal(app.rec.portrait.hidden, true);
  assert.equal(app.rec.bossSet, "agy");
  c.messages.push(speech("antigravity", "claude-opus-5-5-medium", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.char, "claude-opus-5-5-medium");
  assert.equal(app.rec.portrait.hidden, true);
  assert.equal(app.rec.portrait.children.length, 0);
});

test("a former boss's forward alone is not dispatch or hosting evidence", () => {
  const app = office(), c = { ...claimed(), messages: [{ ...old, kind: "forward" }, handover] };
  c.messages.push(speech("antigravity", "claude-opus-5-5-medium", "2026-11-04T14:25:03Z"));
  app.update(c);
  assert.equal(app.rec.portrait.hidden, true);
  assert.equal(app.rec.portrait.children.length, 0);
});

test("a portrait keeps its saved identity when older work leaves the visible message window", () => {
  const app = office(), c = claimed();
  const current = speech("antigravity", "gemini-4-argon", "2026-11-04T14:25:03Z");
  c.boss_messages = [old, handover, current];
  c.messages = [current];
  app.update(c);
  assert.equal(app.rec.char, "gemini-4-argon");
  assert.equal(app.rec.portrait.hidden, false);
  assert.equal(app.rec.portrait.children[0].name, old.speaker.model);
});

test("the same character stays off the wall across model variants and reappears for a different character", () => {
  const app = office((_host, model) => model.startsWith("claude-") ? "claude" : "gemini");
  const c = claimed();
  const current = speech("antigravity", "gemini-4-argon", "2026-11-04T14:25:03Z");
  c.messages.push(current);
  app.update(c);
  assert.equal(app.rec.portrait.hidden, false);
  current.speaker.model = "claude-sonnet-4-6";
  app.update(c);
  assert.equal(app.rec.char, "claude");
  assert.equal(app.rec.portrait.hidden, true);
  current.speaker.model = "gemini-4-argon";
  app.update(c);
  assert.equal(app.rec.portrait.hidden, false);
  assert.equal(app.rec.portrait.children[0].name, "claude");
});


function auditUI(english = false) {
  const cancelled = [];
  const deletedIsolated = [];
  const h = (tag, attributes = {}, ...children) => ({
    tag, attributes, children: children.flat(Infinity).filter(x => x != null && x !== false), dataset: {}, listeners: {}, textContent: "",
    replaceChildren(...items) { this.children = items; },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    set innerHTML(_) { throw new Error("Audit data must never become HTML"); },
  });
  const AN = common(english);
  const context = vm.createContext({
    AN, Actions: { cancelHandover: id => cancelled.push(id), deleteIsolated: (group, d) => deletedIsolated.push([group, d.id]) }, state: { peers: new Set(), expanded: new Set() }, h,
    L: (zh, en) => english ? en : zh,
    ART: { version: () => 1, character: () => "claude", head: () => h("img") },
    deskChar: () => "claude",
    cliName: name => name,
    clock: time => `clock:${time}`, fullTime: time => `full:${time}`,
    setText: (element, text) => { element.textContent = text; },
    setAttr: (element, key, value) => { element.attributes[key] = value; },
    setClass: (element, value) => { element.className = value; },
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-messages.js`, "utf8"), context);
  context.renderMarkdown = () => { throw new Error("Audit data must never become Markdown"); };
  vm.runInContext(readFileSync(`${__dirname}/../chat-topics.js`, "utf8"), context);
  const ctx = { c: {}, desks: new Map(), rooms: new Map() };
  return {
    AN, cancelled, deletedIsolated, context,
    show(approval, existing) {
      const m = { id: "audit", kind: "approval", approval, time: "2026-11-05T00:00:00Z", speaker: { role: "member", provider: "claude" } };
      const rec = existing || context.createMessage(m);
      context.updateMessage(rec, m, ctx, m.id);
      return { rec, event: context.eventText(m, ctx) };
    },
  };
}

test("old message signatures and recipients without identity stay unknown beside a renamed desk", () => {
  const { context } = auditUI();
  const ctx = { desks: new Map([["a", { key: "a", role: "当前职责", identity: identities.explorer }]]) };
  const old = { desk: "a", speaker: { role: "member", provider: "claude", model: "old-model" }, to: [{ key: "a", role: "旧职责", model: "old-model" }] };
  assert.equal(context.speakerName(old), "未记录身份");
  assert.equal(context.speakerRole(old), "");
  const [target] = context.targets(old, ctx);
  const [legacy] = context.targets({ ...old, to: undefined, recipients: [{ focus: "a", role: "旧职责" }], thread: { kind: "desk", key: "a" } }, ctx);
  for (const recipient of [target, legacy, { kind: "desk", who: old.to[0] }]) {
    const chip = context.chip(recipient);
    assert.equal(chip.children.at(-1).children.at(-1), "未记录身份");
    assert.equal(chip.attributes.title, "未记录身份，重连宿主后更新");
    assert.equal(chip.children.some(child => child?.tag === "span.at-role"), false);
  }
});

test("service identities preserve historical duties and duplicate ordinals in signatures and chips", () => {
  const { context } = auditUI();
  const identity = { ...identities.explorer, model: "route/claude-x", ordinal: 4, numbered: true,
    handle: "@route/claude-x·Claude Code·探索派 #4", handle_short: "@claude-x·Claude Code·探索派 #4", spoken_name: "Claude #4" };
  const who = { identity, role: "过时职责", model: "wrong-model" };
  const m = { speaker: who, to: [{ key: "a", ...who }] };
  const ctx = { desks: new Map([["a", { key: "a", identity: { ...identities.pragmatist, key: "a" } }]]) };
  assert.equal(context.speakerName(m), "claude-x #4");
  assert.equal(context.speakerRole(m), "探索派");
  assert.equal(context.speakerSub(m), "Claude Code");
  const chip = context.chip(context.targets(m, ctx)[0]);
  const name = chip.children.find(child => child?.tag === "span.at-name");
  assert.equal(name.children.at(-1), "claude-x #4");
  assert.equal(name.children.find(child => child?.tag === "span.at-role").children[0], "探索派");
  assert.equal(chip.attributes.title, identity.handle);
  context.AN.setFullModelNames(true);
  assert.equal(context.speakerName(m), "route/claude-x #4");
});

test("forward sources localize only their final timestamp and preserve the year of older sources", () => {
  const { context } = auditUI();
  Object.assign(context, { dayLabel: context.AN.dayLabel, clock: context.AN.clock, fullTime: context.AN.fullTime });
  const now = new Date().toISOString();
  const title = "https://logs/2026-11-07T03:51:27Z · 主持人";
  assert.equal(context.localizeSourceTime(`${title} · ${now}`), `${title} · ${context.AN.dayLabel(now)} ${context.AN.clock(now)}`);
  const invalid = `${title} · 2024-13-45T99:99:99Z`;
  assert.equal(context.localizeSourceTime(invalid), invalid);
  const previousYear = `${new Date().getFullYear() - 1}-01-02T03:04:05Z`;
  assert.equal(context.localizeSourceTime(`${title} · ${previousYear}`), `${title} · ${context.AN.fullTime(previousYear)}`);
});

test("a private task shows one forward tag per quoted message, dead when the original is out of view", () => {
  const { context } = auditUI();
  const rec = { fwd: context.h("div.fwds", { hidden: true }), fwdBtns: [] };
  const ctx = { sources: new Set(["c:t:a"]) };
  const task = { forwards: [{ message_id: "c:t:a", source: "@a #1" }, { message_id: "c:e:9", source: "" }] };
  context.showForwards(rec, task, ctx);
  assert.equal(rec.fwd.hidden, false);
  assert.equal(rec.fwd.children.length, 2);
  assert.deepEqual(rec.fwd.children.map(btn => btn.textContent), ["↪ #1 转发自 @a #1", "↪ #2 转发自 其他消息"]);
  assert.deepEqual(rec.fwd.children.map(btn => btn.className), ["fwd", "fwd dead"]);
  assert.deepEqual(rec.fwd.children.map(btn => btn.fwdId), ["c:t:a", null]);
  context.showForwards(rec, { forward: { message_id: "c:t:a", source: "room · 转发" } }, ctx);
  assert.equal(rec.fwd.children.length, 1, "a meeting forward still shows a single tag");
  assert.equal(rec.fwd.children[0].textContent, "↪ 转发自 room · 转发", "a single forward keeps its unnumbered label");
  context.showForwards(rec, {}, ctx);
  assert.equal(rec.fwd.hidden, true);
});

test("native prompt event rows state every outcome in both languages and update in place", () => {
  const cases = [
    [{ kind: "permission", tool_name: "Write", status: "pending" }, "等 boss 决定：Write", "Waiting for boss to decide: Write"],
    [{ kind: "question", status: "pending" }, "等 boss 回答提问", "Waiting for boss to answer the question"],
    [{ kind: "permission", tool_name: "Write", status: "answered", answer: { actor: "boss", decision: "allow" } }, "boss 允许了 Write", "boss allowed Write"],
    [{ kind: "permission", tool_name: "Bash", status: "answered", answer: { actor: "boss", decision: "deny", message: "outside scope" } }, "boss 拒绝了 Bash：outside scope", "boss denied Bash: outside scope"],
    [{ kind: "question", status: "answered", answer: { actor: "boss", answers: { "Where?": "Here", "Continue?": "Yes" } } }, "boss 回答了提问：Where? → Here；Continue? → Yes", "boss answered the question: Where? → Here; Continue? → Yes"],
    [{ kind: "permission", tool_name: "Write", status: "answered", answer: { actor: "service_cancel", decision: "deny" } }, "取消时自动拒绝了 Write", "Automatically denied Write on cancellation"],
    [{ kind: "question", status: "answered", answer: { actor: "service_cancel", decision: "deny" } }, "取消时自动拒绝了提问", "Automatically denied the question on cancellation"],
    [{ kind: "permission", tool_name: "Write", status: "closed", answer: { actor: "service_cancel", decision: "deny" } }, "取消时自动拒绝了 Write（答复未送达）", "Automatically denied Write on cancellation (answer not delivered)"],
    [{ kind: "permission", tool_name: "Write", status: "closed" }, "Write 的请求已结束，未回答", "Write request closed without an answer"],
    [{ kind: "question", status: "closed" }, "提问已结束，未回答", "Question closed without an answer"],
    [{ kind: "permission", tool_name: "Write", status: "closed", answer: { actor: "boss", decision: "allow" } }, "boss 允许了 Write（答复未送达）", "boss allowed Write (answer not delivered)"],
    [{ kind: "permission", tool_name: "ExitPlanMode", status: "pending" }, "等 boss 批准计划", "Waiting for boss to approve the plan"],
    [{ kind: "permission", tool_name: "ExitPlanMode", status: "answered", answer: { actor: "boss", decision: "allow" } }, "boss 批准了计划", "boss approved the plan"],
    [{ kind: "permission", tool_name: "ExitPlanMode", status: "answered", answer: { actor: "boss", decision: "deny", message: "add tests" } }, "boss 退回了计划：add tests", "boss sent the plan back: add tests"],
    [{ kind: "permission", tool_name: "ExitPlanMode", status: "closed" }, "计划确认已结束，未回答", "Plan approval closed without an answer"],
  ];
  for (const english of [false, true]) {
    const app = auditUI(english);
    let rec;
    for (const [data, zh, en] of cases) {
      const result = app.show(data, rec);
      if (rec) assert.equal(result.rec.el, rec.el);
      rec = result.rec;
      assert.equal(rec.text_.textContent, english ? en : zh);
      assert.ok(result.event.text.endsWith(english ? en : zh));
      assert.equal(rec.time.textContent, "clock:2026-11-05T00:00:00Z");
      assert.equal(rec.time.attributes.title, "full:2026-11-05T00:00:00Z");
      assert.equal(rec.el.tag, "div.note");
      assert.equal(rec.md, undefined);
    }
  }
});

test("native prompt inputs and answers stay literal, with a single line Unicode-safe input excerpt", () => {
  const app = auditUI();
  const literal = "<img src=x onerror=alert(1)> **bold** `code`";
  const data = { kind: "question", status: "answered", answer: { actor: "boss", answers: { [literal]: literal } }, tool_input: JSON.stringify({ command: literal }) };
  const { rec, event } = app.show(data);
  assert.equal(rec.detail.textContent, literal);
  assert.equal(rec.text_.textContent, `boss 回答了提问：${literal} → ${literal}`);
  assert.ok(event.text.endsWith(rec.text_.textContent));
  assert.equal(rec.text_.children.length, 0);
  assert.equal(rec.detail.children.length, 0);
  const update = input => app.show({ ...data, tool_input: input }, rec).rec.detail.textContent;
  assert.equal(update(JSON.stringify({ file_path: "/tmp/a", command: "ignored" })), "/tmp/a");
  assert.equal(update(JSON.stringify({ path: "/tmp/b" })), "/tmp/b");
  assert.equal(update(JSON.stringify({ plan: "Write a.txt\nthen test", planFilePath: "/p.md" })), "Write a.txt then test");
  assert.equal(update("not JSON\n  still text"), "not JSON still text");
  assert.equal(update("😀".repeat(81)), "😀".repeat(79) + "…");
  assert.equal(update(""), "");
});

test("native prompt audit events do not count as task answers or meeting replies", () => {
  const app = auditUI();
  const task = { id: "task", kind: "prompt", time: "one", speaker: { role: "manager" } };
  const approval = { id: "audit", kind: "approval", reply_to: "task", time: "two", speaker: { role: "member" } };
  const room = { ...task, id: "room", conversation: "meeting", round: 1, thread: { kind: "room" } };
  const roomApproval = { ...approval, conversation: "meeting", round: 1, thread: { kind: "room" } };
  const topics = app.context.tpTopics([task, approval, room, roomApproval]);
  assert.equal(topics.length, 2);
  assert.equal(topics[0].replies.length, 0);
  assert.equal(topics[1].replies.length, 0);
});


test("unknown model handles localize without changing service role or duplicate suffix", () => {
  const who = { identity: { model_source: "unknown", model_short: "未记录模型", numbered: true, ordinal: 2,
    handle: "@未记录模型·Codex·评审 #2", handle_short: "@未记录模型·Codex·评审 #2" } };
  for (const english of [false, true]) {
    const names = common(english);
    const expected = english ? "@Model not recorded·Codex·评审 #2" : who.identity.handle;
    assert.equal(names.handleOf(who), expected);
    assert.equal(names.fullHandle(who), expected);
    names.setFullModelNames(true);
    assert.equal(names.handleOf(who), expected);
  }
});

test("only meeting messages name their recipients", () => {
  const { context } = auditUI();
  context.state.drawer = { kind: "room" };
  const ctx = { desks: new Map([["a", { key: "a", identity: identities.explorer }]]), rooms: new Map() };
  const boss = { speaker: { role: "manager" }, to: [{ key: "a", identity: identities.explorer }] };
  assert.equal(context.verbOf({ ...boss, thread: { kind: "desk", key: "a" } }, ctx)[1].length, 0);
  assert.equal(context.verbOf({ ...boss, kind: "forward", thread: { kind: "desk", key: "a" } }, ctx)[1].length, 0);
  assert.equal(context.verbOf({ ...boss, round: 1, thread: { kind: "room", id: "r" } }, ctx)[1].length, 1);
});

// The poll's refresh() with its drawing hooks counted.
function refreshing() {
  const source = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = source.indexOf("function renderAll(");
  const calls = { list: 0, all: 0, voice: 0 };
  let groups = [{ group: "ws", updated: "2026-11-10T00:00:00Z" }], chat = { group: "ws", rooms: [], desks: [], messages: [] }, tick = 1, busy = false, terminal = null;
  const context = vm.createContext({
    AN: { voice: { ingest() { calls.voice++; } } }, state: { loaded: null, selected: "ws", groupsData: [], auto: false },
    narrow: { matches: false }, el: { banner: {}, app: { dataset: {} } },
    ChatTerminal: { refresh: async () => {}, pending: () => [], follow: (group) => group, pendingId: () => null, displayState: () => terminal, column() {}, readMessages() {} },
    contextOf: c => c, followAlias() {}, counts: () => ({}), renderHead() {}, renderConv() {}, renderSideToggle() {}, renderTicker() {},
    renderOffice() { calls.all++; }, showBlank() {}, renderList() { calls.list++; }, ingest() {},
    ART: { version: () => 1 }, document: { documentElement: { lang: "zh" } }, tickOf: () => tick, timeDriven: () => busy,
    load: async path => structuredClone(path === "/app/chats" ? { groups } : chat),
  });
  vm.runInContext(source.slice(start, source.indexOf("for (const link of", start)), context);
  return {
    calls, refresh: () => context.refresh(),
    change: patch => { chat = { ...chat, ...patch }; }, rename: () => { groups = [{ ...groups[0], preview: "new" }]; },
    tick: () => { tick++; }, busy: on => { busy = on; }, terminal: value => { terminal = value; },
  };
}

test("a poll draws the workspace list once, and an unchanged poll draws nothing until the next tick", async () => {
  const app = refreshing();
  await app.refresh();
  assert.deepEqual([app.calls.list, app.calls.all], [1, 1], "one refresh draws the list once");
  await app.refresh();
  assert.deepEqual([app.calls.list, app.calls.all], [1, 1], "the same payload in the same tick is not drawn again");
  assert.equal(app.calls.voice, 2, "the voice player still sees every poll");
  app.rename();
  await app.refresh();
  assert.deepEqual([app.calls.list, app.calls.all], [2, 1], "a changed list redraws only the list");
  app.change({ messages: [{ id: "m1" }] });
  await app.refresh();
  assert.deepEqual([app.calls.list, app.calls.all], [2, 2], "a changed workspace redraws only the office");
  app.tick();
  await app.refresh();
  assert.deepEqual([app.calls.list, app.calls.all], [3, 3], "relative times and sleepers move with the clock");
  app.busy(true);
  await app.refresh();
  assert.equal(app.calls.all, 4, "a desk holding its done frame is drawn every poll");
  app.busy(false);
  for (const value of [[1, false], [1, true], [2, true], null]) {
    const before = app.calls.all;
    app.terminal(value);
    await app.refresh();
    assert.equal(app.calls.all, before + 1, "terminal appearance, exit, replacement and removal redraw the office");
  }
});

test("an unfocused window polls every five seconds and refreshes the moment it returns", async () => {
  const timers = [], windowListeners = {}, classes = {};
  let focused = true, ticks = 0;
  const context = vm.createContext({
    document: {
      hidden: false, hasFocus: () => focused, querySelectorAll: () => [], querySelector: () => null, addEventListener() {},
      documentElement: { lang: "zh", dataset: { idleBlur: "" }, classList: { toggle: (name, on) => { classes[name] = on; } } },
    },
    window: {
      ANAppearance: { applyTheme() {}, reducedMotion: { matches: false } },
      addEventListener: (name, fn) => { (windowListeners[name] ||= []).push(fn); },
      dispatchEvent: event => (windowListeners[event.type] || []).forEach(fn => fn(event)),
    },
    localStorage: { getItem: () => null, setItem() {} }, fetch: async () => ({ ok: false, json: async () => ({}) }),
    setInterval() {}, setTimeout: (fn, delay) => timers.push({ fn, delay }), clearTimeout() {}, Event,
  });
  vm.runInContext(readFileSync(`${__dirname}/../common.js`, "utf8"), context);
  const flush = () => new Promise(resolve => setImmediate(resolve));
  const emit = name => (windowListeners[name] || []).forEach(fn => fn({}));
  context.window.AN.poll(async () => { ticks++; });
  await flush();
  assert.equal(ticks, 1);
  assert.equal(timers.at(-1).delay, 1500);

  focused = false;
  emit("blur");
  timers.splice(0).forEach(timer => timer.fn()); // the settled check, then the poll timer
  await flush();
  assert.equal(classes.blurred, true, "the page stands still");
  assert.equal(ticks, 2);
  assert.equal(timers.at(-1).delay, 5000, "polling slows down");

  focused = true;
  emit("focus");
  await flush();
  assert.equal(ticks, 3, "coming back refreshes at once");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(classes.blurred, false);
  assert.equal(timers.at(-1).delay, 1500, "and polls at the usual pace again");

  const dataset = context.document.documentElement.dataset;
  dataset.pauseUnfocused = "false";
  focused = false;
  emit("blur");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(classes.blurred, false, "with the setting off an unfocused window keeps moving");
  dataset.onBattery = "true";
  emit("blur");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(classes.blurred, false, "battery alone does not pause: standing still on battery is off by default");
  dataset.batteryStill = "true";
  emit("blur");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(classes.blurred, true, "except on battery with figures standing still");
  assert.equal(timers.at(-1).delay, 5000);

  let focusEvents = 0;
  context.window.addEventListener("agentnave:focus", () => { focusEvents++; });
  delete dataset.onBattery; // plugged back in while the window is still behind others
  emit("blur");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(classes.blurred, false, "plugging in lifts the pause");
  assert.equal(focusEvents, 0, "but the window did not come back, so no focus event");
  assert.equal(timers.at(-1).delay, 1500, "and polling returns to the usual pace");
  focused = true;
  emit("focus");
  timers.splice(0).forEach(timer => timer.fn());
  await flush();
  assert.equal(focusEvents, 1, "coming back to the front is the focus event");
});

test("terminal focus resumes the office while native window blur still wins", () => {
  const timers = [], listeners = {}, classes = {}, frameWindow = {};
  let observer;
  const root = { lang: "zh", dataset: { idleBlur: "", windowBlurred: "true" }, classList: { toggle: (name, on) => { classes[name] = on; } } };
  const document = { documentElement: root, hidden: false, activeElement: null, hasFocus: () => false,
    querySelectorAll: () => [], querySelector: () => null, addEventListener() {} };
  const window = { ANAppearance: { applyTheme() {}, reducedMotion: { matches: false } },
    addEventListener: (name, fn) => { (listeners[name] ||= []).push(fn); },
    dispatchEvent: event => (listeners[event.type] || []).forEach(fn => fn(event)) };
  const context = vm.createContext({ document, window, location: { protocol: "agentnave:" },
    localStorage: { getItem: () => null, setItem() {} }, fetch: async () => ({ ok: false }),
    MutationObserver: class { constructor(fn) { this.fn = fn; } observe(_root, options) { if (options.attributeFilter?.includes("data-window-blurred")) observer = this.fn; } },
    setInterval() {}, setTimeout: fn => timers.push(fn), clearTimeout() {}, Event });
  vm.runInContext(readFileSync(`${__dirname}/../common.js`, "utf8"), context);
  const emit = (name, event) => (listeners[name] || []).forEach(fn => fn(event));
  const flush = () => timers.splice(0).forEach(fn => fn());
  assert.equal(classes.blurred, true);
  document.activeElement = { tagName: "IFRAME", contentWindow: frameWindow };
  emit("message", { source: frameWindow, data: { type: "agentnave:terminal", action: "focus" } });
  flush();
  assert.equal(classes.blurred, true, "native blur overrides focus inside the frame");
  delete root.dataset.windowBlurred;
  observer();
  assert.equal(classes.blurred, false, "native focus resumes even if the parent document reports no focus");
  root.dataset.windowBlurred = "true";
  observer();
  delete root.dataset.windowBlurred;
  document.activeElement = null;
  observer();
  assert.equal(classes.blurred, true);
  document.activeElement = { tagName: "IFRAME", contentWindow: frameWindow };
  emit("message", { source: {}, data: { type: "agentnave:terminal", action: "focus" } });
  flush();
  assert.equal(classes.blurred, true, "a different frame cannot trigger a recheck");
  emit("message", { source: frameWindow, data: { type: "agentnave:terminal", action: "focus" } });
  flush();
  assert.equal(classes.blurred, false, "terminal focus triggers the settled recheck");
});

test("walking, flights and folds are drawn in place while the window is hidden or paused for being unfocused", () => {
  const classes = new Set();
  let hidden = false;
  const context = vm.createContext({
    AN: { isHidden: () => hidden }, window: { addEventListener() {} }, ResizeObserver: class { observe() {} }, reduceMotion: { matches: false }, el: { floor: {} },
    document: { documentElement: { classList: { contains: name => classes.has(name) } } },
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8") + "\nglobalThis.canAnimate = canAnimate;", context);
  context.Element = { prototype: { animate() {} } };
  assert.equal(context.canAnimate(), true);
  classes.add("blurred");
  assert.equal(context.canAnimate(), false, "the CSS animations are frozen, so the animate() calls stop too");
  classes.clear(); hidden = true;
  assert.equal(context.canAnimate(), false, "a window the App hid is as good as a hidden page");
});

test("a flight under way stands still with the pause and goes on after it; a fold and a teleport due during the pause do not wait", () => {
  const classes = new Set(), listeners = {}, timers = [], warps = [], calls = [];
  const context = vm.createContext({
    AN: { isHidden: () => false }, ResizeObserver: class { observe() {} }, reduceMotion: { matches: false }, el: { floor: {} },
    window: { addEventListener: (name, fn) => { listeners[name] = fn; } },
    document: { documentElement: { classList: { contains: name => classes.has(name) } } },
    Element: class { animate() {} }, getComputedStyle: () => ({}), setTimeout: fn => timers.push(fn), h: () => token,
    ART: { warp: (_fig, kind) => warps.push(kind) }, FOLD_MS: 1, WALK_MS: 1, HOLD_MS: 1,
  });
  const box = { left: 0, top: 0, width: 10, height: 10 };
  const spot = { isConnected: true, getBoundingClientRect: () => box };
  const run = name => ({ finished: new Promise(() => {}), pause: () => calls.push(`${name} pause`), play: () => calls.push(`${name} play`) });
  const token = { style: {}, animate: () => run("flight"), remove() {} };
  context.el.fx = { getBoundingClientRect: () => box, append() {} };
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  context.fly("dispatch", spot, spot, 0);
  context.foldRoom({ offsetHeight: 40, animate: () => run("fold") });
  classes.add("blurred");
  listeners["agentnave:pause"]();
  classes.clear();
  listeners["agentnave:pause"]();
  assert.deepEqual(calls, ["flight pause", "flight play"], "the fold runs on: the room leaves on a wall-clock timer");

  const end = { dataset: { char: "x" }, style: {}, isConnected: true };
  context.teleport(Object.assign(new context.Element(), { dataset: {} }), end, "x");
  classes.add("blurred");
  timers.splice(0).forEach(fn => fn());
  assert.deepEqual(warps, [], "no entrance plays while the window stands still");
  assert.equal(end.style.visibility, "", "the figure is shown in place");
});

test("a side list patches its relative times in place and lays out only when desks or seats change", () => {
  const read = [];
  const context = vm.createContext({
    AN: { at: Date.parse }, window: { addEventListener() {} }, ResizeObserver: class { observe() {} },
    ART: { head: d => d, version: () => 1 }, deskChar: d => d.key, setText: (node, text) => { node.textContent = text; },
    h: (tag, _attrs, ...kids) => ({ tag, kids: kids.flat(), textContent: kids.find(k => typeof k === "string"), addEventListener() {} }),
    getComputedStyle: () => ({ getPropertyValue: () => "", columnGap: "" }),
    el: { desks: { querySelector: () => { read.push("desk"); return null; } } }, state: { bossRec: null, roomRecs: new Map() }, FIG: 100,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  let built = 0;
  const node = { dataset: {}, querySelector: () => null, replaceChildren() { built++; } };
  const item = time => ({ scope: { kind: "desk", key: "a" }, heads: [{ key: "a" }], name: "a", sub: "s", tone: "", time });
  context.sideList(node, "t", [item("刚刚")], "none");
  context.sideList(node, "t", [item("1 分钟前")], "none");
  assert.equal(built, 1, "a new relative time is not a new list");
  assert.equal(node.times[0].textContent, "1 分钟前");
  context.sideList(node, "t", [item("")], "none");
  assert.equal(built, 2, "a row losing its time is");

  context.layoutIfChanged("1 desk");
  const once = read.length;
  assert.ok(once > 0);
  context.layoutIfChanged("1 desk");
  assert.equal(read.length, once, "the same desks and seats are not measured again");
  context.layoutIfChanged("2 desks");
  assert.equal(read.length, 2 * once);
});

test("a folded column peeks on its button or the window edge and leaves 300ms after pointer and focus have gone", () => {
  const source = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = source.indexOf("// A folded column slides");
  const node = () => ({ hover: false, listeners: {}, inside: false, matches() { return this.hover; }, querySelector() { return this.inside ? {} : null; },
    addEventListener(name, fn) { this.listeners[name] = fn; } });
  const side = node(), conv = node(), sideToggle = node(), convToggle = node(), attrs = {}, timers = [], doc = node();
  let renders = 0;
  const context = vm.createContext({
    state: { sideCollapsed: true, convCollapsed: true, chat: {} }, narrow: { matches: false }, overlay: { matches: false }, rail: { matches: false }, innerWidth: 1200,
    el: { app: { querySelector: () => side, toggleAttribute(name, on) { attrs[name] = on; }, getBoundingClientRect: () => ({ top: 48 }) },
      sideToggle, convToggle, drawer: conv },
    document: { addEventListener: (name, fn) => { doc.listeners[name] = fn; }, activeElement: {} },
    setTimeout: fn => timers.push(fn), clearTimeout: () => {}, renderAll: () => { renders++; },
  });
  vm.runInContext(source.slice(start, source.indexOf("// Drag the borders", start)), context);
  const elapse = () => timers.splice(0).forEach(fn => fn());

  convToggle.listeners.pointerenter();
  assert.equal(attrs["data-conv-peek"], true);
  assert.equal(renders, 1, "the hidden drawer is drawn for the peek");
  conv.hover = true;
  convToggle.listeners.pointerleave();
  elapse();
  assert.equal(attrs["data-conv-peek"], true, "a pointer on the drawer keeps it");
  conv.hover = false; conv.inside = true;
  conv.listeners.pointerleave();
  elapse();
  assert.equal(attrs["data-conv-peek"], true, "so does keyboard focus inside it");
  conv.inside = false;
  conv.listeners.focusout();
  elapse();
  assert.equal(attrs["data-conv-peek"], false);

  doc.listeners.pointermove({ buttons: 0, clientX: 3, clientY: 200 });
  assert.equal(attrs["data-side-peek"], true, "the left edge opens the workspace list");
  attrs["data-side-peek"] = undefined;
  context.state.sideCollapsed = false;
  context.setPeek("side", false);
  doc.listeners.pointermove({ buttons: 0, clientX: 3, clientY: 200 });
  assert.equal(attrs["data-side-peek"], false, "a shown list does not peek");
  context.state.sideCollapsed = true;
  doc.listeners.pointermove({ buttons: 0, clientX: 3, clientY: 20 });
  doc.listeners.pointermove({ buttons: 1, clientX: 1199, clientY: 200 });
  assert.equal(attrs["data-side-peek"], false, "the bar does not open the list");
  assert.equal(attrs["data-conv-peek"], false, "a drag in progress does not open the drawer");
});

test("boss office keeps only the meetings list (including ended meetings) and omits the private chat list", () => {
  const h = (tag, attrs = {}, ...children) => ({
    tag, attrs, children: children.flat(Infinity).filter(x => x != null && x !== false),
    dataset: {}, style: {}, listeners: {}, textContent: "",
    replaceChildren(...items) { this.children = items.flat(); },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
  });
  const boss = h("section#boss");
  const opened = [];
  const context = vm.createContext({
    openScope: (scope, opener) => opened.push([scope.kind, opener]),
    AN: { at: Date.parse, dozing: () => false, ongoing: s => s }, window: { addEventListener() {} }, ResizeObserver: class { observe() {} },
    state: { groupsData: [], drawer: { kind: "all" } }, el: { boss }, FIG: 100, L: zh => zh, h,
    ART: { head: d => d, url: p => p, version: () => 1 }, Laptop: { create: () => h("div.laptop") },
    deskChar: d => d.key, activities: () => [], cliOf: () => "Codex", relTime: () => "刚刚", fullRoom: r => r.status === "running",
    urgent: s => s === "running", pick: (_m, s) => [s, s], GROUP: {},
    setText: (n, t) => { n.textContent = t; }, setAttr: (n, k, v) => { n.attrs[k] = v; },
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  const rec = context.createBoss();
  assert.equal(rec.chats, undefined, "private chat side list is removed");
  const allButton = boss.children[0].children.find(x => x.tag === "button.boss-all");
  let stopped = false;
  allButton.listeners.click[0]({ stopPropagation() { stopped = true; } });
  assert.deepEqual(opened, [["all", boss]], "the header button opens every message even when the boss opens a terminal");
  assert.equal(stopped, true);
  assert.ok(rec.meets, "meetings side list is kept");
  const sides = context.sideItems(
    {
      desks: [{ key: "d1", kind: "task", last_active: "2026-11-14T00:00:00Z" }],
      rooms: [{ id: "r1", title: "已结束评审", status: "idle", seats: [], last_active: "2026-11-14T00:00:00Z" }],
    },
    { desks: new Map() },
  );
  assert.equal("chats" in sides, false);
  assert.equal(sides.meets.length, 1);
  assert.equal(sides.meets[0].name, "已结束评审");
});

test("crossing a breakpoint drops both peeks before the page is drawn again", () => {
  const source = readFileSync(`${__dirname}/../chat-state.js`, "utf8");
  const start = source.indexOf("for (const query of [narrow, overlay, rail])");
  const listeners = [], removed = [], order = [];
  const query = { addEventListener: (_name, fn) => listeners.push(fn) };
  const state = { sidePeek: true, convPeek: true, chat: {} };
  const context = vm.createContext({
    narrow: query, overlay: query, rail: query, state, el: { app: { removeAttribute: name => removed.push(name) } },
    sync() { order.push("sync"); }, renderList() {}, renderAll: () => order.push(`draw ${state.convPeek}`),
  });
  vm.runInContext(source.slice(start, source.indexOf("\n\n", start)), context);
  listeners.forEach(fn => fn());
  assert.deepEqual([state.sidePeek, state.convPeek], [false, false]);
  assert.deepEqual(removed.slice(0, 2), ["data-side-peek", "data-conv-peek"]);
  assert.equal(order.at(-1), "draw false", "the redraw sees the settled state");
});

test("a folded drawer beside the floor keeps its conversation; the overlay's closed drawer does not", () => {
  const source = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = source.indexOf("function renderConv(");
  const calls = [];
  const state = { convCollapsed: true, convPeek: false, convOpen: false, scopeKey: "ws{}" };
  const overlay = { matches: false };
  const drawer = { hidden: true, classList: { toggle() {} } };
  const renderConv = vm.runInNewContext(`${source.slice(start, source.indexOf("function updateJump(", start))}; renderConv`, {
    AN: { voice: { controls: on => calls.push(on) } }, state, overlay, el: { drawer, convToggle: {} }, setAttr() {},
  });
  renderConv({}, {});
  assert.equal(state.scopeKey, "ws{}", "peeking again is not a change of scope, so nothing is rebuilt");
  assert.equal(drawer.hidden, false, "it stays in the page to slide");
  assert.deepEqual(calls, [false], "the voice controls still close");
  overlay.matches = true;
  renderConv({}, {});
  assert.equal(state.scopeKey, "");
  assert.equal(drawer.hidden, true);
});

test("the folded-column breakpoints in the stylesheet are the complements of the script's queries", () => {
  const css = readFileSync(`${__dirname}/../chat.css`, "utf8");
  const stateJs = readFileSync(`${__dirname}/../chat-state.js`, "utf8");
  assert.match(stateJs, /matchMedia\("\(max-width: 920px\)"\)/);
  assert.doesNotMatch(css, /@media \(min-width: (761|921)px\)|@media \(min-width: 921px\) and/, "min-width leaves a gap above max-width at fractional widths");
  assert.match(css, /@media not all and \(max-width: 920px\)/);
  assert.match(css, /@media not all and \(max-width: 760px\)/);
});

test("pending handover card shows takeover command, copy, and cancel only while pending and cancelable", async () => {
  const ui = auditUI();
  const ctx = { desks: new Map(), rooms: new Map() };
  const msg = { id: "h1", kind: "handover", time: "2026-11-14T12:00:00Z", text: "Claude Code 交出工作区，等待接手", detail: "note" };
  const [card] = ui.context.tpDirectory({ group: "s-abc", title: "发版", label: "Claude Code", handover: "pending", conversations: [{ id: "c1" }], messages: [msg] }, ctx);
  const next = card.children.find(n => n?.tag === "div.tp-handover-next");
  assert.equal(next.children[0].children[0], "到任意 boss 的聊天里说：");
  assert.equal(next.children[1].children[0], "接手 AgentNave 工作区「发版」（s-abc）");
  const [copyBtn, cancelBtn] = next.children[2].children;
  await copyBtn.listeners.click();
  assert.deepEqual(ui.AN.copied, ["接手 AgentNave 工作区「发版」（s-abc）"]);
  cancelBtn.listeners.click();
  assert.deepEqual(ui.cancelled, ["s-abc"]);

  const [bossOnly] = ui.context.tpDirectory({ group: "s-empty", title: "空工作区", label: "Claude Code", handover: "pending", conversations: [], messages: [msg] }, ctx);
  assert.equal(bossOnly.children.find(n => n?.tag === "div.tp-handover-next").children[2].children.length, 1);

  const [taken] = ui.context.tpDirectory({ group: "s-abc", title: "发版", label: "Codex", handover: "changed", conversations: [{ id: "c1" }], messages: [msg] }, ctx);
  assert.equal(taken.children.find(n => n?.tag === "div.tp-handover-next"), undefined);

  const earlier = { id: "s-abc:handover:claim:s-1", kind: "handover", status: "claimed", time: "2026-11-13T12:00:00Z", text: "Claude 交出工作区，已由 Codex 接手", detail: null };
  const waiting = { id: "s-abc:handover:pending", kind: "handover", status: "pending", time: "2026-11-14T12:00:00Z", text: "Claude 交出工作区，等待接手", detail: "note" };
  const [first, second] = ui.context.tpDirectory({ group: "s-abc", title: "发版", label: "Claude", handover: "pending", conversations: [{ id: "c1" }], messages: [earlier, waiting] }, ctx);
  assert.ok(first.children.find(n => n?.tag === "div.tp-handover-next"));
  assert.equal(second.children.find(n => n?.tag === "div.tp-handover-next"), undefined);
  assert.equal(second.children.find(n => n?.tag === "details.tp-card-note"), undefined);

  const en = auditUI(true);
  const [enCard] = en.context.tpDirectory({ group: "s-abc", title: "Release", label: "Claude Code", handover: "pending", conversations: [], messages: [msg] }, ctx);
  const enNext = enCard.children.find(n => n?.tag === "div.tp-handover-next");
  assert.equal(enNext.children[0].children[0], "In a chat with any boss, say:");
  assert.equal(enNext.children[1].children[0], 'Take over AgentNave workspace "Release" (s-abc)');
});

test("isolated directory cards disable delete while in use and trigger deletion when idle or unfinished", () => {
  const ui = auditUI();
  const ctx = { c: { group: "s-iso" }, desks: new Map(), rooms: new Map() };
  const using = ui.context.isolatedCard({ id: "wt-1", branch: "agentnave/wt-1", path: "/repo/.agentnave-worktrees/wt-1", status: "using" }, ctx);
  const usingBtn = using.children[0].children[1].children[1];
  assert.equal(usingBtn.attributes.disabled, true);
  assert.equal(usingBtn.listeners.click, undefined);

  const idle = ui.context.isolatedCard({ id: "wt-2", branch: "agentnave/wt-2", path: "/repo/.agentnave-worktrees/wt-2", status: "idle" }, ctx);
  const idleBtn = idle.children[0].children[1].children[1];
  assert.equal(idleBtn.attributes.disabled, false);
  assert.equal(idleBtn.children[0], "删除");
  idleBtn.listeners.click();

  const unfinished = ui.context.isolatedCard({ id: "wt-3", branch: "agentnave/wt-3", path: "/repo/.agentnave-worktrees/wt-3", status: "unfinished", attention: "hook err" }, ctx);
  const unfinishedBtn = unfinished.children[0].children[1].children[1];
  assert.equal(unfinishedBtn.attributes.disabled, false);
  unfinishedBtn.listeners.click();
  assert.deepEqual(ui.deletedIsolated, [["s-iso", "wt-2"], ["s-iso", "wt-3"]]);

  const en = auditUI(true);
  const enIdle = en.context.isolatedCard({ id: "wt-2", branch: "agentnave/wt-2", path: "/repo/.agentnave-worktrees/wt-2", status: "idle" }, ctx);
  assert.equal(enIdle.children[0].children[1].children[1].children[0], "Delete");
});

test("deleteIsolated keeps its button disabled until completion or cancel and ignores re-entry", async () => {
  const h = (tag, attributes = {}, ...children) => ({
    tag, attributes, children: children.flat(Infinity).filter(x => x != null && x !== false),
    classList: { toggle() {} }, listeners: {}, textContent: "",
    replaceChildren(...items) { this.children = items; },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    append(...items) { this.children.push(...items); },
    querySelector() { return { focus() {} }; },
    focus() {},
    showModal() { this.open = true; },
    close() { this.open = false; },
  });
  let resolveInspect, resolveDelete, dialogNode;
  const posts = [];
  const cleanupBtn = h("button#cleanup");
  const context = vm.createContext({
    AN: {
      h, $: id => (id === "cleanup" ? cleanupBtn : null), L: zh => zh, load: async () => ({}), chatName: () => "",
      post: (path, body) => {
        posts.push([path, body]);
        if (path === "/app/isolated/inspect") return new Promise(r => { resolveInspect = r; });
        return new Promise(r => { resolveDelete = r; });
      },
    },
    document: { body: { append(...nodes) { for (const node of nodes) if (node.tag === "dialog.act") dialogNode = node; } }, addEventListener() {} },
    window: { addEventListener() {} },
    state: { groupsData: [], order: [] },
    refresh: async () => {},
    renderList() {}, go() {}, openScope() {},
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(readFileSync(`${__dirname}/../actions.js`, "utf8") + "\nglobalThis.Actions = Actions;", context);
  const btn = { disabled: false };
  const run = context.Actions.deleteIsolated("s-1", { id: "wt-1", path: "/p", branch: "b" }, btn);
  assert.equal(btn.disabled, true);
  await context.Actions.deleteIsolated("s-1", { id: "wt-2", path: "/p2", branch: "b2" }, { disabled: false });
  assert.equal(posts.length, 1);

  resolveInspect({ status: "confirm", id: "wt-1", path: "/p", branch: "b", confirmation: "tok" });
  await new Promise(r => setImmediate(r));
  assert.equal(dialogNode.open, true);
  assert.equal(btn.disabled, true);
  await context.Actions.deleteIsolated("s-1", { id: "wt-2", path: "/p2", branch: "b2" }, { disabled: false });
  assert.equal(posts.length, 1);

  const buttonsRow = dialogNode.children.at(-1);
  const deleteBtn = buttonsRow.children[1];
  deleteBtn.listeners.click();
  await new Promise(r => setImmediate(r));
  assert.equal(posts.length, 2);
  assert.equal(btn.disabled, true);

  resolveDelete({ status: "deleted" });
  await run;
  assert.equal(btn.disabled, false);
});

test("linked handover workspaces keep boss character intact and render plain status, mutual links, or cleaned text", () => {
  const routes = [];
  let stopped = 0;
  const h = (tag, attributes = {}, ...children) => ({
    tag, attributes, children: children.flat(Infinity).filter(x => x != null && x !== false),
    dataset: {}, classList: { toggle() {} }, listeners: {}, textContent: "",
    replaceChildren(...items) { this.children = items; },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
    remove() {},
  });
  function linkedOffice(english = false) {
    const context = vm.createContext({
      AN: { at: Date.parse }, window: { addEventListener() {} }, ResizeObserver: class { observe() {} },
      ART: { character: (_host, model) => model || "default", figure: h, head: name => ({ name }), lid: host => ({ host }), url: path => path },
      el: { boss: h("section#boss") }, state: { groupsData: [], drawer: {} },
      L: (zh, en) => (english ? en : zh), cliName: host => host,
      h, href: path => path, go: path => routes.push(path),
      setText: (element, text) => { element.textContent = text; },
      setAttr: (element, key, value) => { element[key] = value; },
    });
    vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
    const rec = Object.fromEntries(["lid", "scene", "desk", "chair", "open", "closed", "portrait", "cli", "st", "state"].map(key => [key, h("span")]));
    rec.laptop = { laptop: { dress() {} } };
    return { rec, update: c => context.updateBoss(rec, c, { running: 0, review: 0 }) };
  }

  const app = linkedOffice();
  const srcClaimed = {
    host: "claude", label: "Claude Code", handover: null,
    handover_to: [{ status: "claimed", host: "codex", label: "Codex" }],
    messages: [old, { ...handover, text: "Claude Code 交出工作区，已由 Codex 接手" }],
  };
  app.update(srcClaimed);
  assert.equal(app.rec.char, old.speaker.model);
  assert.equal(app.rec.portrait.hidden, true);
  assert.deepEqual(app.rec.state.children, ["已由 Codex 接手", " · ", old.speaker.model]);

  app.update({
    ...srcClaimed,
    handover_to: [{ status: "linked", group: "s-target", host: "codex", label: "Codex", title: "续做发布" }],
  });
  const toLink = app.rec.state.children[0];
  assert.equal(toLink.tag, "a.boss-link");
  assert.equal(toLink.attributes.href, "/chat/s-target");
  assert.deepEqual(toLink.children, ["已由 Codex「续做发布」接手"]);
  toLink.listeners.click({ preventDefault() {}, stopPropagation() { stopped++; } });
  assert.deepEqual(routes, ["/chat/s-target"]);
  assert.equal(stopped, 1);

  app.update({
    host: "codex", label: "Codex", handover: null,
    handover_from: [{ status: "linked", group: "s-source", host: "claude", label: "Claude Code", title: "发版检查" }],
    messages: [speech("codex", "gpt-6", "2026-11-04T15:00:00Z")],
  });
  const fromLink = app.rec.state.children[0];
  assert.equal(fromLink.tag, "a.boss-link");
  assert.deepEqual(fromLink.children, ["接手自 Claude Code「发版检查」"]);
  fromLink.listeners.click({ preventDefault() {}, stopPropagation() { stopped++; } });
  assert.deepEqual(routes, ["/chat/s-target", "/chat/s-source"]);
  assert.equal(stopped, 2);

  app.update({
    host: "codex", label: "Codex", handover: null,
    handover_from: [{ status: "cleaned" }],
    messages: [],
  });
  assert.deepEqual(app.rec.state.children, ["接手自原 agent（已清理）"]);

  const en = linkedOffice(true);
  en.update({
    host: "claude", label: "Claude Code", handover: null,
    handover_to: [{ status: "cleaned", host: "codex", label: "Codex" }],
    messages: [],
  });
  assert.deepEqual(en.rec.state.children, ["Taken over by Codex (cleaned)"]);
});

test("cleanup respects handover_ms, pending state, and expired empty_claimed workspaces", async () => {
  const h = (tag, attributes = {}, ...children) => {
    const kids = children.flat(Infinity).filter(x => x != null && x !== false);
    return {
      tag, attributes, children: kids,
      classList: { toggle() {} }, listeners: {}, textContent: kids.map(k => (typeof k === "string" ? k : k.textContent || "")).join(""),
      setAttribute(k, v) { this.attributes[k] = v; },
      replaceChildren(...items) { this.children = items; },
      addEventListener(name, fn) { this.listeners[name] = fn; },
      append(...items) { this.children.push(...items); },
      querySelector() { return { focus() {} }; },
      focus() {},
      showModal() { this.open = true; },
      close() { this.open = false; },
    };
  };
  const posts = [];
  let dialogNode;
  const cleanupBtn = h("button#cleanup");
  const recentClaimMs = Date.now() - 10 * 864e5;
  const oldConvIso = new Date(Date.now() - 45 * 864e5).toISOString();
  const oldClaimMs = Date.now() - 45 * 864e5;
  const statePayload = {
    items: [
      { id: "c-recent-claim", title: "刚被接手", group: "s-recent-claim", deletable: true, updated: oldConvIso },
      { id: "c-pending", title: "待接手", group: "s-pending", deletable: true, updated: oldConvIso },
      { id: "c-old", title: "已过期", group: "s-old", deletable: true, updated: oldConvIso },
    ],
    workspaces: [
      { id: "s-recent-claim", title: "刚被接手", pending: false, empty_claimed: false, handover_ms: recentClaimMs },
      { id: "s-pending", title: "待接手", pending: true, empty_claimed: false, handover_ms: oldClaimMs },
      { id: "s-old", title: "已过期", pending: false, empty_claimed: false, handover_ms: oldClaimMs },
      { id: "s-empty-old", title: "空已接手", pending: false, empty_claimed: true, handover_ms: oldClaimMs },
      { id: "s-empty-pending", title: "空待接手", pending: true, empty_claimed: false, handover_ms: oldClaimMs },
    ],
  };
  const context = vm.createContext({
    AN: {
      h, $: id => (id === "cleanup" ? cleanupBtn : null), L: zh => zh,
      at: Date.parse,
      load: async () => statePayload,
      chatName: g => g.title || g.group,
      post: async (path, body) => { posts.push([path, body]); return { status: "deleted", results: [{ status: "deleted" }] }; },
    },
    document: { body: { append(...nodes) { for (const node of nodes) if (node.tag === "dialog.act") dialogNode = node; } }, addEventListener() {} },
    window: { addEventListener() {} },
    state: { groupsData: [{ group: "s-old", title: "已过期" }, { group: "s-empty-old", title: "空已接手" }], order: [] },
    refresh: async () => {}, renderList() {}, go() {}, openScope() {},
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(readFileSync(`${__dirname}/../actions.js`, "utf8"), context);
  const cleanupPromise = cleanupBtn.listeners.click();
  await new Promise(r => setImmediate(r));
  // 30d default: s-recent-claim (10d claim), s-pending, s-empty-pending are excluded; s-old and s-empty-old qualify.
  const countLine = dialogNode.children.find(n => n?.tag === "p.act-note" && String(n.textContent).includes("没有动静。"));
  assert.match(countLine.textContent, /有 2 个 agent 超过 30 天没有动静/);
  const nextBtn = dialogNode.children.at(-1).children.at(-1);
  nextBtn.listeners.click();
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
  const deleteBtn = dialogNode.children.at(-1).children.at(-1);
  deleteBtn.listeners.click();
  await cleanupPromise;
  assert.equal(posts.length, 2);
  assert.deepEqual(posts[0][0], "/app/conversations/delete");
  assert.deepEqual([...posts[0][1].ids], ["c-old"]);
  assert.equal(posts[1][0], "/app/workspaces/forget");
  assert.equal(posts[1][1].group, "s-empty-old");
  assert.equal(typeof posts[1][1].updated_before, "number");
});

test("ChatTerminal matches only the latest confirmed workspace and keeps exited dispatched terminals out of pending", async () => {
  const node = (tag = "div") => ({
    tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [],
    classList: { toggle() {}, remove() {} },
    setAttribute(k, v) { this.attributes[k] = v; },
    append(...items) { this.children.push(...items); },
    before() {}, after() {},
    replaceChildren(...items) { this.children = items; },
    addEventListener() {}, remove() {},
  });
  const h = (tag, attributes = {}, ...children) => Object.assign(node(tag), { attributes, children });
  let sessions = [];
  const drawer = node("aside");
  let modes;
  const drAll = Object.assign(node("button"), { after(first) { modes = first; } });
  const tl = node("div");
  const context = vm.createContext({
    AN: {
      h, L: zh => zh, load: async () => ({ sessions, cwd: "/repo" }),
      cliName: p => p, hostTile: p => node(p), href: p => p,
    },
    el: {
      drawer, drAll, tl, drIco: node("span"), drTitle: node("span"), drSub: node("span"), convToggle: node("button"),
    },
    state: { loaded: null, convCollapsed: false },
    Actions: { openMenu() {}, say() {} },
    go() {},
    setText: (el, v) => { el.textContent = v; },
    setAttr: (el, k, v) => { el.attributes[k] = v; },
    addEventListener() {},
    requestAnimationFrame() {},
    location: { origin: "null" },
    Date,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);
  const { ChatTerminal } = context;

  // Term 1 confirmed ws-old then ws-new and exited; Term 2 never confirmed any workspace and exited.
  sessions = [
    { id: 1, provider: "codex", cwd: "/repo/a", exit: 0, active: false, workspaces: ["ws-old", "ws-new"], project_key: "/repo/a" },
    { id: 2, provider: "claude", cwd: "/repo/b", exit: 0, active: false, workspaces: [], project_key: "/repo/b" },
    { id: 3, provider: "claude", cwd: "/repo/c", exit: null, active: true, workspaces: ["ws-prev", "ws-curr"], project_key: "/repo/c" },
  ];
  await ChatTerminal.refresh();

  assert.deepEqual(ChatTerminal.pending().map(x => x.group), ["term-2"]);
  assert.equal(ChatTerminal.pending()[0].status, "idle");
  sessions[1].exit = null;
  await ChatTerminal.refresh();
  assert.equal(ChatTerminal.pending()[0].status, "running");
  assert.equal(ChatTerminal.pending()[0].live, true);
  sessions[1].exit = 0;
  await ChatTerminal.refresh();
  assert.equal(ChatTerminal.pending()[0].live, false);
  assert.equal(ChatTerminal.bossOpensTerminal("ws-new"), false);
  assert.equal(ChatTerminal.bossOpensTerminal("term-2"), true);
  assert.equal(ChatTerminal.bossOpensTerminal("ws-curr"), true);

  // ws-old and ws-prev must NOT fall back to terminals whose latest workspace moved on.
  const recPrev = { label: node(), term: null };
  ChatTerminal.decorate(recPrev, { group: "ws-prev" });
  assert.equal(recPrev.term.hidden, true);
  const recCurr = { label: node(), term: null };
  ChatTerminal.decorate(recCurr, { group: "ws-curr" });
  assert.equal(recCurr.term.hidden, false);
  assert.equal(ChatTerminal.running("ws-curr"), true);
  assert.equal(ChatTerminal.running("ws-prev"), false);
  assert.equal(ChatTerminal.running("ws-new"), false);

  ChatTerminal.column("ws-old");
  assert.equal(modes.hidden, true);
  ChatTerminal.column("ws-prev");
  assert.equal(modes.hidden, true);
  ChatTerminal.column("ws-new");
  assert.equal(modes.hidden, false);
  ChatTerminal.column("ws-curr");
  assert.equal(modes.hidden, false);
  assert.equal(ChatTerminal.follow("term-1"), "ws-new");
  const recProject = { el: node("button"), section: { key: "/repo/a", pinned: false } };
  ChatTerminal.headAction(recProject);
  assert.equal(recProject.add.hidden, false);
  const recPinned = { el: node("button"), section: { key: ":handover", pinned: true } };
  ChatTerminal.headAction(recPinned);
  assert.equal(recPinned.add.hidden, true);
});

test("sidebar ignores legacy groupBy storage, renders empty manual projects, reveals hidden added projects, and restricts remove menu", () => {
  const classes = (set = new Set()) => ({
    set,
    add: (...names) => names.forEach(n => set.add(n)),
    remove: (...names) => names.forEach(n => set.delete(n)),
    toggle: (name, on) => (on ? set.add(name) : set.delete(name)),
    contains: name => set.has(name),
  });
  const makeNode = (tag = "div") => ({
    tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [],
    classList: classes(), listeners: {},
    setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
    append(...items) { this.children.push(...items.flat()); },
    replaceChildren(...items) { this.children = items.flat(); },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    scrollIntoView() {},
    getBoundingClientRect: () => ({ left: 12, bottom: 30 }),
    closest: () => null,
  });
  const h = (tag, attrs = {}, ...children) => {
    const el = Object.assign(makeNode(tag), {
      attributes: { ...attrs },
      children: children.flat(Infinity).filter(x => x != null && x !== false),
    });
    return el;
  };
  const els = new Map([["app", makeNode("div")], ["list", makeNode("div")]]);
  const storage = new Map([
    ["agentnave.office.groupBy", "boss"],
    ["agentnave.office.fold.project:/w/deep", "1"],
  ]);
  const menuCalls = [];
  const rowMenus = [];
  const menuEvents = [];
  const context = vm.createContext({
    AN: {
      h, $: id => els.get(id) || makeNode(id), L: zh => zh,
      load: async () => ({}), href: p => p, GROUP: { idle: ["空闲", "idle"] },
      pick: (_map, key) => ["空闲", key || "idle"], urgent: s => s === "running",
      avatar: () => makeNode("span"), colorClass: () => "c0", relTime: () => "刚刚",
      clock: () => "", fullTime: () => "", dayLabel: () => "",
      setText: (el, v) => { el.textContent = v == null ? "" : String(v); },
      setAttr: (el, k, v) => el.setAttribute(k, v),
      setClass: (el, v) => { el.className = v; },
      reconcile: (parent, nodes) => { parent.children = [...nodes]; },
      stickyOrder: items => items, poll() {}, plain: x => x,
      art: { character: () => "c" }, hostTile: () => makeNode("span"),
      deskName: () => "", deskParts: () => [], handleOf: () => "", fullHandle: () => "",
      cliOf: () => "", chatName: g => g.title || g.group, shownName: g => g.title || g.group,
      activities: () => [], ongoing: s => s || "idle",
    },
    localStorage: {
      getItem: k => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, v),
      removeItem: k => storage.delete(k),
    },
    window: null,
    document: { addEventListener() {}, documentElement: { lang: "zh" } },
    location: { search: "", pathname: "/", hash: "" },
    history: { pushState() {}, replaceState() {} },
    ChatTerminal: {
      headAction(rec) { rec.add ||= makeNode("span.gl-grp-add"); },
      decorate() {},
      menu() { return false; },
    },
    Actions: {
      picked: new Set(), pick: () => false,
      workspaceMenu(id, e) { rowMenus.push([id, e.clientX, e.clientY]); },
      projectMenu(section, e) { menuCalls.push(section.key); menuEvents.push([e.clientX, e.clientY]); },
    },
    addEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    Avatars: null,
    requestAnimationFrame() {}, setTimeout, clearTimeout, Date,
  });
  context.window = context;
  vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
  vm.runInContext(
    `${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nglobalThis.state = state; globalThis.renderList = renderList; globalThis.revealProject = revealProject;`,
    context,
  );

  const agent = (group, key, updated, boss = "codex") => ({
    group, title: group, status: "idle", updated, label: boss === "claude" ? "Claude Code" : "Codex",
    boss_key: `${boss}|${boss}`, host: boss, project_key: key, project_label: key.split("/").pop(),
    project_path: key, preview: "", desks_count: 0, rooms_count: 0,
  });
  const groups = [
    agent("g-codex", "/w/app", "2026-11-12T10:00:00Z", "codex"),
    agent("g-claude", "/w/app", "2026-11-12T09:00:00Z", "claude"),
  ];
  for (let i = 0; i < 10; i++) {
    groups.push(agent(`g-p${i}`, `/w/p${i}`, `2026-11-${String(11 - i).padStart(2, "0")}T00:00:00Z`));
  }
  context.state.groupsData = groups;
  context.state.projects = [
    { key: "/w/app", label: "app", path: "/w/app", added_at: Date.parse("2026-11-01T00:00:00Z") },
    { key: "/w/solo", label: "solo", path: "/w/solo", added_at: Date.parse("2026-11-12T08:00:00Z") },
    { key: "/w/deep", label: "deep", path: "/w/deep", added_at: Date.parse("2026-10-01T00:00:00Z") },
  ];
  context.renderList();

  // Even with boss grouping saved in storage, both bosses under /w/app stay under one project header,
  // and empty manual /w/solo appears without rendering the empty list message.
  const list = els.get("list");
  assert.equal(list.children.some(n => n.tag === "div.gl-empty"), false);
  const headTitles = list.children.filter(n => n.tag === "button.gl-grp").map(n => n.children[1].textContent);
  assert.deepEqual(headTitles.slice(0, 2), ["app", "solo"]);
  assert.equal(headTitles.includes("deep"), false, "project past cap waits until revealed or expanded");

  // Context menu opens on all non-:handover headers, never on :handover or clicking '+'.
  for (const key of ["project:/w/app", "project:/w/p0", "project:/w/solo"]) {
    const rec = context.state.heads.get(key);
    for (const fn of rec.el.listeners.contextmenu) fn({ target: { closest: () => null } });
  }
  assert.deepEqual(menuCalls, ["/w/app", "/w/p0", "/w/solo"]);
  for (const fn of context.state.heads.get("project:/w/solo").el.listeners.contextmenu) {
    fn({ target: { closest: sel => sel === ".gl-grp-add" } });
  }
  assert.deepEqual(menuCalls, ["/w/app", "/w/p0", "/w/solo"]);

  // Right-click opens workspaceMenu on agent rows; neither headers nor rows render hover ⋯ buttons.
  assert.equal(context.state.heads.get("project:/w/app").more, undefined);
  assert.equal(context.state.heads.get("project:/w/app").el.children.some(n => n.tag === "span.gl-grp-more"), false);
  const rowEl = context.state.items.get("g-codex").el;
  assert.equal(rowEl.children.some(n => n.tag === "span.gi-more"), false);
  for (const fn of rowEl.listeners.contextmenu) fn({ clientX: 24, clientY: 48 });
  assert.deepEqual(rowMenus, [["g-codex", 24, 48]]);
  assert.equal(menuEvents.length, 3);

  // Pending handover items stay visible even when :handover is folded, and folded headers show status dots.
  storage.set("agentnave.office.fold.project::handover", "1");
  storage.set("agentnave.office.fold.project:/w/p0", "1");
  context.state.folded.set("project:/w/p0", true);
  context.state.groupsData = [
    ...groups,
    { ...agent("g-handover", "/w/app", "2026-11-12T12:00:00Z"), handover: "pending" },
    { ...agent("g-p0-bad", "/w/p0", "2026-11-11T01:00:00Z"), status: "failed" },
  ];
  context.renderList();
  assert.equal(context.state.items.get("g-handover").el.attributes["data-folded"], undefined);
  assert.equal(context.state.heads.get("project::handover").dot.hidden, false);
  assert.equal(context.state.heads.get("project::handover").dot.attributes["data-tone"], "review");
  assert.equal(context.state.heads.get("project::handover").dot.attributes.title, "待审核/待接手");
  assert.equal(context.state.heads.get("project:/w/p0").dot.hidden, false);
  assert.equal(context.state.heads.get("project:/w/p0").dot.attributes["data-tone"], "bad");
  assert.equal(context.state.heads.get("project:/w/p0").dot.attributes.title, "有失败");

  context.revealProject("/w/deep");
  assert.equal(storage.has("agentnave.office.fold.project:/w/deep"), false);
  assert.equal(context.state.heads.get("project:/w/deep").el.classList.contains("flash"), true);
});

test("a derived session of any state keeps its boss past the cap and shows under a folded project", () => {
  const classes = (set = new Set()) => ({
    set,
    add: (...names) => names.forEach(n => set.add(n)),
    remove: (...names) => names.forEach(n => set.delete(n)),
    toggle: (name, on) => (on ? set.add(name) : set.delete(name)),
    contains: name => set.has(name),
  });
  const makeNode = (tag = "div") => ({
    tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [],
    classList: classes(), listeners: {},
    setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
    append(...items) { this.children.push(...items.flat()); },
    replaceChildren(...items) { this.children = items.flat(); },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    scrollIntoView() {},
    closest: () => null,
  });
  const h = (tag, attrs = {}, ...children) =>
    Object.assign(makeNode(tag), { attributes: { ...attrs }, children: children.flat(Infinity).filter(x => x != null && x !== false) });
  const els = new Map([["app", makeNode("div")], ["list", makeNode("div")]]);
  const storage = new Map();
  let derived = [];
  const context = vm.createContext({
    AN: {
      h, $: id => els.get(id) || makeNode(id), L: zh => zh,
      load: async () => ({}), href: p => p, GROUP: { idle: ["空闲", "idle"], waiting: ["等待确认", "review"] },
      pick: (_map, key) => ["空闲", key || "idle"], urgent: s => s === "running",
      avatar: () => makeNode("span"), colorClass: () => "c0", relTime: () => "刚刚",
      clock: () => "", fullTime: () => "", dayLabel: () => "",
      setText: (el, v) => { el.textContent = v == null ? "" : String(v); },
      setAttr: (el, k, v) => el.setAttribute(k, v),
      setClass: (el, v) => { el.className = v; },
      reconcile: (parent, nodes) => { parent.children = [...nodes]; },
      stickyOrder: items => items, poll() {}, plain: x => x,
      art: { character: () => "c" }, hostTile: () => makeNode("span"),
      deskName: () => "", deskParts: () => [], handleOf: () => "", fullHandle: () => "",
      cliOf: () => "", chatName: g => g.title || g.group, shownName: g => g.title || g.group,
      activities: () => [], ongoing: s => s || "idle",
    },
    localStorage: {
      getItem: k => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => storage.set(k, v),
      removeItem: k => storage.delete(k),
    },
    window: null,
    document: { addEventListener() {}, documentElement: { lang: "zh" } },
    location: { search: "", pathname: "/", hash: "" },
    history: { pushState() {}, replaceState() {} },
    ChatTerminal: {
      headAction(rec) { rec.add ||= makeNode("span.gl-grp-add"); },
      decorate() {},
      menu() { return false; },
      derivedMeta: () => ({ text: "", tone: "idle" }),
      derivedOf: parent => derived.filter(d => d.parent_workspace_id === parent),
      rowPath: g => `/chat/${g.group}`,
      derivedAll: () => derived,
    },
    Actions: { picked: new Set(), pick: () => false, workspaceMenu() {}, projectMenu() {} },
    addEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    Avatars: null,
    requestAnimationFrame() {}, setTimeout, clearTimeout, Date,
  });
  context.window = context;
  vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
  vm.runInContext(
    `${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nglobalThis.state = state; globalThis.renderList = renderList;`,
    context,
  );

  const agent = (group, updated) => ({
    group, title: group, status: "idle", updated, label: "Codex", boss_key: "codex|codex", host: "codex",
    project_key: "/w/calc", project_label: "calc", project_path: "/w/calc", preview: "", desks_count: 0, rooms_count: 0,
  });
  const child = (id, derived_state, extra = {}) => ({
    group: `term-${id}`, terminal: id, host: "antigravity", app: "antigravity", label: "Antigravity", title: `Task ${id}`,
    status: derived_state === "running" || derived_state === "starting" ? "running" : (derived_state === "waiting" || derived_state === "uncollected" ? "waiting" : "idle"),
    live: derived_state === "running" || derived_state === "starting" || derived_state === "waiting",
    updated: "2026-11-12T10:00:00Z", derived: true, derived_state,
    parent_workspace_id: "g-parent", blocked_kind: derived_state === "waiting" ? "permission" : null,
    project_key: "/w/calc", project_label: "calc", project_path: "/w/calc", preview: "",
    ...extra,
  });
  // The parent is the oldest of seven agents in its project, so only a derived child keeps it past the cap.
  const groups = [agent("g-parent", "2026-11-01T00:00:00Z")];
  for (let i = 0; i < 6; i++) groups.push(agent(`g-other${i}`, `2026-11-1${i}T00:00:00Z`));
  context.state.groupsData = groups;
  const rows = () => {
    context.renderList();
    return els.get("list").children.filter(n => n.attributes?.href);
  };
  const shown = () => rows().filter(n => n.attributes["data-folded"] !== "true").map(n => n.attributes.href);

  for (const ds of ["running", "starting", "waiting", "uncollected", "idle", "dormant", "exited"]) {
    derived = [child(20, ds)];
    assert.ok(shown().includes("/chat/term-20"), `${ds} child shows under parent past cap`);
    assert.ok(shown().includes("/chat/g-parent"), `${ds} child keeps parent past cap`);
  }

  // A folded project still shows the boss with every derived child, idle or not.
  context.state.folded.set("project:/w/calc", true);
  derived = [child(20, "waiting"), child(21, "idle"), child(22, "dormant"), child(23, "exited")];
  assert.deepEqual(shown(), ["/chat/g-parent", "/chat/term-20", "/chat/term-21", "/chat/term-22", "/chat/term-23"]);
  assert.deepEqual([...context.state.order], ["g-parent", "term-20", "term-21", "term-22", "term-23"]);
  // A redraw keeps the row: one made anew under the pointer would lose the click.
  const row = context.state.items.get("term-20").el;
  derived = [child(20, "running"), child(21, "idle")];
  assert.equal(rows().includes(row), true);
  derived = [];
  assert.deepEqual(shown(), []);
});

test("#add-project and Actions.projectMenu handle pick, add, reveal, errors, and empty manual project removal", async () => {
  const h = (tag, attributes = {}, ...children) => {
    const kids = children.flat(Infinity).filter(x => x != null && x !== false);
    const classes = new Set();
    return {
      tag, attributes, children: kids, disabled: false, hidden: Boolean(attributes.hidden), style: {},
      classList: { toggle(name, on) { if (on) classes.add(name); else classes.delete(name); }, classes },
      listeners: {}, textContent: kids.map(k => (typeof k === "string" ? k : k.textContent || "")).join(""),
      setAttribute(k, v) { this.attributes[k] = v; },
      replaceChildren(...items) { this.children = items; },
      addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
      append(...items) { this.children.push(...items); },
      getBoundingClientRect: () => ({ width: 120, height: 40 }),
      querySelector: () => null,
      querySelectorAll: () => [],
      contains: () => false,
      focus() {},
      remove() {},
    };
  };
  const bodyChildren = [];
  const posts = [];
  const revealed = [];
  const started = [];
  let refreshes = 0;
  let nextAddError = null;
  const addBtn = h("button#add-project");
  const cleanupBtn = h("button#cleanup");
  const state = { groupsData: [], projects: [], listKey: "cached", order: [] };
  const context = vm.createContext({
    AN: {
      h, $: id => (id === "add-project" ? addBtn : id === "cleanup" ? cleanupBtn : null),
      L: zh => zh, load: async () => ({}), chatName: () => "",
      post: async (path, body) => {
        posts.push([path, body]);
        if (path === "/app/projects/pick") return { cwd: "/tmp/repo/sub" };
        if (path === "/app/projects/add") {
          if (nextAddError) throw new Error(nextAddError);
          const projects = [{ key: "/tmp/repo", label: "repo", path: "~/repo", added_at: 10 }];
          return { project: projects[0], projects };
        }
        if (path === "/app/projects/remove") return { projects: [] };
        if (path === "/app/projects/pin") return { sidebar: { project_aliases: {}, pinned_projects: body.pinned ? [body.key] : [], pinned_groups: [], project_order: [] } };
        if (path === "/app/chats/pin") return { sidebar: { project_aliases: {}, pinned_projects: [], pinned_groups: body.pinned ? [body.group] : [], project_order: [] } };
        if (path === "/app/projects/order") return { sidebar: { project_aliases: {}, pinned_projects: [], pinned_groups: [], project_order: body.order } };
        return {};
      },
    },
    document: {
      body: { append(...nodes) { bodyChildren.push(...nodes); } },
      addEventListener() {},
      querySelectorAll: () => [],
    },
    window: { addEventListener() {} },
    state,
    refresh: async () => { refreshes++; },
    renderList() {}, go() {}, openScope() {},
    revealProject: key => revealed.push(key),
    requestAnimationFrame: fn => setTimeout(fn, 0),
    ChatTerminal: { newAgent: (_e, cwd) => started.push(cwd) },
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../actions.js`, "utf8")}\nglobalThis.Actions = Actions;`, context);

  const plain = v => JSON.parse(JSON.stringify(v));
  const menu = bodyChildren.find(n => n.tag === "div.ctx");
  const toast = bodyChildren.find(n => n.tag === "div.toast");
  assert.ok(menu && toast);

  await addBtn.listeners.click[0]();
  assert.deepEqual(plain(posts), [
    ["/app/projects/pick", {}],
    ["/app/projects/add", { cwd: "/tmp/repo/sub" }],
  ]);
  assert.equal(state.projects.length, 1);
  assert.equal(state.listKey, null);
  assert.equal(refreshes, 1);
  assert.deepEqual(plain(revealed), ["/tmp/repo"]);
  // the CLI menu starts in the project's absolute folder, not its shortened display path
  assert.deepEqual(plain(started), ["/tmp/repo"]);
  assert.equal(addBtn.disabled, false);

  nextAddError = "不在项目目录中";
  await addBtn.listeners.click[0]();
  assert.equal(addBtn.disabled, false);
  assert.equal(toast.textContent, "不在项目目录中");
  assert.equal(toast.classList.classes.has("bad"), true);

  const ev = { clientX: 10, clientY: 10, preventDefault() {}, stopPropagation() {} };
  context.Actions.projectMenu({ key: ":handover", pinned: true, items: [] }, ev);
  assert.equal(menu.hidden, true);

  let popClosed = 0;
  let newChatClosed = 0;
  context.ChatSidebar = { closePopover: () => { popClosed++; }, closeNewChat: () => { newChatClosed++; } };
  context.Actions.projectMenu({ key: "/tmp/repo", manual: true, items: [{ group: "g1" }] }, ev);
  assert.equal(popClosed, 1);
  assert.equal(newChatClosed, 1);
  assert.equal(menu.hidden, false);
  assert.deepEqual(menu.children.map(c => c.textContent), ["在此项目新开 agent", "置顶", "重命名显示名…", "复制路径"]);

  context.Actions.projectMenu({ key: "/tmp/repo", manual: false, items: [] }, ev);
  assert.equal(menu.hidden, false);
  assert.deepEqual(menu.children.map(c => c.textContent), ["在此项目新开 agent", "置顶", "重命名显示名…", "复制路径"]);

  context.Actions.projectMenu({ key: "/tmp/repo", manual: true, items: [] }, ev);
  assert.equal(menu.hidden, false);
  assert.equal(menu.children.at(-1).textContent, "移除项目");
  menu.children.at(-1).listeners.click[0]();
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
  assert.deepEqual(plain(posts.at(-1)), ["/app/projects/remove", { key: "/tmp/repo" }]);
  assert.deepEqual(plain(state.projects), []);
  assert.equal(refreshes, 2);

  // Pin project, pin group (skipping term-*), and reorder projects update state.sidebar.
  await context.Actions.pinProject("/tmp/repo", true);
  assert.deepEqual(plain(state.sidebar.pinned_projects), ["/tmp/repo"]);
  await context.Actions.pinGroup("term-9", true);
  assert.deepEqual(plain(state.sidebar.pinned_groups), []);
  await context.Actions.pinGroup("ws-1", true);
  assert.deepEqual(plain(state.sidebar.pinned_groups), ["ws-1"]);
  await context.Actions.reorderProjects(["/b", "/a"]);
  assert.deepEqual(plain(state.sidebar.project_order), ["/b", "/a"]);
});

test("clicking boss desk returns to terminal with focus, and at most one selection box follows the right column", async () => {
  const makeClasses = () => {
    const set = new Set();
    return {
      set,
      add: (...n) => n.forEach(x => set.add(x)),
      remove: (...n) => n.forEach(x => set.delete(x)),
      toggle: (n, on) => { if (on === undefined ? !set.has(n) : on) set.add(n); else set.delete(n); },
      contains: n => set.has(n),
    };
  };
  const makeNode = (tag = "div") => ({
    tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [], style: {},
    classList: makeClasses(), listeners: {},
    setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
    append(...items) { this.children.push(...items.flat()); },
    before() {}, after() {}, remove() {},
    insertBefore(c, b) { c.parentNode = this; c.nextSibling = b; },
    replaceChildren(...items) { this.children = items.flat(); },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    focus() { this.focused = true; },
    querySelector: () => null,
    querySelectorAll: () => [],
    getBoundingClientRect: () => ({ width: 100, height: 100 }),
  });
  const h = (tag, attrs = {}, ...children) => Object.assign(makeNode(tag), {
    attributes: { ...attrs }, children: children.flat(Infinity).filter(x => x != null && x !== false),
  });
  const posted = [];
  const drawer = makeNode("aside");
  let modes;
  const drAll = Object.assign(makeNode("button"), {
    after(first) { modes = first; },
  });
  const frameProto = () => {
    const f = makeNode("iframe");
    f.dataset.loaded = "1";
    f.contentWindow = { postMessage: (msg) => posted.push(msg) };
    return f;
  };
  const el = {
    app: makeNode("div"),
    boss: makeNode("section#boss"),
    desks: Object.assign(makeNode("div"), { clientWidth: 600 }),
    rooms: makeNode("div"),
    fx: makeNode("div"),
    deskCount: makeNode("span"),
    head: makeNode("header"),
    drawer,
    drAll,
    tl: makeNode("div"),
    drIco: makeNode("span"),
    drTitle: makeNode("span"),
    drSub: makeNode("span"),
    convToggle: makeNode("button"),
  };
  const state = {
    selected: "ws-1", loaded: "ws-1", groupsData: [], drawer: { kind: "all" },
    bossRec: null, deskRecs: new Map(), roomRecs: new Map(), seatRecs: new Map(),
    away: new Map(), closing: new Map(), known: new Set(), ready: true, convCollapsed: false, convPeek: false,
  };
  const ctxObj = {
    c: null, seatDesk: new Map(), desks: new Map([["d1", { key: "d1", provider: "codex", model: "gpt-6.1", first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" }]]),
    drawn: new Set(["d1"]), aliases: new Map(), rooms: new Map(), fullRooms: new Set(), sources: new Set(),
  };
  const context = vm.createContext({
    AN: {
      h, L: zh => zh, at: Date.parse, dozing: () => false, ongoing: s => s,
      load: async () => ({ sessions: [{ id: 7, provider: "claude", cwd: "/repo", exit: null, active: true, workspaces: ["ws-1"], display_title: null }], cwd: "/repo" }),
      cliName: p => (p === "claude" ? "Claude Code" : p),
      hostTile: p => makeNode(p), href: p => p, activities: () => [],
      GROUP: { idle: ["空闲", "idle"] }, pick: () => ["空闲", "idle"], urgent: () => false,
    },
    h, L: zh => zh, el, state, FIG: 100,
    ART: { character: () => "c", figure: makeNode, head: makeNode, lid: makeNode, url: p => p, version: () => 1, canWarp: () => false },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { openMenu() {}, say() {}, roomMenu() {} },
    activities: () => [], cliOf: () => "Codex", deskChar: () => "c", deskName: d => d.model, deskParts: d => ({ name: d.model, effort: "" }),
    fullHandle: d => d.key, relTime: () => "刚刚", fullTime: () => "2026", chatName: g => g.display_title || g.title || g.label,
    shownName: g => g.display_title || g.title || g.label,
    setText: (n, v) => { n.textContent = v; },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.children = nodes; },
    setCollapsed(on) { state.convCollapsed = on; },
    openScope(scope) { state.drawer = scope; context.ChatTerminal.readMessages(); context.renderAll(state.chat); },
    go() {},
    window: { addEventListener() {} },
    ResizeObserver: class { observe() {} },
    getComputedStyle: () => ({ getPropertyValue: () => "2", columnGap: "10px" }),
    requestAnimationFrame: fn => fn(),
    addEventListener() {},
    location: { origin: "null" },
    Date,
    overlay: { matches: false },
  });
  context.h = (tag, attrs = {}, ...children) => (tag.startsWith("iframe") ? frameProto() : h(tag, attrs, ...children));
  context.AN.h = context.h;
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);

  const chat = {
    group: "ws-1", host: "claude", label: "Claude Code", display_title: "统一标题", project: "repo",
    desks: [{ key: "d1", provider: "codex", model: "gpt-6.1", first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" }],
    rooms: [], messages: [],
  };
  state.chat = chat;
  context.renderAll = (c) => {
    context.ChatTerminal.prepare(c.group, c.host);
    context.ChatTerminal.column(c.group, c.host);
    context.renderOffice(c, ctxObj, { running: 0, review: 0 }, false);
  };
  await context.ChatTerminal.refresh();
  context.renderAll(chat);

  const deskEl = state.deskRecs.get("d1").el;
  const selCount = () => [el.boss.classList.contains("sel"), deskEl.classList.contains("sel")].filter(Boolean).length;

  assert.equal(drawer.classList.contains("term-mode"), true);
  assert.equal(el.drTitle.textContent, "boss");
  assert.equal(el.drSub.textContent, "Claude Code");
  assert.equal(el.drSub.attributes.title, "/repo");
  assert.equal(el.boss.classList.contains("sel"), true, "terminal mode highlights boss office");
  assert.equal(selCount(), 1);

  // Clicking a desk switches right column to messages and highlights ONLY that desk.
  deskEl.listeners.click[0]();
  assert.equal(drawer.classList.contains("term-mode"), false);
  assert.equal(deskEl.classList.contains("sel"), true);
  assert.equal(el.boss.classList.contains("sel"), false);
  assert.equal(selCount(), 1);

  // Switching back via mode toggle highlights boss office again, clearing desk highlight.
  modes.children[1].listeners.click[0]();
  assert.equal(drawer.classList.contains("term-mode"), true);
  assert.equal(el.boss.classList.contains("sel"), true);
  assert.equal(deskEl.classList.contains("sel"), false);
  assert.equal(selCount(), 1);

  // Clicking boss desk from messages mode returns to terminal and sends "shown" to focus it.
  deskEl.listeners.click[0]();
  posted.length = 0;
  el.boss.onclick();
  assert.equal(drawer.classList.contains("term-mode"), true);
  assert.equal(el.boss.classList.contains("sel"), true);
  assert.equal(selCount(), 1);
  assert.deepEqual(JSON.parse(JSON.stringify(posted)), [{ type: "agentnave:terminal", action: "shown" }]);
  assert.equal(state.drawer.kind, "all");
  modes.children[0].listeners.click[0]();
  assert.equal(state.drawer.kind, "all");
  assert.equal(el.boss.classList.contains("sel"), true);
  assert.equal(deskEl.classList.contains("sel"), false);

  // When drawer is hidden, 0 selection highlights show on the floor.
  drawer.hidden = true;
  context.renderOffice(chat, ctxObj, { running: 0, review: 0 }, false);
  assert.equal(selCount(), 0);
});

test("terminal confirm rules cover 409 elsewhere, stop terminal conditions, and exited header updates", async () => {
  const node = (tag = "div") => ({
    tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [],
    classList: { set: new Set(), toggle(n, on) { if (on) this.set.add(n); else this.set.delete(n); }, contains(n) { return this.set.has(n); }, remove(n) { this.set.delete(n); } },
    setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
    append(...items) { this.children.push(...items.flat()); },
    before() {}, after() {}, remove() {},
    replaceChildren(...items) { this.children = items.flat(); },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    listeners: {},
  });
  const h = (tag, attrs = {}, ...children) => Object.assign(node(tag), {
    attributes: { ...attrs }, children: children.flat(Infinity).filter(x => x != null && x !== false),
    textContent: children.flat(Infinity).filter(x => typeof x === "string").join(""),
  });
  let sessions = [
    { id: 1, provider: "claude", cwd: "/repo/a", exit: null, active: true, workspaces: ["ws-idle"], display_title: null },
    { id: 2, provider: "pi", cwd: "/repo/b", exit: null, active: true, workspaces: ["ws-pi"], display_title: null },
    { id: 3, provider: "claude", cwd: "/repo/c", exit: null, active: true, workspaces: [], display_title: null },
    { id: 4, provider: "claude", cwd: "/repo/d", exit: null, active: true, workspaces: ["ws-busy"], display_title: null },
    { id: 5, provider: "grok", cwd: "/repo/e", exit: null, active: true, workspaces: ["ws-idle"], display_title: null },
    { id: 6, provider: "antigravity", cwd: "/repo/f", exit: null, active: true, workspaces: ["ws-idle"], display_title: null },
  ];
  const calls = [];
  const asks = [];
  let nextAnswer = null;
  let resume409 = true;
  const msgListeners = [];
  let stopButton;
  const drAll = Object.assign(node("button"), { after(_modes, stop) { stopButton = stop; } });
  const context = vm.createContext({
    AN: {
      h, L: zh => zh, load: async () => ({ sessions, cwd: "/repo" }),
      cliName: p => ({ claude: "Claude Code", grok: "Grok", antigravity: "Antigravity", pi: "Pi" }[p] || p),
      hostTile: p => node(p), href: p => p,
      activities: d => d.activities || [],
    },
    el: {
      app: node("div"), drawer: node("aside"), drAll, tl: node("div"),
      drIco: node("span"), drTitle: node("span"), drSub: node("span"), convToggle: node("button"),
    },
    state: {
      loaded: "ws-idle", selected: "ws-idle", convCollapsed: false,
      groupsData: [{ group: "ws-busy", status: "running" }, { group: "ws-idle", status: "idle" }],
      chat: { group: "ws-idle", desks: [] },
    },
    Actions: {
      openMenu() {}, say() {},
      ask: async (opts) => { asks.push(opts); return nextAnswer; },
    },
    fetch: async (url) => {
      calls.push(url);
      if (url.startsWith("/app/terminal/resume") && resume409 && !url.includes("confirm=1")) {
        return {
          ok: false, status: 409,
          json: async () => ({ error: "这段聊天正在Ghostty里打开（运行中）", elsewhere: { app: "Ghostty", paused: false } }),
        };
      }
      return { ok: true, status: 200, json: async () => ({ session: { id: 9, provider: "claude", cwd: "/repo/x", exit: null, active: true, workspaces: ["ws-x"] } }) };
    },
    go() {}, window: { refresh() {} }, refresh() {}, document: { documentElement: { lang: "zh", dataset: {} } },
    setText: (el, v) => { el.textContent = v; },
    setAttr: (el, k, v) => el.setAttribute(k, v),
    addEventListener(name, fn) { if (name === "message") msgListeners.push(fn); },
    requestAnimationFrame() {},
    location: { origin: "null" },
    Date,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);
  await context.ChatTerminal.refresh();

  // Idle resumable claude, grok, and antigravity end directly without confirmation.
  for (const id of [1, 5, 6]) {
    await context.ChatTerminal.close(id);
    assert.equal(asks.length, 0);
    assert.equal(calls.at(-1), `/app/terminal/close?session=${id}`);
  }

  // Un-dispatched, non-resumable pi, and resumable with running members all require confirmation with Cancel focused.
  nextAnswer = null;
  for (const [id, expectedReason] of [[3, "还没有派过活"], [2, "暂不支持"], [4, "仍有组员在运行"]]) {
    asks.length = 0;
    const before = calls.length;
    await context.ChatTerminal.close(id);
    assert.equal(asks.length, 1);
    assert.equal(asks[0].buttons[0].label, "取消");
    assert.equal(asks[0].buttons[0].focus, true);
    assert.equal(asks[0].buttons[1].label, "结束终端");
    assert.ok(asks[0].body.some(p => p.textContent.includes(expectedReason)));
    assert.equal(calls.length, before, "cancelling must not end");
  }

  // Derived terminal confirmation rules: idle ends directly; busy/blocked confirm; running members add warning.
  sessions = [
    ...sessions,
    { id: 10, provider: "codex", cwd: "/repo/d1", exit: null, active: false, derived: true, derived_busy: false, blocked_kind: null, workspaces: [], display_title: null },
    { id: 11, provider: "codex", cwd: "/repo/d2", exit: null, active: true, derived: true, derived_busy: true, blocked_kind: null, workspaces: [], display_title: null },
    { id: 12, provider: "codex", cwd: "/repo/d3", exit: null, active: false, derived: true, derived_busy: false, blocked_kind: "permission", workspaces: [], display_title: null },
    { id: 13, provider: "codex", cwd: "/repo/d4", exit: null, active: false, derived: true, derived_busy: false, blocked_kind: null, workspaces: ["ws-busy"], display_title: null },
  ];
  await context.ChatTerminal.refresh();
  asks.length = 0;
  await context.ChatTerminal.close(10);
  assert.equal(asks.length, 0);
  assert.equal(calls.at(-1), "/app/terminal/close?session=10");

  nextAnswer = null;
  for (const id of [11, 12]) {
    asks.length = 0;
    const before = calls.length;
    await context.ChatTerminal.close(id);
    assert.equal(asks.length, 1);
    // Only the boss's own dispatch (11) is reported back to it.
    const note = id === 11 ? "这一轮还没做完，结束后会向 boss 回报未完成。" : "这一轮还没做完。";
    assert.ok(asks[0].body.some(p => p.textContent === note));
    assert.equal(asks[0].body.some(p => p.textContent.includes("还没有派过活")), false);
    assert.equal(calls.length, before, "cancelling a derived end must not post");
  }
  nextAnswer = { value: "stop" };
  await context.ChatTerminal.close(11);
  assert.equal(calls.at(-1), "/app/terminal/close?session=11");

  nextAnswer = null;
  asks.length = 0;
  await context.ChatTerminal.close(13);
  assert.equal(asks.length, 1);
  assert.ok(asks[0].body.some(p => p.textContent.includes("仍有组员在运行")));

  // Exited postMessage updates header immediately.
  context.ChatTerminal.column("ws-idle", "claude");
  assert.equal(stopButton.textContent, "结束终端");
  assert.equal(stopButton.attributes.title, "结束 boss（Claude Code）的终端", "the button names whose terminal it ends");
  assert.equal(stopButton.attributes["aria-label"], stopButton.attributes.title);
  assert.equal(context.el.drSub.textContent, "Claude Code");
  msgListeners[0]({ data: { type: "agentnave:terminal", action: "exited", session: 1, code: 0 } });
  assert.equal(context.el.drSub.textContent, "Claude Code · 已结束");
  assert.equal(stopButton.textContent, "续接");
  assert.equal(stopButton.attributes.title, "续接 boss（Claude Code）的终端");
  asks.length = 0;
  await context.ChatTerminal.close(1);
  assert.equal(asks.length, 0, "a terminal whose CLI has exited closes without asking");

  // 409 elsewhere confirm on resume: Cancel aborts, Continue resends with confirm=1.
  sessions = [];
  await context.ChatTerminal.refresh();
  for (const app of ["claude-desktop", "antigravity-desktop"]) {
    context.ChatTerminal.column("ws-desktop", app === "claude-desktop" ? "claude" : "antigravity", app);
    assert.equal(stopButton.textContent, "转交到终端");
    assert.equal(stopButton.hidden, false);
  }
  context.ChatTerminal.column("ws-cli", "claude", "claude-cli");
  assert.equal(stopButton.textContent, "续接");
  for (const host of ["claude", "grok", "antigravity"]) {
    context.ChatTerminal.column("ws-new-resume", host);
    assert.equal(stopButton.hidden, false);
    assert.equal(stopButton.textContent, "续接");
  }
  context.ChatTerminal.column("ws-new-resume", "claude");
  asks.length = 0;
  calls.length = 0;
  nextAnswer = null;
  stopButton.listeners.click[0]();
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
  assert.equal(asks.length, 1);
  assert.ok(asks[0].body.some(p => p.textContent.includes("Ghostty") && p.textContent.includes("运行中")));
  assert.deepEqual(JSON.parse(JSON.stringify(asks[0].buttons.map(b => b.label))), ["取消", "仍然续接"]);
  assert.equal(calls.length, 1);

  nextAnswer = { value: "confirm" };
  stopButton.listeners.click[0]();
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
  assert.equal(calls.length, 3);
  assert.match(calls[2], /confirm=1/);
});

test("display_title unifies names across sidebar/office/right column, directory shows 全部消息 with room context menu, and exited workspace terminal can close", async () => {
  const AN = common(false);
  assert.equal(AN.chatName({ display_title: "CLI 标题", title: "旧标题", label: "Claude Code" }), "CLI 标题");
  assert.equal(AN.chatName({ display_title: null, title: "旧标题", label: "Claude Code" }), "旧标题");
  assert.equal(AN.shownName({ display_title: "CLI 标题", handover: "pending" }), "「待接手」CLI 标题");

  const ui = auditUI();
  const roomMenus = [];
  ui.context.Actions.roomMenu = id => roomMenus.push(id);
  ui.context.document = { documentElement: { lang: "zh" } };
  ui.context.state.nodes = new Map();
  ui.context.el = { drIco: ui.context.h("span"), drTitle: ui.context.h("span"), drSub: ui.context.h("span"), drAll: ui.context.h("button"), jump: ui.context.h("button"), drawer: ui.context.h("aside"), inner: ui.context.h("div"), tl: { scrollTop: 0 } };
  ui.context.hostTile = () => ui.context.h("span");
  ui.context.clock = () => "10:00";
  ui.context.shownName = AN.shownName;
  const ctx = {
    rooms: new Map([["r1", { id: "r1", title: "方案评审", status: "idle", seats: [], pending_review: 0 }]]),
    desks: new Map(),
  };
  ui.context.renderDirectory({ group: "g1", host: "claude", label: "Claude Code", display_title: "统一名称", messages: [] }, ctx);
  assert.equal(ui.context.el.drTitle.textContent, "全部消息");
  assert.equal(ui.context.el.drSub.textContent, "Claude Code");
  const roomCard = ui.context.el.inner.children[0];
  roomCard.listeners.contextmenu({});
  assert.deepEqual(roomMenus, ["r1"]);
});

test("removing the last ended meeting recomputes boss office layout", () => {
  const makeNode = (tag = "div") => {
    const classSet = new Set();
    return {
      tag, hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [], style: {},
      clientWidth: 560, offsetWidth: 100, isConnected: true, scrollTop: 0,
      classList: {
        set: classSet,
        toggle(n, on) { const next = on === undefined ? !classSet.has(n) : Boolean(on); if (next) classSet.add(n); else classSet.delete(n); return next; },
        contains(n) { return classSet.has(n); },
        add(n) { classSet.add(n); },
        remove(n) { classSet.delete(n); },
      },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = v; },
      append(...items) { this.children.push(...items.flat()); },
      insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
      replaceChildren(...items) { this.children = items.flat(); },
      querySelector() { return null; },
      querySelectorAll() { return []; },
      addEventListener() {},
      remove() {},
    };
  };
  const h = (tag, attrs = {}, ...children) => Object.assign(makeNode(tag), {
    attributes: { ...attrs }, children: children.flat(Infinity).filter(x => x != null && x !== false),
  });
  const el = {
    boss: makeNode("section"),
    desks: Object.assign(makeNode("div"), { clientWidth: 560 }),
    rooms: makeNode("div"),
    fx: makeNode("div"),
    deskCount: makeNode("span"),
    office: makeNode("div"),
    floor: makeNode("div"),
  };
  const state = {
    groupsData: [],
    bossRec: null, deskRecs: new Map(), roomRecs: new Map(), seatRecs: new Map(),
    away: new Map(), closing: new Map(), known: new Set(), ready: true, drawer: { kind: "all" },
  };
  const context = vm.createContext({
    AN: { h, L: zh => zh, at: Date.parse, ongoing: s => s, isHidden: () => false, dozing: () => false },
    h, L: zh => zh, el, state, FIG: 100,
    ART: { character: () => "c", figure: makeNode, head: makeNode, lid: makeNode, url: p => p, version: () => 1, canWarp: () => false },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { roomMenu() {} },
    GROUP: { idle: ["空闲", "idle"] }, pick: () => ["空闲", "idle"], urgent: () => false,
    activities: () => [],
    cliOf: () => "Codex", deskChar: () => "c", deskName: d => d.model, deskParts: d => ({ name: d.model, effort: "" }),
    fullHandle: d => d.key, relTime: () => "刚刚", fullTime: () => "2026", shownName: g => g.display_title || g.title || g.label,
    setText: (n, v) => { n.textContent = v; },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.children = nodes; },
    openScope() {}, go() {}, window: { addEventListener() {} }, document: { documentElement: { classList: { contains: () => false } } },
    ResizeObserver: class { observe() {} },
    reduceMotion: { matches: false },
    getComputedStyle: target => ({
      getPropertyValue: () => "2",
      columnGap: "10px",
      gridTemplateColumns: target?.classList?.contains("stack") ? "1fr" : "1fr 225px",
    }),
    Date,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  const ctx = {
    desks: new Map(), drawn: new Set(), rooms: new Map([["r1", { id: "r1" }]]),
  };
  const chatWithEndedRoom = {
    group: "ws-1", host: "claude", label: "Claude Code", display_title: "t",
    desks: [{ key: "d1", provider: "codex", model: "gpt-6.1", first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" }],
    rooms: [{ id: "r1", title: "会", status: "succeeded", seats: [], last_active: "2026-11-19T00:00:00Z" }],
    messages: [],
  };
  context.renderOffice(chatWithEndedRoom, ctx, { running: 0, review: 0 }, false);
  assert.equal(state.bossRec.meets.hidden, false);
  assert.equal(state.bossRec.row.classList.contains("stack"), true);

  // Deleting the last ended meeting leaves 0 active and 0 ended rooms, and must recompute boss layout.
  ctx.rooms.clear();
  context.renderOffice({ ...chatWithEndedRoom, rooms: [] }, ctx, { running: 0, review: 0 }, false);
  assert.equal(state.bossRec.meets.hidden, true);
  assert.equal(state.bossRec.row.classList.contains("stack"), false);
  assert.equal(state.bossRec.scene.style.width, "334px");
});

test("derived terminals nest under parent workspace without counting self-chat as running", async () => {
  const makeNode = (tag) => {
    const cls = new Set();
    return {
      tagName: tag.toUpperCase(),
      hidden: false,
      textContent: "",
      dataset: {},
      attributes: {},
      children: [],
      listeners: {},
      style: {},
      classList: {
        set: cls,
        add: (...names) => names.forEach((n) => cls.add(n)),
        remove: (...names) => names.forEach((n) => cls.delete(n)),
        toggle: (name, on) => (on === undefined ? (cls.has(name) ? (cls.delete(name), false) : (cls.add(name), true)) : on ? (cls.add(name), true) : (cls.delete(name), false)),
        contains: (name) => cls.has(name),
      },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = String(v); },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      replaceChildren(...nodes) { this.children = nodes; },
      append(...nodes) { this.children.push(...nodes); },
      after(...nodes) { this._after = nodes; },
      before() {},
      querySelector() { return null; },
      closest() { return null; },
      focus() {},
      remove() {},
    };
  };
  const h = (tag, attrs = {}, ...children) => Object.assign(makeNode(tag), {
    attributes: { ...attrs }, children: children.flat(Infinity).filter((x) => x != null && x !== false),
  });
  const drawer = makeNode("section");
  const modes = h("div", {}, h("button"), h("button"));
  const stopButton = h("button");
  const drAll = h("div");
  const navigated = [];
  const state = {
    selected: "term-20", loaded: "term-20", groupsData: [], drawer: { kind: "all" },
    ready: true, convCollapsed: false, convPeek: false,
  };
  let mockSessions = [
    { id: 10, provider: "claude", cwd: "/repo", exit: null, active: false, workspaces: ["ws-parent"], derived: false },
    { id: 20, provider: "codex", cwd: "/repo/.wt", exit: null, active: false, workspaces: [], derived: true, parent_boss_key: "claude:boss", parent_workspace_id: "ws-parent", derived_busy: false, blocked_kind: null, display_title: "派生调试" },
  ];
  const el = {
    drawer, drAll, tl: h("div"), drIco: h("span"), drTitle: h("span"), drSub: h("span"),
    convToggle: h("button"), app: { dataset: {} },
  };
  drawer.querySelector = (sel) => ({ ".modes": modes, ".dr-term": stopButton }[sel] || null);
  modes.querySelectorAll = () => modes.children;
  const context = vm.createContext({
    AN: {
      h, L: (zh) => zh, at: Date.parse,
      load: async () => ({ sessions: mockSessions, cwd: "/repo" }),
      act: async () => new Response(JSON.stringify({ ok: true }), { status: 200 }),
      reply: async () => ({ ok: true }),
      cliName: (p) => (p === "codex" ? "Codex" : p === "claude" ? "Claude Code" : p),
      hostTile: (p) => makeNode(p),
      href: (p) => p,
    },
    h, L: (zh) => zh, el, state, Actions: { openMenu() {}, confirm: async () => true },
    setText: (n, v) => { n.textContent = v; },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    go(path) { navigated.push(path); },
    fetch: async () => ({ ok: true, status: 200, json: async () => ({ ok: true }) }),
    addEventListener() {}, location: { origin: "null" }, Date,
    renderOffice() {}, openScope() {},
  });
  context.h = (tag, attrs = {}, ...children) => (tag.startsWith("iframe") ? Object.assign(makeNode("iframe"), { contentWindow: { postMessage() {} } }) : h(tag, attrs, ...children));
  context.AN.h = context.h;
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);

  await context.ChatTerminal.refresh();
  // Derived terminal is excluded from top-level pending() and from owning ws-parent.
  assert.equal(context.ChatTerminal.pending().length, 0);
  assert.equal(context.ChatTerminal.running("ws-parent"), true);
  assert.equal(context.ChatTerminal.follow("term-20"), "term-20");

  // Idle derived terminal (even with exit == null and user typing self-chat) stays idle/non-live.
  const [idleChild] = context.ChatTerminal.derivedOf("ws-parent");
  assert.equal(idleChild.group, "term-20");
  assert.equal(idleChild.derived, true);
  assert.equal(idleChild.status, "idle");
  assert.equal(idleChild.live, false);
  assert.equal(idleChild.title, "派生调试");

  // Busy derived terminal flips to running/live; blocked flips to waiting.
  mockSessions[1].derived_busy = true;
  await context.ChatTerminal.refresh();
  const [busyChild] = context.ChatTerminal.derivedOf("ws-parent");
  assert.equal(busyChild.status, "running");
  assert.equal(busyChild.live, true);

  mockSessions[1].blocked_kind = "permission";
  await context.ChatTerminal.refresh();
  const [blockedChild] = context.ChatTerminal.derivedOf("ws-parent");
  assert.equal(blockedChild.status, "waiting");

  // Parent workspace never embeds a derived terminal even when viewing a derived desk's messages;
  // the derived terminal is hosted directly in its own derived office (ADR-0130).
  mockSessions[1].derived_busy = false;
  mockSessions[1].blocked_kind = null;
  mockSessions[1].native = "sid-1";
  mockSessions[1].desk_key = "thread:1fd2c8006815717d";
  await context.ChatTerminal.refresh();
  state.loaded = "ws-parent";
  state.drawer = { kind: "desk", key: "thread:1fd2c8006815717d" };
  context.ChatTerminal.column("ws-parent", "claude", "claude");
  assert.equal(context.ChatTerminal.showingDerived(), false);
  drAll._after[0].children[1].listeners.click[0]();
  assert.equal(el.drTitle.textContent, "boss");
  assert.equal(context.ChatTerminal.showingDerived(), false);

  // Entering the derived office hosts the derived terminal directly by default when alive.
  const drvGroup = context.ChatTerminal.derivedRowId("ws-parent", "thread:1fd2c8006815717d");
  state.selected = state.loaded = drvGroup;
  state.drawer = { kind: "all" };
  context.ChatTerminal.column(drvGroup, "codex", "codex");
  assert.equal(el.drTitle.textContent, "派生调试");
  assert.equal(context.ChatTerminal.showingDerived(), true);
  state.drawer = { kind: "all" };
  state.selected = state.loaded = "term-20";

  // Stale refresh() responses must not overwrite a newer terminal list.
  const origLoad = context.AN.load;
  let resolveStale;
  const stalePromise = new Promise((resolve) => { resolveStale = resolve; });
  let loadCalls = 0;
  context.AN.load = async (path) => {
    if (path.split("?")[0] !== "/app/terminal/state") return origLoad(path);
    loadCalls += 1;
    if (loadCalls === 1) return stalePromise;
    return { cwd: "/repo", sessions: [{ ...mockSessions[0] }, { ...mockSessions[1], display_title: "最新标题" }] };
  };
  const firstRefresh = context.ChatTerminal.refresh();
  await context.ChatTerminal.refresh();
  resolveStale({ cwd: "/repo", sessions: [{ ...mockSessions[0] }, { ...mockSessions[1], display_title: "过期标题" }] });
  await firstRefresh;
  assert.equal(context.ChatTerminal.derivedOf("ws-parent")[0].title, "最新标题");
  context.AN.load = origLoad;

  // Ended derived terminal not currently watched shows messages and offers Resume.
  mockSessions[1].exit = 0;
  mockSessions[1].derived_busy = false;
  await context.ChatTerminal.refresh();
  context.ChatTerminal.column("term-20", "codex", "codex");
  assert.equal(drAll._after[0].hidden, true);
  assert.equal(drAll._after[1].hidden, false);
  assert.equal(drAll._after[1].textContent, "续接");
  assert.equal(drAll._after[1].classList.contains("start"), true);

  // Closing selected derived terminal returns to its parent workspace.
  mockSessions = [mockSessions[0]];
  await context.ChatTerminal.close(20);
  assert.deepEqual(navigated, ["/chat/ws-parent"]);
});

test("leaving terminal mode clears .term-mode before applyFolds and long/peer/brief bubbles default folded while short bubbles clean up", async () => {
  const AN = common(false);
  const makeNode = (tagName = "div") => {
    const classSet = new Set();
    const [tag, ...initClasses] = String(tagName).split(".");
    for (const c of initClasses) classSet.add(c);
    const node = {
      tagName: tag.toUpperCase(),
      hidden: false,
      textContent: "",
      title: "",
      dataset: {},
      attributes: {},
      children: [],
      style: { setProperty() {}, removeProperty() {} },
      listeners: {},
      parentNode: null,
      clientWidth: 400,
      clientHeight: 500,
      offsetWidth: 100,
      scrollTop: 0,
      get scrollHeight() {
        if (node._scrollHeight != null) return node._scrollHeight;
        const collect = n => typeof n === "string" ? n : `${n?.textContent || ""} ${(n?.children || []).map(collect).join(" ")}`;
        return collect(node).includes("LONG_BUBBLE") ? 640 : 120;
      },
      get isConnected() {
        let cur = node;
        while (cur) {
          if (cur === root) return true;
          cur = cur.parentNode;
        }
        return false;
      },
      get offsetParent() {
        // Mimic .conv.term-mode :is(.tl, .jump, .voice-channel) { display: none; } and [hidden].
        let cur = node;
        while (cur) {
          if (cur.hidden) return null;
          if (cur.classList?.contains?.("tl") && drawer.classList.contains("term-mode")) return null;
          cur = cur.parentNode;
        }
        return root;
      },
      classList: {
        set: classSet,
        add: (...names) => names.forEach(n => classSet.add(n)),
        remove: (...names) => names.forEach(n => classSet.delete(n)),
        toggle: (n, on) => {
          const next = on === undefined ? !classSet.has(n) : Boolean(on);
          if (next) classSet.add(n); else classSet.delete(n);
          return next;
        },
        contains: n => classSet.has(n),
      },
      setAttribute(k, v) {
        if (v == null) {
          delete this.attributes[k];
          if (k.startsWith("data-")) delete this.dataset[k.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())];
        } else {
          this.attributes[k] = v;
          if (k.startsWith("data-")) this.dataset[k.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = v;
        }
      },
      removeAttribute(k) { this.setAttribute(k, null); },
      toggleAttribute(k, on) {
        const next = on === undefined ? !(k in this.attributes) : Boolean(on);
        this.setAttribute(k, next ? "" : null);
        return next;
      },
      append(...items) {
        for (const item of items.flat()) {
          if (item && typeof item === "object") item.parentNode = node;
          node.children.push(item);
        }
      },
      prepend(...items) {
        const flat = items.flat();
        for (const item of flat) if (item && typeof item === "object") item.parentNode = node;
        node.children.unshift(...flat);
      },
      before() {},
      after() {},
      remove() { node.parentNode = null; },
      insertBefore(c, b) { c.parentNode = node; c.nextSibling = b; },
      replaceChildren(...items) {
        for (const old of node.children) if (old && typeof old === "object") old.parentNode = null;
        node.children = [];
        node.append(...items);
      },
      addEventListener(name, fn) { (node.listeners[name] ||= []).push(fn); },
      removeEventListener() {},
      matches: () => false,
      querySelector: () => null,
      querySelectorAll: () => [],
      getBoundingClientRect: () => ({ width: 800, height: 600, top: 48, left: 0, right: 800, bottom: 648 }),
      focus() {},
      click() { (node.listeners.click || []).forEach(fn => fn({ target: node })); },
    };
    return node;
  };
  const h = (tag, attrs = {}, ...children) => {
    const node = makeNode(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "hidden") node.hidden = Boolean(v);
      else node.setAttribute(k, v === true ? "" : v);
    }
    const flat = children.flat(Infinity).filter(x => x != null && x !== false);
    node.append(...flat);
    if (flat.every(x => typeof x === "string")) node.textContent = flat.join("");
    return node;
  };

  const root = makeNode("div.app");
  const side = makeNode("aside.side");
  const drawer = makeNode("aside.conv");
  const tl = makeNode("div.tl");
  const inner = makeNode("div.tl-inner");
  const drAll = makeNode("button.dr-all");
  let modesNode = null;
  drAll.after = (first) => { modesNode = first; };
  drawer.append(drAll, tl);
  tl.append(inner);
  const sideHandle = makeNode("div");
  const convHandle = makeNode("div");
  root.append(side, drawer);
  root.querySelector = (sel) => {
    if (sel === ".side") return side;
    if (sel === '[data-split="side"]') return sideHandle;
    if (sel === '[data-split="conv"]') return convHandle;
    return null;
  };
  const byId = {
    "dr-close": makeNode("button"),
    "announce": makeNode("div"),
  };
  const el = {
    app: root,
    list: makeNode("div"),
    head: makeNode("header"),
    office: makeNode("div"),
    floor: makeNode("div"),
    boss: makeNode("section"),
    desks: Object.assign(makeNode("div"), { clientWidth: 600 }),
    rooms: makeNode("div"),
    fx: makeNode("div"),
    deskCount: makeNode("span"),
    ticker: makeNode("footer"),
    tick: makeNode("span"),
    drawer,
    convToggle: makeNode("button"),
    sideToggle: makeNode("button"),
    drIco: makeNode("span"),
    drTitle: makeNode("span"),
    drSub: makeNode("span"),
    drAll,
    tl,
    inner,
    blank: makeNode("div"),
    jump: makeNode("button"),
    banner: makeNode("div"),
  };
  const state = {
    selected: "ws-1", loaded: "ws-1", groupsData: [], projects: [],
    drawer: { kind: "all" }, convOpen: true, convCollapsed: false, sideCollapsed: false,
    sidePeek: false, convPeek: false, opener: null, scopeKey: "",
    nodes: new Map(), expanded: new Set(), seenIds: new Set(), unseen: 0,
    bossRec: null, deskRecs: new Map(), roomRecs: new Map(), seatRecs: new Map(),
    away: new Map(), closing: new Map(), known: new Set(), ready: true,
    chat: null, ctx: null,
  };
  let termSessions = [{ id: 1, provider: "claude", cwd: "/repo", exit: null, active: true, workspaces: ["ws-1"], display_title: null }];
  const context = vm.createContext({
    AN: {
      ...AN,
      h,
      $: id => byId[id] || null,
      load: async () => ({ sessions: termSessions, cwd: "/repo" }),
      cliName: p => p,
      hostTile: p => makeNode(p),
      href: p => p,
      activities: () => [],
      dozing: () => false,
      ongoing: s => s,
      urgent: () => false,
      pick: () => ["空闲", "idle"],
      GROUP: { idle: ["空闲", "idle"] },
      plain: s => String(s || ""),
      isHidden: () => false,
      reducedMotion: { matches: true },
      voice: { controls() {}, decorate() {}, ingest() {} },
    },
    h,
    $: id => byId[id] || null,
    L: AN.L,
    ART: { character: () => "c", figure: () => makeNode("div"), head: () => makeNode("span"), lid: () => makeNode("span"), url: p => p, version: () => 1, canWarp: () => false },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { openMenu() {}, say() {}, roomMenu() {} },
    activities: () => [],
    cliOf: () => "Codex",
    deskChar: () => "c",
    deskName: d => d.model || "desk",
    deskParts: d => ({ name: d.model || "desk", effort: "" }),
    fullHandle: d => d.key || "",
    relTime: () => "刚刚",
    clock: () => "10:00",
    fullTime: () => "2026",
    dayLabel: () => "今天",
    chatName: g => g.display_title || g.title || g.label,
    shownName: g => g.display_title || g.title || g.label,
    hostTile: p => makeNode(p),
    setText: (n, v) => { n.textContent = v; },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.replaceChildren(...nodes); },
    el,
    state,
    FOLD_PX: 320,
    FIG: 100,
    narrow: { matches: false },
    overlay: { matches: false },
    rail: { matches: false },
    reduceMotion: { matches: true },
    stored: () => null,
    store() {},
    document: {
      hidden: false,
      documentElement: { lang: "zh", dataset: {} },
      addEventListener() {},
      querySelector: () => makeNode("div"),
      querySelectorAll: () => [],
    },
    window: { addEventListener() {} },
    addEventListener() {},
    ResizeObserver: class { observe() {} },
    getComputedStyle: () => ({ getPropertyValue: () => "2", columnGap: "10px" }),
    requestAnimationFrame: fn => fn(),
    setInterval: () => 0,
    setTimeout,
    clearTimeout,
    location: { origin: "null" },
    innerWidth: 1200,
    Date,
  });
  context.contextOf = c => ({
    c,
    seatDesk: new Map([["r1|s1", "d1"], ["r1|s2", "d2"]]),
    desks: new Map(c.desks.map(d => [d.key, d])),
    drawn: new Set(c.desks.map(d => d.key)),
    aliases: new Map(),
    rooms: new Map(c.rooms.map(r => [r.id, r])),
    fullRooms: new Set(),
    sources: new Set(),
  });
  context.plain = s => String(s || "");
  context.nearBottom = () => el.tl.scrollHeight - el.tl.scrollTop - el.tl.clientHeight < 64;
  context.fullRoom = r => r.status === "running";
  context.roundText = () => "第 1 轮";
  context.followAlias = () => {};
  context.counts = () => ({ running: 0, review: 0 });
  context.renderHead = () => {};
  context.renderSideToggle = () => {};
  context.renderTicker = () => {};
  context.renderOffice = () => {};
  context.timeDriven = () => false;
  context.tickOf = () => 1;
  context.locatePlaying = () => false;

  vm.runInContext(readFileSync(`${__dirname}/../chat-topics.js`, "utf8"), context);
  vm.runInContext(readFileSync(`${__dirname}/../chat-messages.js`, "utf8"), context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);
  const chatSrc = readFileSync(`${__dirname}/../chat.js`, "utf8");
  vm.runInContext(chatSrc.slice(0, chatSrc.indexOf("for (const link of")), context);

  // Wrap applyFolds to observe exact .term-mode state and visible records on every call.
  const foldCalls = [];
  const origApplyFolds = context.applyFolds;
  context.applyFolds = (records) => {
    const list = [...records];
    foldCalls.push({
      termModeDuringFold: drawer.classList.contains("term-mode"),
      count: list.length,
      visibleCount: list.filter(r => r.more && r.el.isConnected && r.el.offsetParent).length,
    });
    return origApplyFolds(list);
  };

  const chat = {
    group: "ws-1",
    host: "claude",
    label: "Claude Code",
    desks: [
      { key: "d1", provider: "codex", model: "gpt-6.1", first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" },
      { key: "d2", provider: "claude", model: "claude-x", first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" },
    ],
    rooms: [{ id: "r1", title: "方案评审", status: "idle", seats: [{ seat: "s1", desk: "d1" }, { seat: "s2", desk: "d2" }] }],
    messages: [
      { id: "m-short", kind: "reply", desk: "d1", conversation: "c1", time: "2026-11-19T01:00:00Z", text: "SHORT_BUBBLE", speaker: { role: "member", provider: "codex", model: "gpt-6.1" }, thread: { kind: "desk", key: "d1" } },
      { id: "m-long", kind: "reply", desk: "d1", conversation: "c1", time: "2026-11-19T01:01:00Z", text: "LONG_BUBBLE ".repeat(80), speaker: { role: "member", provider: "codex", model: "gpt-6.1" }, thread: { kind: "desk", key: "d1" } },
      { id: "m-d1-room", kind: "reply", desk: "d1", conversation: "r1", round: 1, visibility: "public", time: "2026-11-19T01:01:30Z", text: "SHORT_BUBBLE", speaker: { role: "member", provider: "codex", model: "gpt-6.1" }, thread: { kind: "room", id: "r1" } },
      { id: "m-peer", kind: "reply", desk: "d2", conversation: "r1", round: 1, visibility: "public", time: "2026-11-19T01:02:00Z", text: "peer first line\npeer second line", speaker: { role: "member", provider: "claude", model: "claude-x" }, thread: { kind: "room", id: "r1" } },
      { id: "m-brief", kind: "handover", desk: "d1", conversation: "c1", time: "2026-11-19T01:03:00Z", text: "Claude Code 交出工作区", detail: "LONG_BUBBLE detail", speaker: { role: "system" }, thread: { kind: "desk", key: "d1" } },
      { id: "m-typing", kind: "typing", desk: "d1", conversation: "c1", time: "2026-11-19T01:04:00Z", text: "LONG_BUBBLE streaming", speaker: { role: "member", provider: "codex", model: "gpt-6.1" }, thread: { kind: "desk", key: "d1" } },
    ],
  };
  state.chat = chat;
  await context.ChatTerminal.refresh();

  // Start in terminal mode.
  context.renderAll(chat);
  assert.equal(drawer.classList.contains("term-mode"), true);

  // Switch from terminal mode to desk messages via openScope (single renderAll pass).
  foldCalls.length = 0;
  context.openScope({ kind: "desk", key: "d1" });
  assert.equal(drawer.classList.contains("term-mode"), false);
  assert.equal(foldCalls.length, 1, "applyFolds runs synchronously inside the same renderAll pass");
  assert.equal(foldCalls[0].termModeDuringFold, false, ".term-mode must already be removed before applyFolds runs");
  assert.equal(foldCalls[0].visibleCount, 5, "all five foldable messages are visible and measured immediately");

  // Short message: no .fold left, button hidden.
  const shortRec = state.nodes.get("m-short");
  assert.equal(shortRec.bub.classList.contains("fold"), false);
  assert.equal(shortRec.more.hidden, true);
  assert.equal(shortRec.dirty, false);

  // Long message: starts folded, button shows 展开全文, only expands on click and collapses on second click.
  const longRec = state.nodes.get("m-long");
  assert.equal(longRec.bub.classList.contains("fold"), true);
  assert.equal(longRec.more.hidden, false);
  assert.equal(longRec.more.textContent, "展开全文");
  assert.equal(longRec.more.attributes["aria-expanded"], "false");
  assert.equal(longRec.dirty, false);

  longRec.more.click();
  assert.equal(state.expanded.has("m-long"), true);
  assert.equal(longRec.bub.classList.contains("fold"), false);
  assert.equal(longRec.more.textContent, "收起");
  assert.equal(longRec.more.attributes["aria-expanded"], "true");

  longRec.more.click();
  assert.equal(state.expanded.has("m-long"), false);
  assert.equal(longRec.bub.classList.contains("fold"), true);
  assert.equal(longRec.more.textContent, "展开全文");
  assert.equal(longRec.more.attributes["aria-expanded"], "false");

  // Peer and brief messages start folded and toggle on click; typing stays unfolded without button.
  const peerRec = state.nodes.get("m-peer");
  const briefRec = state.nodes.get("m-brief");
  const typingRec = state.nodes.get("m-typing");
  assert.equal(peerRec.bub.classList.contains("fold"), true);
  assert.equal(peerRec.more.hidden, false);
  assert.equal(briefRec.bub.classList.contains("fold"), true);
  assert.equal(briefRec.more.hidden, false);
  assert.equal(typingRec.bub.classList.contains("fold"), false);
  assert.equal(typingRec.el.children.includes(typingRec.more), false);

  // A newly created non-typing message starts with .fold before applyFolds even runs (prevents flash while hidden).
  const unmeasured = context.createMessage({ id: "m-new", kind: "reply", text: "LONG_BUBBLE" });
  assert.equal(unmeasured.bub.classList.contains("fold"), true);
  assert.equal(unmeasured.more.hidden, true);

  // Direct ChatTerminal.column() transition out of term-mode also flushes dirty folds synchronously.
  drawer.classList.add("term-mode");
  longRec.bub.classList.remove("fold");
  longRec.dirty = true;
  context.ChatTerminal.readMessages();
  foldCalls.length = 0;
  context.ChatTerminal.column("ws-1", "claude");
  assert.equal(drawer.classList.contains("term-mode"), false);
  assert.ok(foldCalls.length >= 1);
  assert.equal(longRec.bub.classList.contains("fold"), true);
  assert.equal(longRec.dirty, false);

  // Switching to terminal mode while on a desk, receiving a new long message while hidden in term-mode,
  // then returning to messages via renderAll folds the new message immediately without flashing expanded.
  const termSwitchBtn = modesNode?.children?.[1];
  const msgSwitchBtn = modesNode?.children?.[0];
  assert.ok(termSwitchBtn && msgSwitchBtn);
  termSwitchBtn.click();
  assert.equal(drawer.classList.contains("term-mode"), true);

  chat.messages.push({
    id: "m-while-term", kind: "reply", desk: "d1", conversation: "c1", time: "2026-11-19T01:05:00Z",
    text: "LONG_BUBBLE ".repeat(60), speaker: { role: "member", provider: "codex", model: "gpt-6.1" }, thread: { kind: "desk", key: "d1" },
  });
  context.renderAll(chat);
  const whileTermRec = state.nodes.get("m-while-term");
  assert.equal(whileTermRec.dirty, true, "skipped while in term-mode");
  assert.equal(whileTermRec.bub.classList.contains("fold"), true, "already clamped even before measurement");

  foldCalls.length = 0;
  msgSwitchBtn.click();
  assert.equal(drawer.classList.contains("term-mode"), false);
  assert.equal(foldCalls.at(-1).termModeDuringFold, false);
  assert.equal(whileTermRec.dirty, false);
  assert.equal(whileTermRec.bub.classList.contains("fold"), true);
  assert.equal(whileTermRec.more.hidden, false);
  assert.equal(whileTermRec.more.textContent, "展开全文");

  // CSS hides ::after fade mask when .more is still [hidden] so short unmeasured bubbles never show a bottom fade.
  const chatCss = readFileSync(`${__dirname}/../chat.css`, "utf8");
  assert.match(chatCss, /\.bub\.fold:has\(~\s*\.more\[hidden\]\)::after\s*\{\s*display:\s*none;\s*\}/);
});

test("ChatGit prioritizes PR tones, hides desktop sessions and PRs when gh unavailable, and renders isolated Git/PR/jump controls", () => {
  const mkNode = (tag, attributes = {}, ...children) => {
    const node = {
      tag,
      className: tag.split(".").slice(1).join(" "),
      attributes: { ...attributes },
      hidden: Boolean(attributes.hidden),
      children: children.flat(Infinity).filter((x) => x != null && x !== false),
      listeners: {},
      style: {},
      textContent: "",
      setAttribute(k, v) { this.attributes[k] = v; },
      getAttribute(k) { return this.attributes[k] ?? null; },
      hasAttribute(k) { return Object.hasOwn(this.attributes, k); },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener(name, fn) { this.listeners[name] = fn; },
      append(...items) { this.children.push(...items.flat(Infinity).filter((x) => x != null && x !== false)); },
      replaceChildren(...items) { this.children = items.flat(Infinity).filter((x) => x != null && x !== false); },
      insertBefore(item, before) {
        const idx = this.children.indexOf(before);
        if (idx >= 0) this.children.splice(idx, 0, item);
        else this.children.push(item);
      },
      closest() { return null; },
      contains() { return false; },
    };
    node.parentNode = null;
    return node;
  };
  const jumped = [];
  const opened = [];
  const context = vm.createContext({
    AN: {
      h: mkNode,
      L: (zh) => zh,
      setText: (n, t) => { n.textContent = t; },
      setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
      relTime: () => "5 分钟前",
      at: Date.parse,
      deskName: (d) => d?.label || d?.key || "",
      post: async () => ({}),
      load: async () => ({}),
    },
    document: {
      body: { append() {} },
      createElementNS: (_ns, tag) => mkNode(tag),
      addEventListener() {},
    },
    window: { addEventListener() {}, innerWidth: 1200, innerHeight: 800 },
    ChatTerminal: {
      hasTerminal: (group) => group === "ws-term",
      derivedAll: () => [{ group: "term-9", isolation_id: "wt-1", cwd: "/repo/.agentnave-worktrees/wt-1", display_title: "派生修 Bug", label: "Claude Code", status: "running", live: true }],
    },
    go: (path) => jumped.push(path),
    openScope: (scope) => opened.push(scope),
    setTimeout,
    clearTimeout,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-git.js`, "utf8") + "\nglobalThis.ChatGit = ChatGit;", context);
  const { ChatGit } = context;

  // Priority: CI failing / conflict > open > draft > merged > closed.
  assert.equal(ChatGit.prTone({ state: "open", checks: { state: "failing" }, mergeable: "mergeable" }), "bad");
  assert.equal(ChatGit.prTone({ state: "draft", checks: { state: "passing" }, mergeable: "conflicting" }), "bad");
  assert.equal(ChatGit.prTone({ state: "open", checks: { state: "passing" }, mergeable: "mergeable" }), "open");
  assert.equal(ChatGit.prTone({ state: "draft", checks: { state: "none" }, mergeable: "mergeable" }), "draft");
  assert.equal(ChatGit.prTone({ state: "merged", checks: { state: "failing" }, mergeable: "conflicting" }), "merged");
  assert.equal(ChatGit.prTone({ state: "closed", checks: { state: "none" }, mergeable: "unknown" }), "closed");

  // CLI vs desktop eligibility.
  assert.equal(ChatGit.isCliSession({ group: "ws-cli", cli_session: true }), true);
  assert.equal(ChatGit.isCliSession({ group: "ws-term", cli_session: false }), true);
  assert.equal(ChatGit.isCliSession({ group: "term-1", terminal: 1 }), true);
  assert.equal(ChatGit.isCliSession({ group: "term-2", derived: true }), true);
  assert.equal(ChatGit.isCliSession({ group: "ws-desktop", app: "claude-desktop", cli_session: false }), false);
  assert.equal(ChatGit.isCliSession({ group: "ws-unknown" }), false);

  ChatGit._applyForTest({
    gh_status: "ready",
    terminals: {},
    workspaces: {
      "ws-cli": {
        git: { is_git: true, branch: "feat/x", changed_files: 2, additions: 10, deletions: 3 },
        prs: [
          { repo: "o/r", number: 1, title: "Merged", state: "merged", checks: { state: "passing", total: 1, passed: 1, failed: 0, pending: 0 }, url: "https://github.com/o/r/pull/1" },
          { repo: "o/r", number: 2, title: "Failing", state: "open", checks: { state: "failing", total: 2, passed: 1, failed: 1, pending: 0 }, url: "https://github.com/o/r/pull/2" },
        ],
      },
      "ws-desktop": {
        git: { is_git: true, branch: "main", changed_files: 1 },
        prs: [{ repo: "o/r", number: 3, title: "Open", state: "open", checks: { state: "passing", total: 1 }, url: "https://github.com/o/r/pull/3" }],
      },
    },
    isolated: {
      "wt-1": {
        git: { is_git: true, branch: "agentnave/wt-1", default_branch: "main", ahead_of_default: 2, changed_files: 1, additions: 5, deletions: 2 },
        pr: { repo: "o/r", number: 12, title: "PR 12", state: "open", checks: { state: "passing", total: 1, passed: 1, failed: 0, pending: 0 }, url: "https://github.com/o/r/pull/12" },
        merged_pr: null,
      },
    },
  });

  const cliRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(cliRec, { group: "ws-cli", cli_session: true });
  assert.equal(cliRec.prBadge.hidden, false);
  assert.equal(cliRec.prBadge.attributes["data-tone"], "bad");

  const deskRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(deskRec, { group: "ws-desktop", app: "claude-desktop", cli_session: false });
  assert.equal(deskRec.prBadge.hidden, true);

  // Isolated card nodes: git status + PR + clickable derived terminal jump.
  const isoRes = ChatGit.isolatedNodes({ id: "wt-1", path: "/repo/.agentnave-worktrees/wt-1", branch: "agentnave/wt-1", status: "idle" }, { desks: new Map() });
  assert.equal(isoRes.hasActiveTerminal, true);
  const userNode = isoRes.nodes.find((n) => n.className === "iso-user");
  assert.ok(userNode);
  userNode.children[0].listeners.click();
  assert.deepEqual(jumped, ["/chat/term-9"]);

  // When gh is not installed, PRs are hidden while Git status remains visible.
  ChatGit._applyForTest({
    gh_status: "not_installed",
    terminals: {},
    workspaces: {
      "ws-cli": {
        git: { is_git: true, branch: "feat/x", changed_files: 2, additions: 10, deletions: 3 },
        prs: [{ repo: "o/r", number: 2, title: "Failing", state: "open", checks: { state: "failing", total: 1 }, url: "https://github.com/o/r/pull/2" }],
      },
    },
    isolated: {},
  });
  ChatGit.decorateItem(cliRec, { group: "ws-cli", cli_session: true });
  assert.equal(cliRec.prBadge.hidden, false);
  assert.equal(cliRec.prBadge.attributes["data-tone"], "git");
});

test("ChatGit opens only https://github.com/ links via /app/git-pr/open-url and renders status without write actions", async () => {
  const mkNode = (tag, attributes = {}, ...children) => {
    const node = {
      tag,
      className: tag.split(".").slice(1).join(" "),
      attributes: { ...attributes },
      hidden: Boolean(attributes.hidden),
      disabled: Boolean(attributes.disabled),
      value: attributes.value ?? "",
      checked: Boolean(attributes.checked),
      isConnected: false,
      children: children.flat(Infinity).filter((x) => x != null && x !== false),
      listeners: {},
      style: {},
      textContent: "",
      setAttribute(k, v) { this.attributes[k] = v; },
      getAttribute(k) { return this.attributes[k] ?? null; },
      hasAttribute(k) { return Object.hasOwn(this.attributes, k); },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
      fire(name, ev = {}) {
        const event = { preventDefault() {}, stopPropagation() {}, ...ev };
        for (const fn of this.listeners[name] || []) fn(event);
      },
      append(...items) { this.children.push(...items.flat(Infinity).filter((x) => x != null && x !== false)); },
      replaceChildren(...items) { this.children = items.flat(Infinity).filter((x) => x != null && x !== false); },
      insertBefore(item, before) {
        const idx = this.children.indexOf(before);
        if (idx >= 0) this.children.splice(idx, 0, item);
        else this.children.push(item);
      },
      closest() { return null; },
      contains(x) { return x === this || this.children.some((c) => c?.contains?.(x)); },
      getBoundingClientRect() { return { left: 20, right: 120, top: 40, bottom: 60, width: 100, height: 20 }; },
      focus() {},
      showModal() { this.open = true; },
      close() { this.open = false; },
    };
    return node;
  };
  const findAll = (root, pred, out = []) => {
    if (!root || typeof root !== "object") return out;
    if (pred(root)) out.push(root);
    for (const c of root.children || []) findAll(c, pred, out);
    return out;
  };

  const posts = [];
  const toasts = [];
  const bodyChildren = [];
  let postImpl = async (path, payload) => {
    posts.push([path, payload]);
    return { ok: true };
  };
  const context = vm.createContext({
    AN: {
      h: mkNode,
      L: (zh) => zh,
      setText: (n, t) => { n.textContent = t; },
      setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
      relTime: () => "5 分钟前",
      at: Date.parse,
      deskName: (d) => d?.label || d?.key || "",
      post: (...args) => postImpl(...args),
      load: async () => ({}),
    },
    Actions: { say: (msg, bad) => toasts.push([msg, bad]) },
    document: {
      body: { append(...nodes) { for (const n of nodes) { if (n) n.isConnected = true; bodyChildren.push(n); } } },
      createElementNS: (_ns, tag) => mkNode(tag),
      addEventListener() {},
    },
    window: { addEventListener() {}, innerWidth: 1200, innerHeight: 800 },
    setTimeout,
    clearTimeout,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-git.js`, "utf8") + "\nglobalThis.ChatGit = ChatGit;", context);
  const { ChatGit } = context;

  // openUrl strictly allows https://github.com/
  assert.equal(await ChatGit.openUrl("https://evil.example/o/r/pull/1"), false);
  assert.equal(posts.length, 0);
  assert.equal(await ChatGit.openUrl("https://github.com/o/r/pull/2"), true);
  assert.deepEqual(JSON.parse(JSON.stringify(posts[0])), ["/app/git-pr/open-url", { url: "https://github.com/o/r/pull/2" }]);

  ChatGit._applyForTest({
    gh_status: "ready",
    terminals: {},
    workspaces: {
      "ws-idle": {
        working: false,
        git: { is_git: true, branch: "feat/x", default_branch: "main", upstream: "origin/feat/x", ahead: 1, behind: 1, changed_files: 2, additions: 10, deletions: 3, github_repo: "o/r" },
        prs: [
          {
            repo: "o/r", number: 2, title: "Fix link", state: "open", head_ref_name: "other/branch", base_ref_name: "main",
            checks: { state: "failing", total: 2, passed: 1, failed: 1, pending: 0, checks_url: "https://ci.external/run/1" },
            mergeable: "mergeable", url: "https://github.com/o/r/pull/2",
          },
        ],
      },
      "ws-busy": {
        working: true,
        git: { is_git: true, branch: "feat/y", default_branch: "main", upstream: "origin/feat/y", ahead: 1, behind: 0, changed_files: 1, github_repo: "o/r" },
        prs: [],
      },
    },
    isolated: {},
  });

  // Open hover card by clicking badge -> pins card and renders PR link and GitHub-fallback CI link.
  const idleRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(idleRec, { group: "ws-idle", cli_session: true });
  idleRec.prBadge.fire("click");
  const card = bodyChildren.find((n) => n.className === "git-pr-card");
  assert.ok(card && !card.hidden);
  const prLink = findAll(card, (n) => n.className === "gp-pr-head")[0];
  assert.equal(prLink.attributes.href, "https://github.com/o/r/pull/2");
  prLink.fire("click");
  await Promise.resolve();
  assert.deepEqual(JSON.parse(JSON.stringify(posts.at(-1))), ["/app/git-pr/open-url", { url: "https://github.com/o/r/pull/2" }]);

  // External CI URL falls back to PR's GitHub checks URL.
  const ciChip = findAll(card, (n) => n.tag.startsWith("a.gp-chip"))[0];
  assert.equal(ciChip.attributes.href, "https://github.com/o/r/pull/2/checks");

  // Read-only: no commit/pull/push/PR/merge controls are rendered for idle or busy CLI sessions.
  assert.equal(findAll(card, (n) => n.className.includes("gp-op-btn") || n.className.includes("gp-pr-merge")).length, 0);

  // Busy CLI is shown as a status tag only.
  const busyRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(busyRec, { group: "ws-busy", cli_session: true });
  busyRec.prBadge.fire("click");
  assert.equal(findAll(card, (n) => n.className.includes("gp-tag-busy")).length, 1);
});

test("ChatGit queuedReason preserves manual priority over focus and hover", async () => {
  const mkNode = (tag, attributes = {}, ...children) => ({
    tag,
    className: tag.split(".").slice(1).join(" "),
    attributes: { ...attributes },
    hidden: Boolean(attributes.hidden),
    children: children.flat(Infinity).filter((x) => x != null && x !== false),
    listeners: {},
    style: {},
    setAttribute(k, v) { this.attributes[k] = v; },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    append() {},
    replaceChildren() {},
  });
  const refreshReasons = [];
  let releaseFirst;
  const timers = [];
  const winListeners = {};
  const context = vm.createContext({
    AN: {
      h: mkNode,
      L: (zh) => zh,
      setText() {},
      setAttr() {},
      relTime: () => "",
      at: Date.parse,
      deskName: () => "",
      post: (path, payload) => {
        if (path === "/app/git-pr/refresh") {
          refreshReasons.push(payload.reason);
          if (refreshReasons.length === 1) {
            return new Promise((resolve) => {
              releaseFirst = () => resolve({ gh_status: "ready", terminals: {}, workspaces: {}, isolated: {}, updated_at: 1 });
            });
          }
          return Promise.resolve({ gh_status: "ready", terminals: {}, workspaces: {}, isolated: {}, updated_at: 2 });
        }
        return Promise.resolve({});
      },
      load: async () => ({}),
    },
    document: { body: { append() {} }, createElementNS: (_ns, tag) => mkNode(tag), addEventListener() {} },
    window: {
      addEventListener: (name, fn) => { (winListeners[name] ||= []).push(fn); },
      innerWidth: 1200,
      innerHeight: 800,
    },
    setTimeout: (fn) => { timers.push(fn); return timers.length; },
    clearTimeout: (id) => { if (id > 0) timers[id - 1] = null; },
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-git.js`, "utf8") + "\nglobalThis.ChatGit = ChatGit;", context);
  const flushTimers = () => {
    while (timers.some(Boolean)) {
      const batch = timers.splice(0);
      for (const fn of batch) if (fn) fn();
    }
  };

  // Start an in-flight focus refresh.
  context.ChatGit.refresh("focus");
  flushTimers();
  assert.deepEqual(refreshReasons, ["focus"]);

  // Queue manual, then fire focus (lower priority) while first request is still in flight.
  context.ChatGit.refresh("manual");
  flushTimers();
  winListeners.focus.forEach((fn) => fn());
  flushTimers();

  // Finish the first request; follow-up refresh must still run with reason "manual".
  releaseFirst();
  await new Promise((r) => setImmediate(r));
  flushTimers();
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(refreshReasons, ["focus", "manual"]);
});

test("isolatedConfirmBody notes merged PR instead of unmerged-commit warning and falls back when gh has no match", () => {
  const h = (tag, attributes = {}, ...children) => ({
    tag, attributes, children: children.flat(Infinity).filter((x) => x != null && x !== false),
    classList: { toggle() {} }, listeners: {}, textContent: "",
    replaceChildren(...items) { this.children = items; },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    append(...items) { this.children.push(...items); },
    querySelector() { return { focus() {} }; },
    focus() {},
    showModal() { this.open = true; },
    close() { this.open = false; },
  });
  let dialogNode;
  let inspectReply;
  const context = vm.createContext({
    AN: {
      h, $: () => null, L: (zh) => zh, load: async () => ({}), chatName: () => "",
      post: async () => inspectReply,
    },
    document: { body: { append(...nodes) { for (const n of nodes) if (n.tag === "dialog.act") dialogNode = n; } }, addEventListener() {} },
    window: { addEventListener() {} },
    state: { groupsData: [], order: [] },
    refresh: async () => {},
    renderList() {}, go() {}, openScope() {},
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(readFileSync(`${__dirname}/../actions.js`, "utf8") + "\nglobalThis.Actions = Actions;", context);

  // Squash-merged via PR #42: shows calm merged_pr note, no unmerged-commit warning.
  inspectReply = {
    status: "confirm", id: "wt-1", path: "/p", branch: "agentnave/wt-1", confirmation: "tok",
    unique_commits: true, unmerged_commits: ["abc feat"], unmerged_total: 1,
    merged_pr: { number: 42, repo: "o/r", title: "Merged", url: "https://github.com/o/r/pull/42" },
  };
  const p1 = context.Actions.deleteIsolated("s-1", { id: "wt-1", path: "/p", branch: "agentnave/wt-1" }, { disabled: false });
  return new Promise((r) => setImmediate(r)).then(() => {
    const mergedNote = dialogNode.children.find((n) => n.tag === "p.act-note.iso-merged");
    assert.ok(mergedNote, "should display merged PR note");
    const unmergedWarn = dialogNode.children.find((n) => n.tag === "p.act-note.warn" && String(n.children[0]).includes("尚未合入"));
    assert.equal(unmergedWarn, undefined, "must not warn about unmerged commits when merged_pr is confirmed");
    dialogNode.children.at(-1).children[0].listeners.click();
    return p1;
  }).then(() => {
    // Fallback without merged_pr: shows existing unmerged-commit warning.
    inspectReply = {
      status: "confirm", id: "wt-2", path: "/p2", branch: "agentnave/wt-2", confirmation: "tok2",
      unique_commits: true, unmerged_commits: ["def feat"], unmerged_total: 1,
      merged_pr: null,
    };
    const p2 = context.Actions.deleteIsolated("s-1", { id: "wt-2", path: "/p2", branch: "agentnave/wt-2" }, { disabled: false });
    return new Promise((r) => setImmediate(r)).then(() => {
      const mergedNote = dialogNode.children.find((n) => n.tag === "p.act-note.iso-merged");
      assert.equal(mergedNote, undefined);
      const unmergedWarn = dialogNode.children.find((n) => n.tag === "p.act-note.warn" && String(n.children[0]).includes("尚未合入"));
      assert.ok(unmergedWarn, "should fall back to unmerged commit warning when gh has no match");
      dialogNode.children.at(-1).children[0].listeners.click();
      return p2;
    });
  });
});

test("ChatGit never renders literal null in hover card or clean git badge", () => {
  // Mimic native DOM Element.prototype.replaceChildren, which stringifies null/undefined into Text nodes.
  const domReplaceChildren = (target, items) => {
    target.children = items.map((x) => (x && typeof x === "object" ? x : String(x)));
  };
  const mkNode = (tag, attributes = {}, ...children) => ({
    tag,
    className: tag.split(".").slice(1).join(" "),
    attributes: { ...attributes },
    hidden: Boolean(attributes.hidden),
    isConnected: false,
    children: children.flat(Infinity).filter((x) => x != null && x !== false),
    listeners: {},
    style: {},
    textContent: "",
    setAttribute(k, v) { this.attributes[k] = v; },
    getAttribute(k) { return this.attributes[k] ?? null; },
    hasAttribute(k) { return Object.hasOwn(this.attributes, k); },
    removeAttribute(k) { delete this.attributes[k]; },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    fire(name, ev = {}) {
      const event = { preventDefault() {}, stopPropagation() {}, ...ev };
      for (const fn of this.listeners[name] || []) fn(event);
    },
    append(...items) { this.children.push(...items.flat(Infinity).filter((x) => x != null && x !== false)); },
    replaceChildren(...items) { domReplaceChildren(this, items); },
    insertBefore(item, before) {
      const idx = this.children.indexOf(before);
      if (idx >= 0) this.children.splice(idx, 0, item);
      else this.children.push(item);
    },
    closest() { return null; },
    contains(x) { return x === this || this.children.some((c) => typeof c === "object" && c?.contains?.(x)); },
    getBoundingClientRect() { return { left: 20, right: 120, top: 40, bottom: 60, width: 100, height: 20 }; },
  });
  const hasLiteralNull = (node) => {
    if (node === "null" || node === null || node === undefined) return true;
    if (typeof node !== "object") return false;
    if (String(node.textContent) === "null") return true;
    return (node.children || []).some(hasLiteralNull);
  };
  const bodyChildren = [];
  const context = vm.createContext({
    AN: {
      h: mkNode,
      L: (zh) => zh,
      setText: (n, t) => { n.textContent = t; },
      setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
      relTime: () => "5 分钟前",
      at: Date.parse,
      deskName: (d) => d?.label || d?.key || "",
      post: async () => ({}),
      load: async () => ({}),
    },
    document: {
      body: { append(...nodes) { for (const n of nodes) { if (n) n.isConnected = true; bodyChildren.push(n); } } },
      createElementNS: (_ns, tag) => mkNode(tag),
      addEventListener() {},
    },
    window: { addEventListener() {}, innerWidth: 1200, innerHeight: 800 },
    setTimeout,
    clearTimeout,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-git.js`, "utf8") + "\nglobalThis.ChatGit = ChatGit;", context);
  const { ChatGit } = context;

  ChatGit._applyForTest({
    gh_status: "ready",
    terminals: {},
    workspaces: {
      "ws-git-only": {
        git: { is_git: true, branch: "feature/local", changed_files: 0, additions: 0, deletions: 0, ahead_of_default: 0 },
        prs: [],
      },
      "ws-prs-only": {
        git: null,
        prs: [{ repo: "o/r", number: 509, title: "PR only", state: "open", checks: { state: "passing", total: 1, passed: 1 }, url: "https://github.com/o/r/pull/509" }],
      },
    },
    isolated: {},
  });

  // Clean git-only branch badge must not contain "null".
  const gitOnlyRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(gitOnlyRec, { group: "ws-git-only", cli_session: true });
  assert.equal(hasLiteralNull(gitOnlyRec.prBadge), false);

  // Hover card with git only (no PRs) must not render "null" at bottom.
  gitOnlyRec.prBadge.fire("click");
  const card = bodyChildren.find((n) => n.className === "git-pr-card");
  assert.ok(card && !card.hidden);
  assert.equal(hasLiteralNull(card), false);

  // Hover card with PRs only (git: null) must not render "null" at top.
  const prsOnlyRec = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(prsOnlyRec, { group: "ws-prs-only", cli_session: true });
  prsOnlyRec.prBadge.fire("click");
  assert.equal(hasLiteralNull(card), false);
});

test("renderMarkdown renders http(s) as external anchors, file:// as text without raw syntax, and rejects unsafe schemes", () => {
  const AN = common(false);
  const makeNode = (tagName = "div") => {
    const classSet = new Set();
    const [tag, ...initClasses] = String(tagName).split(".");
    for (const c of initClasses) classSet.add(c);
    const node = {
      tagName: tag.toUpperCase(),
      attributes: {},
      children: [],
      classList: {
        contains: (c) => classSet.has(c),
      },
      setAttribute(k, v) { this.attributes[k] = String(v); },
      append(...items) {
        for (const item of items) if (item != null && item !== false) this.children.push(item);
      },
      replaceChildren(...items) { this.children = items.filter((x) => x != null && x !== false); },
    };
    return node;
  };
  const h = (tag, attrs = {}, ...kids) => {
    const node = makeNode(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v != null && v !== false) node.setAttribute(k, v === true ? "" : v);
    }
    node.append(...kids.flat(Infinity));
    return node;
  };
  const findAll = (root, pred, out = []) => {
    if (!root || typeof root !== "object") return out;
    if (pred(root)) out.push(root);
    for (const child of root.children || []) findAll(child, pred, out);
    return out;
  };
  const collectText = (node) => {
    if (typeof node === "string") return node;
    return (node?.children || []).map(collectText).join("");
  };

  const context = vm.createContext({ AN, h, L: AN.L, Array, Object, String, Number, Boolean, Date, JSON, Map, Set, RegExp });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-messages.js`, "utf8")}\nglobalThis.renderMarkdown = renderMarkdown;`, context);

  const md = makeNode("div.md");
  context.renderMarkdown(
    md,
    "见 [`chat.js`](file:///Users/tim/chat.js#L10) 和 [文档](https://example.com/a_(b)) 以及 [bad](javascript:alert(1)) 与 `[literal](https://x)`",
  );
  const fullText = collectText(md);
  assert.equal(fullText.includes("file:///Users/tim/chat.js"), false);
  assert.match(fullText, /chat\.js/);
  assert.match(fullText, /\[literal\]\(https:\/\/x\)/);

  const anchors = findAll(md, (n) => n.tagName === "A");
  assert.equal(anchors.length, 1);
  assert.equal(anchors[0].attributes.href, "https://example.com/a_(b)");
  assert.equal(anchors[0].attributes.target, "_blank");
  assert.equal(anchors[0].attributes.rel, "noopener noreferrer");
  assert.equal(collectText(anchors[0]), "文档");

  const fileSpans = findAll(md, (n) => n.classList?.contains("md-file"));
  assert.equal(fileSpans.length, 1);
  assert.equal(fileSpans[0].attributes.title, "file:///Users/tim/chat.js#L10");
  assert.equal(findAll(fileSpans[0], (n) => n.tagName === "CODE").length, 1);

  // An unclosed link must not backtrack exponentially.
  const started = Date.now();
  context.renderMarkdown(makeNode("div.md"), `[a](${"a".repeat(5000)} `);
  assert.ok(Date.now() - started < 500);

  const chatCss = readFileSync(`${__dirname}/../chat.css`, "utf8");
  assert.match(chatCss, /\.md a\s*\{/);
  assert.match(chatCss, /\.md \.md-file\s*\{/);
});

test("ChatGit delays hover open by 400ms, cancels on leave/hide/detach, switches cleanly, and keeps click/keyboard immediate", () => {
  const docListeners = {};
  const bodyChildren = [];
  const mkNode = (tag, attributes = {}, ...children) => ({
    tag,
    tagName: tag.split(".")[0].toUpperCase(),
    className: tag.split(".").slice(1).join(" "),
    attributes: { ...attributes },
    hidden: Boolean(attributes.hidden),
    isConnected: false,
    children: children.flat(Infinity).filter((x) => x != null && x !== false),
    listeners: {},
    style: {},
    textContent: "",
    setAttribute(k, v) { this.attributes[k] = v; },
    getAttribute(k) { return this.attributes[k] ?? null; },
    hasAttribute(k) { return Object.hasOwn(this.attributes, k); },
    removeAttribute(k) { delete this.attributes[k]; },
    addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); },
    fire(name, ev = {}) {
      const event = { target: this, preventDefault() {}, stopPropagation() {}, ...ev };
      for (const fn of this.listeners[name] || []) fn(event);
    },
    append(...items) { this.children.push(...items.flat(Infinity).filter((x) => x != null && x !== false)); },
    replaceChildren(...items) { this.children = items.flat(Infinity).filter((x) => x != null && x !== false); },
    insertBefore(item, before) {
      const idx = this.children.indexOf(before);
      if (idx >= 0) this.children.splice(idx, 0, item);
      else this.children.push(item);
    },
    closest() { return null; },
    contains(x) { return x === this || this.children.some((c) => typeof c === "object" && c?.contains?.(x)); },
    getBoundingClientRect() { return { left: 20, right: 120, top: 40, bottom: 60, width: 100, height: 20 }; },
  });

  let now = 1000;
  let nextTimerId = 1;
  const scheduled = new Map();
  const refreshReasons = [];
  const context = vm.createContext({
    AN: {
      h: mkNode,
      L: (zh) => zh,
      setText: (n, t) => { n.textContent = t; },
      setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
      relTime: () => "5 分钟前",
      at: Date.parse,
      deskName: () => "",
      post: async (path, payload) => {
        if (path === "/app/git-pr/refresh") refreshReasons.push(payload.reason);
        return {};
      },
      load: async () => ({}),
    },
    document: {
      body: { append(...nodes) { for (const n of nodes) { if (n) n.isConnected = true; bodyChildren.push(n); } } },
      createElementNS: (_ns, tag) => mkNode(tag),
      addEventListener(name, fn) { (docListeners[name] ||= []).push(fn); },
    },
    window: { addEventListener() {}, innerWidth: 1200, innerHeight: 800 },
    Date: Object.assign(function (...args) { return new Date(...args); }, { now: () => now, parse: Date.parse }),
    setTimeout: (fn, ms = 0) => {
      const id = nextTimerId++;
      scheduled.set(id, { id, fn, at: now + ms, delay: ms });
      return id;
    },
    clearTimeout: (id) => { scheduled.delete(id); },
  });
  const advanceBy = (ms) => {
    const target = now + ms;
    while (true) {
      const due = [...scheduled.values()].filter((t) => t.at <= target).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (!due) break;
      scheduled.delete(due.id);
      now = due.at;
      due.fn();
    }
    now = target;
  };

  vm.runInContext(readFileSync(`${__dirname}/../chat-git.js`, "utf8") + "\nglobalThis.ChatGit = ChatGit;", context);
  const { ChatGit } = context;

  ChatGit._applyForTest({
    gh_status: "ready",
    terminals: {},
    workspaces: {
      "ws-a": { git: { is_git: true, branch: "feat/a", changed_files: 1 }, prs: [] },
      "ws-b": { git: { is_git: true, branch: "feat/b", changed_files: 2 }, prs: [] },
    },
    isolated: {},
  });

  const recA = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  const recB = { el: mkNode("a.gi"), top: mkNode("span.gi-top") };
  ChatGit.decorateItem(recA, { group: "ws-a", cli_session: true });
  ChatGit.decorateItem(recB, { group: "ws-b", cli_session: true });
  recA.prBadge.isConnected = true;
  recB.prBadge.isConnected = true;

  // Hovering briefly and leaving before 400ms never opens the card or triggers refresh.
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  const card = bodyChildren.find((n) => n.className === "git-pr-card");
  assert.equal(card, undefined);
  advanceBy(399);
  assert.equal(refreshReasons.length, 0);
  recA.prBadge.fire("pointerleave");
  advanceBy(50);
  assert.equal(bodyChildren.find((n) => n.className === "git-pr-card"), undefined);
  assert.equal(refreshReasons.length, 0);

  // Touch pointerenter does not open on hover.
  recA.prBadge.fire("pointerenter", { pointerType: "touch" });
  advanceBy(500);
  assert.equal(bodyChildren.find((n) => n.className === "git-pr-card"), undefined);

  // Staying 400ms opens card A and triggers hover refresh.
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  advanceBy(399);
  assert.equal(bodyChildren.find((n) => n.className === "git-pr-card"), undefined);
  advanceBy(1);
  const mountedCard = bodyChildren.find((n) => n.className === "git-pr-card");
  assert.ok(mountedCard && !mountedCard.hidden);
  assert.match(JSON.stringify(mountedCard.children), /feat\/a/);
  assert.deepEqual(refreshReasons, ["hover"]);

  // Moving from badge into open card keeps it open; moving back to badge A also keeps it open.
  recA.prBadge.fire("pointerleave");
  mountedCard.fire("pointerenter");
  advanceBy(500);
  assert.equal(mountedCard.hidden, false);
  mountedCard.fire("pointerleave");
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  advanceBy(500);
  assert.equal(mountedCard.hidden, false);

  // Moving from open card A to badge B closes card A at 360ms and opens card B at 400ms.
  recA.prBadge.fire("pointerleave");
  recB.prBadge.fire("pointerenter", { pointerType: "mouse" });
  advanceBy(359);
  assert.equal(mountedCard.hidden, false);
  assert.match(JSON.stringify(mountedCard.children), /feat\/a/);
  advanceBy(1);
  assert.equal(mountedCard.hidden, true);
  advanceBy(39);
  assert.equal(mountedCard.hidden, true);
  advanceBy(1);
  assert.equal(mountedCard.hidden, false);
  assert.match(JSON.stringify(mountedCard.children), /feat\/b/);

  // Escape (hideCard) cancels a pending hover open timer.
  recB.prBadge.fire("pointerleave");
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  for (const fn of docListeners.keydown || []) fn({ key: "Escape" });
  assert.equal(mountedCard.hidden, true);
  advanceBy(500);
  assert.equal(mountedCard.hidden, true);

  // Hiding or detaching badge cancels pending open timer.
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  recA.prBadge.hidden = true;
  advanceBy(500);
  assert.equal(mountedCard.hidden, true);
  recA.prBadge.hidden = false;

  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  recA.prBadge.isConnected = false;
  advanceBy(500);
  assert.equal(mountedCard.hidden, true);
  recA.prBadge.isConnected = true;

  // Click and Enter/Space open immediately and cancel pending timer.
  recA.prBadge.fire("pointerenter", { pointerType: "mouse" });
  recB.prBadge.fire("click");
  assert.equal(mountedCard.hidden, false);
  assert.match(JSON.stringify(mountedCard.children), /feat\/b/);
  advanceBy(500);
  assert.match(JSON.stringify(mountedCard.children), /feat\/b/, "cancelled pending A timer must not overwrite B");
  recB.prBadge.fire("click");
  assert.equal(mountedCard.hidden, true);
  recA.prBadge.fire("keydown", { key: "Enter" });
  assert.equal(mountedCard.hidden, false);
  assert.match(JSON.stringify(mountedCard.children), /feat\/a/);
});

test("GROUP.waiting localizes derived waiting status with review tone without changing urgent()", () => {
  const zh = common(false);
  const en = common(true);
  assert.deepEqual([...zh.pick(zh.GROUP, "waiting")], ["等待确认", "review"]);
  assert.deepEqual([...en.pick(en.GROUP, "waiting")], ["Needs attention", "review"]);
  assert.equal(zh.urgent("waiting"), false);
  assert.equal(zh.urgent("review"), true);
  assert.equal(zh.urgent("running"), true);
});

test("derived lifecycle: sidebar status/duration/fallback, boss summary, shown query, wake/stop/close/confirmStop, and menus", async () => {
  const makeNode = (tag = "div") => {
    const cls = new Set(String(tag).split(".").slice(1));
    const n = {
      tagName: String(tag).split(".")[0].toUpperCase(),
      hidden: false,
      disabled: false,
      textContent: "",
      title: "",
      className: "",
      dataset: {},
      attributes: {},
      children: [],
      listeners: {},
      style: { setProperty() {}, removeProperty() {} },
      classList: {
        set: cls,
        add: (...names) => names.forEach(x => cls.add(x)),
        remove: (...names) => names.forEach(x => cls.delete(x)),
        toggle: (name, on) => (on === undefined ? (cls.has(name) ? (cls.delete(name), false) : (cls.add(name), true)) : on ? (cls.add(name), true) : (cls.delete(name), false)),
        contains: name => cls.has(name),
      },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = String(v); },
      getAttribute(k) { return this.attributes[k] ?? null; },
      hasAttribute(k) { return k in this.attributes; },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      replaceChildren(...nodes) { this.children = nodes.flat().filter(Boolean); },
      append(...nodes) { this.children.push(...nodes.flat().filter(Boolean)); },
      after(...nodes) { this._after = nodes; },
      before() {},
      before() {},
      querySelector() { return null; },
      querySelectorAll() { return []; },
      scrollIntoView() {},
      closest: () => null,
      focus() {},
      remove() { this.removed = true; },
    };
    return n;
  };
  const h = (tag, attrs = {}, ...children) => {
    const node = makeNode(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "hidden") node.hidden = Boolean(v);
      else node.setAttribute(k, v === true ? "" : v);
    }
    node.children = children.flat(Infinity).filter(x => x != null && x !== false);
    if (node.children.every(x => typeof x === "string")) node.textContent = node.children.join("");
    return node;
  };

  let lang = "zh";
  let docHidden = false;
  let nowSec = 1_700_000_000;
  let sessions = [
    { id: 1, provider: "claude", cwd: "/repo", exit: null, active: true, workspaces: ["ws-1"], project_key: "/repo", project_label: "repo" },
    { id: 2, provider: "codex", cwd: "/repo/pending", exit: null, active: false, workspaces: [], project_key: "/repo", project_label: "repo" },
    { id: 10, provider: "claude", cwd: "/repo/w1", exit: null, derived: true, derived_state: "starting", derived_since: nowSec - 10, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t10", native: "t10", resumable: true, has_draft: false, workspaces: [], display_title: "启动项" },
    { id: 11, provider: "codex", cwd: "/repo/w2", exit: null, derived: true, derived_state: "running", derived_since: nowSec - 125, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t11", native: "t11", resumable: true, has_draft: false, workspaces: [], display_title: "运行项" },
    { id: 12, provider: "codex", cwd: "/repo/w3", exit: null, derived: true, derived_state: "waiting", derived_since: nowSec - 30, blocked_kind: "permission", parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t12", native: "t12", resumable: true, has_draft: true, workspaces: [], display_title: "确认项" },
    { id: 13, provider: "antigravity", cwd: "/repo/w4", exit: null, derived: true, derived_state: "uncollected", derived_since: nowSec - 60, pending_results: 2, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t13", native: "t13", resumable: true, has_draft: false, workspaces: [], display_title: "待收项" },
    { id: 14, provider: "claude", cwd: "/repo/w5", exit: null, derived: true, derived_state: "idle", derived_since: nowSec - 15, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t14", native: "t14", resumable: true, has_draft: false, workspaces: [], display_title: "空闲项" },
    { id: 15, provider: "codex", cwd: "/repo/w6", exit: 0, derived: true, derived_state: "dormant", derived_since: nowSec - 300, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t15", native: "t15", resumable: true, has_draft: false, workspaces: [], display_title: "休眠项" },
    { id: 16, provider: "codex", cwd: "/repo/w7", exit: 0, derived: true, derived_state: "exited", derived_since: nowSec - 400, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:t16", native: "t16", resumable: false, has_draft: false, workspaces: [], display_title: "结束项" },
    // Legacy backend fallback (no derived_state)
    { id: 17, provider: "claude", cwd: "/repo/w8", exit: null, derived: true, derived_busy: true, parent_terminal_id: 1, parent_workspace_id: "ws-1", workspaces: [], display_title: "旧运行" },
    { id: 18, provider: "claude", cwd: "/repo/w9", exit: 0, derived: true, derived_busy: false, parent_terminal_id: 1, parent_workspace_id: "ws-1", workspaces: [], display_title: "旧结束" },
  ];

  const loadUrls = [];
  const postCalls = [];
  const says = [];
  const asks = [];
  const menus = [];
  const navigated = [];
  let nextAsk = { value: "stop" };
  const visibility = [];
  let wakeFail = false;
  let wakeGate = null; // holds a wake / open-derived response until released
  let elsewhere = false; // the first attach is refused: the chat is open in another app

  const drawer = makeNode("aside");
  const drAll = makeNode("button");
  const listNode = makeNode("div");
  const appNode = makeNode("div");
  const els = new Map([["app", appNode], ["list", listNode], ["head", makeNode("header")], ["office", makeNode("div")], ["floor", makeNode("div")], ["boss", makeNode("section")], ["desks", makeNode("div")], ["derived-zone", makeNode("section")], ["derived-desks", makeNode("div")], ["derived-count", makeNode("span")], ["rooms", makeNode("div")], ["fx", makeNode("div")], ["desks-count", makeNode("span")], ["ticker", makeNode("footer")], ["tick", makeNode("span")], ["drawer", drawer], ["conv-toggle", makeNode("button")], ["side-toggle", makeNode("button")], ["dr-ico", makeNode("span")], ["dr-title", makeNode("span")], ["dr-sub", makeNode("span")], ["dr-all", drAll], ["timeline", makeNode("div")], ["tl-inner", makeNode("div")], ["blank", makeNode("div")], ["jump", makeNode("button")], ["banner", makeNode("div")]]);

  const AN = {
    h, $: id => els.get(id) || makeNode(id),
    L: (zh, en) => (lang === "en" ? en : zh),
    load: async (url) => { loadUrls.push(url); return { sessions: structuredClone(sessions), cwd: "/repo" }; },
    href: p => p,
    GROUP: { running: ["运行中", "run"], review: ["待审核", "review"], waiting: ["等待确认", "review"], failed: ["有失败", "bad"], idle: ["空闲", "idle"] },
    pick: (tbl, k) => tbl[k] || [k || "—", "idle"],
    urgent: s => s === "running" || s === "review",
    avatar: () => makeNode("span"),
    colorClass: () => "c0",
    relTime: () => "刚刚",
    clock: () => "10:00",
    fullTime: () => "2026",
    dayLabel: () => "今天",
    setText: (n, v) => { n.textContent = v == null ? "" : String(v); },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.children = [...nodes]; },
    stickyOrder: items => items,
    poll() {},
    plain: x => x,
    art: { character: () => "c" },
    hostTile: p => makeNode(p),
    cliName: p => ({ claude: "Claude Code", codex: "Codex", antigravity: "Antigravity" }[p] || p),
    deskName: () => "",
    deskParts: () => [],
    handleOf: () => "",
    fullHandle: () => "",
    cliOf: () => "",
    chatName: g => g.display_title || g.title || g.group,
    shownName: g => g.display_title || g.title || g.group,
    activities: () => [],
    ongoing: s => s || "idle",
    isHidden: () => docHidden,
    onVisibility: fn => visibility.push(fn),
    reducedMotion: { matches: false },
  };
  const FakeDate = class extends Date {
    constructor(...args) { super(...(args.length ? args : [nowSec * 1000])); }
    static now() { return nowSec * 1000; }
    static parse(s) { return Date.parse(s); }
  };
  const createdFrames = [];
  const context = vm.createContext({
    AN,
    URLSearchParams,
    window: null,
    document: { addEventListener() {}, documentElement: { lang: "zh", dataset: {} }, get hidden() { return docHidden; } },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    location: { search: "", pathname: "/", hash: "", origin: "null" },
    history: { pushState() {}, replaceState() {} },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    addEventListener() {},
    requestAnimationFrame() {},
    setTimeout, clearTimeout, Date: FakeDate,
    Actions: {
      picked: new Set(),
      pick: () => false,
      workspaceMenu() {},
      projectMenu() {},
      openMenu: (_e, items) => menus.push(items),
      say: (msg, bad) => says.push([msg, bad]),
      ask: async (opts) => { asks.push(opts); return nextAsk; },
    },
    fetch: async (url) => {
      postCalls.push(url);
      if (url.startsWith("/app/terminal/open-derived")) {
        if (wakeGate) await wakeGate;
        if (wakeFail) return { ok: false, status: 409, json: async () => ({ error: "没有可续接的会话" }) };
        const s = { id: 30, provider: "codex", cwd: "/repo/w9", exit: null, derived: true, derived_state: "idle", derived_since: nowSec, parent_terminal_id: 1, parent_workspace_id: "ws-1", desk_key: "thread:gone", native: "gone", workspaces: [], display_title: "无行工位" };
        sessions.push(s);
        return { ok: true, status: 200, json: async () => ({ session: structuredClone(s) }) };
      }
      if (url.startsWith("/app/terminal/wake")) {
        if (elsewhere && !url.includes("confirm=1")) return { ok: false, status: 409, json: async () => ({ error: "别处在用", elsewhere: { app: "Ghostty", paused: false } }) };
        if (wakeGate) await wakeGate;
        if (wakeFail) return { ok: false, status: 409, json: async () => ({ error: "隔离目录已释放" }) };
        const s = sessions.find(x => x.id === 15);
        s.exit = null;
        s.derived_state = "idle";
        s.derived_since = nowSec;
        return { ok: true, status: 200, json: async () => ({ session: structuredClone(s) }) };
      }
      if (url.startsWith("/app/terminal/close-derived")) return { ok: true, status: 200, json: async () => ({ closed: 7 }) };
      return { ok: true, status: 200, json: async () => ({ ok: true }) };
    },
  });
  context.window = context;
  context.AN.h = (tag, attrs = {}, ...children) => {
    if (String(tag).startsWith("iframe")) {
      const f = Object.assign(makeNode("iframe"), {
        attributes: { ...attrs },
        contentWindow: { postMessage() {} },
      });
      createdFrames.push(f);
      return f;
    }
    return h(tag, attrs, ...children);
  };
  vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nglobalThis.el = el; globalThis.state = state; globalThis.renderList = renderList;`, context);
  context.go = path => {
    navigated.push(path);
    context.state.selected = path.split("/").pop();
    context.state.loaded = context.state.selected;
    context.ChatTerminal?.column(context.state.selected, "codex", "codex");
  };
  context.refresh = () => {};
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);

  await context.ChatTerminal.refresh();
  assert.equal(loadUrls.at(-1), "/app/terminal/state");

  // 1. Check all 7 derived states + fallback + elapsed duration in ZH and EN, fixed rec.meta visible.
  const d10 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 10);
  const d11 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 11);
  const d12 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 12);
  const d13 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 13);
  const d14 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 14);
  const d15 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 15);
  const d16 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 16);
  const d17 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 17);
  const d18 = context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 18);
  assert.equal(d17.derived_state, "running");
  assert.equal(d18.derived_state, "exited");
  assert.equal(context.ChatTerminal.derivedMeta(d10).text, "启动中");
  assert.equal(context.ChatTerminal.derivedMeta(d11).text, "运行中 · 2m");
  assert.equal(context.ChatTerminal.derivedMeta(d12).text, "等待确认");
  assert.equal(context.ChatTerminal.derivedMeta(d13).text, "待 boss 收取");
  assert.equal(context.ChatTerminal.derivedMeta(d14).text, "空闲");
  assert.equal(context.ChatTerminal.derivedMeta(d15).text, "空闲", "a session whose process ended reads as idle");
  assert.equal(context.ChatTerminal.derivedMeta(d16).text, "已结束");

  lang = "en";
  assert.equal(context.ChatTerminal.derivedMeta(d10).text, "Starting");
  assert.equal(context.ChatTerminal.derivedMeta(d11).text, "Running · 2m");
  assert.equal(context.ChatTerminal.derivedMeta(d12).text, "Needs attention");
  assert.equal(context.ChatTerminal.derivedMeta(d13).text, "Awaiting boss");
  assert.equal(context.ChatTerminal.derivedMeta(d14).text, "Idle");
  assert.equal(context.ChatTerminal.derivedMeta(d15).text, "Idle");
  assert.equal(context.ChatTerminal.derivedMeta(d16).text, "Exited");
  assert.equal(context.ChatTerminal.derivedSummary("ws-1").text, "3 running · 1 waiting");
  lang = "zh";
  assert.equal(context.ChatTerminal.derivedSummary("ws-1").text, "3 运行 · 1 待确认");

  // Render list and verify boss row summary + derived row fixed meta, with no dormant look.
  context.state.groupsData = [{
    group: "ws-1", title: "主会话", status: "idle", updated: "2026-11-12T10:00:00Z", label: "Claude Code",
    host: "claude", project_key: "/repo", project_label: "repo", project_path: "/repo", preview: "",
  }];
  context.renderList();
  const bossRec = context.state.items.get("ws-1");
  assert.equal(bossRec.meta.hidden, false);
  assert.equal(bossRec.meta.textContent, "3 运行 · 1 待确认");
  const rec14 = context.state.items.get(d14.group);
  const rec15 = context.state.items.get(d15.group);
  assert.equal(d14.group.startsWith("drv-"), true);
  assert.equal(d17.group, "term-17");
  assert.equal(rec14.meta.hidden, false);
  assert.equal(rec14.meta.textContent, "空闲");
  assert.equal(rec15.meta.hidden, false);
  assert.equal(rec15.meta.textContent, "空闲");
  assert.equal(rec15.el.classList.contains("dormant"), false);

  // 2. shown=<id> query parameter rules
  context.state.loaded = "term-11";
  context.ChatTerminal.column("term-11", "codex", "codex");
  await context.ChatTerminal.refresh();
  assert.equal(loadUrls.at(-1), "/app/terminal/state?shown=11");

  docHidden = true;
  await context.ChatTerminal.refresh();
  assert.equal(loadUrls.at(-1), "/app/terminal/state");
  docHidden = false;

  drawer.classList.add("folded");
  context.state.convPeek = false;
  await context.ChatTerminal.refresh();
  assert.equal(loadUrls.at(-1), "/app/terminal/state");
  drawer.classList.remove("folded");

  context.ChatTerminal.readMessages();
  context.ChatTerminal.column("term-11", "codex", "codex");
  await context.ChatTerminal.refresh();
  assert.equal(loadUrls.at(-1), "/app/terminal/state");

  // 3. Live derived shows mode switch and End derived terminal; dormant shows Resume button without mode switch.
  context.ChatTerminal.readMessages(true);
  context.ChatTerminal.column("term-11", "codex", "codex");
  const stopBtn = drAll._after[1];
  const modesNode = drAll._after[0];
  assert.equal(modesNode.hidden, false);
  assert.equal(stopBtn.hidden, false);
  assert.equal(stopBtn.textContent, "结束派生终端");
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(modesNode.hidden, true);
  assert.equal(stopBtn.hidden, false);
  assert.equal(stopBtn.textContent, "续接");
  assert.equal(stopBtn.classList.contains("start"), true);

  // 4. Clicking a dormant derived row enters its derived office on messages without waking;
  // clicking the header Resume button wakes it while messages stay up and disables the button.
  const s15 = sessions.find(x => x.id === 15);
  const toMessagesBtn = drAll._after[0].children[0];
  const flush = () => new Promise(r => setTimeout(r, 0));
  const wakes = () => postCalls.filter(u => u.startsWith("/app/terminal/wake")).length;
  const hold = () => { let release; wakeGate = new Promise(r => (release = r)); return () => { wakeGate = null; release(); }; };
  const redormant = async () => { Object.assign(s15, { exit: 0, derived_state: "dormant", resumable: true }); await context.ChatTerminal.refresh(); };
  const termMode = () => drawer.classList.contains("term-mode");
  const sub = els.get("dr-sub");
  const inFrame15 = () => createdFrames.at(-1)?.attributes.src === "/terminal/?session=15";
  s15.resumable = false;
  await context.ChatTerminal.refresh();
  context.state.selected = "term-15";
  await context.ChatTerminal.refresh();
  assert.equal(wakes(), 0, "polling never wakes");

  // Clicking a merged sidebar row opens its own derived office (/chat/drv-...) without waking.
  navigated.length = 0;
  context.ChatTerminal.openDerived(d15.group);
  assert.deepEqual(navigated, [`/chat/${d15.group}`]);
  assert.equal(wakes(), 0);

  context.state.selected = context.state.loaded = "term-15";
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);

  // wake via Resume button fails: the reason is shown, the messages stay
  wakeFail = true;
  stopBtn.listeners.click[0]({});
  await flush();
  assert.match(postCalls.at(-1), /^\/app\/terminal\/wake\?session=15&light=0&cols=100&rows=30$/);
  assert.deepEqual(says.at(-1), ["隔离目录已释放", true]);
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);
  wakeFail = false;

  // two clicks while it resumes: one request, button disabled, "正在恢复" in the header and the row, messages up
  const release = hold();
  const before = wakes();
  stopBtn.listeners.click[0]({});
  stopBtn.listeners.click[0]({});
  await flush();
  assert.equal(wakes() - before, 1);
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);
  assert.equal(stopBtn.disabled, true);
  assert.equal(sub.textContent, "正在恢复");
  assert.equal(context.ChatTerminal.derivedMeta(context.ChatTerminal.derivedOf("ws-1").find(x => x.terminal === 15)).text, "正在恢复");
  release();
  await flush();
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), true, "the terminal opens once it is up");
  assert.equal(stopBtn.hidden, false);
  assert.equal(stopBtn.textContent, "结束派生终端");
  assert.ok(inFrame15());
  const oldFrame15 = createdFrames.at(-1);

  // When an actively viewed derived terminal exits on its own, screen + switch stay and Resume appears.
  await redormant();
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), true);
  assert.equal(modesNode.hidden, false);
  assert.equal(stopBtn.hidden, false);
  assert.equal(stopBtn.textContent, "续接");

  // Switching to messages hides the mode switch while keeping Resume.
  toMessagesBtn.listeners.click[0]();
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);
  assert.equal(modesNode.hidden, true);
  assert.equal(stopBtn.hidden, false);

  // the reader leaves while it resumes: the terminal does not take the screen back
  await redormant();
  let release2 = hold();
  stopBtn.listeners.click[0]({});
  await flush();
  context.state.selected = context.state.loaded = "ws-1";
  release2();
  await flush();
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);

  // the reader asks for the messages while it resumes: they stay, whatever the polling does
  await redormant();
  context.state.selected = context.state.loaded = "term-15";
  context.ChatTerminal.column("term-15", "codex", "codex");
  release2 = hold();
  stopBtn.listeners.click[0]({});
  await flush();
  toMessagesBtn.listeners.click[0]();
  release2();
  await flush();
  await context.ChatTerminal.refresh();
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), false);

  // Resume from messages confirms elsewhere when 409 elsewhere is returned
  await redormant();
  context.state.selected = context.state.loaded = "term-15";
  context.ChatTerminal.column("term-15", "codex", "codex");
  elsewhere = true;
  nextAsk = { value: "confirm" };
  const calls = wakes();
  stopBtn.listeners.click[0]({});
  await flush();
  assert.equal(wakes() - calls, 2, "refused once, then confirmed");
  assert.ok(postCalls.at(-1).endsWith("&confirm=1"));
  context.ChatTerminal.column("term-15", "codex", "codex");
  assert.equal(termMode(), true);
  assert.equal(oldFrame15.removed, true, "old iframe must be dropped on wake");
  elsewhere = false;
  nextAsk = { value: "stop" };

  // a terminal left on screen continues when the window comes back to find its process ended; one
  // that ended while the reader was looking does not
  const ended = () => { const x = sessions.find(v => v.id === 15); x.exit = 0; x.derived_state = "dormant"; };
  for (const away of [true, false]) {
    sessions.find(v => v.id === 15).exit = null;
    sessions.find(v => v.id === 15).derived_state = "idle";
    await context.ChatTerminal.refresh();
    context.ChatTerminal.column("term-15", "codex", "codex");
    const before = wakes();
    if (away) {
      docHidden = true;
      visibility.forEach(fn => fn());
      docHidden = false;
      visibility.forEach(fn => fn());
    }
    ended();
    await context.ChatTerminal.refresh();
    await flush();
    assert.equal(wakes() - before, away ? 1 : 0);
  }
  await redormant();

  // a derived office with no terminal row opens through open-derived with the same attach params
  const goneDrv = context.ChatTerminal.derivedRowId("ws-1", "thread:gone");
  context.state.selected = context.state.loaded = goneDrv;
  context.state.drawer = { kind: "all" };
  context.state.chat = { group: goneDrv, derived: true, parent_workspace_id: "ws-1", parent_desk_key: "thread:gone" };
  context.state.ctx = { desks: new Map() };
  wakeFail = true;
  await context.ChatTerminal.revive({ group: goneDrv, parentGroup: "ws-1", deskKey: "thread:gone" });
  assert.equal(postCalls.at(-1), "/app/terminal/open-derived?workspace=ws-1&desk=thread%3Agone&light=0&cols=100&rows=30");
  assert.deepEqual(says.at(-1), ["没有可续接的会话", true]);
  context.ChatTerminal.column(goneDrv, "codex", "codex");
  assert.equal(termMode(), false);
  wakeFail = false;
  const release3 = hold();
  const pendingOpen = context.ChatTerminal.revive({ group: goneDrv, parentGroup: "ws-1", deskKey: "thread:gone" });
  assert.equal(await context.ChatTerminal.revive({ group: goneDrv, parentGroup: "ws-1", deskKey: "thread:gone" }), false, "no second request");
  context.ChatTerminal.column(goneDrv, "codex", "codex");
  assert.equal(sub.textContent, "正在恢复");
  release3();
  assert.equal(await pendingOpen, true);
  context.ChatTerminal.column(goneDrv, "codex", "codex");
  assert.equal(termMode(), true);

  // 5. Context menus: live derived has Open & End derived terminal; dormant has Open, Resume, End derived terminal;
  // boss menu counts only live derived terminals.
  context.ChatTerminal.menu(d11.group, {});
  assert.deepEqual(JSON.parse(JSON.stringify(menus.at(-1).map(i => i.label))), ["打开", "结束派生终端"]);
  context.ChatTerminal.menu(d15.group, {});
  assert.deepEqual(JSON.parse(JSON.stringify(menus.at(-1).map(i => i.label))), ["打开", "续接", "结束派生终端"]);
  context.ChatTerminal.menu("term-1", {});
  const closeAllBossItem = menus.at(-1).find(i => i.label.includes("结束全部派生终端"));
  assert.equal(closeAllBossItem.label, "结束全部派生终端（7）");
  await redormant();

  // 6. Unified confirmStop: ending the boss merges boss + derived active + draft counts in ONE ask, and skips it if no active/draft derived.
  asks.length = 0;
  nextAsk = null;
  await context.ChatTerminal.close(1); // boss 1 has 3 active (10,11,12,17 -> 4 active) + 1 draft (12)
  assert.equal(asks.length, 1);
  assert.ok(asks[0].body.some(p => p.textContent.includes("有 4 个派生会话还没做完")));
  // Only the one the boss dispatched (11) is reported back to it.
  assert.ok(asks[0].body.some(p => p.textContent.includes("其中 1 个结束后会向 boss 回报未完成")));
  assert.ok(asks[0].body.some(p => p.textContent.includes("1 个有未发出的草稿")));

  // closeDerived confirms with counts when active/draft exist, and skips confirm when all idle/dormant/exited without drafts.
  asks.length = 0;
  await context.ChatTerminal.closeDerived("ws-1");
  assert.equal(asks.length, 1);
  assert.ok(asks[0].body.some(p => p.textContent.includes("4 个派生会话还没做完")));

  // Make all derived idle with no draft -> closeDerived & boss close do NOT ask.
  sessions = sessions.map(s => (s.derived ? { ...s, derived_state: "idle", derived_busy: false, blocked_kind: null, has_draft: false } : s));
  await context.ChatTerminal.refresh();
  asks.length = 0;
  await context.ChatTerminal.close(1);
  assert.equal(asks.length, 0, "boss close with only idle derived sessions must not confirm");
  await context.ChatTerminal.closeDerived("ws-1");
  assert.equal(asks.length, 0, "closeDerived with all idle derived sessions must not confirm");

  // When 0 live derived terminals remain, boss menu omits "结束全部派生终端".
  sessions = sessions.map(s => (s.derived ? { ...s, exit: 0, derived_state: "dormant" } : s));
  await context.ChatTerminal.refresh();
  context.ChatTerminal.menu("term-1", {});
  assert.equal(menus.at(-1).some(i => i.label.includes("结束全部派生终端")), false);
});

test("ChatDerived renders boss-side derived list alongside meetings, handles single/both layout, click/menu, and hides empty open desks", async () => {
  const makeNode = (tag = "div") => {
    const cls = new Set(String(tag).split(".").slice(1));
    return {
      tagName: String(tag).split(".")[0].toUpperCase(),
      hidden: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [], listeners: {},
      style: { setProperty() {}, removeProperty() {} },
      classList: {
        set: cls,
        add: (...names) => names.forEach(x => cls.add(x)),
        remove: (...names) => names.forEach(x => cls.delete(x)),
        toggle: (name, on) => (on === undefined ? (cls.has(name) ? (cls.delete(name), false) : (cls.add(name), true)) : on ? (cls.add(name), true) : (cls.delete(name), false)),
        contains: name => cls.has(name),
      },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = String(v); },
      getAttribute(k) { return this.attributes[k] ?? null; },
      hasAttribute(k) { return k in this.attributes; },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      replaceChildren(...nodes) { this.children = nodes.flat().filter(Boolean); },
      append(...nodes) { this.children.push(...nodes.flat().filter(Boolean)); },
      insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
      querySelector() { return null; },
      querySelectorAll() { return []; },
      remove() {},
    };
  };
  const h = (tag, attrs = {}, ...children) => {
    const node = makeNode(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "hidden") node.hidden = Boolean(v);
      else node.setAttribute(k, v === true ? "" : v);
    }
    node.children = children.flat(Infinity).filter(x => x != null && x !== false);
    if (node.children.every(x => typeof x === "string")) node.textContent = node.children.join("");
    return node;
  };
  const el = {
    boss: makeNode("section"),
    desks: Object.assign(makeNode("div"), { clientWidth: 560 }),
    desksZone: Object.assign(makeNode("section"), { hidden: false }),
    rooms: makeNode("div"),
    fx: makeNode("div"),
    deskCount: makeNode("span"),
    office: makeNode("div"),
    floor: makeNode("div"),
  };
  const state = {
    groupsData: [], loaded: "ws-1",
    bossRec: null, deskRecs: new Map(), roomRecs: new Map(), seatRecs: new Map(),
    away: new Map(), closing: new Map(), known: new Set(), ready: true, drawer: { kind: "all" },
  };
  const opened = [];
  const openedOffices = [];
  const resumed = [];
  const menus = [];
  let reviving = false;
  let terminalSessions = [
    { id: 101, derived: true, parent_workspace_id: "ws-1", desk_key: "thread:h1", native: "n1", derived_state: "running", resumable: true },
    { id: 102, derived: true, parent_workspace_id: "ws-1", desk_key: "thread:h2", native: "n2", derived_state: "dormant", resumable: true },
    { id: 103, derived: true, parent_workspace_id: "ws-1", desk_key: "thread:h3", native: "n3", derived_state: "uncollected", resumable: true },
  ];
  let liveOnlyRows = [];
  const ChatTerminal = {
    ofDesk: (group, key) => terminalSessions.find(s => s.parent_workspace_id === group && (s.desk_key === key || `thread:${s.native}` === key)),
    all: () => terminalSessions,
    derivedOf: () => liveOnlyRows,
    derivedStateOf: x => x.derived_state,
    derivedMeta: r => ({ text: r.derived_state === "starting" ? "启动中" : "空闲", tone: r.derived_state === "starting" ? "run" : "idle" }),
    isWaking: () => false,
    reviving: () => reviving,
    openDerived: id => openedOffices.push(["openDerived", id]),
    openDerivedOffice: (g, k, fb) => openedOffices.push(["openDerivedOffice", g, k, fb]),
    resumeDerived: target => resumed.push(target),
    close: async () => {},
    showing: () => false,
    running: () => false,
    bossOpensTerminal: () => false,
  };
  const context = vm.createContext({
    AN: {
      h, L: zh => zh, at: Date.parse, ongoing: s => s, isHidden: () => false, dozing: () => false,
      setText: (n, v) => { n.textContent = v; },
      reconcile: (p, nodes) => { p.children = [...nodes]; },
    },
    h, L: zh => zh, el, state, FIG: 100, ChatTerminal,
    ART: { character: () => "c", figure: makeNode, head: makeNode, lid: makeNode, url: p => p, version: () => 1, canWarp: () => false },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { roomMenu() {}, openMenu: (_e, items) => menus.push(items) },
    GROUP: { idle: ["空闲", "idle"], running: ["运行中", "run"] }, pick: (tbl, k) => tbl[k] || ["空闲", "idle"], urgent: s => s === "running" || s === "review",
    activities: () => [],
    cliOf: () => "Codex", deskChar: () => "c", deskName: d => d.model, deskParts: d => ({ name: d.model, effort: "" }),
    fullHandle: d => d.key, relTime: () => "刚刚", fullTime: () => "2026", shownName: g => g.display_title || g.title || g.label,
    setText: (n, v) => { n.textContent = v; },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.children = [...nodes]; },
    openScope: (scope, _opener, opts) => opened.push([scope, opts || {}]),
    go() {}, window: { addEventListener() {} }, document: { documentElement: { classList: { contains: () => false } } },
    ResizeObserver: class { observe() {} },
    reduceMotion: { matches: false },
    getComputedStyle: () => ({ getPropertyValue: () => "2", columnGap: "10px", gridTemplateColumns: "1fr" }),
    Date,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);

  const baseChat = {
    group: "ws-1", host: "claude", label: "Claude Code", display_title: "t",
    desks: [
      { key: "emp-1", provider: "codex", model: "gpt-6.1", derived: false, first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:00:00Z" },
      { key: "thread:h1", provider: "claude", model: "opus", derived: true, first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:01:00Z" },
      { key: "thread:n2", provider: "codex", model: "gpt-6.1", derived: true, first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:03:00Z" },
      { key: "thread:h3", provider: "antigravity", model: "gemini", derived: true, first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:02:00Z" },
      { key: "thread:missing", provider: "claude", model: "haiku", derived: true, first_seen: "2026-11-19T00:00:00Z", last_active: "2026-11-19T00:04:00Z" },
    ],
    rooms: [], messages: [],
  };
  const ctx = {
    desks: new Map(baseChat.desks.map(d => [d.key, d])),
    drawn: new Set(baseChat.desks.map(d => d.key)),
    rooms: new Map(),
    fullRooms: new Set(),
  };
  context.renderOffice(baseChat, ctx, { running: 0, review: 0 }, false);

  // Open desks exclude derived desks; boss-side shows derived section alone (not .both).
  assert.equal(el.desks.children.length, 1);
  assert.equal(el.deskCount.textContent, " · 1");
  assert.equal(state.bossRec.meets.hidden, false);
  assert.equal(state.bossRec.meets.classList.contains("both"), false);
  assert.equal(state.bossRec.meetsSec.hidden, true);
  assert.equal(state.bossRec.derivedSec.hidden, false);

  // Derived rows sorted active first (h3 & h1), then idle by recency (missing then n2).
  const derivedRows = state.bossRec.derivedSec.children[1].children;
  assert.equal(derivedRows.length, 4);
  assert.equal(derivedRows[0].classList.contains("review"), true);
  assert.equal(derivedRows[1].classList.contains("run"), true);
  assert.equal(derivedRows[2].classList.contains("run"), false);
  assert.equal(derivedRows[2].classList.contains("review"), false);

  // Click historical derived row -> stays in parent office and opens desk messages (no revive, no jump).
  derivedRows[1].listeners.click[0]({ stopPropagation() {} });
  assert.deepEqual(JSON.parse(JSON.stringify(opened.at(-1))), [{ kind: "desk", key: "thread:h1" }, {}]);
  assert.equal(resumed.length, 0);
  assert.equal(openedOffices.length, 0);

  // Context menu on live row vs dormant/missing row.
  const labels = () => JSON.parse(JSON.stringify(menus.at(-1).map(i => i.label)));
  derivedRows[1].listeners.contextmenu[0]({}); // h1 (live running)
  assert.deepEqual(labels(), ["查看消息", "进入派生办公室", "结束派生终端"]);
  derivedRows[3].listeners.contextmenu[0]({}); // n2 (dormant)
  assert.deepEqual(labels(), ["查看消息", "进入派生办公室", "续接"]);
  menus.at(-1).find(i => i.label === "进入派生办公室").run();
  assert.deepEqual(openedOffices.at(-1), ["openDerivedOffice", "ws-1", "thread:n2", undefined]);
  menus.at(-1).find(i => i.label === "续接").run();
  assert.equal(resumed.at(-1).deskKey, "thread:n2");

  // Unpersisted live derived terminal appears in boss-side and clicking it jumps straight to its derived office.
  liveOnlyRows = [{ group: "term-99", terminal: 99, derived: true, derived_state: "starting", host: "codex", label: "Codex", title: "刚启动的派生", updated: "2026-11-19T00:05:00Z" }];
  context.renderOffice(baseChat, ctx, { running: 0, review: 0 }, false);
  const rowsWithLiveOnly = state.bossRec.derivedSec.children[1].children;
  assert.equal(rowsWithLiveOnly.length, 5);
  rowsWithLiveOnly[0].listeners.click[0]({ stopPropagation() {} });
  assert.deepEqual(openedOffices.at(-1), ["openDerived", "term-99"]);
  liveOnlyRows = [];

  // When both meetings and derived exist, boss-side carries .both and shows both sections.
  const withRoom = {
    ...baseChat,
    rooms: [{ id: "r1", title: "架构评审", status: "idle", seats: [], last_active: "2026-11-19T00:05:00Z" }],
  };
  ctx.rooms = new Map([["r1", withRoom.rooms[0]]]);
  context.renderOffice(withRoom, ctx, { running: 0, review: 0 }, false);
  assert.equal(state.bossRec.meets.classList.contains("both"), true);
  assert.equal(state.bossRec.meetsSec.hidden, false);
  assert.equal(state.bossRec.derivedSec.hidden, false);

  // With only derived sessions and no regular desks/optimized, empty open desks zone stays hidden.
  const onlyDerived = { ...baseChat, desks: baseChat.desks.filter(d => d.derived) };
  ctx.desks = new Map(onlyDerived.desks.map(d => [d.key, d]));
  ctx.drawn = new Set(onlyDerived.desks.map(d => d.key));
  ctx.rooms = new Map();
  context.renderOffice(onlyDerived, ctx, { running: 0, review: 0 }, false);
  assert.equal(el.desksZone.hidden, true);
  context.renderOffice({ ...baseChat, desks: [] }, { ...ctx, desks: new Map(), drawn: new Set() }, { running: 0, review: 0 }, false);
  assert.equal(el.desksZone.hidden, false);
  assert.equal(state.bossRec.meets.hidden, true);
});

test("Actions.workspaceMenu includes 结束全部派生终端（N） when workspace has derived sessions", async () => {
  const bodyChildren = [];
  const closedWorkspaces = [];
  const makeMenuNode = (tag, attrs = {}, ...kids) => {
    const flat = kids.flat(Infinity).filter(x => x != null && x !== false);
    const listeners = {};
    return {
      tag, attrs, children: flat, listeners, hidden: Boolean(attrs?.hidden), style: {},
      textContent: flat.every(x => typeof x === "string") ? flat.join("") : "",
      addEventListener(ev, fn) { (listeners[ev] ||= []).push(fn); },
      setAttribute() {},
      replaceChildren(...nodes) { this.children = nodes.flat().filter(Boolean); },
      append(...nodes) { this.children.push(...nodes.flat().filter(Boolean)); },
      getBoundingClientRect: () => ({ width: 140, height: 120 }),
      querySelector() { return null; },
      closest() { return null; },
      focus() {},
    };
  };
  const context = vm.createContext({
    AN: {
      h: makeMenuNode,
      $: () => null,
      L: zh => zh,
      load: async () => ({}),
      chatName: g => g.title || g.group,
    },
    document: { body: { append(...n) { bodyChildren.push(...n); } }, addEventListener() {}, querySelectorAll: () => [] },
    window: { addEventListener() {} },
    state: {
      groupsData: [{ group: "ws-1", title: "主会话", label: "Claude Code" }],
    },
    ChatTerminal: {
      derivedOf: id => (id === "ws-1" ? [{ terminal: 10 }, { terminal: 11 }] : []),
      closeDerived: id => closedWorkspaces.push(id),
      exitedOf: () => null,
    },
    setTimeout, clearTimeout, innerWidth: 1000, innerHeight: 800,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../actions.js`, "utf8")}\nglobalThis.Actions = Actions;`, context);
  context.Actions.workspaceMenu("ws-1", { clientX: 10, clientY: 10, preventDefault() {}, stopPropagation() {} });
  const menu = bodyChildren.find(n => n.tag === "div.ctx");
  const closeAllBtn = menu.children.find(b => b.textContent.includes("结束全部派生终端"));
  assert.ok(closeAllBtn);
  assert.equal(closeAllBtn.textContent, "结束全部派生终端（2）");
  closeAllBtn.listeners.click[0]();
  assert.deepEqual(closedWorkspaces, ["ws-1"]);
});

test("mock.js exposes 5 derived lifecycle states, derived desks, and wake/open-derived/stop/close-derived endpoints", async () => {
  const storage = new Map();
  const win = { fetch: async () => ({ ok: true, json: async () => ({}) }), addEventListener() {}, dispatchEvent() {}, postMessage() {} };
  const context = vm.createContext({
    AN: {},
    URL,
    URLSearchParams,
    Response,
    location: { search: "?mock", protocol: "http:", hostname: "localhost", href: "http://localhost/preview.html?mock" },
    document: { documentElement: { dataset: { theme: "dark" } }, addEventListener() {} },
    localStorage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) },
    window: win,
    fetch: win.fetch,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    Date, Math, JSON, Map, Set, Array, Object, Number, String, Boolean, Promise, RegExp, Error,
    structuredClone, setTimeout, clearTimeout,
  });
  vm.runInContext(readFileSync(`${__dirname}/../mock.js`, "utf8"), context);
  const loadJson = async (url, method = "GET") => (await win.fetch(url, method === "POST" ? { method: "POST", body: "{}" } : {})).json();
  const termState = await loadJson("/app/terminal/state?shown=3");
  assert.equal(termState.derived_titles["thread:drv-old"], "清理旧配置");
  const chatsPayload = await loadJson("/app/chats");
  assert.equal(chatsPayload.groups.find(g => g.group === "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50").derived.length, 6);
  const derivedStates = termState.sessions.filter(s => s.derived).map(s => s.derived_state);
  for (const s of ["running", "waiting", "uncollected", "idle", "dormant"]) {
    assert.ok(derivedStates.includes(s), `mock.js missing derived_state=${s}`);
  }
  const chat = await loadJson("/app/chats/s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50");
  assert.equal(chat.desks.filter(d => d.derived).length, 6);
  const woke = await loadJson("/app/terminal/wake?session=7", "POST");
  assert.equal(woke.session.id, 7);
  assert.equal(woke.session.derived_state, "idle");
  assert.equal(woke.session.exit, null);
  const opened = await loadJson("/app/terminal/open-derived?workspace=s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50&desk=thread%3Adrv-old", "POST");
  assert.equal(opened.session.desk_key, "thread:drv-old");
  assert.equal(opened.session.derived_state, "idle");
  const closed = await loadJson("/app/terminal/close-derived?workspace=s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50", "POST");
  assert.equal(closed.closed, 6);
});

test("historical derived rows merge stably, keep terminal width, highlight when open, and follow project fold/cap", async () => {
  const makeNode = (tag = "div") => {
    const cls = new Set(String(tag).split(".").slice(1));
    const n = {
      tagName: String(tag).split(".")[0].toUpperCase(),
      hidden: false, disabled: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [], listeners: {},
      style: { setProperty() {}, removeProperty() {} },
      classList: {
        set: cls,
        add: (...names) => names.forEach(x => cls.add(x)),
        remove: (...names) => names.forEach(x => cls.delete(x)),
        toggle: (name, on) => (on === undefined ? (cls.has(name) ? (cls.delete(name), false) : (cls.add(name), true)) : on ? (cls.add(name), true) : (cls.delete(name), false)),
        contains: name => cls.has(name),
      },
      setAttribute(k, v) { if (v == null) delete this.attributes[k]; else this.attributes[k] = String(v); },
      getAttribute(k) { return this.attributes[k] ?? null; },
      hasAttribute(k) { return k in this.attributes; },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      replaceChildren(...nodes) { this.children = nodes.flat().filter(Boolean); },
      append(...nodes) { this.children.push(...nodes.flat().filter(Boolean)); },
      after(...nodes) { this._after = nodes; },
      before() {},
      before() {},
      querySelector() { return null; },
      querySelectorAll() { return []; },
      scrollIntoView() {},
      closest: () => null,
      focus() {},
      remove() { this.removed = true; },
    };
    return n;
  };
  const h = (tag, attrs = {}, ...children) => {
    const node = makeNode(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "hidden") node.hidden = Boolean(v);
      else node.setAttribute(k, v === true ? "" : v);
    }
    node.children = children.flat(Infinity).filter(x => x != null && x !== false);
    if (node.children.every(x => typeof x === "string")) node.textContent = node.children.join("");
    return node;
  };

  let sessions = [];
  let derivedTitles = { "thread:saved": "保存的标题" };
  const postCalls = [];
  const menus = [];
  const navigated = [];
  const opened = [];
  const storage = new Map();
  const appNode = makeNode("div");
  const drawer = makeNode("aside");
  const drAll = makeNode("button");
  const listNode = makeNode("div");
  const els = new Map([
    ["app", appNode], ["list", listNode], ["head", makeNode("header")], ["office", makeNode("div")], ["floor", makeNode("div")],
    ["boss", makeNode("section")], ["desks", makeNode("div")], ["derived-zone", makeNode("section")], ["derived-desks", makeNode("div")],
    ["derived-count", makeNode("span")], ["rooms", makeNode("div")], ["fx", makeNode("div")], ["desks-count", makeNode("span")],
    ["ticker", makeNode("footer")], ["tick", makeNode("span")], ["drawer", drawer], ["conv-toggle", makeNode("button")],
    ["side-toggle", makeNode("button")], ["dr-ico", makeNode("span")], ["dr-title", makeNode("span")], ["dr-sub", makeNode("span")],
    ["dr-all", drAll], ["timeline", makeNode("div")], ["tl-inner", makeNode("div")], ["blank", makeNode("div")],
    ["jump", makeNode("button")], ["banner", makeNode("div")],
  ]);
  const AN = {
    h, $: id => els.get(id) || makeNode(id),
    L: zh => zh,
    load: async () => ({ sessions: structuredClone(sessions), cwd: "/repo", derived_titles: structuredClone(derivedTitles) }),
    href: p => p,
    GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
    pick: (tbl, k) => tbl[k] || [k || "—", "idle"],
    urgent: s => s === "running" || s === "review",
    avatar: () => makeNode("span"),
    colorClass: () => "c0",
    relTime: () => "刚刚",
    clock: () => "10:00",
    fullTime: () => "2026",
    dayLabel: () => "今天",
    setText: (n, v) => { n.textContent = v == null ? "" : String(v); },
    setAttr: (n, k, v) => n.setAttribute(k, v),
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const cls of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(cls);
    },
    reconcile: (p, nodes) => { p.children = [...nodes]; },
    stickyOrder: items => items,
    poll() {},
    plain: x => x,
    art: { character: () => "c" },
    hostTile: p => makeNode(p),
    cliName: p => ({ claude: "Claude Code", codex: "Codex" }[p] || p),
    deskName: d => d.model || "",
    deskParts: d => ({ name: d.model || "", effort: "" }),
    handleOf: d => `@${d.model || ""}`,
    fullHandle: d => d.key,
    cliOf: d => d.provider,
    chatName: g => g.display_title || g.title || g.group,
    shownName: g => g.display_title || g.title || g.group,
    activities: () => [],
    ongoing: s => s || "idle",
    isHidden: () => false,
    onVisibility() {},
    reducedMotion: { matches: false },
  };
  const context = vm.createContext({
    AN, URLSearchParams, window: null,
    document: { addEventListener() {}, documentElement: { lang: "zh", dataset: {} }, hidden: false },
    localStorage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: k => storage.delete(k) },
    location: { search: "", pathname: "/", hash: "", origin: "null" },
    history: { pushState() {}, replaceState() {} },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    addEventListener() {}, requestAnimationFrame() {}, setTimeout, clearTimeout, Date,
    Actions: {
      picked: new Set(), pick: () => false, workspaceMenu() {}, projectMenu() {},
      openMenu: (_e, items) => menus.push(items), say() {}, ask: async () => ({ value: "stop" }),
    },
    fetch: async (url) => {
      postCalls.push(url);
      if (url.startsWith("/app/terminal/open-derived")) {
        const s = { id: 99, provider: "claude", cwd: "/repo", exit: null, derived: true, derived_state: "idle", derived_since: 100, parent_workspace_id: "ws-hist", desk_key: "thread:saved", native: "saved", workspaces: [], display_title: "CLI新标题" };
        sessions.push(s);
        return { ok: true, status: 200, json: async () => ({ session: structuredClone(s) }) };
      }
      if (url.startsWith("/app/terminal/close?")) {
        const sid = Number(url.split("session=")[1]);
        sessions = sessions.filter(s => s.id !== sid);
      }
      return { ok: true, status: 200, json: async () => ({ ok: true }) };
    },
  });
  context.window = context;
  context.AN.h = (tag, attrs = {}, ...children) => (
    String(tag).startsWith("iframe")
      ? Object.assign(makeNode("iframe"), { attributes: { ...attrs }, contentWindow: { postMessage() {} } })
      : h(tag, attrs, ...children)
  );
  vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nglobalThis.el = el; globalThis.state = state; globalThis.renderList = renderList; globalThis.syncListSelection = syncListSelection; globalThis.mustShow = mustShow; globalThis.toggleFold = toggleFold;`, context);
  context.go = path => navigated.push(path);
  context.openScope = (scope) => {
    opened.push(scope);
    context.state.drawer = scope;
    context.ChatTerminal.readMessages(false);
    context.ChatTerminal.column(context.state.loaded, "claude", "claude");
    context.syncListSelection();
  };
  context.refresh = () => {};
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);

  // Groups: ws-plain has neither terminals nor derived desks; ws-hist has 2 historical derived desks (one with saved title, one falling back to deskName).
  const makeBoss = (group, project, extra = {}) => ({
    group, title: group, status: "idle", updated: "2026-11-12T10:00:00Z", label: "Claude Code",
    host: "claude", project_key: project, project_label: project.split("/").pop(), project_path: project, preview: "", ...extra,
  });
  context.state.groupsData = [
    makeBoss("ws-plain", "/p0"),
    makeBoss("ws-hist", "/p1", {
      derived: [
        { key: "thread:saved", provider: "claude", model: "opus", first_seen: "2026-11-12T01:00:00Z", last_active: "2026-11-12T02:00:00Z" },
        { key: "thread:fallback", provider: "codex", model: "gpt-6.1", first_seen: "2026-11-12T03:00:00Z", last_active: "2026-11-12T04:00:00Z" },
      ],
    }),
  ];
  await context.ChatTerminal.refresh();
  context.renderList();

  // 1. Title priority + stable ID + idle appearance on historical-only rows.
  const rows = context.ChatTerminal.derivedOf("ws-hist");
  assert.equal(rows.length, 2);
  assert.equal(rows[0].title, "保存的标题");
  assert.equal(rows[1].title, "gpt-6.1");
  assert.equal(rows[0].preview, "空闲");
  assert.equal(rows[0].terminal, null);
  const stableId = rows[0].group;
  assert.equal(stableId.startsWith("drv-"), true);
  const firstEl = context.state.items.get(stableId).el;

  // 2. Width flag (ADR-0130): parent offices without their own terminal use narrow width ("0");
  // derived offices ALWAYS use terminal width ("1").
  context.state.selected = context.state.loaded = "ws-plain";
  context.state.chat = { group: "ws-plain", desks: [] };
  context.state.ctx = { desks: new Map() };
  context.ChatTerminal.prepare("ws-plain");
  context.ChatTerminal.column("ws-plain", "claude", "claude");
  assert.equal(appNode.dataset.hasTerm, "0");

  context.state.selected = context.state.loaded = "ws-hist";
  context.state.chat = { group: "ws-hist", desks: [{ key: "thread:saved", derived: true }, { key: "thread:fallback", derived: true }] };
  context.state.ctx = { desks: new Map([["thread:saved", { key: "thread:saved", derived: true }], ["thread:fallback", { key: "thread:fallback", derived: true }]]) };
  context.state.drawer = { kind: "all" };
  context.ChatTerminal.prepare("ws-hist");
  context.ChatTerminal.column("ws-hist", "claude", "claude");
  assert.equal(appNode.dataset.hasTerm, "0", "parent without own terminal keeps narrow width");

  // 3. Highlight strictly follows state.selected (center canvas); clicking a derived row navigates to its derived office.
  context.renderList();
  assert.equal(context.state.items.get("ws-hist").el.classList.contains("on"), true);
  assert.equal(firstEl.classList.contains("on"), false);

  // Even when a derived desk drawer is open in the parent office, parent stays highlighted in the sidebar.
  context.state.drawer = { kind: "desk", key: "thread:saved" };
  context.syncListSelection();
  assert.equal(context.state.items.get("ws-hist").el.classList.contains("on"), true);
  assert.equal(firstEl.classList.contains("on"), false);

  assert.equal(firstEl.attributes.href, `/chat/${stableId}`);
  assert.equal(context.state.items.get("ws-hist").el.attributes.href, "/chat/ws-hist");

  // Clicking historical derived row navigates to its derived office (no revive).
  firstEl.listeners.click[0]({ button: 0, preventDefault() {} });
  assert.deepEqual(navigated, [`/chat/${stableId}`]);
  assert.equal(postCalls.length, 0);

  // Inside the derived office: terminal width is fixed ("1"), highlight moves to derived row.
  context.state.selected = context.state.loaded = stableId;
  context.state.drawer = { kind: "all" };
  context.ChatTerminal.prepare(stableId);
  context.ChatTerminal.column(stableId, "claude", "claude");
  context.syncListSelection();
  assert.equal(appNode.dataset.hasTerm, "1");
  assert.equal(firstEl.classList.contains("on"), true);
  assert.equal(context.state.items.get("ws-hist").el.classList.contains("on"), false);

  // 4. Context menu on history-only row is ["打开", "续接"]; resuming adds terminal row while preserving the exact same DOM element!
  context.ChatTerminal.menu(stableId, {});
  assert.deepEqual(JSON.parse(JSON.stringify(menus.at(-1).map(i => i.label))), ["打开", "续接"]);
  await context.ChatTerminal.revive({ group: stableId, parentGroup: "ws-hist", deskKey: "thread:saved" });
  context.renderList();
  const afterWake = context.ChatTerminal.derivedOf("ws-hist");
  assert.equal(afterWake[0].group, stableId);
  assert.equal(afterWake[0].terminal, 99);
  assert.equal(afterWake[0].title, "CLI新标题");
  assert.equal(context.state.items.get(stableId).el, firstEl, "DOM element must be reused in place");

  // Closing terminal keeps row in list as idle with the same element and stable id, and stays in derived office on messages.
  await context.ChatTerminal.close(99);
  context.renderList();
  const afterClose = context.ChatTerminal.derivedOf("ws-hist");
  assert.equal(afterClose[0].group, stableId);
  assert.equal(afterClose[0].terminal, null);
  assert.equal(context.state.items.get(stableId).el, firstEl);
  context.ChatTerminal.column(stableId, "claude", "claude");
  assert.equal(appNode.dataset.hasTerm, "1");
  assert.equal(context.ChatTerminal.showing(), false);

  // 5. Visibility: historical-only derived rows do NOT make boss mustShow and hide when project is folded;
  // terminal-backed derived rows DO make boss mustShow and stay visible when folded.
  context.state.selected = "ws-plain";
  assert.equal(context.mustShow(context.state.groupsData[1]), false);
  context.toggleFold("project:/p1");
  assert.equal(context.state.items.get("ws-hist").el.getAttribute("data-folded"), "true");
  assert.equal(firstEl.getAttribute("data-folded"), "true");

  // Now attach a live terminal to thread:saved -> mustShow becomes true and both boss and terminal child stay visible when folded.
  sessions.push({ id: 100, provider: "claude", cwd: "/repo", exit: null, derived: true, derived_state: "idle", parent_workspace_id: "ws-hist", desk_key: "thread:saved", native: "saved", workspaces: [] });
  await context.ChatTerminal.refresh();
  assert.equal(context.mustShow(context.state.groupsData[1]), true);
  context.renderList();
  assert.equal(context.state.items.get("ws-hist").el.getAttribute("data-folded"), null);
  assert.equal(firstEl.getAttribute("data-folded"), null);
  assert.equal(context.state.items.get(afterWake[1].group).el.getAttribute("data-folded"), "true");

  // 6. Asking for messages while a session is being brought back keeps them once it is up.
  sessions = [];
  await context.ChatTerminal.refresh();
  context.state.selected = context.state.loaded = stableId;
  context.state.drawer = { kind: "all" };
  const resuming = context.ChatTerminal.revive({ group: stableId, parentGroup: "ws-hist", deskKey: "thread:saved" });
  context.ChatTerminal.stayOnMessages("ws-hist", "thread:saved");
  await resuming;
  context.ChatTerminal.column(stableId, "claude", "claude");
  assert.equal(context.ChatTerminal.showing(), false);

  // 7. Seamless follow: term-120 -> drv-... once history has it -> child_workspace_id once it dispatches.
  sessions = [{ id: 120, provider: "codex", cwd: "/repo", exit: null, derived: true, derived_state: "starting", parent_workspace_id: "ws-hist", desk_key: null, native: null, workspaces: [] }];
  await context.ChatTerminal.refresh();
  assert.equal(context.ChatTerminal.follow("term-120"), "term-120");
  sessions[0].desk_key = "thread:fresh";
  await context.ChatTerminal.refresh();
  assert.equal(context.ChatTerminal.follow("term-120"), "term-120");
  assert.equal(context.ChatTerminal.office("term-120").terminal, 120);
  context.state.groupsData[1].derived.push({ key: "thread:fresh", provider: "codex", model: "gpt-6.1" });
  const freshDrv = context.ChatTerminal.derivedRowId("ws-hist", "thread:fresh");
  assert.equal(context.ChatTerminal.follow("term-120"), freshDrv);
  // Once it dispatches and spawns a child workspace, drv-... seamlessly follows to child_workspace_id!
  context.state.groupsData[1].derived.at(-1).child_workspace_id = "ws-fresh-child";
  context.state.groupsData.push(makeBoss("ws-fresh-child", "/p1", { parent_workspace_id: "ws-hist", parent_desk_key: "thread:fresh" }));
  assert.equal(context.ChatTerminal.follow(freshDrv), "ws-fresh-child");
});

test("derived desk message view merges user turns as 用户 without mutating c.messages and surfaces log errors", async () => {
  const { context } = auditUI();
  const baseH = context.h;
  const h = (...args) => {
    const node = baseH(...args);
    if (!node.insertBefore) {
      node.insertBefore = (child, ref) => {
        const i = ref ? node.children.indexOf(ref) : -1;
        if (i >= 0) node.children.splice(i, 0, child);
        else node.children.push(child);
        return child;
      };
    }
    if (!("firstChild" in node)) {
      Object.defineProperty(node, "firstChild", { get: () => node.children[0] || null });
    }
    if (!node.classList) {
      const set = new Set(String(node.className || "").split(/\s+/).filter(Boolean));
      node.classList = {
        add: (...ks) => { ks.forEach(k => set.add(k)); node.className = [...set].join(" "); },
        remove: (...ks) => { ks.forEach(k => set.delete(k)); node.className = [...set].join(" "); },
        toggle: (k, force) => {
          const on = force === undefined ? !set.has(k) : Boolean(force);
          if (on) set.add(k); else set.delete(k);
          node.className = [...set].join(" ");
          return on;
        },
        contains: (k) => set.has(k),
      };
    }
    if (!node.replaceChildren) {
      node.replaceChildren = (...kids) => { node.children = kids; };
    }
    if (!node.addEventListener) {
      node.listeners = node.listeners || {};
      node.addEventListener = (ev, fn) => {
        (node.listeners[ev] ||= []).push(fn);
      };
    }
    return node;
  };
  const turnsJs = readFileSync(`${__dirname}/../chat-derived-turns.js`, "utf8");
  const extract = (name) => {
    const start = turnsJs.indexOf(`function ${name}(`);
    const next = turnsJs.indexOf("\nfunction ", start + 1);
    return turnsJs.slice(start, next === -1 ? turnsJs.length : next);
  };
  const notice = h("div.dr-derived-note", { role: "status" });
  notice.hidden = true;
  const inner = h("div.dr-inner");
  const tl = h("div.dr-tl", {}, notice, inner);
  const requests = [];
  let nextReply = {
    turns: [
      {
        index: 1,
        anchor: { prompt_id: "u1" },
        user_text: "用户在终端里追问一句",
        user_time: "2026-11-26T10:01:00Z",
        assistant_text: "终端里的回复",
        assistant_time: "2026-11-26T10:01:05Z",
        status: "succeeded",
        model: "claude-opus-4-6",
      },
    ],
  };
  const renderCalls = [];
  const state = {
    chat: null,
    ctx: null,
    drawer: { kind: "desk", key: "thread:s1" },
    derivedTurns: { cache: new Map(), pending: new Map() },
  };
  const scope = {
    AN: context.AN,
    h,
    state,
    el: { tl, inner, derivedNotice: notice },
    setText: context.setText,
    ChatTerminal: { ofDesk: () => null },
    load: async (url) => {
      requests.push(url);
      return nextReply;
    },
    renderConv: (...args) => renderCalls.push(args),
  };
  vm.runInNewContext(
    [
      extract("syncDerivedNotice"),
      extract("isDerivedDeskView"),
      extract("derivedTurnsVersionKey"),
      extract("refreshDerivedTurns"),
      extract("requestDerivedTurns"),
      extract("buildDerivedUserMessages"),
      extract("mergeDerivedMessages"),
    ].join("\n"),
    scope,
  );
  const chat = {
    group: "s-ws",
    created: "2026-11-26T10:00:00Z",
    messages: [
      {
        id: "m-boss",
        kind: "prompt",
        speaker: { role: "boss", label: "boss" },
        desk: "thread:s1",
        text: "boss prompt",
        time: "2026-11-26T10:00:00Z",
        message_id: "msg-1",
      },
    ],
  };
  const desk = {
    key: "thread:s1",
    derived: true,
    provider: "claude",
    model: "claude-opus-4-6",
    private: { tasks: 1, running: 0 },
    status: "idle",
  };
  const ctx = { desks: new Map([["thread:s1", desk]]) };
  state.chat = chat;
  state.ctx = ctx;

  assert.equal(scope.isDerivedDeskView(chat, ctx, "thread:s1"), true);
  assert.equal(scope.requestDerivedTurns(chat, ctx, "thread:s1"), null);
  await state.derivedTurns.pending.get("s-ws|thread:s1");
  assert.equal(requests[0], "/app/terminal/derived-turns?workspace=s-ws&desk=thread%3As1");
  assert.equal(renderCalls.length, 1);

  const cached = scope.requestDerivedTurns(chat, ctx, "thread:s1");
  assert.equal(cached.turns.length, 1);
  const built = scope.buildDerivedUserMessages(chat, desk, "thread:s1", cached.turns);
  const merged = scope.mergeDerivedMessages(chat.messages, built);
  assert.equal(chat.messages.length, 1, "c.messages must remain untouched");
  assert.deepEqual(
    JSON.parse(JSON.stringify(merged.map(m => m.text))),
    ["boss prompt", "用户在终端里追问一句", "终端里的回复"],
  );
  assert.equal(merged[1].speaker.role, "user");
  assert.equal(context.speakerName(merged[1]), "用户");
  context.renderMarkdown = (node, text) => { context.setText(node, String(text)); };
  const rec = context.createMessage(merged[1]);
  rec.el.toggleAttribute = (name, on) => {
    if (on) rec.el.attributes[name] = "";
    else delete rec.el.attributes[name];
  };
  context.updateMessage(rec, merged[1], { ...ctx, c: chat }, merged[1].id);
  const classes = String(rec.el.className).split(/\s+/);
  assert.equal(classes.includes("msg"), true);
  assert.equal(classes.includes("user"), true);

  // When error comes back, notice banner shows while boss message remains.
  nextReply = { turns: [], error: "找不到原生会话日志（可能已被 CLI 清理）" };
  desk.private.tasks = 2;
  scope.requestDerivedTurns(chat, ctx, "thread:s1");
  await state.derivedTurns.pending.get("s-ws|thread:s1");
  assert.equal(requests.length, 2, "a new task on the desk reads the turns again");
  const errEntry = scope.requestDerivedTurns(chat, ctx, "thread:s1");
  scope.syncDerivedNotice(errEntry.error);
  assert.equal(notice.hidden, false);
  assert.equal(notice.textContent, "找不到原生会话日志（可能已被 CLI 清理）");
  scope.syncDerivedNotice("");
  assert.equal(notice.hidden, true);

  // Entering the desk's messages reads them again even though nothing the key holds changed.
  scope.requestDerivedTurns(chat, ctx, "thread:s1");
  assert.equal(requests.length, 2);
  scope.refreshDerivedTurns("s-ws", "thread:s1");
  assert.equal(scope.requestDerivedTurns(chat, ctx, "thread:s1").error, errEntry.error, "stale turns show meanwhile");
  await state.derivedTurns.pending.get("s-ws|thread:s1");
  assert.equal(requests.length, 3);
});

test("derived subagent workspaces nest under derived rows (1-level), refuse duplicate resume, and count own busy on stop", async () => {
  const makeNode = (tag = "div") => {
    const cls = new Set(String(tag).split(".").slice(1));
    const n = {
      tagName: String(tag).split(".")[0].toUpperCase(),
      hidden: false, disabled: false, textContent: "", title: "", dataset: {}, attributes: {}, children: [], listeners: {}, style: {},
      classList: {
        set: cls,
        add(...xs) { xs.forEach(x => cls.add(x)); },
        remove(...xs) { xs.forEach(x => cls.delete(x)); },
        toggle(x, on) { if (on === undefined) { if (cls.has(x)) cls.delete(x); else cls.add(x); } else if (on) cls.add(x); else cls.delete(x); },
        contains(x) { return cls.has(x); },
      },
      append(...xs) { this.children.push(...xs.flat().filter(Boolean)); },
      after() {},
      before() {},
      setAttribute(k, v) { this.attributes[k] = String(v); },
      removeAttribute(k) { delete this.attributes[k]; },
      remove() {},
      replaceChildren(...xs) { this.children = xs.flat().filter(Boolean); },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      querySelector() { return null; },
      closest() { return null; },
      focus() {},
    };
    return n;
  };
  const h = (tag, attrs = {}, ...kids) => {
    const n = makeNode(tag);
    Object.entries(attrs || {}).forEach(([k, v]) => {
      if (k === "hidden" || k === "disabled" || k === "type" || k === "title") n[k] = v;
      else n.attributes[k] = String(v);
    });
    n.children = kids.flat(Infinity).filter(x => x != null && x !== false);
    if (n.children.every(x => typeof x === "string")) n.textContent = n.children.join("");
    return n;
  };
  const drAllAfter = [];
  const els = new Map([
    ["app", makeNode("div")], ["list", makeNode("div")], ["head", makeNode("header")],
    ["boss", makeNode("section")], ["drawer", makeNode("aside")],
    ["dr-all", { ...makeNode("button"), after(...items) { drAllAfter.push(...items); } }],
    ["dr-ico", makeNode("span")], ["dr-title", makeNode("span")], ["dr-sub", makeNode("span")],
    ["dr-foot", makeNode("div")], ["timeline", makeNode("div")], ["tl-inner", makeNode("div")],
    ["banner", makeNode("div")],
  ]);
  const el = {
    app: els.get("app"), list: els.get("list"), drawer: els.get("drawer"),
    drAll: els.get("dr-all"), drIco: els.get("dr-ico"), drTitle: els.get("dr-title"),
    drSub: els.get("dr-sub"), drFoot: els.get("dr-foot"), tl: els.get("timeline"),
    inner: els.get("tl-inner"), head: els.get("head"), boss: els.get("boss"),
  };
  const asked = [];
  let sessions = [
    {
      id: 50,
      provider: "codex",
      cwd: "/repo",
      exit: null,
      derived: true,
      derived_state: "idle",
      derived_own_busy: true,
      parent_workspace_id: "ws-boss",
      desk_key: "thread:d1",
      native: "n1",
      boss_key: "s-derived-bk",
      workspaces: [],
    },
    {
      id: 1,
      provider: "claude",
      cwd: "/repo",
      exit: null,
      derived: false,
      workspaces: ["ws-boss"],
    },
    // An ended terminal of the subagent workspace, which its live owner still holds.
    { id: 2, provider: "codex", cwd: "/repo", exit: 0, derived: false, workspaces: ["ws-sub"] },
  ];
  const groupsData = [
    {
      group: "ws-boss", host: "claude", app: "claude", label: "Claude Code", title: "Boss",
      status: "idle", project_key: "project:/p1", project_label: "P1", project_path: "/p1",
      updated: "2026-11-26T10:00:00Z", created: "2026-11-26T10:00:00Z",
      derived: [{ key: "thread:d1", provider: "codex", model: "gpt-6.1", boss_key: "s-derived-bk", first_seen: "2026-11-26T10:00:00Z", last_active: "2026-11-26T10:00:00Z" }],
    },
    {
      group: "ws-sub", host: "codex", app: "codex", label: "Codex", title: "Subagent",
      owner: "s-derived-bk", parent_workspace_id: "ws-boss", parent_desk_key: "thread:d1",
      status: "running", project_key: "project:/p1", project_label: "P1", project_path: "/p1",
      updated: "2026-11-26T10:05:00Z", created: "2026-11-26T10:01:00Z",
      derived: [{ key: "thread:too-deep", provider: "claude", model: "opus", boss_key: "s-deep-bk" }],
    },
    {
      group: "ws-too-deep", host: "claude", app: "claude", label: "Claude Code", title: "Too Deep",
      owner: "s-deep-bk", parent_workspace_id: "ws-sub", parent_desk_key: "thread:too-deep",
      status: "idle", project_key: "project:/p1", project_label: "P1", project_path: "/p1",
      updated: "2026-11-26T10:06:00Z", created: "2026-11-26T10:02:00Z",
      derived: [],
    },
  ];
  const commonAn = common();
  const win = { addEventListener() {}, matchMedia: () => ({ matches: false, addEventListener() {} }) };
  const AN = {
    ...commonAn,
    h,
    $: id => els.get(id) || makeNode(id),
    at: Date.parse,
    ongoing: s => s || "idle",
    isHidden: () => false,
    onVisibility: () => {},
    href: p => p,
    load: async (url) => {
      if (url.startsWith("/app/terminal/state")) return { sessions, derived_titles: {}, clis: [], active: [] };
      return {};
    },
    GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
    urgent: s => s === "running" || s === "review",
    avatar: () => makeNode("span"),
    colorClass: () => "c0",
    chatName: g => g.display_title || g.title || g.group,
    shownName: g => g.display_title || g.title || g.group,
    activities: () => [],
    plain: x => x,
    stickyOrder: items => items,
    poll() {},
    reducedMotion: { matches: false },
    relTime: () => "刚刚",
    clock: () => "10:00",
    fullTime: () => "2026",
    dayLabel: () => "今天",
    hostTile: () => h("span"),
    cliName: p => p,
    pick: (tbl, s) => tbl[s] || [s || "—", "idle"],
    setText: (n, v) => { n.textContent = v == null ? "" : String(v); },
    setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = String(v); },
    setClass: (n, v) => {
      n.className = v;
      n.classList.set.clear();
      for (const c of String(v).split(/\s+/).filter(Boolean)) n.classList.set.add(c);
    },
    reconcile: (p, nodes) => { p.children = [...nodes]; },
  };
  const context = vm.createContext({
    AN,
    Actions: {
      picked: new Set(),
      ask: async (opts) => { asked.push(opts); return { value: "cancel" }; },
      say() {},
      workspaceMenu() {},
    },
    CHAT_PREFS: { view: "grouped", filter: "all", hideEmpty: false, itemSort: "updated", projectSort: "updated" },
    fetch: async (url) => {
      if (url.startsWith("/app/terminal/state")) return { ok: true, status: 200, json: async () => ({ sessions, derived_titles: {}, clis: [], active: [] }) };
      return { ok: true, status: 200, json: async () => ({}) };
    },
    go() {},
    renderAll() {},
    refreshOffice() {},
    measureAll() {},
    syncListSelection() {},
    renderSideToggle() {},
    openScope() {},
    URLSearchParams,
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    window: win,
    document: { hidden: false, documentElement: { lang: "zh", dataset: {}, classList: { contains: () => false } }, addEventListener() {} },
    location: { origin: "null", search: "", pathname: "/", hash: "" },
    history: { pushState() {}, replaceState() {} },
    addEventListener() {},
    requestAnimationFrame: fn => fn(),
    Date, Map, Set, Array, Object, JSON, Number, String, Boolean, Math, Error, Promise,
    encodeURIComponent, decodeURIComponent,
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
  context.ChatGroups = win.ChatGroups;
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nthis.state = state;\nthis.el = el;\nthis.renderList = renderList;`, context);
  Object.assign(context.state, {
    groupsData,
    selected: "ws-sub",
    loaded: "ws-sub",
    chat: groupsData[1],
    ctx: { desks: new Map(), rooms: new Map() },
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nthis.ChatDerived = ChatDerived;`, context);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nthis.ChatTerminal = ChatTerminal;`, context);
  await context.ChatTerminal.refresh();

  // 1. Subagent workspace merges into the 2nd-level derived row (2 levels max, no gi-grandchild; ws-too-deep stays top-level).
  assert.deepEqual(
    JSON.parse(JSON.stringify(context.ChatTerminal.derivedChildrenOf("ws-boss", "thread:d1").map(g => g.group))),
    ["ws-sub"],
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(context.ChatTerminal.derivedChildrenOf("ws-sub", "thread:too-deep"))),
    [],
  );
  const derivedRow = context.ChatTerminal.derivedOf("ws-boss")[0];
  assert.equal(derivedRow.group, "ws-sub");
  assert.equal(derivedRow.child_workspace_id, "ws-sub");
  assert.equal(derivedRow.live, true, "child workspace running status keeps derived row live");
  context.renderList();
  assert.deepEqual(
    JSON.parse(JSON.stringify(context.state.order)),
    ["ws-too-deep", "ws-boss", "ws-sub"],
  );
  assert.equal(context.state.items.get("ws-sub").el.classList.contains("gi-child"), true);
  assert.equal(context.state.items.get("ws-sub").el.classList.contains("gi-child-first"), true);
  assert.equal(context.state.items.get("ws-sub").el.classList.contains("gi-child-last"), true);
  assert.equal(context.state.items.get("ws-sub").el.classList.contains("gi-grandchild"), false);
  assert.equal(context.state.items.get("ws-too-deep").el.classList.contains("gi-child"), false);
  assert.equal(context.state.items.get("ws-too-deep").el.classList.contains("gi-child-first"), false);
  assert.equal(context.state.items.get("ws-too-deep").el.classList.contains("gi-child-last"), false);

  // 2. Inside ws-sub (the derived office), live derived terminal is hosted directly (End derived terminal while live; Resume once dormant).
  assert.equal(context.ChatTerminal.liveDerivedOwnerOf("ws-sub").id, 50);
  context.ChatTerminal.column("ws-sub", "codex", "codex");
  const stopBtn = drAllAfter.find(n => n.classList.contains("dr-stop"));
  assert.equal(stopBtn.textContent, "结束派生终端");
  assert.equal(stopBtn.hidden, false);
  assert.equal(context.ChatTerminal.showingDerived(), true);

  // End confirmation on boss AND on derived warns about own busy tasks.
  await context.ChatTerminal.close(50);
  assert.ok(
    asked.at(-1).body.some(node => String(node.textContent).includes("仍有组员在运行")),
    "ending derived with own_busy must warn about its own dispatched tasks",
  );
  await context.ChatTerminal.close(1);
  assert.ok(
    asked.at(-1).body.some(node => String(node.textContent).includes("自己派出")),
    "ending parent boss must warn when derived session has own busy tasks",
  );

  // 3. Once derived session exits (dormant), derived office switches to Resume (and keeps running child status in sidebar).
  sessions[0].exit = 0;
  sessions[0].derived_state = "dormant";
  await context.ChatTerminal.refresh();
  assert.equal(context.ChatTerminal.liveDerivedOwnerOf("ws-sub"), null);
  const afterExitRow = context.ChatTerminal.derivedOf("ws-boss")[0];
  assert.equal(afterExitRow.live, true, "running child workspace keeps dormant derived row active");
  assert.equal(context.ChatTerminal.derivedMeta(afterExitRow).text, "运行中");
  context.ChatTerminal.column("ws-sub", "codex", "codex");
  assert.equal(stopBtn.textContent, "续接");
});

test("derived office renders parent back-link, derived boss mood, and dual message perspectives (direct origin vs directory pinned card)", async () => {
  const { context } = auditUI();
  const baseH = context.h;
  const h = (...args) => {
    const node = baseH(...args);
    if (!node.insertBefore) {
      node.insertBefore = (child, ref) => {
        const i = ref ? node.children.indexOf(ref) : -1;
        if (i >= 0) node.children.splice(i, 0, child);
        else node.children.push(child);
        return child;
      };
    }
    if (!("firstChild" in node)) {
      Object.defineProperty(node, "firstChild", { get: () => node.children[0] || null });
    }
    if (!node.classList) {
      const set = new Set(String(node.className || "").split(/\s+/).filter(Boolean));
      node.classList = {
        add: (...ks) => { ks.forEach(k => set.add(k)); node.className = [...set].join(" "); },
        remove: (...ks) => { ks.forEach(k => set.delete(k)); node.className = [...set].join(" "); },
        toggle: (k, force) => {
          const on = force === undefined ? !set.has(k) : Boolean(force);
          if (on) set.add(k); else set.delete(k);
          node.className = [...set].join(" ");
          return on;
        },
        contains: (k) => set.has(k),
      };
    }
    if (!node.replaceChildren) {
      node.replaceChildren = (...kids) => { node.children = kids; };
    }
    if (!node.addEventListener) {
      node.listeners = node.listeners || {};
      node.addEventListener = (ev, fn) => {
        (node.listeners[ev] ||= []).push(fn);
      };
    }
    return node;
  };
  const navigated = [];
  const openedScopes = [];
  const loadCalls = [];
  let turnsReply = {
    turns: [
      {
        index: 1,
        user_text: "派生里我追问一下",
        user_time: "2026-11-28T00:12:00Z",
        assistant_text: "派生终端回复",
        assistant_time: "2026-11-28T00:12:05Z",
        status: "succeeded",
        model: "claude-opus-4-6",
      },
    ],
  };

  // Load ChatDerived + tpDirectory + renderConv helpers in a focused VM context.
  const els = new Map([
    ["boss", h("section")],
    ["drawer", h("aside")],
    ["dr-ico", h("span")],
    ["dr-title", h("b")],
    ["dr-sub", h("span")],
    ["dr-all", h("button")],
    ["dr-derived", h("button", { hidden: true })],
    ["timeline", h("div")],
    ["tl-inner", h("div")],
    ["jump", h("button")],
    ["conv-toggle", h("button")],
  ]);
  const state = {
    selected: "ws-sub",
    loaded: "ws-sub",
    groupsData: [{ group: "ws-boss", title: "主任务", label: "Codex" }],
    chat: null,
    ctx: null,
    drawer: { kind: "all" },
    convOpen: true,
    convCollapsed: false,
    convPeek: false,
    scopeKey: "",
    nodes: new Map(),
    seenIds: new Set(),
    expanded: new Set(),
    unseen: 0,
    ready: true,
    derivedTurns: { cache: new Map(), pending: new Map() },
  };
  const el = {
    boss: els.get("boss"),
    drawer: els.get("drawer"),
    drIco: els.get("dr-ico"),
    drTitle: els.get("dr-title"),
    drSub: els.get("dr-sub"),
    drAll: els.get("dr-all"),
    drDerived: els.get("dr-derived"),
    tl: els.get("timeline"),
    inner: els.get("tl-inner"),
    jump: els.get("jump"),
    convToggle: els.get("conv-toggle"),
    desks: h("div"),
    rooms: h("div"),
    fx: h("div"),
    deskCount: h("span"),
    office: h("div"),
    floor: h("div"),
  };
  const env = vm.createContext({
    AN: {
      ...context.AN,
      h,
      L: (zh) => zh,
      at: Date.parse,
      setText: context.setText,
      setAttr: context.setAttr,
      setClass: context.setClass,
      reconcile: (p, nodes) => { p.children = [...nodes]; },
      href: p => p,
      shownName: g => g.display_title || g.title || g.group,
      chatName: g => g.display_title || g.title || g.group,
      deskName: d => d.model || d.key || "",
      cliOf: d => d.provider || "",
      cliName: p => p || "",
      relTime: () => "刚刚",
      clock: () => "00:10",
      dayLabel: () => "今天",
      fullTime: () => "2026-11-28",
      plain: s => String(s || ""),
      hostTile: () => h("span"),
      art: { character: () => "c", figure: () => h("div"), head: () => h("span"), lid: () => h("span"), url: p => p, version: () => 1, canWarp: () => false },
      pick: (tbl, k) => tbl?.[k] || [k || "", "idle"],
      MEMBER: { succeeded: ["成功", "done"], failed: ["失败", "bad"] },
      GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
      urgent: s => s === "running" || s === "review",
      ongoing: s => s || "idle",
      isHidden: () => false,
      dozing: () => false,
      activities: () => [],
    },
    h,
    L: (zh) => zh,
    el,
    state,
    ART: { character: () => "c", figure: () => h("div"), head: () => h("span"), lid: () => h("span"), url: p => p, version: () => 1, canWarp: () => false },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { roomMenu() {}, openMenu() {} },
    ChatTerminal: {
      terminalOf: () => ({ id: 77, derived: true, derived_state: "running", exit: null }),
      derivedStateOf: x => x.derived_state,
      isWaking: () => false,
      reviving: () => false,
      ofDesk: () => null,
      showing: () => false,
      bossOpensTerminal: () => true,
      openDerivedOffice: (...args) => navigated.push(["openDerivedOffice", ...args]),
    },
    overlay: { matches: false },
    reduceMotion: { matches: false },
    FIG: 100,
    FOLD_PX: 320,
    NEAR_BOTTOM: 64,
    GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
    pick: (tbl, k) => tbl?.[k] || [k || "", "idle"],
    urgent: s => s === "running" || s === "review",
    activities: () => [],
    plain: s => String(s || ""),
    deskChar: () => "c",
    deskName: d => d.model || d.key || "",
    deskParts: d => ({ name: d.model || "", effort: "" }),
    cliOf: d => d.provider || "",
    hostTile: () => h("span"),
    relTime: () => "刚刚",
    clock: () => "00:10",
    dayLabel: () => "今天",
    fullTime: () => "2026-11-28",
    shownName: g => g.display_title || g.title || g.group,
    chatName: g => g.display_title || g.title || g.group,
    setText: context.setText,
    setAttr: context.setAttr,
    setClass: context.setClass,
    reconcile: (p, nodes) => { p.children = [...nodes]; },
    href: p => p,
    go: p => navigated.push(p),
    load: async (url) => { loadCalls.push(url); return turnsReply; },
    openScope: (scope) => { openedScopes.push(scope); state.drawer = scope; },
    nearBottom: () => true,
    window: { addEventListener() {} },
    document: { documentElement: { lang: "zh", classList: { contains: () => false } }, addEventListener() {} },
    ResizeObserver: class { observe() {} },
    getComputedStyle: () => ({ getPropertyValue: () => "2", columnGap: "10px", gridTemplateColumns: "1fr" }),
    Date, Map, Set, Array, Object, JSON, Number, String, Boolean, Math, Promise, encodeURIComponent,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, env);
  vm.runInContext(readFileSync(`${__dirname}/../chat-derived-turns.js`, "utf8"), env);
  vm.runInContext(readFileSync(`${__dirname}/../chat-messages.js`, "utf8"), env);
  vm.runInContext(readFileSync(`${__dirname}/../chat-topics.js`, "utf8"), env);
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), env);

  // 1. Derived boss mood + parent back-link (.boss-link).
  const derivedOrigin = {
    parent_workspace_id: "ws-boss",
    parent_desk_key: "thread:d1",
    parent_title: "主任务",
    parent_label: "Codex",
    desk: { key: "thread:d1", provider: "claude", model: "claude-opus-4-6", derived: true, status: "idle" },
    messages: [
      { id: "m-up-1", kind: "prompt", speaker: { role: "manager", provider: "codex", label: "Codex" }, desk: "thread:d1", thread: { kind: "desk", key: "thread:d1" }, text: "帮我查登录问题", time: "2026-11-28T00:10:00Z" },
      { id: "m-up-2", kind: "reply", speaker: { role: "member", provider: "claude", model: "claude-opus-4-6", label: "Claude Code" }, desk: "thread:d1", thread: { kind: "desk", key: "thread:d1" }, text: "已定位原因", time: "2026-11-28T00:11:00Z", reply_to: "m-up-1" },
    ],
  };
  const derivedChatNoSub = {
    group: "drv-ws_2dboss-thread_3Ad1",
    derived: true,
    host: "claude",
    label: "Claude Code",
    title: "调试派生",
    parent_workspace_id: "ws-boss",
    parent_desk_key: "thread:d1",
    derived_origin: derivedOrigin,
    desks: [],
    rooms: [],
    messages: [],
  };
  const txt = n => n?.textContent || (n?.children || []).filter(c => typeof c === "string").join("");
  const rec = env.createBoss();
  env.updateBoss(rec, derivedChatNoSub, { running: 0, review: 0 });
  assert.equal(rec.st.textContent, "工作中");
  const backLink = rec.state.children.find(n => n?.tag === "a.boss-link");
  assert.ok(backLink, "derived office must show .boss-link back to parent");
  assert.equal(txt(backLink), "派生自 主任务");
  backLink.listeners.click({ preventDefault() {}, stopPropagation() {} });
  assert.equal(navigated.at(-1), "/chat/ws-boss");

  // Waiting on downstream replies takes priority over derived terminal working state.
  env.updateBoss(rec, derivedChatNoSub, { running: 2, review: 0 });
  assert.equal(rec.st.textContent, "等 2 个回复");

  // 2. Directory with downstream tasks pins "主对话 / 派生对话" card at the top, and clicking opens { kind: "origin" }.
  const derivedChatWithSub = {
    ...derivedChatNoSub,
    group: "ws-sub",
    desks: [{ key: "d-sub", provider: "codex", model: "gpt-6.1", derived: false }],
    messages: [
      { id: "m-sub-1", kind: "prompt", speaker: { role: "manager", provider: "claude", label: "Claude Code" }, desk: "d-sub", thread: { kind: "desk", key: "d-sub" }, text: "写单测", time: "2026-11-28T00:15:00Z" },
    ],
  };
  const subCtx = {
    c: derivedChatWithSub,
    desks: new Map([["d-sub", derivedChatWithSub.desks[0]]]),
    rooms: new Map(),
    seatDesk: new Map(),
    sources: new Set(),
  };
  const dirCards = env.tpDirectory(derivedChatWithSub, subCtx);
  assert.equal(dirCards.length, 2);
  assert.equal(txt(dirCards[0].children[0].children[0]), "主对话");
  assert.equal(txt(dirCards[0].children[0].children[1]), "派生对话");
  (dirCards[0].attributes?.onclick || dirCards[0].listeners?.click || dirCards[0].onclick)();
  assert.deepEqual(JSON.parse(JSON.stringify(openedScopes.at(-1))), { kind: "origin" });

  // 3. Parent directory labels derived tasks as "派生" rather than "任务".
  const parentChat = {
    group: "ws-boss",
    host: "codex",
    label: "Codex",
    desks: [{ key: "thread:d1", provider: "claude", model: "claude-opus-4-6", derived: true }],
    rooms: [],
    messages: derivedOrigin.messages,
  };
  const parentCtx = {
    c: parentChat,
    desks: new Map([["thread:d1", parentChat.desks[0]]]),
    rooms: new Map(),
    seatDesk: new Map(),
    sources: new Set(),
  };
  const parentCards = env.tpDirectory(parentChat, parentCtx);
  assert.equal(txt(parentCards[0].children[0].children[0]), "派生");

  // 4. Requesting derived turns in origin mode hits parent workspace + deskKey and merges user turns.
  state.chat = derivedChatNoSub;
  state.ctx = { c: derivedChatNoSub, desks: new Map(), rooms: new Map(), seatDesk: new Map(), sources: new Set() };
  env.renderConv = () => {};
  env.requestDerivedTurns(derivedChatNoSub, state.ctx, "thread:d1", { workspace: "ws-boss", desk: derivedOrigin.desk, origin: true });
  await state.derivedTurns.pending.get("ws-boss|thread:d1");
  assert.equal(loadCalls[0], "/app/terminal/derived-turns?workspace=ws-boss&desk=thread%3Ad1");
  const entry = env.requestDerivedTurns(derivedChatNoSub, state.ctx, "thread:d1", { workspace: "ws-boss", desk: derivedOrigin.desk, origin: true });
  const merged = env.mergeDerivedMessages(
    derivedOrigin.messages,
    env.buildDerivedUserMessages(derivedChatNoSub, derivedOrigin.desk, "thread:d1", entry.turns),
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(merged.map(m => m.text))),
    ["帮我查登录问题", "已定位原因", "派生里我追问一下", "派生终端回复"],
  );

  // 5. Idle derived terminal with running/review child workspace reports running/review wording in boss-side.
  const sideState = { chat: { group: "ws-boss" } };
  const sideCtx = vm.createContext({
    AN: { L: (zh) => zh, at: (s) => Date.parse(s) || 0, relTime: () => "刚刚", deskName: (d) => d.model || "", cliOf: (d) => d.provider || "", cliName: (p) => p, shownName: (g) => g.title || "" },
    state: sideState,
    ChatTerminal: {
      ofDesk: () => null,
      derivedOf: () => [
        { group: "ws-child-run", desk_key: "thread:d-run", title: "派生跑子任务", child_status: "running", updated: "2026-11-28T00:05:00Z" },
        { group: "ws-child-rev", desk_key: "thread:d-rev", title: "派生待审核", child_status: "review", updated: "2026-11-28T00:04:00Z" },
      ],
      derivedStateOf: () => "idle",
      isWaking: () => false,
      reviving: () => false,
    },
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, sideCtx);
  const sideRows = sideCtx.ChatDerived.sideItems({
    group: "ws-boss",
    desks: [
      { key: "thread:d-run", provider: "claude", model: "claude-opus-4-6", derived: true, last_active: "2026-11-28T00:05:00Z" },
      { key: "thread:d-rev", provider: "codex", model: "gpt-5.4", derived: true, last_active: "2026-11-28T00:04:00Z" },
    ],
  });
  const byId = Object.fromEntries(sideRows.map((r) => [r.id, r]));
  assert.equal(byId["thread:d-run"].tone, "run");
  assert.equal(byId["thread:d-run"].sub.startsWith("运行中"), true);
  assert.equal(byId["thread:d-rev"].tone, "review");
  assert.equal(byId["thread:d-rev"].sub.startsWith("待审核"), true);
});

test("partitionNested prevents pending-handover parent collision and keeps N:1 secondary child workspaces top-level", () => {
  const { context } = auditUI();
  const baseH = context.h;
  const makeNode = (tag = "div") => {
    const set = new Set();
    const node = baseH(tag);
    node.closest = () => null;
    node.classList = {
      add: (...ks) => ks.forEach((k) => set.add(k)),
      remove: (...ks) => ks.forEach((k) => set.delete(k)),
      toggle: (k, f) => { const on = f === undefined ? !set.has(k) : Boolean(f); if (on) set.add(k); else set.delete(k); return on; },
      contains: (k) => set.has(k),
    };
    return node;
  };
  const h = (tag, attrs = {}, ...kids) => {
    const n = baseH(tag, attrs, ...kids);
    const set = new Set();
    n.classList = {
      add: (...ks) => ks.forEach((k) => set.add(k)),
      remove: (...ks) => ks.forEach((k) => set.delete(k)),
      toggle: (k, f) => { const on = f === undefined ? !set.has(k) : Boolean(f); if (on) set.add(k); else set.delete(k); return on; },
      contains: (k) => set.has(k),
    };
    return n;
  };
  const env = vm.createContext({
    AN: {
      ...context.AN,
      h,
      L: (zh) => zh,
      $: () => makeNode(),
      at: (s) => Date.parse(s) || 0,
      setText: context.setText,
      setAttr: context.setAttr,
      setClass: (n, cls) => { n.className = cls; },
      reconcile: (p, nodes) => { p.children = [...nodes]; },
      href: (p) => p,
      shownName: (g) => g.title || g.group,
      chatName: (g) => g.title || g.group,
      colorClass: () => "c0",
      hostTile: () => h("span"),
      pick: (tbl, k) => tbl?.[k] || [k || "", "idle"],
      GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
      urgent: (s) => s === "running" || s === "review",
      ongoing: (s) => s || "idle",
      cliName: (p) => p,
      stored: () => null,
      store: () => {},
    },
    h,
    $: () => makeNode(),
    L: (zh) => zh,
    setText: context.setText,
    setAttr: context.setAttr,
    setClass: (n, cls) => { n.className = cls; },
    reconcile: (p, nodes) => { p.children = [...nodes]; },
    href: (p) => p,
    go: () => {},
    shownName: (g) => g.title || g.group,
    chatName: (g) => g.title || g.group,
    colorClass: () => "c0",
    hostTile: () => h("span"),
    pick: (tbl, k) => tbl?.[k] || [k || "", "idle"],
    GROUP: { running: ["运行中", "run"], idle: ["空闲", "idle"] },
    urgent: (s) => s === "running" || s === "review",
    stored: () => null,
    store: () => {},
    Actions: { picked: new Set(), pick: () => false, workspaceMenu() {}, projectMenu() {} },
    ChatGroups: {
      handover: ":handover",
      caps: { groups: 10, items: 10, groupStep: 5, itemStep: 5 },
      defaultPrefs: { view: "project" },
      arrange: (groups) => {
        const pending = groups.filter((g) => g.handover === "pending");
        const normal = groups.filter((g) => g.handover !== "pending");
        const sections = [];
        if (pending.length) sections.push({ key: ":handover", title: "待接手", pinned: true, items: pending });
        if (normal.length) sections.push({ key: "/proj", title: "proj", pinned: false, items: normal });
        return sections;
      },
      capSections: (sections) => ({
        sections: sections.map((s) => ({ ...s, shown: s.items, hidden: 0 })),
        hiddenGroups: 0,
      }),
    },
    window: { Avatars: { custom: () => null }, matchMedia: () => ({ matches: false, addEventListener() {} }), addEventListener() {} },
    document: { documentElement: { lang: "zh" }, addEventListener() {} },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    Date, Map, Set, Array, Object, JSON, Math, Number, String, Boolean, encodeURIComponent, decodeURIComponent,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-state.js`, "utf8")}\nglobalThis.state = state;`, env);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, env);
  // Provide minimal ChatTerminal stub backed by ChatDerived for partitionNested & groupedNodes.
  const derivedByParent = {
    "ws-pending": [
      { group: "ws-child-pending", derived: true, parent_workspace_id: "ws-pending", desk_key: "thread:d-pend", child_workspace_id: "ws-child-pending", host: "claude", label: "Claude", title: "派生待交接", status: "idle" },
    ],
    "ws-n1": [
      { group: "ws-child-primary", derived: true, parent_workspace_id: "ws-n1", desk_key: "thread:d-n1", child_workspace_id: "ws-child-primary", host: "claude", label: "Claude", title: "主子工作区", status: "idle" },
    ],
  };
  env.ChatTerminal = {
    derivedOf: (p) => derivedByParent[p] || [],
    derivedChildrenOf: (p) => (p === "ws-n1" ? [{ group: "ws-child-primary" }, { group: "ws-child-secondary" }] : []),
    derivedAll: () => Object.values(derivedByParent).flat(),
    derivedMeta: () => ({ text: "空闲", tone: "idle" }),
    derivedSummary: () => null,
    rowPath: (g) => `/chat/${g.group}`,
    decorate: () => {},
    headAction: () => {},
    menu: () => false,
  };
  const groups = [
    { group: "ws-pending", title: "待交接父", host: "codex", label: "Codex", handover: "pending", status: "idle", updated: "2026-11-28T00:05:00Z" },
    { group: "ws-child-pending", title: "待交接父的子", host: "claude", label: "Claude", parent_workspace_id: "ws-pending", parent_desk_key: "thread:d-pend", status: "idle", updated: "2026-11-28T00:04:00Z" },
    { group: "ws-n1", title: "N1父", host: "codex", label: "Codex", status: "idle", updated: "2026-11-28T00:03:00Z" },
    { group: "ws-child-primary", title: "主子工作区", host: "claude", label: "Claude", parent_workspace_id: "ws-n1", parent_desk_key: "thread:d-n1", status: "idle", updated: "2026-11-28T00:02:00Z" },
    { group: "ws-child-secondary", title: "次要子工作区", host: "claude", label: "Claude", parent_workspace_id: "ws-n1", parent_desk_key: "thread:d-n1", status: "idle", updated: "2026-11-28T00:01:00Z" },
  ];
  env.state.groupsData = groups;
  const { topGroups } = env.partitionNested(groups);
  const topIds = topGroups.map((g) => g.group);
  assert.deepEqual(topIds, ["ws-pending", "ws-n1", "ws-child-secondary"]);

  env.groupedNodes(groups, { prefs: { view: "project" }, query: "" });
  assert.deepEqual(
    JSON.parse(JSON.stringify(env.state.order)),
    ["ws-pending", "ws-child-pending", "ws-n1", "ws-child-primary", "ws-child-secondary"],
  );
  assert.equal(new Set(env.state.order).size, env.state.order.length, "state.order must have no duplicate ids");
});

test("merged derived row context menu keeps terminal actions plus child workspace rename/handover/delete while disabling pin and batch pick", async () => {
  const menus = [];
  const makeNode = (tag = "div") => {
    const set = new Set();
    return {
      tag,
      children: [],
      dataset: {},
      style: {},
      hidden: false,
      disabled: false,
      textContent: "",
      attributes: {},
      classList: {
        add: (...ks) => ks.forEach((k) => set.add(k)),
        remove: (...ks) => ks.forEach((k) => set.delete(k)),
        toggle: (k, f) => { const on = f === undefined ? !set.has(k) : Boolean(f); if (on) set.add(k); else set.delete(k); return on; },
        contains: (k) => set.has(k),
      },
      append(...xs) { this.children.push(...xs); },
      after() {},
      before() {},
      replaceChildren(...xs) { this.children = xs; },
      setAttribute(k, v) { this.attributes[k] = v; },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener() {},
      querySelector: () => null,
      querySelectorAll: () => [],
      getBoundingClientRect: () => ({ width: 100, height: 100 }),
      contains: () => false,
    };
  };
  const h = (tag, attrs = {}, ...kids) => {
    const n = makeNode(tag);
    Object.assign(n, attrs);
    n.children = kids.flat(Infinity).filter((x) => x != null && x !== false);
    if (kids.length === 1 && typeof kids[0] === "string") n.textContent = kids[0];
    return n;
  };
  const posted = [];
  const AN = {
    h,
    $: () => null,
    L: (zh) => zh,
    at: (s) => Date.parse(s) || 0,
    load: async (url) => {
      if (url.startsWith("/app/terminal/state")) {
        return {
          sessions: [
            { id: 88, provider: "claude", derived: true, derived_state: "idle", parent_workspace_id: "ws-boss", desk_key: "thread:d-sub", exit: null, workspaces: [] },
          ],
          derived_titles: {},
          cwd: "/repo",
        };
      }
      return {};
    },
    post: async (path, body) => { posted.push([path, body]); return {}; },
    chatName: (g) => g.title || g.group,
    urgent: () => false,
    cliName: (p) => p,
    hostTile: () => h("span"),
    deskName: (d) => d.model || "",
    handleOf: () => "",
    onVisibility: () => {},
    isHidden: () => false,
    href: (p) => p,
  };
  const state = {
    selected: "ws-boss",
    loaded: "ws-boss",
    order: ["ws-boss", "ws-child"],
    groupsData: [
      {
        group: "ws-boss",
        title: "主任务",
        host: "codex",
        label: "Codex",
        status: "idle",
        updated: "2026-11-28T00:05:00Z",
        derived: [
          { key: "thread:d-sub", provider: "claude", model: "claude-opus-4-6", child_workspace_id: "ws-child", last_active: "2026-11-28T00:05:00Z" },
          { key: "thread:d-plain", provider: "codex", model: "gpt-6.1", child_workspace_id: null, last_active: "2026-11-28T00:04:00Z" },
        ],
      },
      {
        group: "ws-child",
        title: "派生子工作区",
        title_id: "conv-child-1",
        host: "claude",
        label: "Claude Code",
        parent_workspace_id: "ws-boss",
        parent_desk_key: "thread:d-sub",
        status: "idle",
        updated: "2026-11-28T00:05:00Z",
      },
    ],
    drawer: { kind: "all" },
    convOpen: false,
  };
  const ctx = vm.createContext({
    AN,
    state,
    el: {
      app: makeNode("div"),
      drawer: makeNode("aside"),
      drAll: makeNode("button"),
      drDerived: makeNode("button"),
      drIco: makeNode("span"),
      drTitle: makeNode("b"),
      drSub: makeNode("span"),
      tl: makeNode("div"),
    },
    setText: (n, s) => { n.textContent = s; },
    setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
    go: () => {},
    refresh: () => {},
    renderList: () => {},
    openScope: () => {},
    addEventListener() {},
    document: { body: makeNode("body"), documentElement: { dataset: {} }, addEventListener() {}, activeElement: null },
    window: { addEventListener() {} },
    innerWidth: 1200,
    innerHeight: 800,
    location: { origin: "http://localhost" },
    Date, Map, Set, Array, Object, JSON, Math, Number, String, Boolean, Promise, encodeURIComponent, decodeURIComponent,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../actions.js`, "utf8")}\nglobalThis.Actions = Actions;`, ctx);
  ctx.Actions.openMenu = (_e, items) => menus.push(items.map((x) => x.label));
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, ctx);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, ctx);
  await ctx.ChatTerminal.refresh();

  // Merged child workspace row gets terminal items + child workspace management items.
  assert.equal(ctx.ChatTerminal.menu("ws-child", { preventDefault() {}, stopPropagation() {} }), true);
  assert.deepEqual([...menus.at(-1)], ["打开", "结束派生终端", "改名…", "交给其他 boss 接手…", "删除 agent…"]);

  // Pure placeholder drv-... row (no child_workspace_id) keeps only terminal items.
  const plainId = ctx.ChatTerminal.derivedRowId("ws-boss", "thread:d-plain");
  assert.equal(ctx.ChatTerminal.menu(plainId, { preventDefault() {}, stopPropagation() {} }), true);
  assert.deepEqual([...menus.at(-1)], ["打开", "续接"]);

  // Merged derived row still disables pinGroup and multi-select pick.
  await ctx.Actions.pinGroup("ws-child", true);
  assert.equal(posted.length, 0);
  assert.equal(ctx.Actions.pick("ws-child", { metaKey: true }), false);
  assert.equal(ctx.Actions.picked.size, 0);
});

test("follow preserves overlay drawer via openOnce on derived route upgrades and recentlyActive falls back to derived updated", async () => {
  const makeNode = (tag = "div") => {
    const set = new Set();
    return {
      tag,
      children: [],
      dataset: {},
      style: {},
      hidden: false,
      disabled: false,
      textContent: "",
      attributes: {},
      classList: {
        add: (...ks) => ks.forEach((k) => set.add(k)),
        remove: (...ks) => ks.forEach((k) => set.delete(k)),
        toggle: (k, f) => { const on = f === undefined ? !set.has(k) : Boolean(f); if (on) set.add(k); else set.delete(k); return on; },
        contains: (k) => set.has(k),
      },
      append(...xs) { this.children.push(...xs); },
      after() {},
      before() {},
      replaceChildren(...xs) { this.children = xs; },
      setAttribute(k, v) { this.attributes[k] = v; },
      removeAttribute(k) { delete this.attributes[k]; },
      addEventListener() {},
      remove() {},
      focus() {},
    };
  };
  const h = (tag, attrs = {}, ...kids) => {
    const n = makeNode(tag);
    Object.assign(n, attrs);
    n.children = kids.flat(Infinity).filter((x) => x != null && x !== false);
    if (kids.length === 1 && typeof kids[0] === "string") n.textContent = kids[0];
    return n;
  };
  let sessions = [
    { id: 51, provider: "claude", derived: true, derived_state: "running", parent_workspace_id: "ws-boss", desk_key: "thread:d1", exit: null, workspaces: [] },
  ];
  const nowIso = new Date(Date.now() - 30_000).toISOString();
  const state = {
    selected: "term-51",
    loaded: "term-51",
    convOpen: true,
    drawer: { kind: "all" },
    groupsData: [
      {
        group: "ws-boss",
        title: "主任务",
        host: "codex",
        label: "Codex",
        status: "idle",
        updated: nowIso,
        derived: [{ key: "thread:d1", provider: "claude", model: "claude-opus-4-6", last_active: nowIso }],
      },
    ],
  };
  const ctx = vm.createContext({
    AN: {
      h,
      L: (zh) => zh,
      at: (s) => Date.parse(s) || 0,
      SLEEP_AFTER: 30 * 60 * 1000,
      load: async () => ({ sessions, derived_titles: {}, cwd: "/repo" }),
      cliName: (p) => p,
      hostTile: () => h("span"),
      deskName: (d) => d.model || "",
      handleOf: () => "",
      onVisibility: () => {},
      isHidden: () => false,
      href: (p) => p,
    },
    state,
    el: {
      app: makeNode("div"),
      drawer: makeNode("aside"),
      drAll: makeNode("button"),
      drDerived: makeNode("button"),
      drIco: makeNode("span"),
      drTitle: makeNode("b"),
      drSub: makeNode("span"),
      tl: makeNode("div"),
      boss: makeNode("section"),
      desks: makeNode("div"),
      rooms: makeNode("div"),
      fx: makeNode("div"),
      floor: makeNode("div"),
    },
    h,
    L: (zh) => zh,
    ART: { character: () => "c", figure: () => h("div"), head: () => h("span"), lid: () => h("span"), url: (p) => p },
    Laptop: { create: () => ({ laptop: { dress() {} } }) },
    Actions: { openMenu() {}, roomMenu() {} },
    setText: (n, s) => { n.textContent = s; },
    setAttr: (n, k, v) => { if (v == null) delete n.attributes[k]; else n.attributes[k] = v; },
    setClass: (n, c) => { n.className = c; },
    go: () => {},
    href: (p) => p,
    addEventListener() {},
    requestAnimationFrame: (fn) => fn(),
    ResizeObserver: class { observe() {} },
    window: { addEventListener() {} },
    document: { documentElement: { dataset: {}, lang: "zh" }, addEventListener() {} },
    location: { origin: "http://localhost" },
    Date, Map, Set, Array, Object, JSON, Math, Number, String, Boolean, Promise, encodeURIComponent, decodeURIComponent,
  });
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-derived.js`, "utf8")}\nglobalThis.ChatDerived = ChatDerived;`, ctx);
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, ctx);
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), ctx);
  await ctx.ChatTerminal.refresh();

  // 1. term-51 -> drv-... while narrow overlay drawer is open sets openOnce.
  const drvId = ctx.ChatTerminal.derivedRowId("ws-boss", "thread:d1");
  assert.equal(ctx.ChatTerminal.follow("term-51"), drvId);
  assert.equal(ctx.ChatTerminal.consumeOpen(drvId), true);

  // 2. drv-... -> child_workspace_id while convOpen is true sets openOnce.
  state.groupsData[0].derived[0].child_workspace_id = "ws-sub-1";
  state.groupsData.push({
    group: "ws-sub-1",
    title: "子工作区",
    host: "claude",
    label: "Claude",
    parent_workspace_id: "ws-boss",
    parent_desk_key: "thread:d1",
    status: "running",
    updated: nowIso,
  });
  assert.equal(ctx.ChatTerminal.follow(drvId), "ws-sub-1");
  assert.equal(ctx.ChatTerminal.consumeOpen("ws-sub-1"), true);

  // 3. When drawer is closed (narrow overlay) or folded (wide), follow does NOT force openOnce even if .term-mode is set.
  ctx.el.drawer.classList.add("term-mode");
  state.convOpen = false;
  ctx.el.drawer.hidden = true;
  assert.equal(ctx.ChatTerminal.follow(drvId), "ws-sub-1");
  assert.equal(ctx.ChatTerminal.consumeOpen("ws-sub-1"), false);
  ctx.el.drawer.hidden = false;
  ctx.el.drawer.classList.add("folded");
  state.convCollapsed = true;
  assert.equal(ctx.ChatTerminal.follow(drvId), "ws-sub-1");
  assert.equal(ctx.ChatTerminal.consumeOpen("ws-sub-1"), false);
  ctx.el.drawer.classList.remove("folded");
  ctx.el.drawer.classList.remove("term-mode");
  state.convCollapsed = false;

  // 3b. Merged derived row prefers child workspace display_title (e.g. custom rename) to match decorateDerivedOffice.
  state.groupsData[1].display_title = "自定义子名称";
  const merged = ctx.ChatTerminal.derivedOf("ws-boss").find((d) => d.group === "ws-sub-1");
  assert.equal(merged?.display_title, "自定义子名称");
  assert.equal(merged?.title, "自定义子名称");

  // 4. recentlyActive falls back to c.updated and ChatTerminal.findDerivedRow(c.group)?.updated for drv-... not in groupsData.
  assert.equal(ctx.recentlyActive({ group: drvId, updated: nowIso }), true);
  assert.equal(ctx.recentlyActive({ group: drvId, updated: "2020-01-01T00:00:00Z" }), false);
  // Without c.updated, falls back to derived row's updated.
  delete state.groupsData[0].derived[0].child_workspace_id;
  state.groupsData.pop();
  assert.equal(ctx.recentlyActive({ group: drvId }), true);
});
