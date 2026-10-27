"use strict";
// Menu bar panel: one card per boss (the host CLI session), headed by the boss with its office below.
// Offices with work going on or a failure are cards; the rest fold into one line each at the end, and
// when nothing is going on the most recent office still shows as a card.
// Every node is created once and then patched in place (keyed by group id / desk key / room id).
// Links are real routes (/chat/<group>); the tray app intercepts them and opens the main window.
const {
  h, $, load, href, MEMBER, GROUP, pick, urgent, colorClass, relTime, fullTime,
  setText, setAttr, setClass, reconcile, stickyOrder, poll, art: ART, cliName, hostTile, deskName, chatName, chatPlace, activities, dozing, ongoing, clip,
} = AN;

const FOLD_AT = 8; // an office with this many desks or more folds its extra cells ...
const SHOWN = 6; //   ... down to this many

if (new URLSearchParams(location.search).has("glass")) document.body.classList.add("glass");
document.querySelector("header.top .top-end").append(AN.themeSwitch());

const ob = AN.onboarding({ variant: "full", onChange: () => syncOb() });
function syncOb() {
  const slot = $("ob-slot");
  const on = ob.visible && stage.hidden && state.calm === "no_data";
  slot.hidden = !on;
  $("calm").hidden = stage.hidden ? on : true; // the card replaces the generic hint
  if (on && ob.el.parentNode !== slot) slot.prepend(ob.el);
}

const stage = $("stage");
const state = {
  blocks: new Map(), // group id -> office block
  quiet: new Map(), // group id -> folded row
  order: [],
  expanded: new Set(), // offices whose folded desks are shown
  seen: new Map(), // `${group}|${desk key}` -> was running (for announcements)
  ready: false,
  appStatus: null,
  calm: "",
};

// ---------------------------------------------------------------- what a desk is doing
const deskChar = (d) => ART.character(d.provider, d.display_model || d.model);
// A reviewed answer waits only briefly on the boss, so it reads as an answer given, not a state of its own.
function brief(d) {
  const acts = activities(d);
  const run = acts.filter((a) => a.status === "running");
  if (run.some((a) => a.kind === "private")) return { mood: "working", word: run.length > 1 ? "私聊中 · 也在开会" : "私聊中" };
  if (run.length) return { mood: "working", word: "会议里作答中" };
  if (acts.some((a) => a.status === "review")) return { mood: "meeting", word: "会议里已作答" };
  if (acts.some((a) => a.kind === "meeting")) return { mood: "meeting", word: "在会议室" };
  if (d.status === "failed") return { mood: "failed", word: "上次失败" };
  if (d.status === "pending") return { mood: "idle", word: "等待中" };
  return dozing(d) ? { mood: "sleep", word: "休息中" } : { mood: "idle", word: "空闲" };
}
const HEAD = { working: "working", sleep: "sleep" };
const roomRound = (r) => {
  const n = r.round;
  if (!n) return pick(GROUP, ongoing(r.status))[0];
  if (urgent(r.status)) return `第 ${n.number} 轮 · ${n.order === "serial" ? "依次回复" : "同时回复"}`;
  return `第 ${n.number} 轮 · ${pick(GROUP, ongoing(r.status))[0]}`;
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
  setText(rec.cli, cliName(d.provider));
  setClass(rec.el, `cell ${s.mood}${d.private_allowed === false ? " only" : ""}`);
  setAttr(rec.el, "href", href(`/chat/${group}#desk:${encodeURIComponent(d.key)}`));
  setAttr(rec.el, "title", `${d.handle || `${deskName(d)} · ${cliName(d.provider)}`}\n${s.word}${d.access === "read" ? " · 声明只读" : d.access === "write" ? " · 声明可写" : ""}${d.private_allowed === false ? " · 仅开会" : ""}\n最近 ${fullTime(d.last_active)}`);
  setText(rec.name, deskName(d));
  setText(rec.role, d.role ? `${clip(d.role, 6)} · ` : "");
  setText(rec.word, s.word);
  return s;
}

// ---------------------------------------------------------------- meeting room row
function createRoom() {
  const rec = { title: h("b"), round: h("span"), seats: h("span.seats"), seatKey: "" };
  rec.el = h("a.room", {}, h("span.wb", {}, BOARD()), h("span.rt", {}, rec.title, rec.round), rec.seats);
  return rec;
}
function updateRoom(rec, group, r, desks) {
  const [word, tone] = pick(GROUP, ongoing(r.status));
  setClass(rec.el, `room tone-${tone}`);
  setAttr(rec.el, "href", href(`/chat/${group}#conv:${encodeURIComponent(r.id)}`));
  setAttr(rec.el, "title", `会议室 · ${r.title}\n${word} · 会议记录 ${r.board ?? 0} 条`);
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
    id: m.group, data: m, tile: h("span.tile"), presence: h("i.presence"), project: h("b"), meta: h("small"), bs: h("span.bs"),
    time: h("time"), cells: h("div.desks"), rooms: h("div.rooms"),
    foldText: h("span"), cellRecs: new Map(), roomRecs: new Map(), tileKey: "",
  };
  rec.boss = h("a.boss", {}, h("span.host", {}, rec.tile, rec.presence), h("span.who", {}, rec.project, rec.meta), h("span.when", {}, rec.bs, rec.time));
  rec.fold = h("button.fold", { type: "button" }, h("i.chev"), rec.foldText);
  rec.el = h("section.blk", {}, rec.boss, rec.cells, rec.fold, rec.rooms);
  rec.fold.addEventListener("click", () => {
    if (state.expanded.has(rec.id)) state.expanded.delete(rec.id);
    else state.expanded.add(rec.id);
    updateBlock(rec, rec.data);
  });
  return rec;
}
function tile(rec, m) {
  const key = `${m.host}|${m.label}`;
  if (rec.tileKey === key) return;
  rec.tileKey = key;
  rec.tile.replaceChildren(hostTile(m.host, m.label));
}
function running(m) {
  let n = 0;
  for (const d of m.desks || []) n += activities(d).filter((a) => a.status === "running").length;
  return n;
}
function updateBlock(rec, m) {
  rec.data = m;
  const desks = m.desks || [], rooms = m.rooms || [];
  const n = running(m);
  const [word] = pick(GROUP, ongoing(m.status));
  const name = chatName(m);
  setClass(rec.el, `blk ${colorClass(m.color)}${n ? " run" : ""}`);
  tile(rec, m);
  setClass(rec.presence, m.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", m.live ? "有任务在运行" : "没有运行中的任务");
  setAttr(rec.boss, "href", href(`/chat/${m.group}`));
  setAttr(rec.boss, "title", `${m.label} 里的 boss${m.project ? ` · ${m.project}` : ""}`);
  setAttr(rec.boss, "aria-label", `${name}，boss 在 ${m.label}，${desks.length} 个工位，${rooms.length} 间会议室，打开办公室`);
  setText(rec.project, name);
  const [bsTone, bsText] = n ? ["run", `${n} 个在工作`] : urgent(m.status) ? ["run", "会议进行中"] : m.status === "failed" ? ["bad", word] : ["", "空闲"];
  setClass(rec.bs, `bs${bsTone ? ` ${bsTone}` : ""}`);
  setText(rec.bs, bsText);
  setText(rec.time, relTime(m.updated));
  setAttr(rec.time, "title", fullTime(m.updated));

  // busy desks first, then idle ones; resting desks are only counted. Folded beyond SHOWN when
  // there are FOLD_AT or more, and the busy ones lead so the fold never hides what is going on.
  const byKey = new Map(desks.map((d) => [d.key, d]));
  const awake = desks.filter((d) => brief(d).mood !== "sleep");
  const busy = (d) => ["working", "meeting", "failed"].includes(brief(d).mood);
  const ordered = [...awake.filter(busy), ...awake.filter((d) => !busy(d))];
  for (const key of rec.cellRecs.keys()) if (!ordered.some((d) => d.key === key)) rec.cellRecs.delete(key);
  const folded = awake.length >= FOLD_AT && !state.expanded.has(m.group);
  let hiddenCount = 0, hiddenBusy = 0;
  reconcile(rec.cells, ordered.map((d, i) => {
    let cell = rec.cellRecs.get(d.key);
    if (!cell) rec.cellRecs.set(d.key, (cell = createCell()));
    const s = updateCell(cell, m.group, d);
    cell.el.hidden = folded && i >= SHOWN;
    if (cell.el.hidden) {
      hiddenCount++;
      if (s.mood === "working") hiddenBusy++;
    }
    announce(m, d, s);
    return cell.el;
  }));
  // a desk that falls asleep right after its run still gets its finish read out
  for (const d of desks) if (!awake.includes(d)) announce(m, d, brief(d));
  const optimized = m.optimized?.count || 0;
  const resting = desks.length - awake.length;
  const meta = [name === m.label ? "" : chatPlace(m), `${desks.length} 工位`, resting && `${resting} 个休息中`, optimized && `已优化 ${optimized}`, rooms.length && `${rooms.length} 会议室`].filter(Boolean).join(" · ");
  setText(rec.meta, meta);
  rec.cells.hidden = !ordered.length;
  rec.fold.hidden = awake.length < FOLD_AT;
  if (!rec.fold.hidden) {
    setText(rec.foldText, folded ? `另外 ${hiddenCount} 个工位${hiddenBusy ? ` · ${hiddenBusy} 个在忙` : ""}` : "收起");
    setAttr(rec.fold, "aria-expanded", folded ? "false" : "true");
    setClass(rec.fold, folded && hiddenBusy ? "fold busy" : "fold");
  }
  const roomById = new Map(rooms.map((r) => [r.id, r]));
  // a seat's desk may be folded into "已优化"; the payload still lists it for the seat
  const seatDesks = new Map([...byKey, ...(m.optimized?.desks || []).map((d) => [d.key, d])]);
  for (const id of rec.roomRecs.keys()) if (!roomById.has(id)) rec.roomRecs.delete(id);
  // rooms in session, plus the most recently finished one; older ones are in the office
  const ended = rooms.filter((r) => !urgent(r.status)).sort((a, b) => AN.at(b.last_active) - AN.at(a.last_active));
  const shown = [...rooms.filter((r) => urgent(r.status)), ...ended.slice(0, 1)];
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
  const rec = { name: h("span.t"), faces: h("span.faces"), facesKey: "" };
  rec.el = h("a.more-row", {}, rec.name, rec.faces);
  return rec;
}
function updateQuiet(rec, m) {
  const desks = m.desks || [];
  setAttr(rec.el, "href", href(`/chat/${m.group}`));
  const name = chatName(m);
  setAttr(rec.el, "title", `${name} · ${chatPlace(m)} · ${pick(GROUP, m.status)[0]} · ${relTime(m.updated)}`);
  setText(rec.name, name);
  const key = desks.slice(0, 5).map(deskChar).join();
  if (rec.facesKey !== key) {
    rec.facesKey = key;
    rec.faces.replaceChildren(...desks.slice(0, 5).map((d) => ART.head(deskChar(d), "sleep")));
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
  const failed = data.managers.some((m) => m.status === "failed");
  const pills = [running ? ["run", `${running} 个在工作`] : failed ? ["bad", "有失败"] : ["idle", "空闲"]];
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
  if (showStage || state.calm === kind) { syncOb(); return; }
  state.calm = kind;
  const copy = {
    loading: ["正在读取…", "", null],
    no_data: ["还没有办公室", "在 boss 里派出任务或开始会议后，它的工位和会议室会自动出现在这里。", ["打开 AgentNave", "/chat/"]],
    quiet: ["没有办公室记录", "历史已清理。在 boss 里派出任务或开始会议后，办公室会出现在这里。", ["打开 AgentNave", "/chat/"]],
    error: ["无法读取办公室数据", "数据目录可能还不存在，或本机服务没有响应。", ["重试", null]],
  }[kind];
  const action = copy[2] && (copy[2][1] ? h("a.calm-link", { href: href(copy[2][1]) }, copy[2][0]) : h("button.calm-link", { type: "button" }, copy[2][0]));
  if (action && !copy[2][1]) action.addEventListener("click", retry);
  calm.replaceChildren(...[h("strong", {}, copy[0]), copy[1] && h("p", {}, copy[1]), action].filter(Boolean));
  syncOb();
}

// ---------------------------------------------------------------- update / service notices
function notice(title, detail, action, target, secondary) {
  const button = (text, to) => {
    const link = h("a.notice-action", { href: to }, text);
    if (to === "/app/copy-service-command")
      link.addEventListener("click", () => { link.textContent = "已复制"; setTimeout(() => (link.textContent = text), 2000); });
    return link;
  };
  return h("div.notice", {}, h("div.notice-text", {}, h("strong", {}, title), detail && h("small", {}, detail)), secondary && button(...secondary), action && button(action, target));
}
function notices(status, offline) {
  const out = [];
  // Only what needs the user: a found update or its progress is the header's update button.
  if (status?.update?.state === "failed") out.push(notice("更新失败", "检查网络后重试", "重试", "/app/update"));
  if (status?.service?.state === "failed") out.push(notice("升级本机服务失败", status.service.error, "重试", "/app/upgrade-service", ["复制命令", "/app/copy-service-command"]));
  if (offline) out.push(notice("读取中断，正在重试", "下方显示的是最近一次成功读取的内容"));
  return out;
}
let lastNotices = "";
function renderNotices(status, offline) {
  const key = JSON.stringify([status?.update, status?.service, offline]);
  if (key === lastNotices) return;
  lastNotices = key;
  $("notices").replaceChildren(...notices(status, offline));
  const update = status?.update;
  const link = $("update");
  link.replaceChildren(update?.state === "installing" ? "正在更新…" : "检查更新", update?.state === "available" ? h("small", {}, `v${update.version} 可用`) : "");
  $("more").querySelector(".more-dot").hidden = update?.state !== "available";
}

// ---------------------------------------------------------------- ⋯ menu
const menu = $("menu"), more = $("more");
function setMenu(open) {
  menu.hidden = !open;
  setAttr(more, "aria-expanded", open ? "true" : "false");
  if (open) menu.querySelector("a")?.focus({ preventScroll: true });
}
more.addEventListener("click", (e) => { e.stopPropagation(); setMenu(menu.hidden); });
document.addEventListener("click", (e) => { if (!menu.hidden && !menu.contains(e.target)) setMenu(false); });
document.addEventListener("keydown", (e) => {
  if (menu.hidden) return;
  if (e.key === "Escape") { setMenu(false); more.focus(); }
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const items = [...menu.querySelectorAll("a")];
    const i = items.indexOf(document.activeElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length].focus();
  }
});

// ---------------------------------------------------------------- refresh
// A boss with work going on or a failure gets a card; the rest fold to the end. With none of those,
// the most recent office (the service lists them most relevant first) stays a card.
const busy = (m) => urgent(m.status) || m.live || m.status === "failed";
function render(data) {
  renderState(data);
  const lead = data.managers.some(busy) ? null : data.managers[0]?.group;
  const active = (m) => busy(m) || m.group === lead;
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
  state.order = stickyOrder(state.order, data.managers.map((m) => ({ id: m.group, rank: urgent(m.status) ? 0 : active(m) ? 1 : 2 })));
  const byGroup = new Map(data.managers.map((m) => [m.group, m]));
  for (const map of [state.blocks, state.quiet]) for (const id of map.keys()) if (!byGroup.has(id)) map.delete(id);
  const full = [], folded = [];
  for (const id of state.order) {
    const m = byGroup.get(id);
    if (active(m)) {
      let rec = state.blocks.get(id);
      if (!rec) {
        state.blocks.set(id, (rec = createBlock(m)));
        if (state.ready && busy(m)) spoken.push(`${chatName(m)} 的 boss 开始工作`);
      }
      updateBlock(rec, m);
      full.push(rec.el);
    } else {
      state.blocks.delete(id);
      let rec = state.quiet.get(id);
      if (!rec) state.quiet.set(id, (rec = createQuiet()));
      updateQuiet(rec, m);
      folded.push(rec.el);
    }
  }
  reconcile($("groups"), full);
  reconcile($("quiet"), folded);
  $("sec-live").hidden = !full.length;
  $("sec-quiet").hidden = !folded.length;
  setText($("live-count"), `${full.length} 个`);
  setText($("quiet-count"), `${folded.length} 个`);
}

async function refresh() {
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

ob.start();
