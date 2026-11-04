const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function office() {
  const node = () => ({
    dataset: {}, classList: { toggle() {} }, children: [],
    replaceChildren(...children) { this.children = children; },
    insertBefore(child, before) { child.parentNode = this; child.nextSibling = before; },
    remove() {},
  });
  const context = vm.createContext({
    AN: { at: Date.parse }, window: {}, ResizeObserver: class { observe() {} },
    ART: { character: (_host, model) => model || "default", figure: node,
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
