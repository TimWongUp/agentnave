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
  return { rec, boss: context.el.boss, update: c => context.updateBoss(rec, c, { running: 0, review: 0 }) };
}

test("a boss whose terminal CLI runs shows as working", () => {
  let open = true;
  const { rec, boss, update } = office(undefined, { running: group => open && group === "ws" });
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.equal(boss.dataset.mood, "working");
  assert.equal(rec.st.textContent, "工作中");
  open = false;
  update({ group: "ws", host: "claude", label: "Claude Code", messages: [] });
  assert.notEqual(boss.dataset.mood, "working");
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

test("Devin boss uses its own furniture and the shared 3D laptop", () => {
  const app = office();
  app.update({ host: "devin", app: "devin", label: "Devin CLI", messages: [speech("devin", "swe-2-flash-medium", "2026-11-08T15:00:00Z")] });
  assert.equal(app.rec.desk.src, "office/boss-desk-devin.webp");
  assert.equal(app.rec.chair.src, "office/boss-chair-devin.webp");
  assert.equal(app.rec.laptop.set, "devin");
  assert.equal(app.rec.lid.children[0].host, "devin");
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
    state: { sideCollapsed: true, convCollapsed: true, chat: {} }, narrow: { matches: false }, overlay: { matches: false }, innerWidth: 1200,
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
  const start = source.indexOf("for (const query of [narrow, overlay])");
  const listeners = [], removed = [], order = [];
  const query = { addEventListener: (_name, fn) => listeners.push(fn) };
  const state = { sidePeek: true, convPeek: true, chat: {} };
  const context = vm.createContext({
    narrow: query, overlay: query, state, el: { app: { removeAttribute: name => removed.push(name) } },
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
      picked: new Set(), pick: () => false, workspaceMenu() {},
      projectMenu(section) { menuCalls.push(section.key); },
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

  // Context menu opens only on empty manual headers, never on non-empty manual, auto, or clicking '+'.
  for (const key of ["project:/w/app", "project:/w/p0", "project:/w/solo"]) {
    const rec = context.state.heads.get(key);
    for (const fn of rec.el.listeners.contextmenu) fn({ target: { closest: () => null } });
  }
  assert.deepEqual(menuCalls, ["/w/solo"]);
  for (const fn of context.state.heads.get("project:/w/solo").el.listeners.contextmenu) {
    fn({ target: { closest: sel => sel === ".gl-grp-add" } });
  }
  assert.deepEqual(menuCalls, ["/w/solo"]);

  context.revealProject("/w/deep");
  assert.equal(storage.has("agentnave.office.fold.project:/w/deep"), false);
  assert.equal(context.state.heads.get("project:/w/deep").el.classList.contains("flash"), true);
});

test("open derived terminals stay under their parent when idle, past the cap and in a folded project", () => {
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
      derivedOf: parent => derived.filter(d => d.parent_workspace_id === parent),
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
  const child = (live) => ({
    group: "term-20", terminal: 20, host: "antigravity", app: "antigravity", label: "Antigravity", title: "Add Sub Function",
    status: live ? "running" : "idle", live, updated: "2026-11-12T10:00:00Z", derived: true,
    parent_workspace_id: "g-parent", blocked_kind: null, project_key: "/w/calc", project_label: "calc", project_path: "/w/calc", preview: "",
  });
  // The parent is the oldest of seven agents in its project, so only a running boss keeps it past the cap.
  const groups = [agent("g-parent", "2026-11-01T00:00:00Z")];
  for (let i = 0; i < 6; i++) groups.push(agent(`g-other${i}`, `2026-11-1${i}T00:00:00Z`));
  context.state.groupsData = groups;
  const rows = () => {
    context.renderList();
    return els.get("list").children.filter(n => n.attributes?.href);
  };
  const shown = () => rows().filter(n => n.attributes["data-folded"] !== "true").map(n => n.attributes.href);

  derived = [child(true)];
  assert.ok(shown().includes("/chat/term-20"), "running child shows under its parent");
  derived = [child(false)];
  assert.deepEqual(shown().slice(shown().indexOf("/chat/g-parent"), shown().indexOf("/chat/g-parent") + 2), ["/chat/g-parent", "/chat/term-20"]);

  context.state.folded.set("project:/w/calc", true);
  const folded = shown();
  assert.deepEqual(folded, ["/chat/g-parent", "/chat/term-20"]);
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

  context.Actions.projectMenu({ key: "/tmp/repo", manual: true, items: [{ group: "g1" }] }, { clientX: 10, clientY: 10, preventDefault() {}, stopPropagation() {} });
  context.Actions.projectMenu({ key: "/tmp/repo", manual: false, items: [] }, { clientX: 10, clientY: 10, preventDefault() {}, stopPropagation() {} });
  assert.equal(menu.hidden, true);

  context.Actions.projectMenu({ key: "/tmp/repo", manual: true, items: [] }, { clientX: 10, clientY: 10, preventDefault() {}, stopPropagation() {} });
  assert.equal(menu.hidden, false);
  assert.equal(menu.children[0].textContent, "移除项目");
  menu.children[0].listeners.click[0]();
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
  assert.deepEqual(plain(posts.at(-1)), ["/app/projects/remove", { key: "/tmp/repo" }]);
  assert.deepEqual(plain(state.projects), []);
  assert.equal(refreshes, 2);
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
  vm.runInContext(`${readFileSync(`${__dirname}/../chat-terminal.js`, "utf8")}\nglobalThis.ChatTerminal = ChatTerminal;`, context);
  await context.ChatTerminal.refresh();

  // Idle resumable claude, grok, and antigravity stop directly without confirmation.
  for (const id of [1, 5, 6]) {
    await context.ChatTerminal.stop(id);
    assert.equal(asks.length, 0);
    assert.equal(calls.at(-1), `/app/terminal/stop?session=${id}`);
  }

  // Un-dispatched, non-resumable pi, and resumable with running members all require confirmation with Cancel focused.
  nextAnswer = null;
  for (const [id, expectedReason] of [[3, "还没有派过活"], [2, "暂不支持"], [4, "仍有组员在运行"]]) {
    asks.length = 0;
    const before = calls.length;
    await context.ChatTerminal.stop(id);
    assert.equal(asks.length, 1);
    assert.equal(asks[0].buttons[0].label, "取消");
    assert.equal(asks[0].buttons[0].focus, true);
    assert.equal(asks[0].buttons[1].label, "结束终端");
    assert.ok(asks[0].body.some(p => p.textContent.includes(expectedReason)));
    assert.equal(calls.length, before, "cancelling must not stop");
  }

  // Exited postMessage updates header immediately.
  context.ChatTerminal.column("ws-idle", "claude");
  assert.equal(stopButton.textContent, "结束终端");
  assert.equal(context.el.drSub.textContent, "Claude Code");
  msgListeners[0]({ data: { type: "agentnave:terminal", action: "exited", session: 1, code: 0 } });
  assert.equal(context.el.drSub.textContent, "Claude Code · 已结束");
  assert.equal(stopButton.textContent, "续接");

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
      querySelector() { return null; },
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

  // Drawer shows derived title instead of "boss", and hides restart on ended derived terminal.
  mockSessions[1].exit = 0;
  mockSessions[1].derived_busy = false;
  await context.ChatTerminal.refresh();
  context.ChatTerminal.column("term-20", "codex", "codex");
  assert.equal(el.drTitle.textContent, "派生调试");
  assert.equal(drAll._after[1].hidden, true);

  // Closing selected derived terminal returns to its parent workspace.
  mockSessions = [mockSessions[0]];
  await context.ChatTerminal.close(20);
  assert.deepEqual(navigated, ["/chat/ws-parent"]);
});
