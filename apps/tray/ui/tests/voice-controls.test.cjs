const test = require("node:test");
const assert = require("node:assert/strict");
const { identities } = require("./helpers/common.cjs");
const { office, panel, settingsFrame, flush, message } = require("./helpers/voice.cjs");

const open = app => {
  const menu = app.nodes.find(node => node.tag === "div.voice-menu");
  if (menu.hidden) app.nodes.find(node => node.tag === "button.bar-voice").listeners.click();
};
const node = (app, tag) => app.nodes.find(item => item.tag === tag);
const roomMessages = () => [1, 2, 3].map(n => message(`m${n}`, `2026-11-01T00:00:0${n}Z`, { conversation: "room", thread: { kind: "room", id: "room" } }));

test("the top bar only opens a player with named icon controls and the current row cannot pause it", async () => {
  const app = await office();
  await app.feed(roomMessages());
  open(app);
  assert.equal(app.sounds[0].paused, false);
  assert.deepEqual(node(app, "div.voice-controls").children.map(button => button.attributes["aria-label"]), ["点读后连播", "上一条", "暂停", "下一条", "停止全部"]);
  assert.equal(app.nodes.some(item => ["button.voice-cover-play", "button.voice-toggle"].includes(item.tag)), false);
  const row = node(app, "ol.voice-queue").children[0];
  assert.equal(row.state.textContent, "正在播放");
  assert.equal(row.play.disabled, true);
  row.play.listeners.click(); await app.command("play", "m1");
  assert.equal(app.sounds[0].paused, false);
  node(app, "ol.voice-queue").children[2].play.listeners.click(); await flush();
  assert.equal(app.sounds.at(-1).text, "m3");
});

test("previous goes back one message and what it interrupted comes back after it", async () => {
  const app = await office();
  await app.feed(roomMessages());
  assert.equal(app.sounds.at(-1).text, "m1");
  await app.end();
  assert.equal(app.sounds.at(-1).text, "m2");
  const interrupted = app.sounds.at(-1);
  await app.command("previous");
  assert.equal(app.sounds.at(-1).text, "m1");
  assert.equal(interrupted.paused, true);
  await app.end();
  assert.equal(interrupted.paused, false, "the interrupted message carries on where it was");
});

test("a chain switch that fails to save keeps the setting and reports why", async () => {
  let refuse = true;
  const app = await office(false, undefined, undefined, "all", async (path, options) =>
    refuse && path === "/app/voice" && options?.method === "POST" ? { ok: false, json: async () => ({ error: "没保存" }) } : null);
  await app.command("flip", "continue_read");
  assert.equal(app.published.at(-1).continue_read, true);
  assert.equal(app.published.at(-1).error, "没保存");
  refuse = false;
  await app.command("flip", "continue_read");
  assert.equal(app.published.at(-1).continue_read, false);
});

test("a new read replaces what previous set aside when chaining is off", async () => {
  const app = await office(false);
  await app.settings({ continue_read: false });
  const [m1, m2, m3] = roomMessages();
  const read = async item => {
    const rec = { bub: { prepend() {} } };
    app.voice.decorate(rec, item, app.ctx);
    rec.read.listeners.click(); await flush();
  };
  await read(m1); await app.end();
  await read(m2);
  await app.command("previous");
  assert.equal(app.playbacks.at(-1).text, "m1");
  await read(m3);
  await app.end();
  assert.equal(app.playbacks.at(-1).text, "m3");
  assert.equal(app.published.at(-1).jobs.length, 0, "m2 does not carry on after it");
});

test("the chain button in the top bar menu switches continued reading", async () => {
  const app = await office();
  await app.feed(roomMessages());
  open(app);
  const chain = node(app, "button.voice-chain");
  assert.equal(chain.attributes["aria-pressed"], "true");
  await chain.listeners.click(); await flush();
  assert.equal(chain.attributes["aria-pressed"], "false");
});

test("the panel's player has the chain button too, off until the office has told it the setting", async () => {
  const app = await office();
  const clicks = [], panel = app.AN.voicePlayer((...click) => clicks.push(click));
  const chain = panel.el.children.at(-1).children[0];
  panel.draw(app.AN.voiceState({ enabled: true, ready: true, jobs: [] }));
  assert.equal(chain.disabled, true);
  panel.draw(app.AN.voiceState({ enabled: true, ready: true, jobs: [], continue_read: true }));
  assert.equal(chain.attributes["aria-pressed"], "true");
  chain.listeners.click();
  assert.deepEqual(clicks, [["flip", "continue_read"]]);
});

test("the bar labels waiting, pause, preparation and missing configuration without starting playback", async () => {
  let release;
  const app = await office(false, () => new Promise(resolve => { release = resolve; }));
  assert.equal(node(app, "span.bar-voice-label").textContent, "语音");
  await app.command("listen", { group: "workspace" });
  assert.equal(node(app, "span.bar-voice-label").textContent, "收听中");
  open(app);
  assert.equal(node(app, "button.voice-primary").disabled, false, "waiting listening can pause");
  node(app, "button.voice-primary").listeners.click(); await flush();
  assert.equal(node(app, "span.bar-voice-label").textContent, "已暂停");
  await app.feed([message("late", "2026-11-01T00:00:01Z")]);
  assert.equal(app.requests.length, 0);
  node(app, "button.voice-primary").listeners.click(); await flush();
  assert.equal(node(app, "span.bar-voice-label").textContent, "准备中");
  assert.equal(node(app, "div.bar-voice-wrap").dataset.sounding, "false");
  release(); await flush();
  assert.equal(node(app, "span.bar-voice-label").textContent, "语音");
  assert.equal(node(app, "div.bar-voice-wrap").dataset.sounding, "true");
  await app.settings({ keys: { elevenlabs: false } });
  assert.equal(node(app, "span.bar-voice-label").textContent, "未配置");
  assert.equal(node(app, "button.voice-primary").attributes["aria-label"], "语音设置");
  assert.equal(node(app, "button.listen-here").hidden, false);
  assert.equal(node(app, "button.listen-here").disabled, true);
  await app.settings({ enabled: false });
  assert.equal(node(app, "div.bar-voice-wrap").hidden, true);
  assert.equal(node(app, "button.listen-here").hidden, true);
});

test("next skips the current clip, remains paused when paused and does not mark skipped audio as heard", async () => {
  const app = await office(false), list = roomMessages();
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  await app.command("toggle");
  const sent = app.requests.length;
  await app.command("next");
  assert.equal(app.published.at(-1).paused, true);
  assert.equal(app.requests.length, sent);
  assert.equal(app.published.at(-1).jobs[0].id, "m2");
  await app.command("toggle");
  assert.equal(app.sounds.at(-1).text, "m2");
  await app.command("next");
  assert.equal(app.sounds.at(-1).text, "m3");
  open(app);
  assert.equal(node(app, "button.voice-next").disabled, true);
  await app.command("stop_all");
  await app.command("listen", { group: "workspace", room: "room" });
  assert.equal(app.sounds.at(-1).text, "m1");
});

test("next during backlog retains the paused state and never starts synthesis", async () => {
  const app = await office();
  await app.feed(roomMessages());
  await app.tick(300000);
  await app.end();
  const sent = app.requests.length;
  await app.command("next");
  assert.equal(app.published.at(-1).paused, true);
  assert.equal(app.published.at(-1).backlog, true);
  assert.equal(app.published.at(-1).bar_label, "已暂停");
  assert.equal(app.requests.length, sent);
  assert.equal(app.published.at(-1).jobs[0].id, "m3");
});

test("stop listening preserves a manual clip while stop all immediately stops every source", async () => {
  const app = await office();
  await app.settings({ continue_read: false });
  await app.feed(roomMessages());
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, message("manual", "2026-11-01T00:00:04Z"), app.ctx);
  rec.read.listeners.click(); await flush();
  await app.command("unlisten");
  assert.equal(app.sounds.at(-1).paused, false);
  assert.deepEqual(app.published.at(-1).jobs.map(job => job.id), ["manual"]);
  await app.command("stop_all");
  assert.ok(app.sounds.every(sound => sound.paused));
  assert.equal(app.published.at(-1).follow, null);
  assert.equal(app.published.at(-1).enabled, true);
  assert.equal(app.published.at(-1).state, "idle");
  const sent = app.requests.length;
  await app.feed([...roomMessages(), message("later", "2026-11-01T00:00:05Z")]);
  assert.equal(app.requests.length, sent);
});

test("stop all preserves only completed progress and cancelling chaining in settings removes its tail", async () => {
  const app = await office(false), list = roomMessages();
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  await app.end();
  await app.command("stop_all");
  await app.command("listen", { group: "workspace", room: "room" });
  assert.equal(app.sounds.at(-1).text, "m2");
  await app.command("stop_all");
  await app.room(list).listeners.click();
  await app.settings({ continue_read: false });
  await app.end();
  assert.equal(app.published.at(-1).state, "idle");
});

test("failed audio is retryable and authentication failures offer settings instead", async () => {
  let broken = true;
  const app = await office(true, async () => { if (broken) throw new Error("temporary failure"); });
  await app.feed([message("retry", "2026-11-01T00:00:01Z")]);
  open(app);
  assert.equal(node(app, "button.voice-primary").attributes["aria-label"], "重试");
  assert.equal(app.published.at(-1).retryable, true);
  broken = false;
  node(app, "button.voice-primary").listeners.click(); await flush();
  assert.equal(app.sounds.at(-1).text, "retry");
  assert.equal(app.published.at(-1).status, "正在播放");
  const config = await office(true, undefined, undefined, "all", async path => path === "/app/voice/speak" ? { ok: false, status: 400, json: async () => ({ error: "API Key 未配置" }) } : null);
  await config.feed([message("blocked", "2026-11-01T00:00:01Z")]);
  assert.equal(config.published.at(-1).bar_label, "出错");
  assert.equal(config.published.at(-1).primary.command, "settings");
  assert.equal(config.published.at(-1).retryable, false);
});

test("after a stop the player keeps what played for a minute and can play it again, unless something new plays", async () => {
  const app = await office(false);
  const said = message("said", "2026-11-01T00:00:01Z");
  await app.feed([said]);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, said, app.ctx);
  rec.read.listeners.click(); await flush();
  await app.command("stop_all");
  open(app);
  assert.equal(app.published.at(-1).jobs.length, 0);
  assert.equal(app.published.at(-1).recent.id, "said");
  assert.equal(node(app, "div.voice-album").hidden, false);
  assert.equal(node(app, "button.voice-primary").dataset.icon, "play");
  assert.equal(node(app, "button.voice-primary").disabled, false);
  await app.tick(59000);
  assert.equal(app.published.at(-1).recent.id, "said");
  await app.tick(2000);
  assert.equal(app.published.at(-1).recent, null);
  assert.equal(node(app, "div.voice-album").hidden, true);
  rec.read.listeners.click(); await flush();
  await app.command("stop_all");
  assert.equal(app.published.at(-1).recent.id, "said");
  node(app, "button.voice-primary").listeners.click(); await flush();
  assert.equal(app.published.at(-1).recent, null, "playing again replaces it");
  assert.equal(app.published.at(-1).jobs[0].id, "said");
});

test("stopping a listen that said nothing does not bring back an earlier message", async () => {
  const app = await office(false);
  const said = message("said", "2026-11-01T00:00:01Z");
  await app.feed([said]);
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, said, app.ctx);
  rec.read.listeners.click(); await flush();
  await app.end();
  await app.command("listen", { group: "workspace" });
  await app.command("unlisten");
  assert.equal(app.published.at(-1).recent, null);
});

test("listen buttons remain pure listening controls", async () => {
  const app = await office(false);
  await app.command("listen", { group: "workspace" });
  open(app);
  assert.equal(node(app, "p.voice-status").textContent, "等待新消息");
  const [workspace, column] = app.nodes.filter(item => item.tag === "button.listen-here");
  const label = workspace.children[0];
  assert.equal(label.textContent, "停止收听");
  assert.equal(workspace.children.length, 1);
  await app.command("toggle");
  assert.equal(label.textContent, "停止收听");
  assert.equal(node(app, "button.voice-primary").attributes["aria-label"], "继续");
  assert.equal(app.nodes.some(item => item.id === "voice-banner"), false);
  await app.command("unlisten");
  app.AN.setLanguage("en");
  await app.voice.refresh();
  assert.equal(workspace.children[0].textContent, "Listen to workspace");
  assert.equal(column.children[0].textContent, "Listen to this column");
  app.AN.setLanguage("zh-CN");
  await app.voice.refresh();
  assert.equal(workspace.children[0].textContent, "收听工作区");
  assert.equal(column.children[0].textContent, "收听此列消息");
});

const desk = (key, running = false, kind = "task") => ({ key, kind, provider: "codex", model: key, status: running ? "running" : "idle", last_active: "2026-11-01T00:00:01Z", activities: running ? [{ kind: "private", status: "running" }] : [] });
const room = (id, status = "finished") => ({ id, title: id, status, last_active: id === "old" ? "2026-11-01T00:00:01Z" : "2026-11-01T00:00:02Z", seats: [] });
const manager = (group, desks = [], rooms = [], extra = {}) => ({ group, label: group, host: "codex", updated: "2026-11-01T00:00:01Z", desks, rooms, status: "idle", ...extra });

test("the panel folds every idle office and displays only running non-seat desks in a working card", async () => {
  const app = await panel([manager("idle", [desk("old")], [], { live: true, status: "failed" })]);
  assert.equal(app.elements.get("groups").children.length, 0, "live presence and failure do not count as running work");
  assert.equal(app.elements.get("quiet").children.length, 1);
  const desks = [desk("running", true), desk("idle"), desk("seat", true, "seat"), ...Array.from({ length: 8 }, (_, n) => desk(`sleep-${n}`))];
  app.render([manager("busy", desks, [room("meeting", "running")])]);
  const card = app.elements.get("groups").children[0];
  const cells = card.children.find(child => child.tag === "div.desks");
  assert.equal(cells.children.length, 1);
  assert.equal(cells.children[0].attributes.href, "/chat/busy#desk:running");
  assert.equal(card.children.find(child => child.tag === "button.fold").hidden, true, "folding counts filtered desks");
  assert.equal(card.children[0].children.length, 1, "workspace cards have no listening button");
  assert.equal(app.elements.get("quiet").children.length, 0);
});

test("a meeting waiting for the boss's review keeps its office expanded", async () => {
  const app = await panel([manager("workspace", [], [room("waiting", "review")])]);
  app.show(app.snapshot({}));
  assert.equal(app.elements.get("groups").children.length, 1);
  assert.equal(app.elements.get("quiet").children.length, 0);
});

test("a panel listening snapshot pins its office and ended meeting, then returns them to idle", async () => {
  const app = await panel([manager("workspace", [], [room("old"), room("latest")])]);
  const snap = app.snapshot({ follow: { group: "workspace", room: "old", title: "old", expires_at: Date.now() + 3600000 } });
  app.show(snap);
  const card = app.elements.get("groups").children[0];
  const rooms = card.children.find(child => child.tag === "div.rooms");
  assert.equal(rooms.children.length, 2);
  assert.equal(rooms.children[0].children[1].textContent, "停止收听");
  assert.equal(node(app, "div.voice-now").hidden, false);
  assert.equal(node(app, "p.voice-status").textContent, snap.status);
  assert.equal(node(app, "button.voice-primary").disabled, false);
  app.show(app.snapshot({}));
  assert.equal(app.elements.get("groups").children.length, 0);
  assert.equal(app.elements.get("quiet").children.length, 1);
  assert.equal(node(app, "div.voice-now").hidden, true);
});

test("meeting listen requests show opening and deferred states, and their cancel button stays visible", async () => {
  const app = await panel([manager("workspace", [], [room("old", "running")])], async () => ({ ok: true, json: async () => ({ opening: false }) }));
  const listen = node(app, "button.voice-listen");
  assert.equal(listen.textContent, "收听");
  listen.listeners.click();
  assert.equal(listen.textContent, "取消收听");
  assert.equal(node(app, "p.voice-status").textContent, "正在打开办公室…");
  await flush();
  assert.equal(node(app, "p.voice-status").textContent, "等待返回办公室");
  app.render([manager("workspace", [], [room("old")])]);
  assert.equal(app.elements.get("groups").children.length, 1);
  listen.listeners.click(); await flush();
  assert.equal(app.commands.at(-1).kind, "unlisten");
  assert.equal(node(app, "div.voice-now").hidden, true);
});

test("a failed panel listen request has a visible retry action and cancels the waiting command", async () => {
  let broken = true;
  const app = await panel([manager("workspace", [], [room("old", "running")])], async () => {
    if (broken) throw new Error("offline");
    return { ok: true, json: async () => ({ opening: true }) };
  });
  node(app, "button.voice-listen").listeners.click(); await flush(); await flush();
  assert.equal(node(app, "p.voice-status").textContent, "无法开始收听");
  assert.equal(node(app, "button.voice-primary").attributes["aria-label"], "重试");
  assert.equal(node(app, "button.voice-primary").dataset.icon, "retry");
  assert.equal(app.commands.at(-1).kind, "unlisten");
  broken = false;
  node(app, "button.voice-primary").listeners.click(); await flush();
  assert.equal(app.commands.at(-1).kind, "listen");
  await app.tick(20000);
  assert.equal(node(app, "p.voice-status").textContent, "无法开始收听");
});

test("the panel uses the office snapshot for backlog, controls and retry instead of guessing from one job", async () => {
  const app = await panel([]);
  const snap = app.snapshot({ paused: true, backlog: true, can_next: true, can_stop: true, state: "paused", jobs: [{ id: "current", album: { title: "album", link: "conv:room" }, group: "workspace" }] });
  app.show(snap);
  assert.equal(node(app, "p.voice-status").textContent, "消息积压，已暂停收听");
  assert.equal(node(app, "button.voice-next").disabled, false, "the truncated panel snapshot still knows there is a next clip");
  node(app, "button.voice-next").listeners.click(); await flush();
  assert.equal(app.commands.at(-1).kind, "next");
  node(app, "button.voice-stop-all").listeners.click(); await flush();
  assert.equal(app.commands.at(-1).kind, "stop_all");
  app.show(app.snapshot({ error: "bad audio", error_source: "playback", halted: true, retryable: true }));
  assert.equal(node(app, "button.voice-primary").attributes["aria-label"], "重试");
  assert.equal(node(app, "p.voice-status").textContent, "播放已停下：bad audio");
  assert.equal(app.nodes.some(item => item.tag === "ol.voice-queue"), false);
});

test("the panel folds eight visible running desks and still announces a seat that disappears on completion", async () => {
  const desks = Array.from({ length: 8 }, (_, n) => desk(`worker-${n}`, true));
  desks.push({ ...desk("seat", true, "seat"), model: "gpt-x", identity: { ...identities.review, key: "seat" } }, desk("idle"));
  const app = await panel([manager("workspace", desks)]);
  const card = app.elements.get("groups").children[0];
  const cells = card.children.find(child => child.tag === "div.desks");
  assert.equal(cells.children.length, 8);
  assert.equal(cells.children.filter(child => child.hidden).length, 2);
  const fold = card.children.find(child => child.tag === "button.fold");
  assert.equal(fold.hidden, false);
  fold.listeners.click();
  assert.equal(cells.children.filter(child => child.hidden).length, 0);
  await app.feed([manager("workspace", desks.map(item => ({ ...item, activities: [], status: "succeeded" })))]);
  assert.equal(app.elements.get("groups").children.length, 0);
  assert.match(app.elements.get("announce").textContent, /gpt-x succeeded/,
    "the completed seat is announced by its service identity even though it was never a visible cell");
  assert.match(app.elements.get("announce").textContent, /未记录身份 succeeded/,
    "old desks without identity remain unknown instead of being named from their key or raw model");
});

test("pausing synthesis keeps that clip selected for next, bubbles and settings changes", async () => {
  const pending = [];
  const app = await office(false, () => new Promise(resolve => pending.push(resolve)));
  const list = roomMessages();
  await app.room(list).listeners.click();
  await app.command("toggle");
  open(app);
  assert.equal(app.published.at(-1).jobs[0].id, "m1");
  const rec = { bub: { prepend() {} } };
  app.voice.decorate(rec, list[0], app.ctx);
  assert.equal(rec.readLabel.textContent, "继续");
  await app.settings({ continue_read: false });
  assert.deepEqual(app.published.at(-1).jobs.map(job => job.id), ["m1"], "disabling chaining preserves the paused current clip");
  await app.command("toggle");
  pending.forEach(resolve => resolve()); await flush();
  assert.equal(app.sounds[0].text, "m1");
  const listening = await office(true, () => new Promise(() => {}));
  await listening.feed(list);
  await listening.command("toggle");
  await listening.command("next");
  assert.equal(listening.published.at(-1).jobs[0].id, "m2");
  assert.equal(listening.published.at(-1).paused, true);
});

test("choosing another playlist row explicitly starts it while a click on the paused current row does nothing", async () => {
  const app = await office();
  await app.feed(roomMessages());
  await app.command("toggle");
  open(app);
  const rows = node(app, "ol.voice-queue").children;
  rows[0].play.listeners.click(); await flush();
  assert.equal(app.published.at(-1).paused, true);
  rows[2].play.listeners.click(); await flush();
  assert.equal(app.published.at(-1).paused, false);
  assert.equal(app.sounds.at(-1).text, "m3");
});

test("settings overlays a playing office without replacing audio, listening, album or queue", async () => {
  const app = await office(false), list = roomMessages();
  app.room(list);
  await app.command("listen", { group: "workspace", room: "room" });
  const clip = app.sounds[0], before = app.published.at(-1);
  clip.currentTime = 8.75;
  await app.command("settings");
  const frame = node(app, "iframe.office-settings"), source = frame.src;
  assert.match(source, /\/settings\/\?embedded=1#voice-settings$/);
  assert.equal(frame.hidden, false);
  assert.equal(app.navigations.length, 0, "the main document never navigates to settings");
  assert.equal(clip.paused, false);
  assert.equal(clip.currentTime, 8.75);
  frame.contentWindow.draft = "unsaved edit";
  app.navigate("/chat/"); await flush();
  assert.equal(frame.hidden, true);
  assert.equal(clip.paused, false);
  const after = app.published.at(-1);
  assert.deepEqual(after.follow, before.follow);
  assert.deepEqual(after.jobs, before.jobs);
  assert.equal(app.requests.filter(request => request.text === "m1").length, 1);
  app.navigate("/settings/");
  assert.equal(node(app, "iframe.office-settings"), frame);
  assert.equal(frame.src, source);
  assert.equal(frame.contentWindow.draft, "unsaved edit");
  app.settingsMessage({ action: "navigate", path: "http://agentnave.localhost/chat/workspace#conv:room" });
  assert.equal(frame.hidden, true);
  assert.deepEqual(app.navigations, ["/chat/workspace#conv:room"]);
  await app.end();
  assert.equal(app.sounds.at(-1).text, "m2", "the retained queue continues normally");
});

test("saved settings and closing the frame refresh voice configuration without reloading the office", async () => {
  const app = await office(false), list = roomMessages();
  await app.room(list).listeners.click();
  app.navigate("/settings/#members");
  app.setConfig({ continue_read: false });
  app.settingsMessage({ action: "changed" }); await flush();
  await app.end();
  assert.equal(app.published.at(-1).state, "idle", "saved chaining changes are applied while settings stays open");
  app.setConfig({ enabled: false });
  app.navigate("/chat/"); await flush();
  assert.equal(app.published.at(-1).enabled, false, "closing settings also reads the latest configuration");
  assert.equal(app.navigations.length, 0);
});

test("settings anchors reuse the retained frame and an unrelated window cannot send navigation", async () => {
  const app = await office(false);
  app.navigate("/settings/#general");
  const frame = node(app, "iframe.office-settings"), source = frame.src;
  app.settingsMessage({ action: "ready" });
  app.navigate("/settings/#voice-settings");
  assert.equal(frame.src, source);
  assert.equal(frame.contentWindow.messages.at(-2).action, "section");
  assert.equal(frame.contentWindow.messages.at(-2).hash, "#voice-settings");
  app.settingsMessage({ action: "navigate", path: "/chat/elsewhere" }, {});
  assert.equal(frame.hidden, false);
  assert.equal(app.navigate("https://example.com/settings/"), false);
  assert.equal(app.navigations.length, 0);
});

test("office navigation highlights settings, then the brand or Office returns to the same office", async () => {
  const app = await office(false);
  const [officeLink, settingsLink] = app.navLinks;
  app.clickLink(settingsLink);
  const frame = node(app, "iframe.office-settings");
  assert.equal(frame.hidden, false);
  assert.equal(settingsLink.attributes["aria-current"], "page");
  assert.equal(officeLink.attributes["aria-current"], undefined);
  app.clickLink(officeLink);
  assert.equal(frame.hidden, true);
  assert.equal(officeLink.attributes["aria-current"], "page");
  app.clickLink(settingsLink);
  app.clickLink({ href: "http://agentnave.localhost/chat/" });
  assert.equal(frame.hidden, true);
  assert.equal(app.navigations.length, 0);
});

test("embedded settings routes office links outward and keeps its own section navigation local", () => {
  const app = settingsFrame();
  assert.equal(app.document.documentElement.dataset.embedded, "true");
  app.emit("DOMContentLoaded");
  assert.equal(app.context.location.hash, "#voice");
  assert.equal(app.messages.at(-1).action, "ready");
  let prevented = 0;
  const click = (href, raw = href) => app.document.listeners.click({ button: 0, target: { closest: () => ({ href, getAttribute: () => raw }) }, preventDefault() { prevented++; }, stopImmediatePropagation() {} });
  click("http://agentnave.localhost/chat/workspace#conv:room");
  assert.equal(prevented, 1);
  assert.equal(app.messages.at(-1).path, "http://agentnave.localhost/chat/workspace#conv:room");
  assert.equal(app.context.location.pathname, "/settings/");
  click("http://agentnave.localhost/settings/?embedded=1#members", "#members");
  assert.equal(prevented, 1);
  const sent = app.messages.length;
  app.message({ action: "section", hash: "#members" }, {});
  assert.equal(app.context.location.hash, "#voice");
  app.message({ action: "section", hash: "#members" });
  assert.equal(app.context.location.hash, "#members");
  app.message({ action: "preferences", general: { theme: "dark" } });
  assert.equal(app.messages.length, sent, "preferences forwarded from the office do not echo back");
  app.message({ action: "visible", visible: false });
  assert.equal(app.emitted.at(-1), "agentnave:settings-hidden");
  app.api.changed();
  assert.equal(app.messages.at(-1).action, "changed");
  assert.equal(settingsFrame(false).api, undefined, "standalone settings keeps normal navigation");
});

test("update navigation opens the update flow over settings without replacing the office or its player", async () => {
  const app = await office();
  await app.feed(roomMessages());
  const clip = app.sounds[0];
  app.navigate("/settings/#about");
  app.navigate("/chat/?check-update");
  assert.equal(node(app, "iframe.office-settings").hidden, false);
  assert.deepEqual(app.navigations, ["check-update"]);
  assert.equal(clip.paused, false);
  assert.equal(app.published.at(-1).follow.group, "workspace");
});

test("a section requested while settings is loading is delivered when that frame becomes ready", async () => {
  const app = await office(false);
  app.navigate("/settings/#general");
  app.navigate("/settings/#voice-settings");
  const frame = node(app, "iframe.office-settings");
  app.settingsMessage({ action: "ready" });
  assert.equal(frame.contentWindow.messages.filter(item => item.action === "section").at(-1).hash, "#voice-settings");
});

test("the response CSP permits only same-origin settings frames", () => {
  const source = require("node:fs").readFileSync(`${__dirname}/../../src/data.rs`, "utf8");
  const csp = /pub const CSP: &str = "([^"]+)"/.exec(source)[1];
  const directives = new Map(csp.split(";").map(part => { const [name, ...values] = part.trim().split(/\s+/); return [name, values]; }));
  assert.deepEqual(directives.get("frame-src"), ["'self'"]);
  assert.deepEqual(directives.get("frame-ancestors"), ["'self'"]);
  assert.deepEqual(directives.get("script-src"), ["'self'"]);
});

test("panel listening commands work while the retained settings frame remains open", async () => {
  const app = await office(false);
  app.room(roomMessages());
  app.navigate("/settings/#general");
  const frame = node(app, "iframe.office-settings");
  await app.command("listen", { group: "workspace", room: "room" });
  assert.equal(frame.hidden, false);
  assert.equal(app.published.at(-1).follow.room, "room");
  assert.equal(app.sounds[0].text, "m1");
  await app.command("toggle");
  assert.equal(app.sounds[0].paused, true);
  await app.command("toggle");
  assert.equal(app.sounds[0].paused, false);
});
