"use strict";
// Menu bar panel: one card per boss (the host CLI session), headed by the boss with its office below.
// Offices with running work or active listening are cards; the rest fold into one line each.
// Every node is created once and then patched in place (keyed by group id / desk key / room id).
// Links are real routes (/chat/<group>); the tray app intercepts them and opens the main window.
const {
  h, $, L, load, href, MEMBER, GROUP, pick, urgent, colorClass, relTime, fullTime,
  setText, setAttr, setClass, reconcile, stickyOrder, poll, art: ART, cliOf, fullHandle, deskName, chatName, shownName, chatPlace, activities, dozing, ongoing, clip,
} = AN;

const ACTIVE_SHOWN = 8; // most working desks one office card shows
const IDLE_SHOWN = 4; // most recently finished desks it shows besides

if (new URLSearchParams(location.search).has("glass")) document.body.classList.add("glass");
document.querySelector("header.top .top-end").append(AN.themeSwitch());

// ---------------------------------------------------------------- voice
// The office owns player decisions; the panel only owns a request waiting for that office.
let voiceNow = null, voiceConfig = null, voiceAsked = null, voiceIssue = null;
const voiceReady = () => voiceNow ? voiceNow.enabled && voiceNow.ready
  : Boolean(voiceConfig?.enabled && voiceConfig.keys?.[voiceConfig.engine]);
function voiceCommand(kind, value) {
  if (kind === "retry" && voiceIssue) { ask(voiceIssue.target); return Promise.resolve(); }
  if (["unlisten", "stop_all"].includes(kind)) { voiceAsked = null; voiceIssue = null; redrawListen(); }
  return fetch("/app/voice/player/command", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
    body: JSON.stringify({ kind, value }) }).catch(() => {});
}
const listening = target => Boolean(voiceNow?.follow && !voiceNow.follow.desk
  && voiceNow.follow.group === target.group && (voiceNow.follow.room || null) === (target.room || null));
const asked = target => Boolean(voiceAsked && voiceAsked.target.group === target.group && voiceAsked.target.room === (target.room || null));
function failListen(request) {
  if (voiceAsked !== request) return;
  void voiceCommand("unlisten");
  voiceIssue = { target: request.target, message: L("无法开始收听", "Unable to start listening") }; redrawListen();
}
function ask(target) {
  voiceIssue = null;
  const request = voiceAsked = { target: { group: target.group, room: target.room || null }, deferred: false };
  redrawListen();
  fetch("/app/voice/player/command", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
    body: JSON.stringify({ kind: "listen", value: request.target }) })
    .then(async response => { if (!response.ok) throw new Error(); return response.json(); })
    .then(reply => { if (voiceAsked === request && reply.opening === false) { request.deferred = true; redrawListen(); } })
    .catch(() => failListen(request));
  setTimeout(() => { if (voiceAsked === request && !request.deferred) failListen(request); }, 20000);
}
function listenButton() {
  const button = h("button.voice-listen", { type: "button", hidden: true });
  button.addEventListener("click", () => {
    if (button.disabled || !button.target) return;
    if (asked(button.target) || listening(button.target)) voiceCommand("unlisten");
    else ask(button.target);
  });
  return button;
}
// A model-name choice changes the names shown; draw them again now.
addEventListener("agentnave:model-names", () => { if (state.lastData) render(state.lastData); });
function redrawListen() {
  for (const button of document.querySelectorAll("button.voice-listen")) if (button.target) drawListen(button);
  drawVoice();
  if (state.lastData) render(state.lastData);
}
function drawListen(button, target = button.target) {
  button.target = target;
  button.hidden = !(voiceNow ? voiceNow.enabled : voiceConfig?.enabled);
  button.disabled = !voiceReady() && !asked(target) && !listening(target);
  const on = listening(target), waiting = !on && asked(target);
  setText(button, waiting ? L("取消收听", "Cancel listening") : on ? L("停止收听", "Stop listening") : L("收听", "Listen"));
  button.dataset.waiting = String(waiting);
  button.dataset.on = String(on);
  setAttr(button, "aria-pressed", String(on));
}
const voicePlayer = AN.voicePlayer(voiceCommand), voiceCard = h("div.voice-now", { hidden: true }, voicePlayer.el);
$("scroll").prepend(voiceCard);
function drawVoice() {
  let snap = voiceNow || AN.voiceState({ enabled: Boolean(voiceConfig?.enabled), ready: voiceReady(), jobs: [], state: "idle", can_stop: false });
  if (voiceAsked || voiceIssue) snap = { ...snap,
    status: voiceIssue?.message || (voiceAsked.deferred ? L("等待返回办公室", "Waiting to return to the office") : L("正在打开办公室…", "Opening the office…")),
    primary: voiceIssue ? { icon: "retry", label: L("重试", "Retry"), command: "retry", disabled: false } : { label: L("暂停", "Pause"), command: "toggle", disabled: true }, can_next: false, can_stop: true };
  voiceCard.hidden = !(voiceAsked || voiceIssue || snap.enabled && (snap.playing || snap.jobs?.length || snap.recent || snap.follow || snap.paused || snap.backlog || snap.error || !snap.ready));
  voicePlayer.draw(snap);
}
function showVoice(snap) {
  voiceNow = snap;
  if (voiceAsked && listening(voiceAsked.target) || voiceIssue && listening(voiceIssue.target)) { voiceAsked = null; voiceIssue = null; }
  redrawListen();
}
const voiceSettings = () => load("/app/voice").then(config => { voiceConfig = config; redrawListen(); }, () => {});
addEventListener("agentnave:voice-player", ({ detail }) => showVoice(detail));
document.addEventListener("visibilitychange", () => { if (!document.hidden) void voiceSettings(); });
load("/app/voice/player").then(showVoice, () => {});
void voiceSettings();

const stage = $("stage");
const state = {
  blocks: new Map(), // group id -> office block
  quiet: new Map(), // group id -> folded row
  order: [],
  seen: new Map(), // `${group}|${desk key}` -> was running (for announcements)
  ready: false,
  appStatus: null,
  calm: "",
  lastData: null,
};

// ---------------------------------------------------------------- what a desk is doing
const deskChar = (d) => ART.character(d.provider, d.art_model || d.model);
// A reviewed answer waits only briefly on the boss, so it reads as an answer given, not a state of its own.
// A desk that just stopped stays a while, so how it ended can be read.
const LINGER = 5 * 60e3;
const lingering = (d) => d.kind !== "seat" && Date.now() - AN.at(d.last_active) < LINGER;
function brief(d) {
  const acts = activities(d);
  const run = acts.filter((a) => a.status === "running");
  if (run.some((a) => a.kind === "private")) return { mood: "working", word: run.length > 1 ? L("私聊中 · 也在开会", "In chat · in meeting") : L("私聊中", "In chat") };
  if (run.length) return { mood: "working", word: L("会议里作答中", "Replying in meeting") };
  if (acts.some((a) => a.status === "review")) return { mood: "meeting", word: L("会议里已作答", "Replied in meeting") };
  if (acts.some((a) => a.kind === "meeting")) return { mood: "meeting", word: L("在会议室", "In meeting") };
  if (d.status === "failed") return { mood: "failed", word: L("上次失败", "Last failed") };
  if (d.status === "pending") return { mood: "idle", word: L("等待中", "Pending") };
  if (lingering(d)) return { mood: "idle", word: L("已完成", "Finished") };
  return dozing(d) ? { mood: "sleep", word: L("休息中", "Resting") } : { mood: "idle", word: L("摸鱼", "Slacking off") };
}
const HEAD = { working: "working", sleep: "sleep" };
const roomRound = (r) => {
  const n = r.round;
  if (!n) return pick(GROUP, ongoing(r.status))[0];
  if (urgent(r.status)) return L(`第 ${n.number} 轮 · ${n.order === "serial" ? "依次回复" : "同时回复"}`, `Round ${n.number} · ${n.order === "serial" ? "In turn" : "In parallel"}`);
  return L(`第 ${n.number} 轮 · ${pick(GROUP, ongoing(r.status))[0]}`, `Round ${n.number} · ${pick(GROUP, ongoing(r.status))[0]}`);
};
const BOARD = () => {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  svg.innerHTML = '<rect x="2" y="2.5" width="12" height="8.5" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M5 6.2h6M5 8.4h3.6M5.5 11l-1.3 3M10.5 11l1.3 3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>';
  return svg;
};

// ---------------------------------------------------------------- desk cell
function createCell() {
  const rec = { face: h("span.face"), cli: h("span.cli"), name: h("b"), role: h("em"), word: h("span.w"), faceKey: "" };
  rec.el = h("a.cell", {}, rec.face, h("span.nm", {}, h("span.nm-top", {}, rec.name, rec.cli), h("span.nm-sub", {}, rec.role, rec.word)));
  return rec;
}
function updateCell(rec, group, d) {
  const s = brief(d);
  const faceKey = `${deskChar(d)}|${HEAD[s.mood] || "idle"}`;
  if (rec.faceKey !== faceKey) {
    rec.faceKey = faceKey;
    rec.face.replaceChildren(ART.head(deskChar(d), HEAD[s.mood] || "idle"));
  }
  setText(rec.cli, cliOf(d));
  setClass(rec.el, `cell ${s.mood}${d.private_allowed === false ? " only" : ""}`);
  setAttr(rec.el, "href", href(`/chat/${group}#desk:${encodeURIComponent(d.key)}`));
  setAttr(rec.el, "title", `${fullHandle(d)}\n${s.word}${d.access === "read" ? L(" · 声明只读", " · Read-only") : d.access === "write" ? L(" · 声明可写", " · Writable") : ""}${d.private_allowed === false ? L(" · 仅开会", " · Meetings only") : ""}\n${L(`最近 ${fullTime(d.last_active)}`, `Last active ${fullTime(d.last_active)}`)}`);
  setText(rec.name, deskName(d));
  setText(rec.role, d.role ? `${clip(d.role, 6)} · ` : "");
  setText(rec.word, s.word);
  return s;
}

// ---------------------------------------------------------------- meeting room row
function createRoom() {
  const rec = { title: h("b"), round: h("span"), seats: h("span.seats"), seatKey: "", listen: listenButton() };
  rec.link = h("a.room", {}, h("span.wb", {}, BOARD()), h("span.rt", {}, rec.title, rec.round), rec.seats);
  rec.el = h("div.room-row", {}, rec.link, rec.listen);
  return rec;
}
function updateRoom(rec, group, r, desks) {
  const [word, tone] = pick(GROUP, ongoing(r.status));
  setClass(rec.link, `room tone-${tone}`);
  setAttr(rec.link, "href", href(`/chat/${group}#conv:${encodeURIComponent(r.id)}`));
  drawListen(rec.listen, { group, room: r.id });
  setAttr(rec.link, "title", L(`会议室 · ${r.title}\n${word} · 会议记录 ${r.board ?? 0} 条`, `Meeting room · ${r.title}\n${word} · ${r.board ?? 0} notes`));
  setText(rec.title, r.title);
  setText(rec.round, roomRound(r));
  const seated = r.seats.map((s) => desks.get(s.desk)).filter(Boolean);
  const key = seated.map(deskChar).join();
  if (rec.seatKey !== key) {
    rec.seatKey = key;
    rec.seats.replaceChildren(...seated.slice(0, 5).map((d) => ART.head(deskChar(d))));
  }
}

// ---------------------------------------------------------------- one boss and its office
// One card: the boss heads it, its desks and meeting rooms sit below.
function createBlock(m) {
  const rec = {
    id: m.group, data: m, tile: h("span.tile"), project: h("b"), meta: h("small"), bs: h("span.bs"),
    time: h("time"), cells: h("div.desks"), rooms: h("div.rooms"),
    cellRecs: new Map(), roomRecs: new Map(), tileKey: "",
  };
  rec.boss = h("a.boss", {}, h("span.host", {}, rec.tile), h("span.who", {}, rec.project, rec.meta), h("span.when", {}, rec.bs, rec.time));
  rec.el = h("section.blk", {}, h("div.boss-row", {}, rec.boss), rec.cells, rec.rooms);
  return rec;
}
// The boss shows as the character of the model it last spoke with, else its CLI's.
function bossChar(m) {
  const spoken = (m.boss_messages || []).filter((x) => x.kind === "prompt" && x.speaker?.model)
    .sort((a, b) => (b.speaker.provider === m.host) - (a.speaker.provider === m.host) || AN.at(b.time) - AN.at(a.time))[0];
  return ART.character(spoken?.speaker.provider || m.host, spoken?.speaker.model || "");
}
function tile(rec, m, mood) {
  const key = `${bossChar(m)}|${mood}`;
  if (rec.tileKey === key) return;
  rec.tileKey = key;
  rec.tile.replaceChildren(ART.head(bossChar(m), mood));
}
function running(m) {
  let n = 0;
  for (const d of m.desks || []) n += activities(d).filter((a) => a.status === "running").length;
  return n;
}
function updateBlock(rec, m) {
  rec.data = m;
  const desks = m.desks || [], rooms = m.rooms || [];
  const total = desks.length + (m.optimized?.count || 0);
  const n = running(m);
  const name = shownName(m);
  setClass(rec.el, `blk ${colorClass(m.color)}${n ? " run" : ""}`);
  tile(rec, m, n ? "working" : "idle");
  setAttr(rec.boss, "href", href(`/chat/${m.group}`));
  setAttr(rec.boss, "title", `${L(`${m.label} 里的 boss`, `boss in ${m.label}`)}${m.project ? ` · ${m.project}` : ""}`);
  setAttr(rec.boss, "aria-label", L(`${name}，boss 在 ${m.label}，${total} 个工位，${rooms.length} 间会议室，打开办公室`, `${name}, boss in ${m.label}, ${total} desks, ${rooms.length} meeting rooms, open office`));
  setText(rec.project, name);
  const [bsTone, bsText] = n ? ["run", L(`${n} 个在工作`, `${n} working`)] : urgent(m.status) ? ["run", L("会议进行中", "In meeting")] : ["", L("空闲", "Idle")];
  setClass(rec.bs, `bs${bsTone ? ` ${bsTone}` : ""}`);
  setText(rec.bs, bsText);
  setText(rec.time, relTime(m.updated));
  setAttr(rec.time, "title", fullTime(m.updated));

  const byKey = new Map(desks.map((d) => [d.key, d]));
  const working = d => brief(d).mood === "working";
  const shownDesks = desks.filter(d => d.kind !== "seat");
  const recent = (a, b) => AN.at(b.last_active) - AN.at(a.last_active);
  const ordered = [...shownDesks.filter(working).sort(recent).slice(0, ACTIVE_SHOWN), ...shownDesks.filter(d => !working(d)).sort(recent).slice(0, IDLE_SHOWN)];
  for (const key of rec.cellRecs.keys()) if (!ordered.some(d => d.key === key)) rec.cellRecs.delete(key);
  reconcile(rec.cells, ordered.map((d) => {
    let cell = rec.cellRecs.get(d.key);
    if (!cell) rec.cellRecs.set(d.key, (cell = createCell()));
    updateCell(cell, m.group, d);
    return cell.el;
  }));
  for (const d of [...desks, ...(m.optimized?.desks || [])]) announce(m, d, brief(d));
  // The payload carries the running desks and the latest others; the rest are counted with them.
  const meta = [name === m.label ? "" : chatPlace(m), L(`${total} 工位`, `${total} desks`), rooms.length && L(`${rooms.length} 会议室`, `${rooms.length} rooms`)].filter(Boolean).join(" · ");
  setText(rec.meta, meta);
  rec.cells.hidden = !ordered.length;
  const roomById = new Map(rooms.map((r) => [r.id, r]));
  // a seat's desk may be folded into "已优化"; the payload still lists it for the seat
  const seatDesks = new Map([...byKey, ...(m.optimized?.desks || []).map((d) => [d.key, d])]);
  for (const id of rec.roomRecs.keys()) if (!roomById.has(id)) rec.roomRecs.delete(id);
  // The meeting in session (else the most recent one); older ones are in the office. Listened-to rooms stay.
  const byRecent = (a, b) => AN.at(b.last_active) - AN.at(a.last_active);
  const latest = [...rooms].sort((a, b) => urgent(b.status) - urgent(a.status) || byRecent(a, b))[0];
  const shown = rooms.filter(r => r === latest || listening({ group: m.group, room: r.id }) || asked({ group: m.group, room: r.id })).sort(byRecent);
  reconcile(rec.rooms, shown.map((r) => {
    let row = rec.roomRecs.get(r.id);
    if (!row) rec.roomRecs.set(r.id, (row = createRoom()));
    updateRoom(row, m.group, r, seatDesks);
    return row.el;
  }));
}

// ---------------------------------------------------------------- folded office
function createQuiet() {
  // Only the title and who sat there; where and when stay in the tooltip.
  const rec = { boss: h("span.quiet-boss"), name: h("span.t"), faces: h("span.faces"), facesKey: "", bossKey: "" };
  rec.el = h("a.more-row", {}, rec.boss, rec.name, rec.faces);
  return rec;
}
function updateQuiet(rec, m) {
  const desks = m.desks || [];
  setAttr(rec.el, "href", href(`/chat/${m.group}`));
  const name = shownName(m);
  setAttr(rec.el, "title", `${name} · ${chatPlace(m)} · ${pick(GROUP, m.status)[0]} · ${relTime(m.updated)}`);
  setText(rec.name, name);
  const boss = bossChar(m);
  if (rec.bossKey !== boss) {
    rec.bossKey = boss;
    rec.boss.replaceChildren(ART.head(boss, "sleep"));
  }
  // One face per character, the most recently active first.
  const chars = [...new Set([...desks].sort((a, b) => AN.at(b.last_active) - AN.at(a.last_active)).map(deskChar))].slice(0, 8);
  const key = chars.join();
  if (rec.facesKey !== key) {
    rec.facesKey = key;
    rec.faces.replaceChildren(...chars.map((c) => ART.head(c, "sleep")));
  }
}

// Only status *changes* are spoken to assistive tech.
let spoken = [];
function announce(m, d, s) {
  const id = `${m.group}|${d.key}`;
  const was = state.seen.get(id);
  const busy = s.mood === "working";
  state.seen.set(id, busy);
  if (state.ready && was && !busy) spoken.push(`${chatName(m)}：${deskName(d)} ${pick(MEMBER, ["idle", "review"].includes(d.status) ? "succeeded" : d.status)[0]}`);
}

// ---------------------------------------------------------------- header, ask strip, body states
let lastState = "";
function renderState(data) {
  const { running } = data.counts;
  const meeting = (data.managers || []).some((m) => urgent(m.status));
  const pills = [running ? ["run", L(`${running} 个在工作`, `${running} working`)] : meeting ? ["run", L("会议进行中", "In meeting")] : ["idle", L("空闲", "Idle")]];
  const key = JSON.stringify(pills);
  if (key === lastState) return;
  lastState = key;
  $("state").replaceChildren(...pills.map(([tone, text]) => h(`span.state.tone-${tone}`, {}, h("i.dot"), text)));
}
function showBody(kind, retry) {
  const showStage = kind === "stage";
  stage.hidden = !showStage;
  const calm = $("calm");
  calm.hidden = showStage;
  if (showStage || state.calm === kind) return;
  state.calm = kind;
  const copy = {
    loading: [L("正在读取…", "Loading…"), "", null],
    no_data: [L("还没有办公室", "No offices yet"), L("在 boss 里派出任务或开始会议后，它的工位和会议室会自动出现在这里。", "Once you dispatch a task or start a meeting in boss, its desks and meeting rooms will appear here."), [L("打开 AgentNave", "Open AgentNave"), "/chat/"]],
    quiet: [L("没有办公室记录", "No office history"), L("历史已清理。在 boss 里派出任务或开始会议后，办公室会出现在这里。", "History was cleared. Dispatch a task or start a meeting in boss to see offices here."), [L("打开 AgentNave", "Open AgentNave"), "/chat/"]],
    error: [L("无法读取办公室数据", "Failed to load office data"), L("数据目录可能还不存在，或本机服务没有响应。", "The data folder may not exist yet, or the local service is not responding."), [L("重试", "Retry"), null]],
  }[kind];
  const action = copy[2] && (copy[2][1] ? h("a.calm-link", { href: href(copy[2][1]) }, copy[2][0]) : h("button.calm-link", { type: "button" }, copy[2][0]));
  if (action && !copy[2][1]) action.addEventListener("click", retry);
  calm.replaceChildren(...[h("strong", {}, copy[0]), copy[1] && h("p", {}, copy[1]), action].filter(Boolean));
}

// ---------------------------------------------------------------- update / service notices
function notice(title, detail, action, target, secondary) {
  const button = (text, to) => {
    const link = h("a.notice-action", { href: to }, text);
    if (to === "/app/copy-service-command")
      link.addEventListener("click", () => { link.textContent = L("已复制", "Copied"); setTimeout(() => (link.textContent = text), 2000); });
    return link;
  };
  return h("div.notice", {}, h("div.notice-text", {}, h("strong", {}, title), detail && h("small", {}, detail)), secondary && button(...secondary), action && button(action, target));
}
function notices(status, offline) {
  const out = [];
  // Only what needs the user: a found update or its progress is the header's update button.
  if (status?.update?.state === "failed") out.push(notice(L("更新失败", "Update failed"), status.update.error || L("检查网络后重试", "Check your network and try again"), L("重试", "Retry"), "/app/update"));
  if (status?.service?.state === "failed") out.push(notice(L("升级本机服务失败", "Service upgrade failed"), status.service.error, L("重试", "Retry"), "/app/upgrade-service", [L("复制命令", "Copy command"), "/app/copy-service-command"]));
  if (offline) out.push(notice(L("读取中断，正在重试", "Connection interrupted, retrying"), L("下方显示的是最近一次成功读取的内容", "Showing the last successfully loaded state below")));
  return out;
}
let lastNotices = "";
function renderNotices(status, offline) {
  const key = JSON.stringify([status?.update, status?.service, offline]);
  if (key === lastNotices) return;
  lastNotices = key;
  $("notices").replaceChildren(...notices(status, offline));
}

// ---------------------------------------------------------------- refresh
// Running work and active listening keep an office expanded.
const busy = m => running(m) > 0 || (m.rooms || []).some(r => urgent(r.status)) || (m.desks || []).some(lingering);
function render(data) {
  state.lastData = data;
  renderState(data);
  // The office that worked last stays a card until another one starts working and takes its place.
  const working = m => running(m) > 0 || (m.rooms || []).some(r => urgent(r.status));
  const recency = m => (working(m) ? 1e15 : 0) + (AN.at(m.updated) || 0);
  const lead = data.managers.reduce((top, m) => (!top || recency(m) > recency(top) ? m : top), null)?.group;
  const active = m => busy(m) || m.group === lead || voiceNow?.follow?.group === m.group || voiceAsked?.target.group === m.group;
  if (!data.managers.length) {
    showBody(data.empty === "no_data" ? "no_data" : "quiet");
    state.blocks.clear();
    state.quiet.clear();
    state.order = [];
    reconcile($("groups"), []);
    reconcile($("quiet"), []);
    return;
  }
  showBody("stage");
  state.order = stickyOrder(state.order, data.managers.map((m) => ({ id: m.group, rank: m.group === lead ? 0 : active(m) ? 1 : 2 })));
  const byGroup = new Map(data.managers.map((m) => [m.group, m]));
  for (const map of [state.blocks, state.quiet]) for (const id of map.keys()) if (!byGroup.has(id)) map.delete(id);
  const full = [], folded = [];
  for (const id of state.order) {
    const m = byGroup.get(id);
    if (active(m)) {
      let rec = state.blocks.get(id);
      if (!rec) {
        state.blocks.set(id, (rec = createBlock(m)));
        if (state.ready && busy(m)) spoken.push(L(`${chatName(m)} 的 boss 开始工作`, `boss in ${chatName(m)} started working`));
      }
      updateBlock(rec, m);
      full.push(rec.el);
    } else {
      state.blocks.delete(id);
      let rec = state.quiet.get(id);
      if (!rec) state.quiet.set(id, (rec = createQuiet()));
      updateQuiet(rec, m);
      for (const d of [...(m.desks || []), ...(m.optimized?.desks || [])]) announce(m, d, brief(d));
      folded.push(rec.el);
    }
  }
  reconcile($("groups"), full);
  reconcile($("quiet"), folded);
  $("sec-live").hidden = !full.length;
  $("sec-quiet").hidden = !folded.length;
  setText($("live-count"), L(`${full.length} 个`, `${full.length}`));
  setText($("quiet-count"), L(`${folded.length} 个`, `${folded.length}`));
}

async function refresh() {
  void AN.providerHealth?.refresh(document.getElementById("provider-health-alert"));
  const [stageResult, statusResult] = await Promise.allSettled([load("/app/stage"), load("/app/status")]);
  if (statusResult.status === "fulfilled") state.appStatus = statusResult.value;
  if (stageResult.status === "rejected") {
    const shown = state.blocks.size + state.quiet.size > 0;
    if (!shown) showBody("error", () => { state.calm = ""; run(); });
    renderNotices(state.appStatus, shown);
    return;
  }
  spoken = [];
  render(stageResult.value);
  state.ready = true;
  renderNotices(state.appStatus, false);
  if (spoken.length) $("announce").textContent = spoken.join("；");
}

showBody("loading");
const run = poll(refresh);
for (const link of document.querySelectorAll("a[href]")) setAttr(link, "href", href(link.getAttribute("href")));
