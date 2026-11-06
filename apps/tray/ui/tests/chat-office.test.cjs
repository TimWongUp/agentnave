const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { common, identities } = require("./helpers/common.cjs");

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
  return { rec, update: c => context.updateBoss(rec, c, { running: 0, review: 0 }) };
}

const speech = (provider, model, time) => ({ kind: "prompt", time, speaker: { role: "manager", provider, model } });
const old = speech("claude", "claude-opus-5-5", "2026-11-04T13:30:00Z");
const handover = { kind: "handover", time: "2026-11-04T13:37:42Z", speaker: { role: "system" } };
const claimed = () => ({ host: "antigravity", app: "antigravity", label: "Antigravity", handover: "changed", previous_host: "claude", messages: [old, handover] });

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
  assert.equal(app.rec.open.src, "office/boss-laptop-agy.webp");
  assert.equal(app.rec.closed.src, "office/boss-laptop-closed-agy.webp");
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
    assert.equal(chip.children.at(-1), "未记录身份");
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
  assert.equal(chip.children.at(-1), "claude-x #4");
  assert.equal(chip.children.find(child => child?.tag === "span.at-role").children[0], "探索派");
  assert.equal(chip.attributes.title, identity.handle);
  context.AN.setFullModelNames(true);
  assert.equal(context.speakerName(m), "route/claude-x #4");
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
