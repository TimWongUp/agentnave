"use strict";
// Menu bar panel, "stage": managers (host CLI sessions) on the left, AgentNave in the middle,
// members grouped by manager on the right, wired in the manager's colour. Every node is
// created once and then patched in place (keyed by group id / member key); wires are an SVG
// overlay recomputed from layout after each render, resize and fold. Links are real routes
// (/chat/<group>); the tray app intercepts them and opens the main window.
const {
  h, $, load, href, MEMBER, GROUP, pick, urgent, avatar, hostAvatar, colorClass,
  relTime, fullTime, setText, setAttr, setClass, reconcile, stickyOrder, poll,
} = AN;

const FOLD_AT = 6; // a manager with this many members or more folds its extra rows ...
const SHOWN = 4; //   ... down to this many
const SVG = "http://www.w3.org/2000/svg";

if (new URLSearchParams(location.search).has("glass")) document.body.classList.add("glass");

const stage = $("stage");
const groupsEl = $("groups");
const state = {
  blocks: new Map(), // group id -> block record
  order: [], // sticky manager order
  expanded: new Set(), // groups whose folded members are shown
  seen: new Map(), // `${group}|${key}` -> last member status (for announcements)
  ready: false, // first render done: announcements start after it
  failures: 0,
  appStatus: null,
  calm: "", // which body state is drawn: "", loading, empty, error
};

// ---------------------------------------------------------------- blocks
function createRow() {
  const who = h("b.who"), prov = h("span.prov"), dot = h("i.dot"), word = h("span"), line = h("span.line");
  const ava = h("span.avatar");
  const el = h("a.row", {}, ava, h("span.row-main", {}, h("span.row-top", {}, who, prov, h("span.st", {}, dot, word)), line));
  return { el, ava, who, prov, word, line, text: null, avatarKey: "" };
}

function updateRow(rec, group, m) {
  const [label, tone] = pick(MEMBER, m.status);
  const row = rec;
  if (row.avatarKey !== m.provider + m.model) {
    row.avatarKey = m.provider + m.model;
    const next = avatar(m.provider, m.model);
    row.ava.replaceWith(next);
    row.ava = next;
  }
  setClass(row.el, `row tone-${tone} s-${m.status}`);
  setAttr(row.el, "href", href(`/chat/${group}#${encodeURIComponent(m.key)}`));
  setAttr(row.el, "title", `${m.model} · ${m.provider}\n${m.line}\n${fullTime(m.updated)}`);
  setText(row.who, m.model);
  setText(row.prov, `· ${m.provider}`);
  setText(row.word, label);
  setClass(row.line, m.status === "running" ? "line live" : "line");
  if (row.text !== m.line) {
    const first = row.text === null;
    row.text = m.line;
    setText(row.line, m.line);
    if (!first && m.status === "running") {
      row.line.classList.remove("tick");
      void row.line.offsetWidth; // restart the flash; one element, cheap
      row.line.classList.add("tick");
    }
  }
}

function createBlock(m) {
  const mgrState = h("span.mgr-state", {}, h("i.dot"), h("span"));
  const presence = h("i.presence");
  const mgrName = h("span.mgr-name");
  const mgr = h("a.mgr", {}, h("span.mgr-top", {}, h("span.mgr-ava", {}, hostAvatar(m.host, m.label), presence), mgrName), mgrState);
  const headCount = h("span.count"), headRun = h("span.tag.tag-run"), headReview = h("span.tag.tag-review"), headTime = h("time");
  const head = h("a.grp-head", {}, headCount, headRun, headReview, headTime);
  const list = h("div.rows");
  const foldText = h("span");
  const fold = h("button.fold", { type: "button" }, h("i.chev"), foldText);
  const grp = h("div.grp", {}, head, list, fold);
  const root = h("section.block.enter", {}, mgr, h("span.gap", { "aria-hidden": "true" }), grp);
  const rec = { id: m.group, root, mgr, mgrName, mgrState, presence, grp, head, headCount, headRun, headReview, headTime, list, fold, foldText, rows: new Map(), order: [], data: m };
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
  const n = m.members.length;
  const running = m.members.filter((x) => x.status === "running").length;
  const review = m.members.filter((x) => x.status === "review").length;
  setClass(rec.root, `block ${colorClass(m.color)}${rec.root.classList.contains("enter") ? " enter" : ""}`);
  setClass(rec.mgr, `mgr tone-${tone}${m.live ? "" : " off"}`);
  setAttr(rec.mgr, "href", href(`/chat/${m.group}`));
  setAttr(rec.mgr, "aria-label", `${m.label}，${word}${m.live ? "" : "，离线"}，${n} 个组员，打开群聊`);
  setText(rec.mgrName, m.label);
  setText(rec.mgrState.lastChild, m.live ? word : `${word} · 离线`);
  setClass(rec.presence, m.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", m.live ? "主控在线" : "主控已离线，仅有历史记录");
  setAttr(rec.head, "href", href(`/chat/${m.group}`));
  setText(rec.headCount, `${n} 位组员`);
  setText(rec.headRun, running ? `${running} 进行中` : "");
  setText(rec.headReview, review ? `${review} 待审核` : "");
  setText(rec.headTime, relTime(m.updated));

  // rows: sticky order, folded beyond SHOWN when n >= FOLD_AT
  const items = m.members.map((x) => ({ id: x.key, rank: urgent(x.status) ? 0 : 1 }));
  rec.order = stickyOrder(rec.order, items);
  const byKey = new Map(m.members.map((x) => [x.key, x]));
  for (const key of rec.rows.keys()) if (!byKey.has(key)) rec.rows.delete(key);
  const folded = n >= FOLD_AT && !state.expanded.has(m.group);
  let hiddenCount = 0, hiddenRunning = 0;
  rec.order.forEach((key, i) => {
    let row = rec.rows.get(key);
    if (!row) rec.rows.set(key, (row = createRow()));
    const member = byKey.get(key);
    updateRow(row, m.group, member);
    row.visible = !(folded && i >= SHOWN);
    row.el.hidden = !row.visible;
    if (!row.visible) {
      hiddenCount++;
      if (member.status === "running") hiddenRunning++;
    }
    announce(m, member);
  });
  reconcile(rec.list, rec.order.map((key) => rec.rows.get(key).el));

  const foldable = n >= FOLD_AT;
  rec.fold.hidden = !foldable;
  if (foldable) {
    setText(rec.foldText, folded ? `另外 ${hiddenCount} 位组员${hiddenRunning ? ` · ${hiddenRunning} 进行中` : ""}` : "收起");
    setAttr(rec.fold, "aria-expanded", folded ? "false" : "true");
    setClass(rec.fold, folded && hiddenRunning ? "fold has-run" : "fold");
  }
}

// Only status *changes* are spoken to assistive tech, never tail updates.
let spoken = [];
function announce(m, member) {
  const id = `${m.group}|${member.key}`;
  const was = state.seen.get(id);
  state.seen.set(id, member.status);
  if (state.ready && was && was !== member.status && was === "running")
    spoken.push(`${m.label}：${member.model} ${pick(MEMBER, member.status)[0]}`);
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
  if (showStage || state.calm === kind) return;
  state.calm = kind;
  const ghost = h("div.ghost", { "aria-hidden": "true" }, h("span.ghost-node"), h("i.ghost-wire"), h("span.hub", {}, h("span.mark", {}, "N")), h("i.ghost-wire"), h("span.ghost-node"));
  const copy = {
    loading: [null, "正在读取…", "", null],
    no_data: [ghost, "还没有工作室记录", "在主控里派出任务或开始节目后，它和组员会自动出现在这里。", ["查看全部群聊", "/chat/"]],
    quiet: [ghost, "最近两小时没有活动", "历史会话仍可在群聊中查看。", ["查看全部群聊", "/chat/"]],
    empty: [ghost, "最近没有主控在工作", "在主控 Agent（Claude Code、Codex CLI 等）里派出任务或开始节目，它和它的组员会带着颜色连线出现在这里。", ["查看全部群聊", "/chat/"]],
    error: [ghost, "无法读取工作室数据", "工作室数据目录可能还不存在，或本机服务没有响应。", ["重试", null]],
  }[kind];
  const action = copy[3] && (copy[3][1] ? h("a.calm-link", { href: href(copy[3][1]) }, copy[3][0]) : h("button.calm-link", { type: "button" }, copy[3][0]));
  if (action && !copy[3][1]) action.addEventListener("click", retry);
  calm.replaceChildren(...[copy[0], h("strong", {}, copy[1]), copy[2] && h("p", {}, copy[2]), action].filter(Boolean));
}

// ---------------------------------------------------------------- update / service notices
function older(version, than) {
  const a = String(version).split(".").map(Number), b = than.split(".").map(Number);
  for (let i = 0; i < b.length; i++) if ((a[i] || 0) !== b[i]) return (a[i] || 0) < b[i];
  return false;
}
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
  else if (service?.state === "done") out.push(notice(`本机服务已升级到 v${service.version}`, "各宿主会在空闲时自动切换到新版本"));
  else if (service?.state === "failed") out.push(notice("升级本机服务失败", service.error, "重试", "/app/upgrade-service", ["复制命令", "/app/copy-service-command"]));
  else if (service) out.push(notice(`本机服务 v${service.version} 比托盘 v${status.version} 旧`, older(service.version, "0.18.0") ? "升级后需重启一次各宿主，此后宿主会自动切换" : "升级后各宿主会在空闲时自动切换", "一键升级", "/app/upgrade-service"));
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
//   junction --(S-curve, cubic with horizontal tangents)--> left edge of the group card at
//   each visible member row's centre; folded rows get one dotted stub to the fold button.
// All elements are pooled by id and patched, so nothing is rebuilt while rows update.
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
    const m = rel(rec.mgr), g = rel(rec.grp);
    railX = (m.r + g.l) / 2;
    const y = m.cy;
    lastY = Math.max(lastY, y);
    shape("wires", `m:${id}`, "path", `wire ${c}${wireTone(rec.data.status)}`, seen).setAttribute("d", `M${num(m.r)} ${num(y)}H${num(railX)}`);
    shape("nodes", `j:${id}`, "circle", `node junction ${c}`, seen).setAttribute("cx", num(railX));
    pool.get(`j:${id}`).setAttribute("cy", num(y));
    pool.get(`j:${id}`).setAttribute("r", "4");
    for (const key of rec.order) {
      const row = rec.rows.get(key);
      if (!row || !row.visible) continue;
      const r = rel(row.el);
      const status = rec.data.members.find((x) => x.key === key)?.status;
      shape("wires", `w:${id}/${key}`, "path", `wire ${c}${wireTone(status)}`, seen).setAttribute("d", curve(railX, y, g.l, r.cy));
      const dot = shape("nodes", `e:${id}/${key}`, "circle", `node end ${c}`, seen);
      dot.setAttribute("cx", num(g.l));
      dot.setAttribute("cy", num(r.cy));
      dot.setAttribute("r", "2.5");
    }
    if (!rec.fold.hidden) {
      const f = rel(rec.fold);
      shape("wires", `f:${id}`, "path", `wire stub ${c}`, seen).setAttribute("d", curve(railX, y, g.l, f.cy));
    }
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
