"use strict";
// Menu bar panel: one row per boss (the host CLI session) wired to a thumbnail of its office. Things
// waiting on the user come first; offices with nothing going on fold into one line each at the end.
// Every node is created once and then patched in place (keyed by group id / desk key / room id).
// Links are real routes (/chat/<group>); the tray app intercepts them and opens the main window.
const {
  h, $, load, href, MEMBER, GROUP, pick, urgent, colorClass, relTime, fullTime,
  setText, setAttr, setClass, reconcile, stickyOrder, poll, art: ART, cliName, hostTile, deskName, activities, clip,
} = AN;

const FOLD_AT = 8; // an office with this many desks or more folds its extra cells ...
const SHOWN = 6; //   ... down to this many
const SLEEP_AFTER = 20 * 60e3; // an idle desk dozes off after this long

if (new URLSearchParams(location.search).has("glass")) document.body.classList.add("glass");
document.querySelector("header.top").append(AN.themeSwitch());

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
function brief(d) {
  const acts = activities(d);
  const run = acts.filter((a) => a.status === "running");
  if (run.some((a) => a.kind === "private")) return { mood: "working", word: run.length > 1 ? "私聊中 · 也在开会" : "私聊中" };
  if (run.length) return { mood: "working", word: "会议里作答中" };
  if (acts.some((a) => a.status === "review")) return { mood: "review", word: "交卷 · 待审核" };
  if (acts.some((a) => a.kind === "meeting")) return { mood: "meeting", word: "在会议室" };
  if (d.status === "failed") return { mood: "failed", word: "上次失败" };
  const idle = Date.now() - AN.at(d.last_active);
  return idle > SLEEP_AFTER ? { mood: "sleep", word: `空闲 · ${relTime(d.last_active)}` } : { mood: "idle", word: "空闲" };
}
const HEAD = { working: "working", review: "review", sleep: "sleep" };
const roomRound = (r) => {
  if (r.pending_review) return "有回答待审核";
  const n = r.round;
  if (!n) return pick(GROUP, r.status)[0];
  if (r.status === "running") return `第 ${n.number} 轮 · ${n.order === "serial" ? "依次发言" : "同时作答"}`;
  return `第 ${n.number} 轮 · ${pick(GROUP, r.status)[0]}`;
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
  const rec = { face: h("span.face"), cli: h("span.cli"), name: h("b"), role: h("em"), word: h("span.w"), faceKey: "", cliKey: "" };
  rec.el = h("a.cell", {}, h("span.face-wrap", {}, rec.face, rec.cli), h("span.nm", {}, h("span.nm-top", {}, rec.name, rec.role), rec.word));
  return rec;
}
function updateCell(rec, group, d) {
  const s = brief(d);
  const faceKey = `${deskChar(d)}|${HEAD[s.mood] || "idle"}`;
  if (rec.faceKey !== faceKey) {
    rec.faceKey = faceKey;
    rec.face.replaceChildren(ART.head(deskChar(d), HEAD[s.mood] || "idle"));
  }
  if (rec.cliKey !== d.provider) {
    rec.cliKey = d.provider;
    rec.cli.replaceChildren(hostTile(d.provider));
  }
  setClass(rec.el, `cell ${s.mood}${d.private_allowed === false ? " only" : ""}`);
  setAttr(rec.el, "href", href(`/chat/${group}#desk:${encodeURIComponent(d.key)}`));
  setAttr(rec.el, "title", `${d.handle || `${deskName(d)} · ${cliName(d.provider)}`}\n${s.word}${d.access === "read" ? " · 声明只读" : d.access === "write" ? " · 声明可写" : ""}${d.private_allowed === false ? " · 仅开会" : ""}\n最近 ${fullTime(d.last_active)}`);
  setText(rec.name, deskName(d));
  setText(rec.role, d.role ? ` · ${clip(d.role, 6)}` : "");
  setText(rec.word, s.word);
  return s;
}

// ---------------------------------------------------------------- meeting room row
function createRoom() {
  const rec = { title: h("b"), round: h("span"), seats: h("span.seats"), review: h("span.tag.tag-review"), seatKey: "" };
  rec.el = h("a.room", {}, h("span.wb", {}, BOARD()), h("span.rt", {}, rec.title, rec.round), rec.seats, rec.review);
  return rec;
}
function updateRoom(rec, group, r, desks) {
  const [word, tone] = pick(GROUP, r.status);
  setClass(rec.el, `room tone-${tone}`);
  setAttr(rec.el, "href", href(`/chat/${group}#conv:${encodeURIComponent(r.id)}`));
  setAttr(rec.el, "title", `会议室 · ${r.title}\n${word} · 会议记录 ${r.board ?? 0} 条${r.pending_review ? ` · ${r.pending_review} 条待审核` : ""}`);
  setText(rec.title, r.title);
  setText(rec.round, roomRound(r));
  rec.review.hidden = !r.pending_review;
  setText(rec.review, `${r.pending_review} 待审`);
  const seated = r.seats.map((s) => desks.get(s.desk)).filter(Boolean);
  const key = seated.map(deskChar).join();
  if (rec.seatKey !== key) {
    rec.seatKey = key;
    rec.seats.replaceChildren(...seated.slice(0, 5).map((d) => ART.head(deskChar(d))));
  }
}

// ---------------------------------------------------------------- one boss and its office
function createBlock(m) {
  const rec = {
    id: m.group, data: m, tile: h("span.tile"), presence: h("i.presence"), project: h("b"), host: h("small"), bs: h("span.bs"),
    count: h("span.count"), time: h("time"), cells: h("div.desks"), rooms: h("div.rooms"),
    foldText: h("span"), cellRecs: new Map(), roomRecs: new Map(), tileKey: "",
  };
  rec.boss = h("a.boss", {}, h("span.host", {}, rec.tile, rec.presence), rec.project, rec.host, rec.bs);
  rec.head = h("a.o-head", {}, h("b", {}, "办公室"), rec.count, rec.time);
  rec.fold = h("button.fold", { type: "button" }, h("i.chev"), rec.foldText);
  rec.el = h("section.blk", {}, rec.boss, h("div.office", {}, rec.head, rec.cells, rec.fold, rec.rooms));
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
function tally(m) {
  let running = 0;
  for (const d of m.desks || []) running += activities(d).filter((a) => a.status === "running").length;
  const review = (m.rooms || []).reduce((n, r) => n + (r.pending_review || 0), 0);
  return { running, review };
}
function updateBlock(rec, m) {
  rec.data = m;
  const desks = m.desks || [], rooms = m.rooms || [];
  const n = tally(m);
  const [word] = pick(GROUP, m.status);
  const name = m.project || m.label;
  setClass(rec.el, `blk ${colorClass(m.color)}${n.running ? " run" : ""}`);
  tile(rec, m);
  setClass(rec.presence, m.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", m.live ? "有任务在运行" : "没有运行中的任务");
  for (const a of [rec.boss, rec.head]) setAttr(a, "href", href(`/chat/${m.group}`));
  setAttr(rec.boss, "title", `${m.label} 里的 boss${m.project ? ` · ${m.project}` : ""}`);
  setAttr(rec.boss, "aria-label", `${name}，boss 在 ${m.label}，${desks.length} 个工位，${rooms.length} 间会议室，打开办公室`);
  setText(rec.project, name);
  setText(rec.host, m.project ? m.label : "");
  const [bsTone, bsText] = n.review ? ["review", `${n.review} 份待审核`] : n.running ? ["run", `${n.running} 个在工作`] : m.status === "failed" ? ["bad", word] : ["", "空闲"];
  setClass(rec.bs, `bs${bsTone ? ` ${bsTone}` : ""}`);
  setText(rec.bs, bsText);
  const optimized = m.optimized?.count || 0;
  setText(rec.count, `· ${desks.length} 工位${optimized ? ` · 已优化 ${optimized}` : ""}${rooms.length ? ` · ${rooms.length} 会议室` : ""}`);
  setText(rec.time, relTime(m.updated));
  setAttr(rec.time, "title", fullTime(m.updated));

  // desks in the office's own (first seen) order; folded beyond SHOWN when there are FOLD_AT or more,
  // with busy desks moved up so the fold never hides what is going on
  const byKey = new Map(desks.map((d) => [d.key, d]));
  const busy = (d) => ["working", "review", "meeting"].includes(brief(d).mood);
  const ordered = desks.length >= FOLD_AT ? [...desks.filter(busy), ...desks.filter((d) => !busy(d))] : desks;
  for (const key of rec.cellRecs.keys()) if (!byKey.has(key)) rec.cellRecs.delete(key);
  const folded = desks.length >= FOLD_AT && !state.expanded.has(m.group);
  let hiddenCount = 0, hiddenBusy = 0;
  reconcile(rec.cells, ordered.map((d, i) => {
    let cell = rec.cellRecs.get(d.key);
    if (!cell) rec.cellRecs.set(d.key, (cell = createCell()));
    const s = updateCell(cell, m.group, d);
    cell.el.hidden = folded && i >= SHOWN;
    if (cell.el.hidden) {
      hiddenCount++;
      if (s.mood === "working" || s.mood === "review") hiddenBusy++;
    }
    announce(m, d, s);
    return cell.el;
  }));
  rec.fold.hidden = desks.length < FOLD_AT;
  if (!rec.fold.hidden) {
    setText(rec.foldText, folded ? `另外 ${hiddenCount} 个工位${hiddenBusy ? ` · ${hiddenBusy} 个在忙` : ""}` : "收起");
    setAttr(rec.fold, "aria-expanded", folded ? "false" : "true");
    setClass(rec.fold, folded && hiddenBusy ? "fold busy" : "fold");
  }
  const roomById = new Map(rooms.map((r) => [r.id, r]));
  // a seat's desk may be folded into "已优化"; the payload still lists it for the seat
  const seatDesks = new Map([...byKey, ...(m.optimized?.desks || []).map((d) => [d.key, d])]);
  for (const id of rec.roomRecs.keys()) if (!roomById.has(id)) rec.roomRecs.delete(id);
  // rooms in session first; at most three finished ones stay listed
  const shown = [...rooms.filter((r) => urgent(r.status)), ...rooms.filter((r) => !urgent(r.status)).slice(0, Math.max(0, 3 - rooms.filter((r) => urgent(r.status)).length))];
  reconcile(rec.rooms, shown.map((r) => {
    let row = rec.roomRecs.get(r.id);
    if (!row) rec.roomRecs.set(r.id, (row = createRoom()));
    updateRoom(row, m.group, r, seatDesks);
    return row.el;
  }));
}

// ---------------------------------------------------------------- folded office
function createQuiet() {
  const rec = { tile: h("span.tile"), name: h("span.t"), faces: h("span.faces"), time: h("time"), tileKey: "", facesKey: "" };
  rec.el = h("a.more-row", {}, rec.tile, rec.name, rec.faces, rec.time);
  return rec;
}
function updateQuiet(rec, m) {
  const desks = m.desks || [];
  tile(rec, m);
  setAttr(rec.el, "href", href(`/chat/${m.group}`));
  setAttr(rec.el, "title", `${m.project || m.label} · ${m.label} · ${pick(GROUP, m.status)[0]}`);
  const name = m.project || m.label;
  const small = `${m.project ? `${m.label} · ` : ""}${desks.length} 工位${m.rooms?.length ? ` · ${m.rooms.length} 会议室` : ""}`;
  if (rec.name.dataset.key !== name + small) {
    rec.name.dataset.key = name + small;
    rec.name.replaceChildren(name, h("small", {}, small));
  }
  const key = desks.slice(0, 5).map(deskChar).join();
  if (rec.facesKey !== key) {
    rec.facesKey = key;
    rec.faces.replaceChildren(...desks.slice(0, 5).map((d) => ART.head(deskChar(d), "sleep")));
  }
  setText(rec.time, relTime(m.updated));
}

// Only status *changes* are spoken to assistive tech.
let spoken = [];
function announce(m, d, s) {
  const id = `${m.group}|${d.key}`;
  const was = state.seen.get(id);
  const busy = s.mood === "working";
  state.seen.set(id, busy);
  if (state.ready && was && !busy) spoken.push(`${m.project || m.label}：${deskName(d)} ${s.mood === "review" ? "交了回答，等待审核" : pick(MEMBER, d.status === "idle" ? "succeeded" : d.status)[0]}`);
}

// ---------------------------------------------------------------- header, ask strip, body states
let lastState = "";
function renderState(data) {
  const { running, review } = data.counts;
  const failed = data.managers.some((m) => m.status === "failed");
  const pills = [];
  if (running) pills.push(["run", `${running} 个在工作`]);
  if (review) pills.push(["review", `${review} 待审核`]);
  if (!pills.length) pills.push(failed ? ["bad", "有失败"] : ["idle", "空闲"]);
  const key = JSON.stringify(pills);
  if (key === lastState) return;
  lastState = key;
  $("state").replaceChildren(...pills.map(([tone, text]) => h(`span.state.tone-${tone}`, {}, h("i.dot"), text)));
}
// Answers waiting for review are the one thing that waits on the user: they head the panel.
function renderAsk(managers) {
  const ask = $("ask");
  const waiting = managers.flatMap((m) => (m.rooms || []).filter((r) => r.pending_review).map((r) => ({ m, r })));
  const total = waiting.reduce((n, { r }) => n + r.pending_review, 0);
  ask.hidden = !total;
  if (!total) return;
  const { m, r } = waiting[0];
  const where = `${m.project || m.label} · ${r.title}${waiting.length > 1 ? ` 等 ${waiting.length} 处` : ""}`;
  const key = `${total}|${where}|${m.group}|${r.id}`;
  if (ask.dataset.key === key) return;
  ask.dataset.key = key;
  setAttr(ask, "href", href(`/chat/${m.group}#conv:${encodeURIComponent(r.id)}`));
  ask.replaceChildren(
    h("span.ask-ico", { "aria-hidden": "true" }, String(total)),
    h("span.t", {}, `${total} 份回答等 boss 审核`, h("small", {}, `${where} · 在 boss 里公开或丢弃`)),
    h("span.go", {}, "查看 →"),
  );
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
    quiet: ["最近两小时没有动静", "以前的办公室仍可在 AgentNave 里查看。", ["打开 AgentNave", "/chat/"]],
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
  const update = status?.update;
  if (update?.state === "available") out.push(notice(`AgentNave v${update.version} 可用`, `当前 v${status.version}，更新后自动重启`, "更新并重启", "/app/update"));
  else if (update?.state === "installing") out.push(notice("正在下载更新", "完成后自动重启"));
  else if (update?.state === "failed") out.push(notice("更新失败", "检查网络后重试", "重试", "/app/update"));
  const service = status?.service;
  if (service?.state === "upgrading") out.push(notice("正在升级本机服务", `从 GitHub 安装 v${status.version}，约需一分钟`));
  else if (service?.state === "done") out.push(notice(`本机服务已升级到 v${service.version}`, "版本变化本身无需重启宿主；需要重新加载时设置页会单独提示", "查看连接状态", "/settings/#hosts"));
  else if (service?.state === "failed") out.push(notice("升级本机服务失败", service.error, "重试", "/app/upgrade-service", ["复制命令", "/app/copy-service-command"]));
  else if (service) out.push(notice(`本机服务 v${service.version} 比托盘 v${status.version} 旧`, "升级后在空闲时自动切换；若接口未同步，再提示对应宿主重连", "一键升级", "/app/upgrade-service"));
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
// A boss with work going on or recent activity gets a full row; the rest fold to the end.
const RECENT = 30 * 60e3;
const active = (m) => urgent(m.status) || m.live || Date.now() - AN.at(m.updated) < RECENT;
function render(data) {
  renderState(data);
  renderAsk(data.managers);
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
        if (state.ready) spoken.push(`${m.project || m.label} 的 boss 开始工作`);
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
