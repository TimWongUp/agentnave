const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const { office, flush, message } = require("./helpers/voice.cjs");
const { identities } = require("./helpers/common.cjs");

test("an unknown DeepSeek speaker keeps the character's family voice as member or boss", async () => {
  const app = await office(true);
  app.avatars.family = () => null;
  app.ctx.c.host = "deepseek";
  const messages = [];
  for (const role of ["member", "manager"]) {
    messages.push(message(role, `2026-11-01T00:00:0${messages.length + 1}Z`, {
      speaker: { role, provider: "deepseek", model: "unknown" },
    }));
    await app.feed(messages);
    assert.equal(app.requests.at(-1).family, "deepseek");
    assert.equal(app.requests.at(-1).boss, role === "manager");
    await app.end();
  }
});

test("changing settings after repeated failures restores playback and resets the failure count", async () => {
  let broken = true;
  const app = await office(true, async () => { if (broken) throw new Error("synthesis failed"); });
  const list = [];
  for (let n = 1; n <= 3; n++) {
    list.push(message(`fail-${n}`, `2026-11-01T00:00:0${n}Z`)); await app.feed(list);
  }
  assert.equal(app.published.at(-1).status, "播放已停下：synthesis failed");
  assert.equal(app.published.at(-1).bar_label, "出错");
  list.length = 0;
  await app.settings({ summary: { enabled: false } });
  assert.equal(app.published.at(-1).error, null);
  await app.command("listen", { group: "workspace" });
  list.push(message("retry-failure", "2026-11-01T00:00:04Z")); await app.feed(list);
  assert.equal(app.published.at(-1).status, "本条朗读失败，已跳过");
  broken = false;
  list.push(message("recovered", "2026-11-01T00:00:05Z")); await app.feed(list);
  assert.equal(app.sounds.at(-1).text, "recovered");
  assert.equal(app.published.at(-1).status, "正在播放", "the next effective state replaces a skipped-message error");
  await app.end();
  assert.equal(app.published.at(-1).error, null);
});

test("a 204 skip neither clears playback errors nor resets consecutive failures", async () => {
  const app = await office(true, async () => { throw new Error("synthesis failed"); }, undefined, "all",
    async (path, options) => path === "/app/voice/speak" && JSON.parse(options.body).text === "skip" ? { status: 204 } : null);
  const list = [message("fail-1", "2026-11-01T00:00:01Z")]; await app.feed(list);
  list.push(message("skip", "2026-11-01T00:00:02Z")); await app.feed(list);
  assert.equal(app.published.at(-1).status, "本条朗读失败，已跳过");
  assert.equal(app.sounds.length, 0);
  for (let n = 2; n <= 3; n++) {
    list.push(message(`fail-${n}`, `2026-11-01T00:00:0${n + 1}Z`)); await app.feed(list);
  }
  assert.equal(app.published.at(-1).status, "播放已停下：synthesis failed");
});

test("a notice belongs to its playing job and clears only after a later job plays successfully", async () => {
  const app = await office(true, undefined, undefined, "all", async (path, options) => {
    if (path !== "/app/voice/speak" || JSON.parse(options.body).text !== "notice") return null;
    return { ok: true, status: 200, headers: { get: () => encodeURIComponent("fallback notice") }, blob: async () => ({ text: "notice" }) };
  });
  await app.feed(["first", "notice", "last"].map((id, n) => message(id, `2026-11-01T00:00:0${n + 1}Z`)));
  assert.equal(app.published.at(-1).error, null);
  await app.end(); assert.equal(app.published.at(-1).status, "fallback notice");
  await app.end(); assert.equal(app.published.at(-1).status, "fallback notice");
  await app.end(); assert.equal(app.published.at(-1).error, null);
});

test("finishing a playing message does not clear a full-conversation read failure", async () => {
  let broken = true;
  const app = await office(false, undefined, async data => ({ ok: !broken, json: async () => structuredClone(data) }));
  const button = app.room([message("chosen", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" } })]);
  await button.listeners.click();
  assert.equal(app.published.at(-1).bar_label, "出错");
  await app.end(); await app.feed(app.ctx.c.messages);
  assert.equal(app.published.at(-1).error_source, "read");
  broken = false;
  await app.command("retry");
  assert.equal(app.published.at(-1).error, null);
});

test("settings-load errors survive playback and clear on an unchanged successful reload", async () => {
  let broken = false;
  const app = await office(true, undefined, undefined, "all", async path =>
    path === "/app/voice" && broken ? { ok: false, json: async () => ({ error: "load failed" }) } : null);
  await app.feed([message("playing", "2026-11-01T00:00:01Z")]);
  broken = true; await app.feed(app.ctx.c.messages); await app.end();
  assert.equal(app.published.at(-1).error_source, "load");
  assert.equal(app.published.at(-1).primary.command, "settings");
  broken = false; await app.feed(app.ctx.c.messages);
  assert.equal(app.published.at(-1).error, null);
});

test("settings loading recovers after initial failures without requiring a configuration change", async () => {
  let broken = true;
  const app = await office(true, undefined, undefined, "all", async path =>
    path === "/app/voice" && broken ? { ok: false, json: async () => ({ error: "load failed" }) } : null);
  assert.equal(app.published.at(-1).bar_label, "出错");
  broken = false; await app.feed([]);
  assert.equal(app.published.at(-1).error, null);
});

test("private instructions use the boss identity and precede each employee's answer in the same round", async () => {
  for (const listening of [false, true]) {
    const app = await office(listening);
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
    if (listening) await app.feed(messages);
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
  const audit = { ...answer, id: "audit", kind: "approval" };
  assert.deepEqual(Array.from(threaded([topic, instruction, audit, answer, other, next]), m => m.id), ["topic", "audit", "instruction", "answer", "other-room", "next"]);
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

test("returning to the directory keeps playing and hides the whole voice channel", async () => {
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
  assert.equal(app.nodes.find(n => n.id === "voice-actions").hidden, true);
  await app.feed([roomReply, message("private-answer", "2026-11-01T00:00:02Z")]);
  await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["meeting-answer", "private-answer"]);
});

test("desk play-from-start reads what that desk view shows, meeting messages included", async () => {
  const app = await office(false);
  const messages = [
    message("old-question", "2025-01-01T00:00:01Z", { desk: "employee", kind: "prompt", speaker: { role: "manager", provider: "codex" } }),
    message("old-answer", "2025-01-01T00:00:02Z", { desk: "employee" }),
    message("other-employee", "2025-01-01T00:00:03Z"),
    message("meeting-answer", "2025-01-01T00:00:04Z", { desk: "employee", conversation: "room", thread: { kind: "room", id: "room" } }),
    message("meeting-peer", "2025-01-01T00:00:05Z", { desk: "peer", conversation: "room", thread: { kind: "room", id: "room" } }),
    message("later-meeting", "2025-01-01T00:00:06Z", { desk: "peer", conversation: "room", thread: { kind: "room", id: "room" } }),
  ];
  const shown = new Set(["old-question", "old-answer", "meeting-answer", "meeting-peer"]);
  const button = app.private({ key: "employee", provider: "codex", model: "gpt" }, messages, chat => chat.messages.filter(m => shown.has(m.id)));
  assert.equal(button.hidden, false);
  await button.listeners.click(); await flush();
  await app.end(); await app.end(); await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["old-question", "old-answer", "meeting-answer", "meeting-peer"]);
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
  assert.deepEqual(app.requests.map(r => r.text), [earlier.text, "topic"], "only the next clip is prepared ahead");
  assert.equal(app.requests[0].boss, true);
  for (let index = 0; index < 5; index++) await app.end();
  assert.deepEqual(app.requests.map(r => r.text), [earlier.text, "topic", "instruction", "member", "forward"]);
  assert.ok(app.requests.every(r => r.full === false));
  await app.feed(messages);
  assert.equal(app.requests.length, 5);
});

test("resuming a paused meeting goes back two seconds and keeps the remaining queue", async () => {
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
  assert.equal(app.nodes.find(n => n.tag === "button.voice-primary").attributes["aria-label"], "继续");
  const requests = app.requests.length;
  app.stop();
  await flush();
  assert.equal(app.sounds[0], clip);
  assert.equal(clip.currentTime, 5.5);
  assert.equal(clip.paused, false);
  assert.equal(clip.plays, 2);
  assert.equal(app.requests.length, requests, "resume must reuse the current audio and prefetch");
  await app.end();
  assert.equal(app.sounds.length, 2, "the rest of the meeting continues after the resumed clip");
  app.sounds[1].currentTime = 1.2;
  app.stop();
  app.stop();
  await flush();
  assert.equal(app.sounds[1].currentTime, 0, "a clip under two seconds in starts over");
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
  complete({}); await flush();
  assert.equal(app.sounds.length, 1);
  assert.equal(app.requests.length, 2, "cancelled synthesis is retried only after resume");
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

test("the boss box listens to the workspace and the column button to what the column shows", async () => {
  const app = await office(false);
  const messages = [message("latest", "2026-11-01T00:00:01Z", { conversation: "room" })];
  await app.feed(messages);
  const [workspace, column] = app.nodes.filter(n => n.tag === "button.listen-here");
  assert.equal(app.voice.workspaceListen, workspace);
  let stopped = false;
  await workspace.listeners.click({ stopPropagation() { stopped = true; } }); await flush();
  assert.ok(stopped, "a click here does not also open the boss's conversation");
  assert.equal(workspace.attributes["aria-pressed"], "true");
  assert.deepEqual([app.published.at(-1).follow.group, app.published.at(-1).follow.room], ["workspace", null]);
  assert.equal(column.hidden, true, "the whole workspace's column leaves listening to the boss box");
  await workspace.listeners.click(); await flush();
  assert.equal(app.published.at(-1).follow, null);
  app.room(messages);
  assert.equal(column.hidden, false);
  await column.listeners.click(); await flush();
  assert.equal(app.published.at(-1).follow.room, "room");
  assert.equal(workspace.attributes["aria-pressed"], "false");
  assert.equal(column.attributes["aria-pressed"], "true");
});

test("listening to a private chat reads its column and nothing else in the workspace", async () => {
  const app = await office(false);
  const own = message("own", "2026-11-01T00:00:01Z", { desk: "desk" });
  const messages = [own];
  app.private({ key: "desk", provider: "codex", model: "desk" }, messages);
  await app.feed(messages);
  await app.nodes.filter(n => n.tag === "button.listen-here")[1].listeners.click(); await flush();
  assert.equal(app.published.at(-1).follow.desk, "desk");
  assert.equal(app.published.at(-1).follow.room, null);
  await app.end();
  messages.push(message("other", "2026-11-01T00:00:02Z", { desk: "other" }));
  messages.push(message("next", "2026-11-01T00:00:03Z", { desk: "desk" }));
  await app.feed(messages);
  assert.deepEqual(app.sounds.map(sound => sound.text), ["own", "next"]);
});

test("an album's cover is everyone in that meeting or private chat, whichever message started it", async () => {
  const boss = (id, time, extra) => message(id, time, { kind: "instruction", desk: undefined, speaker: { role: "manager", provider: "claude", model: "boss" }, ...extra });
  const seats = ["s1", "s2", "s3", "s4", "s5"].map(key => ({ key, provider: "codex", model: key }));
  const thread = { kind: "room", id: "room" };
  const meeting = [boss("ask", "2026-11-01T00:00:01Z", { conversation: "room", thread }),
    message("answer", "2026-11-01T00:00:02Z", { desk: "s1", conversation: "room", thread })];
  const coverOf = async id => {
    const app = await office(false);
    app.room(meeting, "room", seats);
    await app.feed(meeting);
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, meeting.find(m => m.id === id), app.ctx);
    rec.read.listeners.click(); await flush();
    const job = app.published.at(-1).jobs[0];
    return [job.album.link, job.album.cover.map(p => p.model)];
  };
  assert.deepEqual(await coverOf("ask"), await coverOf("answer"));
  assert.deepEqual(await coverOf("ask"), ["conv:room", ["boss", "s1", "s2", "s3", "s4", "s5"]]);

  const app = await office(false);
  const chat = [boss("task-1", "2026-11-01T00:00:01Z", { conversation: "t1", thread: { kind: "desk", key: "s1" }, to: [{ key: "s1" }] }),
    message("task-2", "2026-11-01T00:00:02Z", { desk: "s1", conversation: "t2", thread: { kind: "desk", key: "s1" } })];
  app.private(seats[0], chat);
  await app.feed(chat);
  const albums = [];
  for (const m of chat) {
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, m, app.ctx);
    rec.read.listeners.click(); await flush();
    albums.push(app.published.at(-1).jobs[0].album);
  }
  assert.deepEqual(albums[0], albums[1], "every task with one desk is the same private chat");
  assert.equal(albums[0].link, "desk:s1");
  assert.deepEqual(albums[0].cover.map(p => p.model), ["boss", "s1"]);
});

test("workspace listening follows every meeting and switching meetings keeps its queue", async () => {
  const app = await office();
  const meeting = (id, room, second) => message(id, `2026-11-01T00:00:0${second}Z`, {
    conversation: room, thread: { kind: "room", id: room }, round: 1,
  });
  app.room([], "a");
  await app.feed([]);
  const a1 = meeting("a1", "a", 1), b1 = meeting("b1", "b", 2), a2 = meeting("a2", "a", 3);
  await app.feed([b1, a2, a1]);
  assert.deepEqual(app.requests.map(r => r.text), ["a1", "a2"], "the same album plays its waiting batch first");
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
  assert.equal(app.requests.length, 1, "no lookahead before the first clip starts");
  pending.get("m1")({}); await flush();
  assert.equal(app.sounds.length, 1);
  assert.deepEqual(app.requests.map(r => r.text), ["m1", "m2"]);
  pending.get("m2")({}); await flush();
  await app.end();
  assert.equal(app.sounds.length, 2);
  assert.equal(app.requests.length, 3, "refill one clip of lookahead after the next clip starts");
  app.voice.stop();
  assert.equal(signals.get("m3").aborted, true);
  pending.get("m3")({}); await flush();
  assert.equal(app.requests.length, 3, "stopping must never start m4");
  assert.equal(app.sounds.length, 2, "late synthesis results must not start playback");
});

test("room playback reads the complete meeting in time order with listening off and can stop", async () => {
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

test("reading one message by hand keeps what listening queued, the cut clip included", async () => {
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
  assert.deepEqual(app.playbacks.map(s => s.text), ["first", "third", "first", "second"], "the cut followed clip resumes after the chosen one");
  assert.equal(app.sounds[0].plays, 2, "the same audio is reused");
  assert.deepEqual(app.requests.map(r => r.text), ["first", "second", "third"], "both current and prefetched audio are reused");
});

test("opening another meeting keeps a chained read and what listening queued meanwhile", async () => {
  const app = await office();
  const old = message("old", "2025-01-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" } });
  const button = app.room([old]);
  await button.listeners.click(); await flush();
  assert.deepEqual(app.requests.map(r => r.text), ["old"]);
  await app.feed([old, message("fresh", "2026-11-01T00:00:01Z")]);
  app.room([], "other");
  await flush();
  assert.equal(app.sounds[0].paused, false, "the history clip plays on");
  await app.end();
  assert.deepEqual(app.sounds.map(s => s.text), ["old", "fresh"]);
});

test("hiding the window or opening another workspace keeps the queue playing", async () => {
  const app = await office();
  await app.feed([message("first", "2026-11-01T00:00:01Z"), message("second", "2026-11-01T00:00:02Z")]);
  await app.hide();
  app.voice.ingest({ group: "elsewhere", messages: [] }, app.ctx);
  await flush();
  assert.equal(app.sounds[0].paused, false);
  await app.end();
  assert.deepEqual(app.sounds.map(s => s.text), ["first", "second"]);
});

test("the top bar lists what plays and what comes next, and a row plays at a click", async () => {
  const app = await office();
  await app.feed(["first", "second", "third"].map((id, n) => message(id, `2026-11-01T00:00:0${n + 1}Z`)));
  const playlist = app.nodes.find(n => n.tag === "ol.voice-queue");
  app.nodes.find(n => n.tag === "button.bar-voice").listeners.click();
  const rows = () => [...playlist.children].map(row => row.play);
  assert.deepEqual(rows().map(b => b.attributes["aria-current"]), ["true", "false", "false"]);
  rows()[2].listeners.click();
  await flush();
  await app.end();
  await app.end();
  assert.deepEqual(app.playbacks.map(s => s.text), ["first", "third", "first", "second"]);
});

test("the menu bar panel sees what the top bar shows and its clicks drive the office", async () => {
  const app = await office(false);
  const messages = [1, 2].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  }));
  await app.room(messages).listeners.click();
  await flush();
  const shown = app.published.at(-1);
  assert.equal(shown.state, "speaking");
  assert.deepEqual(shown.jobs.map(job => [job.id, job.state]), [["m1", "speaking"]], "the panel gets only the album playing");
  await app.command("toggle");
  assert.equal(app.sounds[0].paused, true);
  assert.equal(app.published.at(-1).state, "paused");
  await app.command("toggle");
  assert.equal(app.published.at(-1).state, "speaking");
});

test("listening to a meeting from the panel starts at the beginning, then follows new messages", async () => {
  const app = await office(false);
  const say = n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } });
  const other = message("x1", "2026-11-01T00:00:05Z", { conversation: "elsewhere" });
  app.rooms.set("/app/chats/elsewhere/focus/room", { group: "elsewhere", host: "codex", desks: [], rooms: [], messages: [say(1), say(2), other] });
  await app.command("listen", { group: "elsewhere", room: "room" });
  await flush();
  assert.deepEqual(app.sounds.map(sound => sound.text), ["m1"], "the first message plays at once");
  assert.deepEqual(app.published.at(-1).follow, { group: "elsewhere", room: "room", desk: null, title: "elsewhere", expires_at: 3602000 });
  app.rooms.get("/app/chats/elsewhere/focus/room").messages.push(say(3), message("x2", "2026-11-01T00:00:06Z", { conversation: "elsewhere" }));
  await app.tick();
  await app.end(); await app.end();
  assert.deepEqual(app.sounds.map(sound => sound.text), ["m1", "m2", "m3"], "a new message of the meeting follows; another conversation does not");
  await app.command("unlisten");
  assert.equal(app.published.at(-1).follow, null);
});

test("stopping listening drops its queued messages", async () => {
  const app = await office(true);
  const say = n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } });
  app.rooms.set("/app/chats/elsewhere/focus/room", { group: "elsewhere", host: "codex", desks: [], rooms: [], messages: [say(1)] });
  await app.command("listen", { group: "elsewhere", room: "room" });
  app.rooms.get("/app/chats/elsewhere/focus/room").messages.push(say(2));
  await app.tick();
  assert.deepEqual(app.published.at(-1).follow?.room, "room");
  await app.end();
  assert.deepEqual(app.sounds.map(sound => sound.text), ["m1", "m2"], "the listened meeting reads its new messages");
  await app.command("unlisten");
  assert.equal(app.published.at(-1).follow, null);
  assert.equal(app.published.at(-1).state, "idle");
  assert.equal(app.published.at(-1).recent?.id, "m2", "the player keeps the message that was playing");
});

test("listening to an already playing or queued message never reads its id twice", async () => {
  for (const queued of [false, true]) {
    const app = await office(false);
    const messages = [1, 2].slice(0, queued ? 2 : 1).map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));
    await app.room(messages).listeners.click();
    app.rooms.set("/app/chats/workspace", { ...app.ctx.c, messages });
    await app.command("listen", { group: "workspace" });
    for (let n = 0; n < 4 && app.published.at(-1).state !== "idle"; n++) await app.end();
    const played = app.sounds.map(sound => sound.text);
    assert.equal(played.filter(text => text === messages.at(-1).id).length, 1, "listening reuses the same message id");
    assert.equal(app.requests.filter(request => request.text === messages.at(-1).id).length, 1, "an already prepared clip is reused");
    assert.equal(app.published.at(-1).state, "idle");
  }
});

test("a clicked meeting chain stays ahead of followed messages while the office changes workspaces", async () => {
  const app = await office();
  const meeting = n => message(`meeting-${n}`, `2026-11-01T00:00:0${n + 3}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  });
  const messages = [1, 2, 3].map(meeting);
  await app.feed([message("interloper", "2026-11-01T00:00:02Z"), messages[1]]);
  const other = messages => app.voice.ingest({ group: "other", messages }, { ...app.ctx, c: { ...app.ctx.c, group: "other", messages } });
  other([]); await flush();
  other([message("old-workspace", "2026-11-01T00:00:01Z")]); await flush();
  await app.feed([messages[1]]);
  await app.room(messages).listeners.click();
  await app.feed([...messages, message("new-interloper", "2026-11-01T00:00:03Z")]);
  for (let n = 0; n < 3; n++) await app.end();
  assert.deepEqual(app.playbacks.slice(1, 4).map(sound => sound.text), ["meeting-1", "meeting-2", "meeting-3"]);
  await app.end();
  assert.deepEqual(app.playbacks.slice(4).map(sound => sound.text), ["interloper", "new-interloper"]);
});

test("the album playing heads the playlist and its title opens the conversation", async () => {
  const app = await office();
  app.room([]);
  app.ctx.c.conversations = [{ id: "room", title: "发布前评审" }];
  await app.feed([message("said", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" } })]);
  app.nodes.find(n => n.tag === "button.bar-voice").listeners.click();
  const title = app.nodes.find(n => n.tag === "button.voice-album-title");
  assert.equal(title.textContent, "发布前评审");
  title.listeners.click();
  assert.deepEqual([...app.navigations], ["/chat/workspace#conv%3Aroom"]);
});

test("a private album is titled by the role and its model line goes to the message playing", async () => {
  const app = await office(false);
  const said = message("said", "2026-11-01T00:00:01Z", { desk: "desk", speaker: { role: "member", provider: "codex", model: "gpt-x", identity: identities.review } });
  app.private({ key: "desk", provider: "codex", model: "gpt-x", role: "评审", identity: identities.review }, [said]);
  await app.feed([said]);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, said, app.ctx);
  rec.read.listeners.click(); await flush();
  app.voice.decorate(rec, said, app.ctx);
  assert.equal(rec.read.dataset.sounding, "true");
  assert.equal(app.published.at(-1).jobs[0].album.title, "评审");
  const bar = app.nodes.find(n => n.tag === "button.bar-voice");
  bar.listeners.click();
  assert.equal(app.nodes.find(n => n.tag === "button.voice-album-who").textContent, "gpt-x");
  app.nodes.find(n => n.tag === "button.voice-album-who").listeners.click();
  assert.equal(app.located[0].id, "said");
  assert.equal(app.navigations.length, 0, "the office decides how to reach it, so the conversation's end does not win");
});

test("a click from the panel waits for the office to be shown before it touches the list", async () => {
  const app = await office(false);
  const shown = () => { app.document.hidden = false; app.document.listeners.visibilitychange(); };
  app.document.hidden = true;
  await app.command("locate", { id: "said" });
  assert.equal(app.located.length, 0);
  shown();
  assert.equal(app.located[0]?.id, "said");
  app.document.hidden = true;
  await app.command("open", { group: "workspace", album: { link: "conv:room" } });
  assert.equal(app.navigations.length, 0);
  shown();
  assert.deepEqual([...app.navigations], ["/chat/workspace#conv%3Aroom"]);
});

test("an old spoken message without identity stays unnamed beside a desk's current identity", async () => {
  const app = await office(false);
  const old = message("old", "2026-11-01T00:00:01Z", {
    desk: "desk", speaker: { role: "member", provider: "claude", model: "claude-old" },
  });
  const start = app.private({ key: "desk", provider: "codex", model: "gpt-x", role: "评审", identity: identities.review }, [old]);
  await start.listeners.click();
  const request = app.requests[0], job = app.published.at(-1).jobs[0];
  assert.equal(job.who, "未记录身份");
  assert.deepEqual([request.scene.name, request.scene.role], ["", ""]);
  assert.equal(request.family, "claude", "the old message retains its recorded voice family");
});

test("speech preserves service duplicate ordinals and duties for speakers and addressees", async () => {
  const app = await office(false);
  const identity = { ...identities.explorer, ordinal: 4, numbered: true,
    handle: "@claude-x·Claude Code·探索派 #4", handle_short: "@claude-x·Claude Code·探索派 #4", spoken_name: "Claude #4" };
  const peer = { ...identity, key: "b", ordinal: 1,
    handle: "@claude-x·Claude Code·探索派 #1", handle_short: "@claude-x·Claude Code·探索派 #1", spoken_name: "Claude #1" };
  const desks = [{ key: "a", provider: "claude", model: "claude-x", identity },
    { key: "b", provider: "claude", model: "claude-x", identity: peer }];
  const messages = [
    message("brief", "2026-11-01T00:00:01Z", { conversation: "room", round: 1, thread: { kind: "room", id: "room" },
      kind: "instruction", desk: "a", to: [{ key: "a", identity }], speaker: { role: "manager", provider: "codex" } }),
    message("answer", "2026-11-01T00:00:02Z", { conversation: "room", round: 1, thread: { kind: "room", id: "room" },
      desk: "a", speaker: { role: "member", provider: "claude", model: "claude-x", identity } }),
  ];
  await app.room(messages, "room", desks).listeners.click();
  assert.deepEqual(app.requests[0].scene.to, [{ name: "Claude #4", role: "探索派", twin: false }]);
  await app.end();
  assert.deepEqual([app.requests[1].scene.name, app.requests[1].scene.role], ["Claude #4", "探索派"]);
  assert.equal(app.requests[1].family, "claude");
});

test("the drawer shows only a speaker of the view it is on", async () => {
  const app = await office();
  app.room([]);
  await app.feed([message("private", "2026-11-01T00:00:01Z")]);
  const roster = app.nodes.find(n => n.tag === "div.voice-roster");
  assert.equal(app.sounds[0].paused, false);
  assert.deepEqual([...roster.children].map(el => el.tag), ["div.voice-person.boss"], "a private clip stays out of the meeting's channel");
  assert.equal(roster.children[0].dataset.speaking, "false");
});

test("with chaining off a clicked message is read alone", async () => {
  const app = await office(false);
  const messages = [1, 2].map(n => message(`c${n}`, `2026-11-01T00:00:0${n}Z`, {
    conversation: "room", thread: { kind: "room", id: "room" },
  }));
  const start = app.room(messages);
  await app.settings({ continue_read: false });
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
    { key: "a", provider: "claude", model: "claude-x", role: "探索派", identity: identities.explorer },
    { key: "b", provider: "claude", model: "claude-y", role: "实用派", identity: identities.pragmatist },
    { key: "c", provider: "codex", model: "gpt-z", role: "", identity: identities.coder },
  ];
  const room = { kind: "room", id: "room" };
  const list = [
    message("topic", "2026-11-01T00:00:01Z", { conversation: "room", round: 1, thread: room, kind: "prompt", desk: null, to: desks.map(d => ({ key: d.key, identity: d.identity })), speaker: { role: "manager" } }),
    message("brief", "2026-11-01T00:00:02Z", { conversation: "room", round: 1, thread: room, kind: "instruction", desk: "a", to: [{ key: "a", identity: identities.explorer }], speaker: { role: "manager" } }),
    message("answer", "2026-11-01T00:00:03Z", { conversation: "room", round: 1, thread: room, desk: "a", message_id: "m-answer", speaker: { role: "member", provider: "claude", model: "claude-x", identity: identities.explorer } }),
    message("task", "2026-11-01T00:00:04Z", { conversation: "t", thread: { kind: "desk", key: "c" }, kind: "prompt", desk: "c", to: [{ key: "c", identity: identities.coder }], speaker: { role: "manager" } }),
    message("ask", "2026-11-01T00:00:05Z", { conversation: "room", round: 2, thread: room, kind: "prompt", desk: null, to: [{ key: "a", identity: identities.explorer }, { key: "c", identity: identities.coder }], speaker: { role: "manager" } }),
    message("relay", "2026-11-01T00:00:06Z", { conversation: "room", round: 2, thread: room, kind: "forward", desk: null, forward: { message_id: "m-answer" }, speaker: { role: "manager" } }),
    message("old-brief", "2026-11-01T00:00:07Z", { conversation: "room", round: 2, thread: room, kind: "instruction", desk: "a", to: [{ key: "a", role: "旧职责", model: "claude-old" }], speaker: { role: "manager", provider: "codex", model: "gpt-boss" } }),
    message("old-task", "2026-11-01T00:00:08Z", { conversation: "t", thread: { kind: "desk", key: "a" }, kind: "prompt", desk: "a", to: [{ key: "a" }], speaker: { role: "manager", provider: "codex", model: "gpt-boss" } }),
    message("legacy-brief", "2026-11-01T00:00:09Z", { conversation: "room", round: 2, thread: room, kind: "instruction", desk: "a", speaker: { role: "manager", provider: "codex", model: "gpt-boss" } }),
    message("job", "2026-11-01T00:00:10Z", { conversation: "t2", thread: { kind: "desk", key: "c" }, kind: "prompt", desk: "c", message_id: "m-task", to: [{ key: "c", identity: identities.coder }], speaker: { role: "manager" } }),
    message("job-done", "2026-11-01T00:00:11Z", { conversation: "t2", thread: { kind: "desk", key: "c" }, desk: "c", message_id: "m-task", speaker: { role: "member", provider: "codex", model: "gpt-z", identity: identities.coder } }),
    message("relay-task", "2026-11-01T00:00:12Z", { conversation: "room", round: 2, thread: room, kind: "forward", desk: null, message_id: "m-relay", forward: { message_id: "m-task" }, speaker: { role: "manager" } }),
    message("relay-relay", "2026-11-01T00:00:13Z", { conversation: "room", round: 2, thread: room, kind: "forward", desk: null, forward: { message_id: "m-relay" }, speaker: { role: "manager" } }),
    message("relay-gone", "2026-11-01T00:00:14Z", { conversation: "room", round: 2, thread: room, kind: "forward", desk: null, forward: { message_id: "m-gone" }, speaker: { role: "manager" } }),
  ];
  app.ctx.c.messages = list;
  app.ctx.desks = new Map(desks.map(d => [d.key, d]));
  app.ctx.rooms.set("room", { seats: desks.map(d => ({ desk: d.key, identity: d.identity })) });
  const said = text => {
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, list.find(m => m.id === text), app.ctx);
    rec.read.listeners.click({ stopPropagation() {} });
    return flush().then(() => app.requests.at(-1).scene);
  };
  assert.deepEqual(await said("topic"), { name: "boss", role: "", room: true, private: false, to: [], forward: null },
    "a message to every seat names nobody");
  const brief = await said("brief");
  assert.equal(brief.private, true);
  assert.deepEqual(brief.to, [{ name: "Claude", role: "探索派", twin: true }], "the model learns the service's spoken name, duty and same-family seat");
  const answer = await said("answer");
  assert.deepEqual([answer.room, answer.private, answer.to], [true, false, []]);
  assert.deepEqual([answer.name, answer.role], ["Claude", "探索派"]);
  assert.equal(app.requests.at(-1).family, "claude", "service names do not change the model-family voice");
  const task = await said("task");
  assert.deepEqual([task.private, task.to], [true, [{ name: "GPT", role: "", twin: false }]]);
  const ask = await said("ask");
  assert.deepEqual([ask.private, ask.to.map(p => p.role)], [false, ["探索派", ""]], "a meeting question to some seats names them");
  assert.deepEqual((await said("relay")).forward, { name: "Claude", role: "探索派", twin: true }, "a forward says whose words it relays");
  const coder = { name: "GPT", role: "", twin: false };
  assert.deepEqual((await said("relay-task")).forward, coder, "a forwarded private task relays the reply, not boss's question");
  assert.deepEqual((await said("relay-relay")).forward, coder, "forwarding a forward relays the original speaker");
  assert.deepEqual((await said("relay-gone")).forward, { name: "原发言人", role: "" }, "a source out of view is relayed, never read as boss speaking now");
  for (const id of ["old-brief", "old-task", "legacy-brief"]) {
    const old = await said(id);
    assert.equal(old.private, true);
    assert.deepEqual(old.to, id === "legacy-brief" ? [] : [{ name: "", role: "", twin: false }],
      "missing historical identity never borrows a name, model or duty from the current desk or raw recipient");
    assert.equal(app.requests.at(-1).family, "gpt", "a boss keeps its own voice when the recipient is unknown");
  }
  const fromStart = app.requests.length;
  const playAll = app.room(list.filter(m => m.conversation === "room"), "room", desks);
  await playAll.listeners.click();
  await flush();
  assert.deepEqual(app.requests.slice(fromStart).find(r => r.text === "brief").scene, brief,
    "reading from the start addresses same-family seats exactly like reading one message");
});


test("followed reads respect all, members, boss and meetings scopes", async () => {
  const messages = [
    message("private-boss", "2026-11-01T00:00:01Z", { kind: "prompt", speaker: { role: "manager" } }),
    message("private-member", "2026-11-01T00:00:02Z"),
    message("room-boss", "2026-11-01T00:00:03Z", { kind: "prompt", speaker: { role: "manager" }, thread: { kind: "room", id: "room" } }),
    message("room-member", "2026-11-01T00:00:04Z", { thread: { kind: "room", id: "room" } }),
  ];
  for (const [scope, expected] of [
    ["all", ["private-boss", "private-member", "room-boss", "room-member"]],
    ["members", ["private-member", "room-member"]],
    ["boss", ["private-boss", "room-boss"]],
    ["meetings", ["room-boss", "room-member"]],
  ]) {
    const app = await office(true, undefined, undefined, scope);
    await app.feed(messages);
    for (const _ of expected) await app.end();
    assert.deepEqual(app.requests.map(r => r.text), expected, scope);
  }
});

test("a manually selected message is always spoken and only its chain uses the read scope", async () => {
  for (const [scope, firstBoss, expected] of [
    ["all", true, ["chosen", "member", "boss"]],
    ["members", true, ["chosen", "member"]],
    ["boss", false, ["chosen", "boss"]],
  ]) {
    const app = await office(false, undefined, undefined, scope);
    const messages = [
      message("chosen", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" }, kind: firstBoss ? "prompt" : "reply", speaker: { role: firstBoss ? "manager" : "member" } }),
      message("member", "2026-11-01T00:00:02Z", { conversation: "room", thread: { kind: "room", id: "room" } }),
      message("boss", "2026-11-01T00:00:03Z", { conversation: "room", thread: { kind: "room", id: "room" }, kind: "prompt", speaker: { role: "manager" } }),
    ];
    const button = app.room(messages);
    await button.listeners.click();
    for (const _ of expected) await app.end();
    assert.deepEqual(app.requests.map(r => r.text), expected, scope);
  }
  const app = await office(false, undefined, undefined, "meetings");
  const desk = { key: "a", provider: "codex" };
  const messages = [1, 2].map(n => message(`private-${n}`, `2026-11-01T00:00:0${n}Z`, { desk: "a", thread: { kind: "desk", key: "a" } }));
  const button = app.private(desk, messages);
  await button.listeners.click();
  await app.end();
  assert.deepEqual(app.requests.map(r => r.text), ["private-1"], "meetings scope still allows the clicked private message");
});


test("changing scope filters queued followed reads while a manual clip stays selected", async () => {
  const app = await office();
  const room = { kind: "room", id: "room" };
  const messages = [
    message("chosen", "2026-11-01T00:00:01Z", { conversation: "room", thread: room, kind: "prompt", speaker: { role: "manager" } }),
    message("excluded", "2026-11-01T00:00:02Z", { conversation: "room", thread: room, kind: "prompt", speaker: { role: "manager" } }),
    message("member", "2026-11-01T00:00:03Z", { conversation: "room", thread: room }),
  ];
  const button = app.room(messages);
  await button.listeners.click();
  await app.scope("members");
  assert.equal(app.sounds[0].paused, false, "the explicitly selected boss clip continues");
  await app.end();
  assert.deepEqual(app.sounds.map(s => s.text), ["chosen", "member"], "the excluded prefetched clip never plays");
  await app.end();
  assert.deepEqual(app.sounds.map(s => s.text), ["chosen", "member"]);
});


test("boss-only playback omits private instructions whose paired reply is outside the scope", async () => {
  const room = { kind: "room", id: "room" };
  const messages = [
    message("topic", "2026-11-01T00:00:01Z", { conversation: "room", round: 1, thread: room, kind: "prompt", speaker: { role: "manager" } }),
    message("instruction", "2026-11-01T00:00:02Z", { conversation: "room", round: 1, thread: room, desk: "a", kind: "instruction", speaker: { role: "manager" } }),
    message("answer", "2026-11-01T00:00:03Z", { conversation: "room", round: 1, thread: room, desk: "a" }),
    message("summary", "2026-11-01T00:00:04Z", { conversation: "room", round: 1, thread: room, kind: "prompt", speaker: { role: "manager" } }),
  ];
  for (const listening of [false, true]) {
    const app = await office(listening, undefined, undefined, "boss");
    const button = app.room(messages);
    if (listening) await app.feed(messages);
    else await button.listeners.click();
    await app.end();
    await app.end();
    assert.deepEqual(app.requests.map(r => r.text), ["topic", "summary"]);
    assert.deepEqual(app.sounds.map(s => s.text), ["topic", "summary"]);
  }
});

test("changing scope interrupts an excluded followed clip and plays the eligible queued reply", async () => {
  const app = await office();
  const messages = [
    message("current-boss", "2026-11-01T00:00:01Z", { kind: "prompt", speaker: { role: "manager" } }),
    message("queued-boss", "2026-11-01T00:00:02Z", { kind: "prompt", speaker: { role: "manager" } }),
    message("member", "2026-11-01T00:00:03Z"),
  ];
  await app.feed(messages);
  assert.equal(app.sounds[0].text, "current-boss");
  assert.equal(app.sounds[0].paused, false);
  await app.scope("members");
  assert.equal(app.sounds[0].paused, true, "an followed out-of-scope clip stops immediately");
  assert.deepEqual(app.sounds.map(s => s.text), ["current-boss", "member"]);
  assert.equal(app.sounds[1].paused, false);
  await app.end();
  assert.deepEqual(app.sounds.map(s => s.text), ["current-boss", "member"], "the excluded prefetched boss clip is never played");
});


test("expiry finishes the speaking clip, aborts lookahead and never queues later messages", async () => {
  const signals = new Map();
  const app = await office(true, async (body, signal) => { signals.set(body.text, signal); });
  const messages = [1, 2, 3, 4, 5].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`));
  await app.feed(messages);
  const clip = app.sounds[0];
  assert.deepEqual(app.requests.map(r => r.text), ["m1", "m2"]);
  await app.tick(8 * 3600000);
  assert.equal(app.published.at(-1).follow, null);
  assert.equal(clip.paused, false, "expiry must let the speaking clip finish");
  assert.equal(signals.get("m1").aborted, false);
  assert.equal(signals.get("m2").aborted, true);
  assert.equal(signals.has("m3"), false);
  assert.equal(app.published.at(-1).status, "收听已结束");
  await app.end();
  const late = message("late", "2026-11-02T00:00:01Z");
  await app.feed([...messages, late]);
  await app.tick();
  assert.deepEqual(app.sounds.map(s => s.text), ["m1"]);
  assert.equal(app.requests.length, 2, "expiry never starts unprepared or newly arrived messages");
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, late, app.ctx);
  rec.read.listeners.click(); await flush();
  assert.equal(app.sounds.at(-1).text, "late", "manual reading still works after expiry");
  await app.end();
});

test("waking past expiry cannot synthesize the next clip before the timer runs", async () => {
  const app = await office();
  await app.feed([1, 2, 3, 4].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`)));
  app.advance(8 * 3600000);
  await app.end();
  assert.equal(app.requests.length, 2);
  assert.deepEqual(app.sounds.map(s => s.text), ["m1"]);
  assert.equal(app.published.at(-1).follow, null);
});

test("expiry cancels a pending listen response and an unfinished synthesis result", async () => {
  for (const waitingFor of ["conversation", "audio"]) {
    let release;
    const pending = new Promise(resolve => { release = resolve; });
    const app = await office(false,
      async () => { if (waitingFor === "audio") await pending; },
      async data => { if (waitingFor === "conversation") await pending; return { ok: true, json: async () => structuredClone(data) }; });
    await app.feed([message("latest", "2026-11-01T00:00:01Z")]);
    app.room([message("latest", "2026-11-01T00:00:01Z", { conversation: "room", thread: { kind: "room", id: "room" } })]);
    await app.command("listen", { group: "workspace", room: "room" });
    app.advance(8 * 3600000);
    release(); await flush(); await flush();
    assert.equal(app.published.at(-1).follow, null);
    assert.equal(app.sounds.length, 0, waitingFor);
    assert.equal(app.requests.length, waitingFor === "conversation" ? 0 : 1);
    await app.tick();
    assert.equal(app.sounds.length, 0, "late results cannot restart playback");
  }
});

test("expiry preserves the speaking manual clip but withdraws its followed chain", async () => {
  for (const asleep of [false, true]) {
    const app = await office();
    const messages = [1, 2, 3, 4].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, {
      conversation: "room", thread: { kind: "room", id: "room" },
    }));
    await app.feed(messages);
    app.room(messages);
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, messages[1], app.ctx);
    if (asleep) app.advance(8 * 3600000);
    rec.read.listeners.click(); await flush(); await flush();
    const manual = app.sounds.at(-1);
    await app.tick(8 * 3600000);
    assert.equal(manual.paused, false);
    await app.end(); await app.end(); await app.end();
    assert.deepEqual(app.sounds.map(s => s.text), asleep ? ["m1", "m2", "m3", "m4"] : ["m1", "m2"]);
    assert.equal(app.published.at(-1).state, "idle");
  }
});

test("listening captures its deadline, renews on another click and does not survive a new player", async () => {
  const app = await office(false);
  await app.feed([message("latest", "2026-11-01T00:00:01Z")]);
  assert.equal(app.requests.length, 0, "enabling voice alone never plays messages");
  await app.command("listen", { group: "workspace" });
  const first = app.published.at(-1).follow.expires_at;
  await app.settings({ listen_minutes: 30 });
  assert.equal(app.published.at(-1).follow.expires_at, first, "a settings change leaves this session's deadline alone");
  await app.command("listen", { group: "workspace" });
  const renewed = app.published.at(-1).follow.expires_at;
  assert.ok(renewed < first, "the next listen uses the new duration");
  await app.tick(1000);
  app.rooms.set("/app/chats/elsewhere", { ...app.ctx.c, group: "elsewhere", messages: [] });
  await app.command("listen", { group: "elsewhere" });
  assert.equal(app.published.at(-1).follow.expires_at, renewed + 1000);
  const restarted = await office(false);
  assert.equal(restarted.published.at(-1).follow, null);
});
