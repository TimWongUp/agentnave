const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const flush = () => new Promise(resolve => setImmediate(resolve));
const message = (id, time, extra = {}) => ({
  id, time, text: id, kind: "reply", status: "succeeded", desk: id,
  speaker: { role: "member", provider: "codex" }, thread: { kind: "desk" }, visibility: "private", ...extra,
});

async function office(auto_read = true, synthesize = async () => ({}), loadRoom = async data => ({ ok: true, json: async () => structuredClone(data) })) {
  let config = { enabled: true, auto_read, engine: "elevenlabs", keys: { elevenlabs: true }, summary: { enabled: true } };
  const requests = [], sounds = [], nodes = [];
  const rooms = new Map();
  function h(tag, attributes = {}, ...children) {
    const node = { tag, attributes: { ...attributes }, dataset: {}, children, textContent: children.filter(c => typeof c === "string").join(""), listeners: {},
      setAttribute(key, value) { this.attributes[key] = value; }, before() {},
      append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; }, remove() { this.removed = true; },
      addEventListener(name, fn) { this.listeners[name] = fn; } };
    nodes.push(node);
    return node;
  }
  const document = { hidden: false, getElementById: id => Object.assign(h("div"), { id }), addEventListener() {} };
  class Audio {
    constructor() { this.currentTime = 0; this.paused = true; this.plays = 0; sounds.push(this); }
    play() { this.paused = false; this.plays++; return Promise.resolve(); }
    pause() { this.paused = true; }
    removeAttribute() {}
    load() {}
  }
  const AN = { h, deskName: d => d.model || d.key,
    art: { character: (provider, model) => model || provider || "default", head: name => h("span.head", { name }) },
    reconcile: (root, items) => { root.children = items; },
  };
  vm.runInNewContext(readFileSync(`${__dirname}/../voice.js`, "utf8"), {
    AN, document, Audio, AbortController, Map, Event, queueMicrotask,
    Avatars: { family: (provider, model) => model?.includes("claude") ? "claude" : provider === "codex" ? "gpt" : provider || null },
    Date: class extends Date { static now() { return 0; } },
    window: { dispatchEvent() {}, addEventListener() {} },
    URL: { createObjectURL: () => "blob:test", revokeObjectURL() {} },
    async fetch(path, options) {
      if (rooms.has(path)) return loadRoom(rooms.get(path));
      if (path === "/app/voice/speak") {
        requests.push(JSON.parse(options.body));
        return { ok: true, status: 200, headers: { get: () => null }, blob: () => synthesize(JSON.parse(options.body), options.signal) };
      }
      if (options?.method === "POST") config = JSON.parse(options.body);
      return { ok: true, json: async () => structuredClone(config) };
    },
  });
  await flush();
  const ctx = { c: { group: "workspace", host: "codex", messages: [], threads: [] }, desks: new Map(), rooms: new Map() };
  const readable = m => m.text && ["member", "manager"].includes(m.speaker?.role) && ["prompt", "instruction", "forward", "reply"].includes(m.kind)
    && !["running", "pending", "not_started", "review"].includes(m.status);
  // Clicking the first message of a conversation with chaining on reads it to the end.
  const starter = messages => ({ hidden: !messages.some(readable), listeners: { async click() {
    const first = messages.filter(readable).sort((x, y) => Date.parse(x.time) - Date.parse(y.time))[0];
    if (!first) return;
    const rec = { bub: { prepend() {} } };
    AN.voice.decorate(rec, first, ctx);
    rec.read.listeners.click();
    await flush();
  } } });
  const feed = async messages => { ctx.c.messages = messages; AN.voice.ingest({ group: "workspace", messages }, ctx); await flush(); };
  await feed([]);
  AN.voice.controls(true, null, ctx);
  return {
    voice: AN.voice, requests, sounds, ctx, feed, nodes,
    stop() { nodes.find(n => n.tag === "button.voice-stop").listeners.click(); },
    private(desk, messages = []) {
      ctx.desks = new Map([[desk.key, desk]]);
      rooms.set("/app/chats/workspace/history", { ...ctx.c, messages, desks: [desk] });
      AN.voice.controls(true, null, ctx, desk.key);
      return starter(messages);
    },
    room(messages, id = "room", desks = null) {
      const members = messages.filter(m => m.speaker?.role === "member" && m.conversation === id && m.kind === "reply");
      ctx.desks = new Map(desks ? desks.map(d => [d.key, d]) : members.map(m => [m.desk, { key: m.desk, provider: m.speaker.provider, model: m.speaker.model || m.desk }]));
      ctx.c.messages = messages;
      ctx.rooms.set(id, { id, seats: [...ctx.desks.values()].map(d => ({ desk: d.key })) });
      rooms.set(`/app/chats/workspace/focus/${id}`, { group: "workspace", host: "codex", desks: [...ctx.desks.values()], rooms: [...ctx.rooms.values()], messages });
      AN.voice.controls(true, { group: "workspace", id }, ctx);
      return starter(messages.filter(m => m.conversation === id));
    },
    async end() { sounds.at(-1).onended(); await flush(); },
    async automatic() { await nodes.find(n => n.tag === "button.voice-toggle").listeners.click(); await flush(); },
  };
}

test("private instructions use the boss identity and precede each employee's answer in the same round", async () => {
  for (const automatic of [false, true]) {
    const app = await office(automatic);
    const roomMessage = (id, second, extra = {}) => message(id, `2026-11-01T00:00:${String(second).padStart(2, "0")}Z`, {
      conversation: "room", thread: { kind: "room", id: "room" }, round: 1, ...extra,
    });
    const topic = roomMessage("topic", 1, { kind: "prompt", speaker: { role: "manager", provider: "codex", model: "gpt-boss" } });
    const answer = (desk, second, round = 1) => roomMessage(`answer-${desk}-${round}`, second, {
      desk, round, speaker: { role: "member", provider: "claude", model: "claude-worker" },
    });
    const instruction = (desk, round = 1) => roomMessage(`instruction-${desk}-${round}`, 1, {
      desk, round, kind: "instruction", speaker: { role: "manager", provider: "codex" },
    });
    const messages = [topic, instruction("a"), instruction("b"), answer("b", 2), answer("a", 3),
      roomMessage("topic-2", 4, { round: 2, kind: "prompt", speaker: { role: "manager", provider: "codex", model: "gpt-boss" } }),
      { ...instruction("a", 2), time: "2026-11-01T00:00:04Z" }, answer("a", 5, 2)];
    const button = app.room(messages);
    if (automatic) await app.feed(messages);
    else { await button.listeners.click(); await flush(); }
    for (let index = 0; index < messages.length; index++) await app.end();
    assert.deepEqual(app.requests.map(r => r.text), ["topic", "instruction-b-1", "answer-b-1", "instruction-a-1", "answer-a-1", "topic-2", "instruction-a-2", "answer-a-2"]);
    for (const request of app.requests) {
      const boss = !request.text.startsWith("answer-");
      assert.equal(request.boss, boss);
      assert.equal(request.family, boss ? "gpt" : "claude", request.text);
    }
  }
});

test("a manually read instruction never borrows its recipient's model when boss model is absent", async () => {
  const app = await office(false);
  app.private({ key: "employee", provider: "claude", model: "claude-worker", role: "实现" });
  const m = message("instruction", "2026-11-01T00:00:01Z", {
    desk: "employee", kind: "instruction", speaker: { role: "manager", provider: "codex" },
  });
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, m, app.ctx);
  let stoppedPropagation = false;
  rec.read.listeners.click({ stopPropagation() { stoppedPropagation = true; } });
  await flush();
  assert.equal(stoppedPropagation, true, "reading a peer must not also expand its folded bubble");
  assert.equal(app.requests[0].family, "gpt");
  assert.equal(app.requests[0].boss, true);
  await app.end();
});

test("instruction model fallback uses its own round's boss and the message list pairs the same briefing", async () => {
  const app = await office(false);
  const topic = message("topic", "2026-11-01T00:00:01Z", {
    conversation: "room", round: 1, kind: "prompt", speaker: { role: "manager", provider: "codex", model: "claude-boss" },
  });
  const answer = message("answer", "2026-11-01T00:00:02Z", { conversation: "room", round: 1, desk: "worker", focus: "worker", reply_to: topic.id });
  const instruction = message("instruction", topic.time, {
    conversation: "room", round: 1, desk: "worker", recipients: [{ focus: "worker" }], reply_to: topic.id, kind: "instruction", speaker: { role: "manager", provider: "codex" },
  });
  const next = { ...topic, id: "next", round: 2, time: "2026-11-01T00:00:03Z", speaker: { role: "manager", provider: "codex", model: "gpt-boss" } };
  const other = { ...instruction, id: "other-room", conversation: "other", reply_to: null };
  app.ctx.c.messages = [topic, instruction, answer, next];
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, instruction, app.ctx);
  rec.read.listeners.click();
  await flush();
  assert.equal(app.requests[0].family, "claude", "the latest boss model in a different round must not override this topic");
  await app.end();
  const source = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = source.indexOf("function threaded(list)");
  const end = source.indexOf("function renderConv(", start);
  const threaded = vm.runInNewContext(`${source.slice(start, end)}; threaded`, { AN: { voice: app.voice }, isBoss: m => m.speaker?.role === "manager" });
  assert.deepEqual(Array.from(threaded([topic, instruction, answer, other, next]), m => m.id), ["topic", "instruction", "answer", "other-room", "next"]);
});

test("live private briefings wait for their own completed answers across polls", async () => {
  const app = await office();
  app.room([]);
  const instruction = desk => message(`instruction-${desk}`, desk === "a" ? "2026-11-01T00:00:00Z" : "1969-12-31T23:59:59Z", {
    desk, conversation: "room", round: 1, kind: "instruction", thread: { kind: "room", id: "room" }, speaker: { role: "manager", provider: "codex" },
  });
  const answer = (desk, status = "succeeded") => message(`answer-${desk}`, "2026-11-01T00:00:01Z", {
    desk, conversation: "room", round: 1, status, thread: { kind: "room", id: "room" },
  });
  const instructions = [instruction("a"), instruction("b")];
  await app.feed(instructions);
  await app.feed([...instructions, answer("a", "review")]);
  assert.equal(app.requests.length, 0, "neither a running nor a review-only answer releases the briefing");
  await app.feed([...instructions, answer("b")]);
  assert.deepEqual(app.requests.map(r => r.text), ["instruction-b", "answer-b"], "an earlier briefing uses its newly completed answer's playback eligibility");
  await app.end(); await app.end();
  await app.feed([...instructions, answer("b"), answer("a")]);
  await app.end(); await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["instruction-b", "answer-b", "instruction-a", "answer-a"]);
  await app.feed([...instructions, answer("b"), answer("a")]);
  assert.equal(app.requests.length, 4);
});

test("returning to the directory keeps playing, keeps the channel and reads the whole workspace", async () => {
  const app = await office();
  app.room([]);
  const roomReply = message("meeting-answer", "2026-11-01T00:00:01Z", {
    conversation: "room", thread: { kind: "room", id: "room" },
  });
  await app.feed([roomReply]);
  const clip = app.sounds[0];
  const source = readFileSync(`${__dirname}/../chat.js`, "utf8");
  const start = source.indexOf("function renderConv(");
  const end = source.indexOf("function updateJump(", start);
  let directory = false;
  const renderConv = vm.runInNewContext(`${source.slice(start, end)}; renderConv`, {
    AN: { voice: app.voice }, state: { drawer: { kind: "all" }, convCollapsed: false }, overlay: { matches: false },
    el: { drawer: { classList: { toggle() {} } }, convToggle: {} }, setText() {}, setAttr() {}, renderDirectory() { directory = true; },
  });
  renderConv(app.ctx.c, app.ctx);
  assert.equal(directory, true);
  assert.equal(clip.paused, false);
  assert.equal(app.nodes.find(n => n.id === "voice-actions").hidden, false);
  await app.feed([roomReply, message("private-answer", "2026-11-01T00:00:02Z")]);
  await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["meeting-answer", "private-answer"]);
});

test("private play-from-start reads only that employee's private history and excludes meeting excerpts", async () => {
  const app = await office(false);
  const messages = [
    message("old-question", "2025-01-01T00:00:01Z", { desk: "employee", kind: "prompt", speaker: { role: "manager", provider: "codex" } }),
    message("old-answer", "2025-01-01T00:00:02Z", { desk: "employee" }),
    message("other-employee", "2025-01-01T00:00:03Z"),
    message("meeting-excerpt", "2025-01-01T00:00:04Z", { desk: "employee", thread: { kind: "room", id: "room" } }),
  ];
  const button = app.private({ key: "employee", provider: "codex", model: "gpt" }, messages);
  assert.equal(button.hidden, false);
  await button.listeners.click(); await flush();
  await app.end(); await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["old-question", "old-answer"]);
});

test("existing boss messages and employee answers use their displayed text in time order", async () => {
  const app = await office();
  const later = message("member", "2026-11-01T00:00:04Z");
  const earlier = message("boss", "2026-11-01T00:00:01Z", {
    kind: "prompt", status: "", speaker: { role: "manager" }, text: "请分析 **这个问题**。",
  });
  const topic = message("topic", "2026-11-01T00:00:02Z", {
    kind: "prompt", status: "", speaker: { role: "manager" }, thread: { kind: "room", id: "room" }, round: 2,
  });
  const instruction = message("instruction", "2026-11-01T00:00:03Z", { kind: "instruction", speaker: { role: "manager" } });
  const forward = message("forward", "2026-11-01T00:00:05Z", { kind: "forward", speaker: { role: "manager" } });
  const typing = message("typing", "2026-11-01T00:00:00Z", { kind: "typing", status: "running" });
  const pending = message("pending", "2026-11-01T00:00:00Z", { status: "review" });
  const notice = message("notice", "2026-11-01T00:00:00Z", { kind: "notice", speaker: { role: "system" } });
  const messages = [later, typing, forward, instruction, earlier, topic, pending, notice];
  await app.feed(messages);
  assert.deepEqual(app.requests.map(r => r.text), [earlier.text, "topic", "instruction"], "the next two are prepared ahead");
  assert.equal(app.requests[0].boss, true);
  for (let index = 0; index < 5; index++) await app.end();
  assert.deepEqual(app.requests.map(r => r.text), [earlier.text, "topic", "instruction", "member", "forward"]);
  assert.ok(app.requests.every(r => r.full === false));
  await app.feed(messages);
  assert.equal(app.requests.length, 5);
});

test("pausing a meeting preserves the exact clip position and remaining queue for resume", async () => {
  const app = await office(false);
  const messages = [1, 2].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  }));
  await app.room(messages).listeners.click();
  await flush();
  const clip = app.sounds[0];
  clip.currentTime = 7.5;
  app.stop();
  assert.equal(clip.paused, true);
  assert.equal(app.nodes.find(n => n.tag === "button.voice-stop").textContent, "继续");
  const requests = app.requests.length;
  app.stop();
  await flush();
  assert.equal(app.sounds[0], clip);
  assert.equal(clip.currentTime, 7.5);
  assert.equal(clip.paused, false);
  assert.equal(clip.plays, 2);
  assert.equal(app.requests.length, requests, "resume must reuse the current audio and prefetch");
  await app.end();
  assert.equal(app.sounds.length, 2, "the rest of the meeting continues after the resumed clip");
  await app.end();
});

test("pausing during synthesis does not start audio until resumed", async () => {
  let complete;
  const app = await office(false, () => new Promise(resolve => { complete = resolve; }));
  const m = message("worker", "2026-11-01T00:00:01Z");
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, m, app.ctx);
  rec.read.listeners.click();
  await flush();
  app.stop();
  complete({});
  await flush();
  assert.equal(app.sounds.length, 0);
  app.stop();
  await flush();
  assert.equal(app.sounds.length, 1);
  assert.equal(app.requests.length, 1);
  await app.end();
});

test("private channel shows boss and the selected employee role before playback", async () => {
  const app = await office(false);
  app.private({ key: "employee", provider: "codex", model: "model", role: "正确性评审" });
  const roster = app.nodes.find(n => n.tag === "div.voice-roster");
  assert.equal(roster.children.length, 2);
  assert.match(roster.children[0].attributes["aria-label"], /boss/);
  assert.match(roster.children[1].attributes["aria-label"], /正确性评审/);
  app.private({ key: "other", provider: "claude", model: "other-model", role: "实现" });
  assert.equal(roster.children.length, 2);
  assert.match(roster.children[1].attributes["aria-label"], /实现/);
});

test("automatic playback follows every meeting and switching meetings keeps its queue", async () => {
  const app = await office();
  const meeting = (id, room, second) => message(id, `2026-11-01T00:00:0${second}Z`, {
    conversation: room, thread: { kind: "room", id: room }, round: 1,
  });
  app.room([], "a");
  await app.feed([]);
  const a1 = meeting("a1", "a", 1), b1 = meeting("b1", "b", 2), a2 = meeting("a2", "a", 3);
  await app.feed([b1, a2, a1]);
  assert.deepEqual(app.requests.map(r => r.text), ["a1", "b1", "a2"]);
  app.room([], "b");
  assert.equal(app.sounds[0].paused, false);
  await app.end(); await app.end(); await app.end();
  assert.equal(app.sounds.length, 3);
  await app.feed([a1, a2, b1]);
  assert.equal(app.requests.length, 3, "each message is read once");
});

test("meeting audio is prepared ahead, plays in message order and stops scheduling on cancel", async () => {
  const pending = new Map(), signals = new Map();
  const app = await office(false, (body, signal) => new Promise(resolve => {
    pending.set(body.text, resolve); signals.set(body.text, signal);
  }));
  const messages = [1, 2, 3, 4, 5].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  }));
  await app.room(messages).listeners.click();
  await flush();
  assert.equal(app.requests.length, 3, "prepare current audio plus two ahead before the first clip finishes");
  pending.get("m3")({}); pending.get("m2")({});
  await flush();
  assert.equal(app.sounds.length, 0, "later audio must wait for the first message");
  pending.get("m1")({});
  await flush();
  assert.equal(app.sounds.length, 1);
  await app.end();
  assert.equal(app.sounds.length, 2);
  assert.equal(app.requests.length, 4, "refill the two-message lookahead while the next clip plays");
  app.voice.stop();
  assert.equal(signals.get("m4").aborted, true);
  pending.get("m4")({});
  await flush();
  assert.equal(app.requests.length, 4, "stopping must never start m5");
  assert.equal(app.sounds.length, 2, "late synthesis results must not start playback");
});

test("room playback reads the complete meeting in time order with automatic reading off and can stop", async () => {
  const app = await office(false);
  const roomMessage = (id, second, extra = {}) => message(id, `2026-11-01T00:00:0${second}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" }, ...extra,
  });
  const first = roomMessage("question", 1, { kind: "prompt", speaker: { role: "manager" }, round: 1 });
  const last = roomMessage("summary", 3, { kind: "prompt", speaker: { role: "manager" }, round: 2 });
  const button = app.room([last, roomMessage("other-room", 0, { conversation: "other", thread: { kind: "room", id: "other" } }), roomMessage("answer", 2, { round: 2 }), first,
    roomMessage("typing", 4, { kind: "typing", status: "running" })]);
  assert.ok(button, "a meeting needs a play-from-start button");
  await button.listeners.click();
  await flush();
  assert.equal(app.requests[0].text, "question", "the focus response includes earlier messages outside the recent timeline");
  await app.end();
  await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["question", "answer", "summary"]);
  app.stop();
  await flush();
  assert.equal(app.requests.length, 3, "the channel stop button stops the whole queue");
  await button.listeners.click();
  await flush();
  for (let index = 0; index < 3; index++) await app.end();
  assert.deepEqual(app.requests.slice(3).map(r => r.text), ["question", "answer", "summary"]);
});

test("automatic switch stops new automatic reads while final answers remain manually readable", async () => {
  const app = await office(false);
  const final = message("private-final", "2026-11-01T00:00:01Z");
  await app.feed([final]);
  assert.equal(app.requests.length, 0);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, final, app.ctx);
  assert.equal(rec.read.hidden, false);
  assert.equal(rec.read.disabled, false);
  rec.read.listeners.click();
  await flush();
  assert.equal(app.requests[0].text, "private-final");
  await app.end();
  await app.automatic();
  await app.feed([final]);
  assert.equal(app.requests.length, 1, "enabling automatic playback must not replay old answers");
  await app.feed([final, message("next-final", "2026-11-01T00:00:02Z")]);
  assert.equal(app.requests[1].text, "next-final");
  await app.end();
  assert.equal(app.requests.length, 2);
});

test("reading one message by hand keeps what automatic playback queued after it", async () => {
  const app = await office();
  const first = message("first", "2026-11-01T00:00:01Z"), second = message("second", "2026-11-01T00:00:02Z");
  const third = message("third", "2026-11-01T00:00:03Z");
  await app.feed([first, second, third]);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, third, app.ctx);
  rec.read.listeners.click();
  await flush();
  assert.equal(app.sounds[0].paused, true, "the clip in progress stops");
  assert.equal(app.sounds.length, 2);
  await app.end();
  await app.end();
  assert.equal(app.sounds.length, 3, "the queued second message still plays");
  assert.deepEqual(app.requests.map(r => r.text), ["first", "second", "third"], "a clip prepared ahead is reused");
});

test("leaving reading from the start keeps what auto read queued meanwhile", async () => {
  const app = await office();
  const old = message("old", "2025-01-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" } });
  const button = app.room([old]);
  await button.listeners.click(); await flush();
  assert.deepEqual(app.requests.map(r => r.text), ["old"]);
  await app.feed([old, message("fresh", "2026-11-01T00:00:01Z")]);
  app.room([], "other");
  await flush();
  assert.equal(app.sounds[0].paused, true, "the history clip stops");
  assert.deepEqual(app.requests.map(r => r.text), ["old", "fresh"]);
  assert.equal(app.sounds.length, 2);
});

test("with chaining off a clicked message is read alone", async () => {
  const app = await office(false);
  const messages = [1, 2].map(n => message(`c${n}`, `2026-11-01T00:00:0${n}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  }));
  const start = app.room(messages);
  const chain = app.nodes.filter(n => n.tag === "button.voice-toggle")[1];
  await chain.listeners.click(); await flush();
  assert.equal(chain.attributes["aria-pressed"], "false");
  await start.listeners.click();
  await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["c1"]);
});

test("the voice channel shows host and speaker avatars without office animation state", async () => {
  const app = await office(false);
  const question = message("boss-question", "2026-11-01T00:00:01Z", {
    conversation: "room", thread: { kind: "room", id: "room" }, kind: "prompt", speaker: { role: "manager" },
  });
  const answer = message("employee", "2026-11-01T00:00:02Z", {
    conversation: "room", thread: { kind: "room", id: "room" },
  });
  const button = app.room([question, answer]);
  const roster = app.nodes.find(n => n.tag === "div.voice-roster");
  assert.equal(roster.children.length, 2);
  assert.match(roster.children[0].attributes["aria-label"], /主持人/);
  assert.match(roster.children[1].attributes["aria-label"], /发言人/);
  await button.listeners.click();
  await flush();
  assert.equal(roster.children[0].dataset.speaking, "true");
  assert.equal(roster.children[1].dataset.speaking, "false");
  await app.end();
  assert.equal(roster.children[0].dataset.speaking, "false");
  assert.equal(roster.children[1].dataset.speaking, "true");
  await app.feed([question, answer, message("live-task", "2026-11-01T00:00:03Z", { kind: "typing", status: "running" })]);
  assert.equal(roster.children[1].dataset.speaking, "true", "workspace activity does not interrupt the independent audio channel");
  app.stop();
  await flush();
  assert.ok(roster.children.every(p => p.dataset.speaking === "false"));
  assert.equal(app.voice.roomActive, undefined);
  assert.equal(app.voice.animations, undefined);
});

test("after a handover the channel keeps the boss who spoke until the new boss speaks there", async () => {
  const app = await office(false);
  app.ctx.c.host = "claude";
  const old = message("old", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" }, kind: "prompt", speaker: { role: "manager", provider: "codex", model: "gpt-old" } });
  app.room([old]);
  const roster = app.nodes.find(n => n.tag === "div.voice-roster");
  assert.equal(roster.children[0].children[0].children[0].attributes.name, "gpt-old");
  const next = message("new", "2026-11-01T00:00:02Z", { conversation: "room", thread: { kind: "room", id: "room" }, kind: "prompt", speaker: { role: "manager", provider: "claude", model: "claude-new" } });
  app.ctx.c.messages = [old, next];
  app.voice.controls(true, { group: "workspace", id: "room" }, app.ctx);
  assert.equal(roster.children[0].children[0].children[0].attributes.name, "claude-new");
});

test("speech says who is talking, to whom, and names the one employee a boss message is for", async () => {
  const app = await office(false);
  const desks = [
    { key: "a", provider: "claude", model: "claude-x", role: "探索派" },
    { key: "b", provider: "claude", model: "claude-y", role: "实用派" },
    { key: "c", provider: "codex", model: "gpt-z", role: "" },
  ];
  const room = { kind: "room", id: "room" };
  const list = [
    message("topic", "2026-11-01T00:00:01Z", { conversation: "room", round: 1, thread: room, kind: "prompt", desk: null, to: desks.map(d => ({ key: d.key })), speaker: { role: "manager" } }),
    message("brief", "2026-11-01T00:00:02Z", { conversation: "room", round: 1, thread: room, kind: "instruction", desk: "a", to: [{ key: "a" }], speaker: { role: "manager" } }),
    message("answer", "2026-11-01T00:00:03Z", { conversation: "room", round: 1, thread: room, desk: "a", speaker: { role: "member", provider: "claude", model: "claude-x" } }),
    message("task", "2026-11-01T00:00:04Z", { conversation: "t", thread: { kind: "desk", key: "c" }, kind: "prompt", desk: "c", to: [{ key: "c" }], speaker: { role: "manager" } }),
  ];
  app.ctx.desks = new Map(desks.map(d => [d.key, d]));
  app.ctx.rooms.set("room", { seats: desks.map(d => ({ desk: d.key })) });
  const said = text => {
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, list.find(m => m.id === text), app.ctx);
    rec.read.listeners.click({ stopPropagation() {} });
    return flush().then(() => app.requests.at(-1).scene);
  };
  assert.deepEqual(await said("topic"), { name: "boss", role: "", room: true, to: null });
  const brief = await said("brief");
  assert.equal(brief.to.role, "探索派");
  assert.match(brief.to.name, /探索派/, "seats sharing a family are told apart by role");
  const answer = await said("answer");
  assert.equal(answer.room, true);
  assert.equal(answer.to, null);
  assert.equal((await said("task")).to.name, "GPT");
  const fromStart = app.requests.length;
  const playAll = app.room(list.filter(m => m.conversation === "room"), "room", desks);
  await playAll.listeners.click();
  await flush();
  assert.deepEqual(app.requests.slice(fromStart).find(r => r.text === "brief").scene, brief,
    "reading from the start addresses same-family seats exactly like reading one message");
});
