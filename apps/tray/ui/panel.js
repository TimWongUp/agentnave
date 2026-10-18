"use strict";
// Menu bar panel, "stage": managers (host CLI sessions) on the left, AgentNave in the middle,
// a thumbnail of each manager's office on the right (desk cells and meeting-room rows), wired in
// the manager's colour. Every node is created once and then patched in place (keyed by group id /
// desk key / room id); wires are an SVG overlay recomputed from layout after each render, resize and
// fold. Links are real routes (/chat/<group>); the tray app intercepts them and opens the main window.
const {
  h, $, load, href, MEMBER, GROUP, pick, urgent, deskState, avatar, colorClass,
  relTime, fullTime, setText, setAttr, setClass, reconcile, stickyOrder, poll,
} = AN;

const FOLD_AT = 8; // an office with this many desks or more folds its extra cells ...
const SHOWN = 6; //   ... down to this many
const SVG = "http://www.w3.org/2000/svg";

if (new URLSearchParams(location.search).has("glass")) document.body.classList.add("glass");

const ob = AN.onboarding({ variant: "full", onChange: () => syncOb() });
function syncOb() {
  const slot = $("ob-slot");
  const on = ob.visible && stage.hidden && state.calm === "no_data";
  slot.hidden = !on;
  $("calm").hidden = stage.hidden ? on : true; // the card replaces the ghost + generic hint
  if (on && ob.el.parentNode !== slot) slot.prepend(ob.el);
}

const stage = $("stage");
const groupsEl = $("groups");
const state = {
  blocks: new Map(), // group id -> block record
  order: [], // sticky manager order
  expanded: new Set(), // groups whose folded members are shown
  seen: new Map(), // `${group}|${desk key}` -> last desk status (for announcements)
  ready: false, // first render done: announcements start after it
  failures: 0,
  appStatus: null,
  calm: "", // which body state is drawn: "", loading, empty, error
};

// ---------------------------------------------------------------- blocks
function createCell() {
  const dot = h("i.dot"), name = h("span.cell-name"), room = h("i.cell-room", { "aria-hidden": "true" }, "会");
  const ava = h("span.avatar");
  const wrap = h("span.cell-ava", {}, ava, dot);
  const el = h("a.cell", {}, wrap, name, room);
  return { el, wrap, ava, name, room, avatarKey: "" };
}

function updateCell(rec, group, d, room) {
  const state = deskState(d, room);
  if (rec.avatarKey !== d.provider + d.model) {
    rec.avatarKey = d.provider + d.model;
    const next = avatar(d.provider, d.model);
    rec.ava.replaceWith(next);
    rec.ava = next;
  }
  setClass(rec.el, `cell tone-${state.tone}${state.working ? " working" : ""}${room ? " away" : ""}${d.private_allowed ? "" : " only"}`);
  setAttr(rec.el, "href", href(`/chat/${group}#desk:${encodeURIComponent(d.key)}`));
  setAttr(rec.el, "title", `${d.label}\n${state.word}${d.private_allowed ? "" : " · 仅会议"}\n${fullTime(d.last_active)}`);
  setText(rec.name, d.role || d.model || d.provider);
  rec.room.hidden = !room;
}

function createRoomRow() {
  const title = h("span.rr-title"), seats = h("span.rr-seats"), board = h("span.rr-board"), review = h("span.tag.tag-review");
  const el = h("a.rrow", {}, h("i.rr-glyph", { "aria-hidden": "true" }, "会"), title, seats, board, review);
  return { el, title, seats, board, review, seatKey: "" };
}

function updateRoomRow(rec, group, r, seated) {
  const [word, tone] = pick(GROUP, r.status);
  setClass(rec.el, `rrow tone-${tone}`);
  setAttr(rec.el, "href", href(`/chat/${group}#conv:${encodeURIComponent(r.id)}`));
  setAttr(rec.el, "title", `${r.title}\n${word} · 白板 ${r.board} 条${r.pending_review ? ` · 待审 ${r.pending_review}` : ""}`);
  setText(rec.title, r.title);
  setText(rec.board, `白板 ${r.board}`);
  rec.review.hidden = !r.pending_review;
  setText(rec.review, `待审 ${r.pending_review}`);
  const key = seated.map((d) => d.provider + d.model).join();
  if (rec.seatKey !== key) {
    rec.seatKey = key;
    rec.seats.replaceChildren(...seated.slice(0, 4).map((d) => avatar(d.provider, d.model)));
  }
}

function createBlock(m) {
  const mgrState = h("span.mgr-state", {}, h("i.dot"), h("span"));
  const presence = h("i.presence");
  const mgrName = h("span.mgr-name");
  const mgr = h("a.mgr", {}, h("span.mgr-top", {}, h("span.mgr-ava", {}, presence), mgrName), mgrState);
  const headCount = h("span.count"), headRun = h("span.tag.tag-run"), headReview = h("span.tag.tag-review"), headTime = h("time");
  const head = h("a.grp-head", {}, headCount, headRun, headReview, headTime);
  const cells = h("div.cells");
  const rooms = h("div.rrows");
  const foldText = h("span");
  const fold = h("button.fold", { type: "button" }, h("i.chev"), foldText);
  const grp = h("div.grp", {}, head, cells, fold, rooms);
  const root = h("section.block.enter", {}, mgr, h("span.gap", { "aria-hidden": "true" }), grp);
  const rec = { id: m.group, root, mgr, mgrName, mgrState, presence, grp, head, headCount, headRun, headReview, headTime, cells, rooms, fold, foldText, cellRecs: new Map(), roomRecs: new Map(), order: [], data: m };
  fold.addEventListener("click", () => {
    if (state.expanded.has(m.group)) state.expanded.delete(m.group);
    else state.expanded.add(m.group);
    updateBlock(rec, rec.data);
    scheduleWires();
  });
  root.addEventListener("animationend", () => root.classList.remove("enter"), { once: true });
  return rec;
}

function updateBlock(rec, m) {
  rec.data = m;
  const [word, tone] = pick(GROUP, m.status);
  const desks = m.desks || [];
  const rooms = m.rooms || [];
  const running = desks.filter((d) => d.status === "running").length;
  const review = rooms.reduce((n, r) => n + (r.pending_review || 0), 0);
  setClass(rec.root, `block ${colorClass(m.color)}${rec.root.classList.contains("enter") ? " enter" : ""}`);
  setClass(rec.mgr, `mgr tone-${tone}${m.live ? "" : " off"}`);
  setAttr(rec.mgr, "href", href(`/chat/${m.group}`));
  setAttr(rec.mgr, "aria-label", `${m.label}，${word}${m.live ? "" : "，离线"}，${desks.length} 个工位，${rooms.length} 间会议室，打开办公室`);
  setText(rec.mgrName, m.label);
  setText(rec.mgrState.lastChild, m.live ? word : `${word} · 离线`);
  setClass(rec.presence, m.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", m.live ? "主控在线" : "主控已离线，仅有历史记录");
  setAttr(rec.head, "href", href(`/chat/${m.group}`));
  setText(rec.headCount, `${desks.length} 工位 · ${rooms.length} 会议室`);
  setText(rec.headRun, running ? `${running} 进行中` : "");
  setText(rec.headReview, review ? `${review} 待审` : "");
  setText(rec.headTime, relTime(m.updated));

  // desk cells in the office's own (first seen) order; folded beyond SHOWN when there are FOLD_AT or more
  const roomById = new Map(rooms.map((r) => [r.id, r]));
  const byKey = new Map(desks.map((d) => [d.key, d]));
  for (const key of rec.cellRecs.keys()) if (!byKey.has(key)) rec.cellRecs.delete(key);
  const folded = desks.length >= FOLD_AT && !state.expanded.has(m.group);
  let hiddenCount = 0, hiddenRunning = 0;
  const cellNodes = desks.map((d, i) => {
    let cell = rec.cellRecs.get(d.key);
    if (!cell) rec.cellRecs.set(d.key, (cell = createCell()));
    updateCell(cell, m.group, d, d.meeting.room ? roomById.get(d.meeting.room) : null);
    cell.el.hidden = folded && i >= SHOWN;
    if (cell.el.hidden) {
      hiddenCount++;
      if (d.status === "running") hiddenRunning++;
    }
    announce(m, d);
    return cell.el;
  });
  reconcile(rec.cells, cellNodes);

  const foldable = desks.length >= FOLD_AT;
  rec.fold.hidden = !foldable;
  if (foldable) {
    setText(rec.foldText, folded ? `另外 ${hiddenCount} 个工位${hiddenRunning ? ` · ${hiddenRunning} 进行中` : ""}` : "收起");
    setAttr(rec.fold, "aria-expanded", folded ? "false" : "true");
    setClass(rec.fold, folded && hiddenRunning ? "fold has-run" : "fold");
  }

  for (const id of rec.roomRecs.keys()) if (!roomById.has(id)) rec.roomRecs.delete(id);
  reconcile(rec.rooms, rooms.map((r) => {
    let row = rec.roomRecs.get(r.id);
    if (!row) rec.roomRecs.set(r.id, (row = createRoomRow()));
    updateRoomRow(row, m.group, r, desks.filter((d) => d.meeting.room === r.id));
    return row.el;
  }));
}

// Only status *changes* are spoken to assistive tech, never tail updates.
let spoken = [];
function announce(m, desk) {
  const id = `${m.group}|${desk.key}`;
  const was = state.seen.get(id);
  state.seen.set(id, desk.status);
  if (state.ready && was === "running" && desk.status !== "running") spoken.push(`${m.label}：${desk.label} ${pick(MEMBER, desk.status === "idle" ? "succeeded" : desk.status)[0]}`);
}

// ---------------------------------------------------------------- header, body states
let lastState = "";
function renderState(stageData) {
  const { running, review } = stageData.counts;
  const failed = stageData.managers.some((m) => m.status === "failed");
  const pills = [];
  if (running) pills.push(["run", `${running} 进行中`]);
  if (review) pills.push(["review", `${review} 待审核`]);
  if (!pills.length) pills.push(failed ? ["bad", "有失败"] : ["idle", "空闲"]);
  const key = JSON.stringify(pills);
  if (key === lastState) return;
  lastState = key;
  $("state").replaceChildren(...pills.map(([tone, text]) => h("span.state.tone-" + tone, {}, h("i.dot"), text)));
  $("hub").classList.toggle("busy", running > 0);
}

function showBody(kind, retry) {
  const showStage = kind === "stage";
  stage.hidden = !showStage;
  const calm = $("calm");
  calm.hidden = showStage;
  if (showStage || state.calm === kind) { syncOb(); return; }
  state.calm = kind;
  const ghost = h("div.ghost", { "aria-hidden": "true" }, h("span.ghost-node"), h("i.ghost-wire"), h("span.hub", {}, h("span.mark", {}, "N")), h("i.ghost-wire"), h("span.ghost-node"));
  const copy = {
    loading: [null, "正在读取…", "", null],
    no_data: [ghost, "还没有工作室记录", "在主控里派出任务或开始会议后，它的工位和会议室会自动出现在这里。", ["打开办公室", "/chat/"]],
    quiet: [ghost, "最近两小时没有活动", "历史会话仍可在办公室中查看。", ["打开办公室", "/chat/"]],
    empty: [ghost, "最近没有主控在工作", "在主控 Agent（Claude Code、Codex CLI 等）里派出任务或开始会议，它的办公室会带着颜色连线出现在这里。", ["打开办公室", "/chat/"]],
    error: [ghost, "无法读取工作室数据", "工作室数据目录可能还不存在，或本机服务没有响应。", ["重试", null]],
  }[kind];
  const action = copy[3] && (copy[3][1] ? h("a.calm-link", { href: href(copy[3][1]) }, copy[3][0]) : h("button.calm-link", { type: "button" }, copy[3][0]));
  if (action && !copy[3][1]) action.addEventListener("click", retry);
  calm.replaceChildren(...[copy[0], h("strong", {}, copy[1]), copy[2] && h("p", {}, copy[2]), action].filter(Boolean));
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
  else if (service?.state === "done") out.push(notice(`本机服务已升级到 v${service.version}`, "请重启正在使用的 AI 应用，或重新连接 AgentNave MCP，以加载新版工具接口", "查看生效方式", "/settings/#hosts"));
  else if (service?.state === "failed") out.push(notice("升级本机服务失败", service.error, "重试", "/app/upgrade-service", ["复制命令", "/app/copy-service-command"]));
  else if (service) out.push(notice(`本机服务 v${service.version} 比托盘 v${status.version} 旧`, "升级后请重启正在使用的 AI 应用，或重新连接 AgentNave MCP", "一键升级", "/app/upgrade-service"));
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
  link.textContent = update?.state === "available" ? `更新到 v${update.version}` : update?.state === "installing" ? "正在更新…" : "检查更新";
  link.classList.toggle("has-update", update?.state === "available");
}

// ---------------------------------------------------------------- wires
// One SVG overlay over the stage, coordinates = stage-relative pixels. Per manager block:
//   manager card --(straight)--> rail junction (railX, card centre)
//   junction --(S-curve, cubic with horizontal tangents)--> left edge of the office card at its head.
// All elements are pooled by id and patched, so nothing is rebuilt while cells update.
const pool = new Map();
const layers = { rail: $("layer-rail"), wires: $("layer-wires"), nodes: $("layer-nodes") };
let wirePending = false;
function scheduleWires() {
  if (wirePending) return;
  wirePending = true;
  requestAnimationFrame(layoutWires);
}
function shape(layer, id, tag, cls, seen) {
  let el = pool.get(id);
  if (!el) { el = document.createElementNS(SVG, tag); layers[layer].append(el); pool.set(id, el); }
  if (el.getAttribute("class") !== cls) el.setAttribute("class", cls);
  seen.add(id);
  return el;
}
const num = (x) => (Math.round(x * 2) / 2).toString();
const curve = (x1, y1, x2, y2) => {
  if (Math.abs(y2 - y1) < 1) return `M${num(x1)} ${num(y1)}H${num(x2)}`;
  const dx = (x2 - x1) * 0.55;
  return `M${num(x1)} ${num(y1)}C${num(x1 + dx)} ${num(y1)} ${num(x2 - dx)} ${num(y2)} ${num(x2)} ${num(y2)}`;
};
const wireTone = (status) => (status === "running" ? " on" : status === "review" ? " hold" : ["failed", "blocked", "interrupted"].includes(status) ? " bad" : "");
function layoutWires() {
  wirePending = false;
  if (stage.hidden) return;
  const box = stage.getBoundingClientRect();
  const rel = (el) => {
    const r = el.getBoundingClientRect();
    return { l: r.left - box.left, r: r.right - box.left, cy: (r.top + r.bottom) / 2 - box.top };
  };
  $("wires").setAttribute("viewBox", `0 0 ${box.width.toFixed(1)} ${box.height.toFixed(1)}`);
  const seen = new Set();
  const railTop = stage.firstElementChild.offsetHeight - 4; // the sticky header keeps its flow slot
  let railX = 0, lastY = railTop;
  for (const id of state.order) {
    const rec = state.blocks.get(id);
    if (!rec) continue;
    const c = colorClass(rec.data.color);
    const m = rel(rec.mgr), g = rel(rec.grp), top = rel(rec.head);
    railX = (m.r + g.l) / 2;
    const y = m.cy;
    lastY = Math.max(lastY, y);
    shape("wires", `m:${id}`, "path", `wire ${c}${wireTone(rec.data.status)}`, seen).setAttribute("d", `M${num(m.r)} ${num(y)}H${num(railX)}`);
    shape("nodes", `j:${id}`, "circle", `node junction ${c}`, seen).setAttribute("cx", num(railX));
    pool.get(`j:${id}`).setAttribute("cy", num(y));
    pool.get(`j:${id}`).setAttribute("r", "4");
    shape("wires", `w:${id}`, "path", `wire ${c}${wireTone(rec.data.status)}`, seen).setAttribute("d", curve(railX, y, g.l, top.cy));
    const dot = shape("nodes", `e:${id}`, "circle", `node end ${c}`, seen);
    dot.setAttribute("cx", num(g.l));
    dot.setAttribute("cy", num(top.cy));
    dot.setAttribute("r", "2.5");
  }
  shape("rail", "rail", "path", "rail", seen).setAttribute("d", `M${num(railX)} ${num(railTop)}V${num(lastY)}`);
  for (const [id, el] of pool) if (!seen.has(id)) { el.remove(); pool.delete(id); }
}
new ResizeObserver(scheduleWires).observe(stage);
window.addEventListener("resize", scheduleWires);
if (document.fonts?.ready) document.fonts.ready.then(scheduleWires);

// ---------------------------------------------------------------- refresh
function render(data) {
  state.failures = 0;
  renderState(data);
  if (!data.managers.length) {
    showBody(data.empty === "no_data" ? "no_data" : "quiet");
    state.blocks.clear();
    groupsEl.replaceChildren();
    state.order = [];
    return;
  }
  showBody("stage");
  const items = data.managers.map((m) => ({ id: m.group, rank: urgent(m.status) ? 0 : 1 }));
  state.order = stickyOrder(state.order, items);
  const byGroup = new Map(data.managers.map((m) => [m.group, m]));
  for (const id of state.blocks.keys()) if (!byGroup.has(id)) state.blocks.delete(id);
  for (const id of state.order) {
    const m = byGroup.get(id);
    let rec = state.blocks.get(id);
    if (!rec) {
      state.blocks.set(id, (rec = createBlock(m)));
      if (state.ready) spoken.push(`新主控 ${m.label} 出现`);
    }
    updateBlock(rec, m);
  }
  reconcile(groupsEl, state.order.map((id) => state.blocks.get(id).root));
  scheduleWires();
}

async function refresh() {
  const [stageResult, statusResult] = await Promise.allSettled([load("/app/stage"), load("/app/status")]);
  if (statusResult.status === "fulfilled") state.appStatus = statusResult.value;
  if (stageResult.status === "rejected") {
    state.failures++;
    if (!state.blocks.size) showBody("error", () => { state.calm = ""; run(); });
    renderNotices(state.appStatus, state.blocks.size > 0);
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
