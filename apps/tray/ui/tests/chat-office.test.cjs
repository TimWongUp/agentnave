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
});

function office(character = (_host, model) => model || "default") {
  const node = () => ({
    dataset: {}, classList: { toggle() {} }, children: [],
    replaceChildren(...children) { this.children = children; },
    insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
    remove() {},
  });
  const context = vm.createContext({
    AN: { at: Date.parse }, window: {}, ResizeObserver: class { observe() {} },
    ART: { character, figure: node,
      head: name => ({ name }), lid: host => ({ host }), url: path => path },
    el: { boss: node() }, state: { groupsData: [], drawer: {} },
    L: zh => zh, cliName: host => host,
    setText: (element, text) => { element.textContent = text; },
    setAttr: (element, key, value) => { element[key] = value; },
  });
  vm.runInContext(readFileSync(`${__dirname}/../chat-office.js`, "utf8"), context);
  const rec = Object.fromEntries(["lid", "scene", "desk", "chair", "open", "closed", "portrait", "cli", "st", "state"].map(key => [key, node()]));
  rec.laptop = { laptop: { dress(set) { rec.laptop.set = set; } } };
  return { rec, update: c => context.updateBoss(rec, c, { running: 0, review: 0 }) };
}

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
    AN: {}, window: {},
    ResizeObserver: class { constructor(callback) { resize = callback; } observe() {} },
    requestAnimationFrame: callback => frames.push(callback),
    el: { floor: {}, desks: { querySelector: selector => selector === ".desk > .scene" ? { offsetWidth: deskWidth } : null } },
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

  const chatSource = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = chatSource.indexOf("function renderAll(");
  const end = chatSource.indexOf("for (const link of", start);
  const blanks = [];
  const drawer = { hidden: true };
  let measuredWithDrawerHidden = null;
  const chatContext = vm.createContext({
    AN: {}, state: { loaded: "ws-a", selected: "ws-b", groupsData: [], auto: false },
    narrow: { matches: false }, el: { banner: {}, app: { dataset: {} } },
    contextOf: c => c, followAlias() {}, counts: () => ({}),
    renderHead() {}, renderConv() { drawer.hidden = false; }, renderTicker() {},
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
  const h = (tag, attributes = {}, ...children) => ({
    tag, attributes, children, dataset: {}, textContent: "",
    replaceChildren(...items) { this.children = items; },
    addEventListener() {},
    set innerHTML(_) { throw new Error("Audit data must never become HTML"); },
  });
  const context = vm.createContext({
    AN: common(english), state: { peers: new Set() }, h,
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
    context,
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
  assert.deepEqual(rec.fwd.children.map(btn => btn.textContent), ["↪ 转发自 @a #1", "↪ 转发自 其他消息"]);
  assert.deepEqual(rec.fwd.children.map(btn => btn.className), ["fwd", "fwd dead"]);
  assert.deepEqual(rec.fwd.children.map(btn => btn.fwdId), ["c:t:a", null]);
  context.showForwards(rec, { forward: { message_id: "c:t:a", source: "room · 转发" } }, ctx);
  assert.equal(rec.fwd.children.length, 1, "a meeting forward still shows a single tag");
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
  let groups = [{ group: "ws", updated: "2026-11-10T00:00:00Z" }], chat = { group: "ws", rooms: [], desks: [], messages: [] }, tick = 1, busy = false;
  const context = vm.createContext({
    AN: { voice: { ingest() { calls.voice++; } } }, state: { loaded: null, selected: "ws", groupsData: [], auto: false },
    narrow: { matches: false }, el: { banner: {}, app: { dataset: {} } },
    contextOf: c => c, followAlias() {}, counts: () => ({}), renderHead() {}, renderConv() {}, renderTicker() {},
    renderOffice() { calls.all++; }, showBlank() {}, renderList() { calls.list++; }, ingest() {},
    ART: { version: () => 1 }, document: { documentElement: { lang: "zh" } }, tickOf: () => tick, timeDriven: () => busy,
    load: async path => structuredClone(path === "/app/chats" ? { groups } : chat),
  });
  vm.runInContext(source.slice(start, source.indexOf("for (const link of", start)), context);
  return {
    calls, refresh: () => context.refresh(),
    change: patch => { chat = { ...chat, ...patch }; }, rename: () => { groups = [{ ...groups[0], preview: "new" }]; },
    tick: () => { tick++; }, busy: on => { busy = on; },
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
});

test("a side list patches its relative times in place and lays out only when desks or seats change", () => {
  const read = [];
  const context = vm.createContext({
    AN: { at: Date.parse }, window: {}, ResizeObserver: class { observe() {} },
    ART: { head: d => d, version: () => 1 }, deskChar: d => d.key, setText: (node, text) => { node.textContent = text; },
    h: (tag, _attrs, ...kids) => ({ tag, kids: kids.flat(), textContent: kids.find(k => typeof k === "string"), addEventListener() {} }),
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
