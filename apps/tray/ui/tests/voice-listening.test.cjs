const { test } = require("node:test");
const assert = require("node:assert/strict");
const { office, flush, message } = require("./helpers/voice.cjs");

test("column listening reads all history, resumes completed progress in memory and waits when caught up", async () => {
  const app = await office(false);
  const list = [1, 2, 3].map(n => message(`history-${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  assert.equal(app.sounds[0].text, "history-1");
  await app.end();
  await app.command("unlisten");
  await app.command("listen", { group: "workspace", room: "room" });
  assert.equal(app.sounds.at(-1).text, "history-2", "an unfinished clip is retried; a completed clip is skipped");
  await app.end(); await app.end();
  const heard = app.playbacks.length;
  await app.command("unlisten");
  await app.command("listen", { group: "workspace", room: "room" });
  await app.tick();
  assert.equal(app.playbacks.length, heard, "a completed album does not repeat its last message");
  list.push(message("new", "2026-11-01T00:00:04Z", { conversation: "room", thread: { kind: "room", id: "room" } }));
  await app.tick();
  assert.equal(app.sounds.at(-1).text, "new");
  const restarted = await office(false);
  restarted.room(list);
  await restarted.command("listen", { group: "workspace", room: "room" });
  assert.equal(restarted.sounds[0].text, "history-1", "a new page has no saved progress");
});

test("private listening loads complete history once and polls the recent workspace timeline", async () => {
  const paths = [];
  const app = await office(false, undefined, undefined, "all", async path => { paths.push(path); return null; });
  const list = Array.from({ length: 305 }, (_, n) => message(`private-${n}`, new Date(Date.UTC(2026, 10, 1) + n * 1000).toISOString(), { desk: "thread:one", conversation: `task-${n}`, thread: { kind: "desk", key: "thread:one" } }));
  app.private({ key: "thread:one", provider: "codex" }, list);
  await app.feed(list.slice(-300));
  paths.length = 0;
  await app.command("listen", { group: "workspace", desk: "thread:one" });
  assert.equal(paths[0], "/app/chats/workspace/history");
  assert.equal(app.sounds[0].text, "private-0");
  await app.tick();
  assert.equal(paths.filter(path => path === "/app/chats/workspace/history").length, 1);
  assert.ok(paths.includes("/app/chats/workspace"));
  for (let n = 0; n < list.length; n++) await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), list.map(m => m.text), "history beyond the recent 300 is read without truncation");
});

test("a listened workspace the office keeps feeding is not fetched again by listening", async () => {
  const paths = [];
  const app = await office(false, undefined, undefined, "all", async path => { paths.push(path); return null; });
  const pending = message("pending", "2026-11-01T00:00:02Z", { status: "running" });
  await app.feed([pending]);
  await app.command("listen", { group: "workspace" });
  paths.length = 0;
  await app.tick();
  assert.deepEqual(paths.filter(path => path.startsWith("/app/chats/")), ["/app/chats/workspace"], "without the office, listening polls itself");
  paths.length = 0;
  app.voice.ingest({ group: "workspace", messages: [{ ...pending, status: "succeeded" }] }, { whole: true });
  await app.tick();
  assert.deepEqual(paths.filter(path => path.startsWith("/app/chats/")), []);
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["pending"], "what the office brought in is still heard");
});

test("workspace listening ignores existing readable messages and follows transitions to readability", async () => {
  const app = await office(false);
  const old = message("old", "2026-11-01T00:00:01Z");
  const pending = message("pending", "2026-11-01T00:00:02Z", { status: "running" });
  await app.feed([old, pending]);
  await app.command("listen", { group: "workspace" });
  await app.tick();
  assert.equal(app.requests.length, 0);
  await app.feed([old, { ...pending, status: "succeeded" }]);
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["pending"]);
  await app.end(); await app.tick();
  assert.equal(app.requests.length, 1);
});

test("album batches contain at most three units and keep a briefing beside its answer", async () => {
  const app = await office();
  app.ctx.desks = new Map(["a", "c"].map(key => [key, { key, provider: "codex", model: "same-name" }]));
  const a = (id, second, extra = {}) => message(id, new Date(Date.UTC(2026, 10, 1) + second * 1000).toISOString(), { desk: "a", conversation: `task-${id}`, ...extra });
  const b = (id, second) => message(id, new Date(Date.UTC(2026, 10, 1) + second * 1000).toISOString(), { desk: id, conversation: "room-b", thread: { kind: "room", id: "room-b" } });
  const list = [a("a1", 1), a("brief-a2", 2, { kind: "instruction", conversation: "task-pair", round: 1, speaker: { role: "manager" } }),
    a("a2", 4, { conversation: "task-pair", round: 1 }), a("a3", 6), a("a4", 8),
    b("b1", 3), b("b2", 5), b("b3", 7), b("b4", 9), a("c1", 10, { desk: "c" })];
  await app.feed(list.slice().reverse());
  for (let n = 0; n < list.length; n++) await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["a1", "brief-a2", "a2", "a3", "b1", "b2", "b3", "a4", "b4", "c1"]);
  assert.equal(app.requests.length, list.length, "private tasks share their desk album and identically named desks stay separate");
});

test("new arrivals cannot join a batch that is already playing", async () => {
  const app = await office();
  const m = (id, desk, second) => message(id, `2026-11-01T00:00:0${second}Z`, { desk, conversation: `task-${id}` });
  const list = [m("a1", "a", 1), m("b1", "b", 2), m("a2", "a", 4), m("b2", "b", 5)];
  await app.feed(list);
  list.push(m("a3", "a", 6), m("b3", "b", 7));
  await app.feed(list);
  assert.equal(app.sounds[0].paused, false, "arrivals do not interrupt the current clip");
  for (let n = 0; n < list.length; n++) await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["a1", "a2", "b1", "b2", "b3", "a3"]);
});

test("a five-minute backlog cancels lookahead, finishes the speaking clip and waits for explicit resume", async () => {
  const signals = [], releases = [];
  const app = await office(true, (body, signal) => {
    signals.push({ text: body.text, signal });
    if (body.text === "waiting" && signals.filter(entry => entry.text === body.text).length === 1) return new Promise(resolve => releases.push(resolve));
  });
  const list = [message("speaking", "2026-11-01T00:00:01Z"), message("waiting", "2026-11-01T00:00:02Z")];
  await app.feed(list);
  const deadline = app.published.at(-1).follow.expires_at;
  await app.tick(299999);
  assert.equal(app.published.at(-1).error, null);
  await app.tick(1);
  assert.equal(signals[1].signal.aborted, true);
  assert.equal(app.sounds[0].paused, false, "the clip already heard is allowed to finish");
  assert.equal(app.published.at(-1).error, "消息积压，已暂停收听");
  list.push(message("later", "2026-11-01T00:00:03Z"));
  await app.feed(list);
  releases[0](); await flush();
  await app.end();
  assert.equal(app.published.at(-1).paused, true);
  const requests = app.requests.length;
  await app.tick(600000);
  assert.equal(app.requests.length, requests);
  assert.equal(app.published.at(-1).follow.expires_at, deadline);
  await app.command("toggle");
  assert.equal(app.sounds.at(-1).text, "waiting");
  assert.equal(app.published.at(-1).error, null);
  await app.tick(299999);
  assert.equal(app.published.at(-1).error, null, "resume starts a fresh five-minute wait");
  await app.tick(1);
  assert.equal(app.published.at(-1).error, "消息积压，已暂停收听");
  await app.end();
  await app.command("toggle");
  assert.equal(app.sounds.at(-1).text, "later", "backlogged messages are retained");
});

test("historical column backlog never starts the five-minute guard", async () => {
  const app = await office(false);
  const list = [1, 2, 3].map(n => message(`old-${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  await app.tick(6 * 60000);
  assert.equal(app.published.at(-1).error, null);
  await app.end();
  assert.equal(app.sounds.at(-1).text, "old-2");
  assert.equal(app.published.at(-1).paused, false);
});

test("manual pauses freeze wait time, retain arrivals and leave the listening deadline unchanged", async () => {
  const app = await office();
  const list = [message("first", "2026-11-01T00:00:01Z"), message("second", "2026-11-01T00:00:02Z")];
  await app.feed(list);
  const deadline = app.published.at(-1).follow.expires_at;
  await app.tick(120000);
  await app.command("toggle");
  list.push(message("during-pause", "2026-11-01T00:00:03Z"));
  const sent = app.requests.length;
  await app.feed(list);
  await app.tick(600000);
  assert.equal(app.requests.length, sent);
  assert.equal(app.published.at(-1).paused, true);
  assert.equal(app.published.at(-1).error, null);
  assert.equal(app.published.at(-1).follow.expires_at, deadline);
  await app.command("toggle");
  await app.tick(179999);
  assert.equal(app.published.at(-1).error, null);
  await app.tick(1);
  assert.equal(app.published.at(-1).error, "消息积压，已暂停收听", "only unpaused waiting counts");
  await app.end();
  await app.command("toggle");
  await app.end(); await app.end();
  assert.ok(app.playbacks.some(sound => sound.text === "during-pause"));
});

test("pausing cancels pending lookahead and never synthesizes a paused arrival", async () => {
  const signals = [], pending = [];
  const app = await office(true, (body, signal) => {
    signals.push({ text: body.text, signal });
    if (body.text === "next" && signals.filter(entry => entry.text === "next").length === 1) return new Promise(resolve => pending.push(resolve));
  });
  const list = [message("current", "2026-11-01T00:00:01Z"), message("next", "2026-11-01T00:00:02Z")];
  await app.feed(list);
  await app.command("toggle");
  assert.equal(signals[1].signal.aborted, true);
  list.push(message("new", "2026-11-01T00:00:03Z"));
  await app.feed(list);
  pending[0](); await flush();
  assert.equal(app.requests.length, 2);
  assert.equal(app.sounds.length, 1);
  await app.command("toggle");
  await app.end(); await app.end(); await app.end();
  assert.deepEqual(app.sounds.map(sound => sound.text), ["current", "next", "new"]);
});

test("the first listened clip keeps its audio and exact position across a manual read", async () => {
  const app = await office(false);
  await app.settings({ continue_read: false });
  const list = [1, 2].map(n => message(`listen-${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  const first = app.sounds[0]; first.currentTime = 11.25;
  const manual = message("manual", "2026-11-01T00:00:03Z");
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, manual, app.ctx);
  rec.read.listeners.click(); await flush();
  assert.equal(first.paused, true);
  await app.end();
  assert.equal(first.paused, false);
  assert.equal(first.currentTime, 11.25);
  assert.equal(first.plays, 2);
  assert.deepEqual(app.playbacks.slice(0, 3), [{ text: "listen-1", time: 0 }, { text: "manual", time: 0 }, { text: "listen-1", time: 11.25 }]);
  await app.end(); await app.end();
  assert.equal(app.requests.filter(request => request.text === "listen-1").length, 1);
});

test("a manual read takes priority while the first listening synthesis is still pending", async () => {
  let release;
  const app = await office(true, body => body.text === "first" ? new Promise(resolve => { release = resolve; }) : undefined);
  await app.settings({ continue_read: false });
  await app.feed([message("first", "2026-11-01T00:00:01Z")]);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, message("manual", "2026-11-01T00:00:02Z"), app.ctx);
  rec.read.listeners.click(); await flush();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["manual"]);
  release(); await flush();
  await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["manual", "first"]);
  assert.equal(app.requests.filter(request => request.text === "first").length, 1);
});

test("manual chaining freezes the click snapshot even when the full conversation response arrives later", async () => {
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const app = await office(false, undefined, async data => { await gate; return { ok: true, json: async () => structuredClone(data) }; });
  const list = [1, 2].map(n => message(`existing-${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  list.push(message("pending-at-click", "2026-11-01T00:00:03Z", { conversation: "room", thread: { kind: "room", id: "room" }, status: "running" }));
  await app.room(list).listeners.click();
  list.push(message("new-after-click", "2026-11-01T00:00:04Z", { conversation: "room", thread: { kind: "room", id: "room" } }));
  list[2].status = "succeeded";
  release(); await flush();
  for (let n = 0; n < 4; n++) await app.end();
  assert.deepEqual(app.requests.map(request => request.text), ["existing-1", "existing-2"]);
});

test("column listening applies read scope to its first clip and waits when everything is filtered", async () => {
  const app = await office(false, undefined, undefined, "members");
  const boss = message("boss", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" }, kind: "prompt", speaker: { role: "manager" } });
  const list = [boss];
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  await app.tick();
  assert.equal(app.requests.length, 0);
  assert.equal(app.published.at(-1).follow.room, "room");
  list.push(message("member", "2026-11-01T00:00:02Z", { conversation: "room", thread: { kind: "room", id: "room" } }));
  await app.tick();
  assert.deepEqual(app.requests.map(request => request.text), ["member"]);
  await app.scope("boss");
  assert.equal(app.sounds[0].paused, true, "a scope change also applies to the first listened clip already speaking");
  await app.tick();
  assert.equal(app.sounds.at(-1).text, "boss");
});

test("seat listening uses its own messages while manual chaining can still include visible peers", async () => {
  const app = await office(false);
  const own = message("own", "2026-11-01T00:00:01Z", { desk: "seat:one", conversation: "room", thread: { kind: "room", id: "room" } });
  const peer = message("peer", "2026-11-01T00:00:02Z", { desk: "seat:two", conversation: "room", thread: { kind: "room", id: "room" } });
  const list = [own, peer];
  app.private({ key: "seat:one", provider: "codex" }, list, chat => chat.messages);
  app.voice.controls(true, null, app.ctx, "seat:one", chat => chat.messages, chat => chat.messages.filter(m => m.desk === "seat:one"));
  await app.nodes.filter(node => node.tag === "button.listen-here")[1].listeners.click(); await flush();
  await app.end();
  list.push({ ...peer, id: "peer-new", text: "peer-new" }, { ...own, id: "own-new", text: "own-new" });
  await app.feed(list);
  await app.end();
  assert.deepEqual(app.requests.map(request => request.text), ["own", "own-new"]);
});

test("a listening batch opens when listening gets its turn after a manual clip", async () => {
  const app = await office();
  await app.settings({ continue_read: false });
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, message("manual", "2026-11-01T00:00:00Z"), app.ctx);
  rec.read.listeners.click(); await flush();
  const m = (id, desk, n) => message(id, `2026-11-01T00:00:0${n}Z`, { desk });
  const list = [m("a1", "a", 1), m("b1", "b", 2)];
  await app.feed(list);
  list.push(m("a2", "a", 3), m("a3", "a", 4), m("a4", "a", 5));
  await app.feed(list);
  for (let n = 0; n <= list.length; n++) await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["manual", "a1", "a2", "a3", "b1", "a4"]);
});

test("widening a listened workspace's read scope reads what comes next, not what it passed over", async () => {
  const app = await office(true, undefined, undefined, "members");
  const boss = (id, second) => message(id, `2026-11-01T00:00:0${second}Z`, { kind: "prompt", speaker: { role: "manager" } });
  const list = [boss("old", 1)];
  await app.feed(list);
  await app.scope("all");
  await app.tick();
  assert.equal(app.requests.length, 0);
  list.push(boss("new", 2));
  await app.feed(list);
  assert.deepEqual(app.requests.map(request => request.text), ["new"]);
});

test("a manual read starts at once from a pause or a backlog", async () => {
  const paused = await office(true);
  await paused.settings({ continue_read: false });
  await paused.feed([message("first", "2026-11-01T00:00:01Z")]);
  await paused.command("toggle");
  const rec = { bub: { prepend() {} } };
  paused.voice.decorate(rec, message("manual", "2026-11-01T00:00:02Z"), paused.ctx);
  rec.read.listeners.click(); await flush();
  assert.equal(paused.playbacks.at(-1).text, "manual");
  assert.equal(paused.published.at(-1).paused, false);

  const held = await office(true);
  await held.settings({ continue_read: false });
  await held.feed([message("first", "2026-11-01T00:00:01Z"), message("second", "2026-11-01T00:00:02Z")]);
  await held.tick(300000);
  assert.equal(held.published.at(-1).backlog, true);
  const manual = { bub: { prepend() {} } };
  held.voice.decorate(manual, message("manual", "2026-11-01T00:00:03Z"), held.ctx);
  manual.read.listeners.click(); await flush();
  assert.equal(held.playbacks.at(-1).text, "manual");
  assert.equal(held.published.at(-1).backlog, false);
});

test("backlog before the first audio cancels synthesis and keeps the pending message resumable", async () => {
  const signals = [], releases = [];
  const app = await office(true, (body, signal) => {
    signals.push(signal);
    if (signals.length === 1) return new Promise(resolve => releases.push(resolve));
  });
  await app.feed([message("waiting-for-audio", "2026-11-01T00:00:01Z")]);
  await app.tick(300000);
  assert.equal(signals[0].aborted, true);
  assert.equal(app.published.at(-1).paused, true);
  assert.equal(app.sounds.length, 0);
  releases[0](); await flush();
  assert.equal(app.sounds.length, 0);
  await app.command("toggle");
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["waiting-for-audio"]);
  assert.equal(app.requests.length, 2);
});

test("expiry while paused and page exit cancel requests without restarting listening", async () => {
  for (const ending of ["expiry", "exit"]) {
    const signals = [];
    const app = await office(true, (body, signal) => { signals.push(signal); return new Promise(() => {}); });
    await app.feed([message("pending", "2026-11-01T00:00:01Z")]);
    await app.command("toggle");
    assert.equal(signals[0].aborted, true);
    if (ending === "expiry") await app.tick(8 * 3600000);
    else await app.leave();
    await app.command("toggle");
    await app.tick();
    assert.equal(app.requests.length, 1, ending);
    assert.equal(app.sounds.length, 0, ending);
    if (ending === "expiry") assert.equal(app.published.at(-1).follow, null);
  }
});

test("a late manual-chain response cannot recreate followed jobs withdrawn at expiry", async () => {
  let delayed = false, release;
  const gate = new Promise(resolve => { release = resolve; });
  const app = await office(true, undefined, async data => { if (delayed) await gate; return { ok: true, json: async () => structuredClone(data) }; });
  const list = [1, 2, 3, 4].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  await app.feed(list);
  app.room(list);
  delayed = true;
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, list[1], app.ctx);
  rec.read.listeners.click(); await flush();
  await app.tick(8 * 3600000);
  release(); await flush();
  for (let n = 0; n < 4; n++) await app.end();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["m1", "m2"]);
  assert.equal(app.published.at(-1).follow, null);
});

test("a late chain response does not repeat messages completed while it was loading", async () => {
  let delayed = false, release;
  const gate = new Promise(resolve => { release = resolve; });
  const app = await office(true, undefined, async data => { if (delayed) await gate; return { ok: true, json: async () => structuredClone(data) }; });
  const list = [1, 2, 3, 4].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
  await app.feed(list);
  app.room(list);
  delayed = true;
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, list[1], app.ctx);
  rec.read.listeners.click(); await flush();
  for (let n = 0; n < 5; n++) await app.end();
  const played = app.playbacks.length;
  release(); await flush();
  await app.tick();
  assert.equal(app.playbacks.length, played);
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["m1", "m2", "m1", "m3", "m4"]);
});

test("an office poll before the first read finishes does not queue the history it brings", async () => {
  const list = [1, 2, 3].map(n => message(`old-${n}`, `2026-11-01T00:00:0${n}Z`));
  let release, held = false;
  const gate = new Promise(resolve => { release = resolve; });
  const app = await office(false, undefined, undefined, "all", async path => {
    if (held || path !== "/app/chats/workspace") return null;
    held = true;
    await gate;
    return { ok: true, json: async () => ({ group: "workspace", host: "codex", messages: list, desks: [], rooms: [] }) };
  });
  const listening = app.command("listen", { group: "workspace" });
  await flush();
  app.voice.ingest({ group: "workspace", messages: list }, { whole: true });
  await flush();
  release(); await listening;
  await app.tick();
  assert.deepEqual(app.playbacks.map(sound => sound.text), [], "history that predates the first read is not read on an office poll");
  list.push(message("new", "2026-11-01T00:00:04Z"));
  app.voice.ingest({ group: "workspace", messages: list }, { whole: true });
  await app.tick();
  assert.deepEqual(app.playbacks.map(sound => sound.text), ["new"], "what arrives afterwards is still heard");
});
