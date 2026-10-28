"use strict";
// App office: one workspace per boss session. Left: workspace list (running pinned). Middle: the
// selected workspace as a painted, read-only office -- the boss's office, one meeting room per saved
// group and one desk per (CLI, model, role). Right: the conversation of whatever is selected (the whole
// workspace, one desk or one room). Everything is patched in place (list items keyed by group id, desks
// by desk key, rooms by room id, message nodes by message id) so a 1.5 s poll never rebuilds the DOM,
// never flickers and never moves the scroll position of a reader who scrolled up. Movement (dispatch,
// return, walking to a meeting) is inferred from the difference between two polls.
const {
  h,
  $,
  load,
  href,
  GROUP,
  pick,
  urgent,
  avatar,
  colorClass,
  relTime,
  clock,
  fullTime,
  dayLabel,
  setText,
  setAttr,
  setClass,
  reconcile,
  stickyOrder,
  poll,
  plain,
  art: ART,
  cliName,
  hostTile,
  deskName,
  chatName,
  chatPlace,
  activities,
} = AN;

const mockOn = !!(window.AgentNaveMock && window.AgentNaveMock.enabled());
const narrow = window.matchMedia("(max-width: 760px)");
const overlay = window.matchMedia("(max-width: 860px)"); // the conversation slides over the floor
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const FOLD_PX = 260; // a bubble taller than this is folded behind "展开全文"
const NEAR_BOTTOM = 64; // within this distance of the bottom the reader "follows" the chat
const MAX_FLIGHTS = 8; // more new messages than this between two polls are not animated
const SLEEP_AFTER = 20 * 60e3; // an idle boss dozes off after this long
const COLLAPSED_KEY = "agentnave.office.convCollapsed";
// Layout preferences survive restarts; storage may be unavailable, which only loses them.
function stored(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}
function store(key, value) {
  try {
    if (value == null) localStorage.removeItem(key); else localStorage.setItem(key, value);
  } catch { /* storage may be unavailable */ }
}
const FIG = 100; // character size at meeting seats before any desk is drawn, in px (matches --fig in chat.css)
const DONE_FOR = 8e3; // a desk that just finished shows its done frame this long
const WALK_MS = 1600; // a figure's walk between its desk and a meeting seat
const HOLD_MS = 10e3; // an ended room stays empty this long after its seats are back
const FOLD_MS = 500; // then the desks below push it shut
const LEAVE_MS = WALK_MS + HOLD_MS + FOLD_MS;

// Tags stay short; the full text is in the tooltip.
const deskChar = (d) => ART.character(d.provider, d.display_model || d.model);
// How a desk that requested no model got the name it shows.
const MODEL_NOTE = {
  cli_default: "\n调用时没有指定模型，这里显示 CLI 自己设置的默认模型，以 CLI 回报为准",
  unknown: "\n调用时没有指定模型，CLI 也没有回报，用的是它自己的默认模型",
};
const handleOf = (d) =>
  d.handle ||
  `@${deskName(d)}·${cliName(d.provider)}${d.role ? `·${d.role}` : ""}`;

const nearBottom = () =>
  el.tl.scrollHeight - el.tl.scrollTop - el.tl.clientHeight < NEAR_BOTTOM;
const el = {
  app: $("app"),
  list: $("list"),
  head: $("head"),
  office: $("office"),
  floor: $("floor"),
  boss: $("boss"),
  desks: $("desks"),
  rooms: $("rooms"),
  fx: $("fx"),
  deskCount: $("desks-count"),
  ticker: $("ticker"),
  tick: $("tick"),
  drawer: $("drawer"),
  convToggle: $("conv-toggle"),
  drIco: $("dr-ico"),
  drTitle: $("dr-title"),
  drSub: $("dr-sub"),
  drAll: $("dr-all"),
  drNote: $("dr-note"),
  tl: $("timeline"),
  inner: $("tl-inner"),
  blank: $("blank"),
  jump: $("jump"),
  banner: $("banner"),
  count: $("side-count"),
};
const state = {
  selected: null, // workspace id shown in the middle
  auto: false, // selected by default (no workspace in the URL), not by the user
  groupsData: [],
  order: [],
  items: new Map(),
  sections: {},
  chat: null,
  ctx: null, // last payload and its lookup tables
  loaded: null, // workspace id whose nodes are currently on screen
  hashKey: null,
  focused: false,
  headKey: "",
  // office
  bossRec: null,
  deskRecs: new Map(),
  roomRecs: new Map(),
  seatRecs: new Map(),
  away: new Map(), // desk key -> "room|seat" it currently sits in, for walking animations
  closing: new Map(), // room id -> when the ended room leaves the floor
  known: new Set(),
  tickId: null,
  // conversation: {kind:"all"} | {kind:"desk", key} | {kind:"room", id}
  drawer: { kind: "all" },
  convOpen: false, // only matters while the conversation is an overlay
  convCollapsed: stored(COLLAPSED_KEY) === "1", // the reader folded the conversation column away
  opener: null,
  scopeKey: "",
  nodes: new Map(),
  expanded: new Set(),
  seenIds: new Set(),
  unseen: 0,
  flashId: null,
  flashTimer: 0,
  ready: false,
  refreshing: false,
  again: false,
};

const ob = AN.onboarding({ variant: "compact", onChange: () => placeOb() });
function placeOb() {
  const none = state.groupsData.length === 0;
  const full = none && !(narrow.matches && el.app.dataset.view === "list");
  ob.setVariant(full ? "full" : "compact");
  const slot = full ? (el.blank.hidden ? null : el.blank) : $("onboard-side");
  const description = el.blank.querySelector(":scope > p");
  if (description && none) description.hidden = ob.visible;
  if (!ob.visible || !slot) {
    ob.el.remove();
    return;
  }
  if (ob.el.parentNode !== slot) slot.append(ob.el);
}

// ---------------------------------------------------------------- routing
function groupFromLocation() {
  if (mockOn && new URLSearchParams(location.search).has("group"))
    return new URLSearchParams(location.search).get("group");
  const m = /^\/chat\/([^/]+)/.exec(location.pathname);
  try {
    return m ? decodeURIComponent(m[1]) : null;
  } catch {
    return null;
  }
}
const keyFromHash = () => {
  try {
    return location.hash.length > 1
      ? decodeURIComponent(location.hash.slice(1))
      : null;
  } catch {
    return null;
  }
};
function go(path, replace) {
  history[replace ? "replaceState" : "pushState"]({}, "", href(path));
  sync();
  refresh();
}
function sync() {
  const fromUrl = groupFromLocation();
  state.hashKey = keyFromHash();
  state.focused = false;
  if (fromUrl) {
    state.selected = fromUrl;
    state.auto = false;
  } else if (narrow.matches) {
    state.selected = null;
    state.auto = false;
  } else state.auto = true; // wide window without a workspace: first one is picked in refresh()
  el.app.dataset.view =
    fromUrl || (!narrow.matches && state.selected) ? "chat" : "list";
}
window.addEventListener("popstate", () => {
  sync();
  refresh();
});
window.addEventListener("hashchange", () => {
  sync();
  refresh();
});
for (const query of [narrow, overlay])
  query.addEventListener("change", () => {
    sync();
    renderList();
    placeOb();
    if (state.chat) renderAll(state.chat);
  });

// ---------------------------------------------------------------- workspace list
function sectionEl(id, text) {
  return (state.sections[id] ||= h("div.gl-sec", {}, text));
}
function createItem(g) {
  const rec = {
    id: g.group,
    tile: h("span.gi-tile"),
    presence: h("i.presence"),
    label: h("b.gi-label"),
    meta: h("span.gi-meta"),
    time: h("time"),
    prev: h("span.gi-prev"),
    pill: h("span.pill"),
    tileKey: null,
  };
  rec.el = h(
    "a.gi",
    {},
    h("span.gi-host", {}, rec.tile, rec.presence),
    h(
      "span.gi-body",
      {},
      h("span.gi-top", {}, rec.label, rec.time),
      rec.meta,
      h("span.gi-bottom", {}, rec.prev, rec.pill),
    ),
  );
  rec.el.addEventListener("click", (e) => {
    if (e.button) return;
    e.preventDefault();
    if (!Actions.pick(rec.id, e)) go(`/chat/${rec.id}`);
  });
  rec.el.addEventListener("contextmenu", (e) => Actions.workspaceMenu(rec.id, e));
  return rec;
}
function updateItem(rec, g) {
  const [word, tone] = pick(GROUP, AN.ongoing(g.status));
  const on = g.group === state.selected;
  const name = chatName(g);
  setClass(
    rec.el,
    `gi ${colorClass(g.color)} tone-${tone}${on ? " on" : ""}${urgent(g.status) ? " hot" : ""}${Actions.picked.has(g.group) ? " picked" : ""}`,
  );
  setAttr(rec.el, "href", href(`/chat/${g.group}`));
  setAttr(rec.el, "aria-current", on ? "true" : null);
  setAttr(rec.el, "aria-label", `${name}，${handoverWord(g)}boss 在 ${g.label}，${word}`);
  setAttr(rec.el, "title", `${name} · ${chatPlace(g)}`);
  const tileKey = `${g.host}|${g.label}|${!!window.Avatars?.custom("host", g.host, "")}`;
  if (rec.tileKey !== tileKey) {
    rec.tileKey = tileKey;
    rec.tile.replaceChildren(hostTile(g.host, g.label));
  }
  setClass(rec.presence, g.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", g.live ? "有任务在运行" : null);
  setText(rec.label, name);
  setText(
    rec.meta,
    `${handoverWord(g)}${chatPlace(g)} · ${g.desks_count ?? 0} 工位 · ${g.rooms_count ?? 0} 会议室`,
  );
  setText(rec.time, relTime(g.updated));
  setText(rec.prev, g.preview);
  rec.pill.hidden = g.status === "idle";
  setClass(rec.pill, `pill tone-${tone}`);
  setText(rec.pill, word);
}
// A workspace waiting for, or taken over by, another boss (ADR-0033).
function handoverWord(g) {
  return { pending: "待接手 · ", changed: "boss 已更换 · " }[g.handover] || "";
}
function renderList() {
  const groups = state.groupsData;
  setText(el.count, groups.length ? String(groups.length) : "");
  const by = new Map(groups.map((g) => [g.group, g]));
  state.order = stickyOrder(
    state.order,
    groups.map((g) => ({ id: g.group, rank: urgent(g.status) ? 0 : 1 })),
  );
  for (const id of state.items.keys()) if (!by.has(id)) state.items.delete(id);
  const nodes = [];
  let section = -1;
  for (const id of state.order) {
    const g = by.get(id);
    const rank = urgent(g.status) ? 0 : 1;
    if (rank !== section) {
      section = rank;
      nodes.push(
        rank === 0 ? sectionEl("hot", "进行中") : sectionEl("rest", "最近"),
      );
    }
    let rec = state.items.get(id);
    if (!rec) state.items.set(id, (rec = createItem(g)));
    updateItem(rec, g);
    nodes.push(rec.el);
  }
  if (!groups.length) nodes.push(h("div.gl-empty", {}, "还没有工作区"));
  reconcile(el.list, nodes);
}

// ---------------------------------------------------------------- what everyone is doing
function counts(c) {
  let running = 0;
  for (const d of c.desks)
    running += activities(d).filter((a) => a.status === "running").length;
  const review = c.rooms.reduce((n, r) => n + (r.pending_review || 0), 0);
  return { running, review };
}
// Mood picks the character frame; word is the plain status shown under the desk.
function deskMood(d, ctx) {
  const acts = activities(d);
  const meeting =
    acts.find((a) => a.kind === "meeting" && ctx.fullRooms.has(a.room)) ||
    seatedIn(d, ctx);
  const privateRun = acts.some(
    (a) => a.kind === "private" && a.status === "running",
  );
  if (privateRun)
    return {
      mood: "working",
      say: "dots",
      word: meeting ? "私聊处理中 · 另一席在开会" : "私聊处理中",
      away: null,
    };
  if (meeting) {
    const room = ctx.rooms.get(meeting.room);
    return {
      mood: "idle",
      say: "去开会了",
      word: `在会议室「${room?.title || "会议"}」`,
      away: `${meeting.room}|${meeting.seat}`,
    };
  }
  if (d.status === "failed")
    return { mood: "failed", say: "上次失败", word: "上次失败", away: null };
  if (d.status === "pending")
    return { mood: "idle", say: "", word: "等待中", away: null };
  return AN.dozing(d)
    ? {
        mood: "sleep",
        say: "zz",
        word: "休息中",
        away: null,
      }
    : { mood: "idle", say: "", word: "空闲", away: null };
}
// A seat that has answered stays at the table until its round ends, so it is never drawn twice.
function seatedIn(d, ctx) {
  if (!d.room || !ctx.fullRooms.has(d.room)) return null;
  const seat = ctx.rooms.get(d.room)?.seats.find((s) => s.desk === d.key);
  return seat ? { room: d.room, seat: seat.seat } : null;
}
function setSay(node, say) {
  const key = String(say);
  if (node.dataset.say === key) return;
  node.dataset.say = key;
  if (say === "dots")
    node.replaceChildren(h("span.dots", {}, h("i"), h("i"), h("i")));
  else if (say === "zz") node.replaceChildren(h("span.zz", {}, "z z"));
  else if (Array.isArray(say))
    node.replaceChildren(say[0], h("span.dots", {}, h("i"), h("i"), h("i")));
  else node.replaceChildren(say || "");
}
const img = (cls, file) =>
  h(`img.${cls}`, {
    src: ART.url(`office/${file}.webp`),
    alt: "",
    draggable: "false",
  });
// Swap the character figure only when the model family changes; the frames stay decoded otherwise.
// The figure sits right before `before` (the desk), so the desk covers its lower half.
function dress(rec, name, slot, before) {
  if (rec.char !== name) {
    rec.char = name;
    const fig = ART.figure(name);
    if (rec.fig) rec.fig.remove();
    rec.fig = fig;
  }
  if (rec.fig.parentNode !== slot || rec.fig.nextSibling !== before)
    slot.insertBefore(rec.fig, before);
}
function dressLid(rec, provider) {
  const key = `${provider}|${!!window.Avatars?.custom("host", provider, "")}`;
  if (rec.lidKey === key) return;
  rec.lidKey = key;
  rec.lid.replaceChildren(ART.lid(provider));
}

// ---------------------------------------------------------------- floor head
function renderHead(c, n) {
  el.head.hidden = false;
  if (state.headKey !== c.group) {
    state.headKey = c.group;
    const back = h(
      "a.back",
      { href: href("/chat/"), "aria-label": "返回工作区列表" },
      "‹ 工作区",
    );
    back.addEventListener("click", (e) => {
      e.preventDefault();
      state.selected = null;
      go("/chat/");
    });
    state.head = {
      title: h("h1"),
      sub: h("span.fh-sub"),
      run: h("span.pill.tone-run"),
    };
    el.head.replaceChildren(
      back,
      state.head.title,
      state.head.sub,
      h("span.fh-sum", {}, state.head.run),
    );
  }
  const t = state.head;
  setText(t.title, chatName(c));
  setText(t.sub, `${handoverWord(c)}boss 在 ${c.label} 里${c.project && c.project !== chatName(c) ? ` · ${c.project}` : ""}`);
  t.run.hidden = !n.running && !n.review;
  setText(t.run, n.running ? `${n.running} 个在工作` : "会议进行中");
}

// ---------------------------------------------------------------- boss office
function bossModel(c) {
  for (let i = c.messages.length - 1; i >= 0; i--) {
    const s = c.messages[i].speaker;
    if (s?.role === "manager" && s.model) return s.model;
  }
  return "";
}
// `live` only means a server is running work right now; an idle boss dozes off after a while.
function recentlyActive(c) {
  const g = state.groupsData.find((x) => x.group === c.group);
  return !!g && Date.now() - AN.at(g.updated) < SLEEP_AFTER;
}
function createBoss() {
  const rec = {
    say: h("span.say"),
    lid: h("span.lid"),
    scene: null,
    nm: h("div.nm", {}, "boss"),
    cli: h("div.cli"),
    st: h("div.st"),
    state: h("span.r"),
  };
  rec.desk = img("l-desk", "boss-desk");
  rec.scene = h(
    "div.scene",
    {},
    img("l-chair", "chair"),
    rec.desk,
    img("l-laptop.l-open", "laptop"),
    img("l-laptop.l-closed", "laptop-closed"),
    rec.lid,
  );
  // boss sits in the middle; recent chats on the left, meetings on the right
  rec.chats = h("div.boss-side");
  rec.meets = h("div.boss-side");
  el.boss.replaceChildren(
    h("h2.zone-h", {}, "boss 办公室", rec.state),
    h(
      "div.boss-row",
      {},
      rec.chats,
      // the speech bubble sits above the head, never over it
      h("div.boss-mid", {}, rec.say, rec.scene, h("div.label", {}, rec.nm, rec.cli, rec.st)),
      rec.meets,
    ),
  );
  el.boss.onclick = () => openScope({ kind: "all" }, el.boss);
  return rec;
}
function updateBoss(rec, c, n) {
  const model = bossModel(c);
  dress(rec, ART.character(c.host, model), rec.scene, rec.desk);
  dressLid(rec, c.host);
  const mood = n.running || n.review ? "working" : recentlyActive(c) ? "idle" : "sleep";
  el.boss.dataset.mood = mood;
  setSay(
    rec.say,
    n.running ? [`等 ${n.running} 个回复`] : mood === "sleep" ? "zz" : "",
  );
  setText(rec.cli, c.label);
  setText(
    rec.st,
    n.running ? `等 ${n.running} 个回复` : n.review ? "会议进行中" : "空闲",
  );
  setText(rec.state, model ? model : "");
  setAttr(
    el.boss,
    "aria-label",
    `boss 办公室，${rec.st.textContent}，查看全部消息`,
  );
  el.boss.classList.toggle("sel", state.drawer.kind === "all");
}

// ---------------------------------------------------------------- beside the boss
// Recent chats (running first) and meetings (in session first) beside the boss.
function sideItems(c, ctx) {
  const recent = (a, b) => AN.at(b.last_active) - AN.at(a.last_active);
  const running = (d) => activities(d).some((a) => a.status === "running");
  const chats = [...c.desks]
    .sort((a, b) => Number(running(b)) - Number(running(a)) || recent(a, b))
    .map((d) => {
      const s = deskMood(d, ctx);
      return {
        scope: { kind: "desk", key: d.key },
        heads: [d],
        name: deskName(d),
        sub: `${cliName(d.provider)} · ${s.word}`,
        tone: running(d) ? "run" : s.mood === "failed" ? "bad" : "",
        time: relTime(d.last_active),
      };
    });
  const meets = [...c.rooms]
    .sort((a, b) => Number(fullRoom(b)) - Number(fullRoom(a)) || recent(a, b))
    .map((r) => ({
      scope: { kind: "room", id: r.id },
      heads: r.seats.map((s) => ctx.desks.get(s.desk)).filter(Boolean),
      name: r.title,
      sub: `${roundText(r)} · ${r.seats.length} 位参会`,
      tone: urgent(r.status) ? "run" : r.status === "failed" ? "bad" : "",
      time: r.last_active ? relTime(r.last_active) : "",
    }));
  return { chats, meets };
}
function sideList(node, title, items, empty) {
  const key = JSON.stringify([title, items, ART.version()]);
  if (node.dataset.key === key) return;
  node.dataset.key = key;
  const rows = [];
  for (const item of items) {
    const row = h(
      `button.td-row${item.tone ? `.${item.tone}` : ""}`,
      { type: "button", title: `${item.name}\n${item.sub}` },
      h("span.td-heads", {}, item.heads.slice(0, 3).map((d) => ART.head(deskChar(d)))),
      h("span.td-main", {}, h("span.td-name", {}, item.name), h("span.td-sub", {}, item.sub)),
      item.time && h("span.td-time", {}, item.time),
    );
    row.addEventListener("click", (e) => {
      e.stopPropagation();
      openScope(item.scope, row);
    });
    if (item.scope.kind === "room") row.addEventListener("contextmenu", (e) => Actions.roomMenu(item.scope.id, e));
    rows.push(row);
  }
  if (!items.length) rows.push(h("div.td-more", {}, empty));
  // three rows show at once; the rest scroll inside the list
  const list = h("div.side-rows", {}, rows);
  const top = node.querySelector(".side-rows")?.scrollTop || 0;
  node.replaceChildren(
    h("h3.td-h", {}, title, items.length ? h("b", {}, String(items.length)) : null),
    list,
  );
  list.scrollTop = top;
}
function renderSides(rec, c, ctx) {
  const { chats, meets } = sideItems(c, ctx);
  sideList(rec.chats, "最近对话", chats, "还没有私聊");
  sideList(rec.meets, "会议", meets, "还没有开过会");
}

// ---------------------------------------------------------------- desks
function createDesk(d) {
  const rec = {
    key: d.key,
    say: h("span.say"),
    lid: h("span.lid"),
    nm: h("div.nm"),
    cliText: h("span"),
    role: h("div.role"),
    access: h("span.tag.tag-access", {
      hidden: true,
      title: "boss 声明的访问等级，只是约定，不改变 CLI 的实际权限",
    }),
    only: h(
      "span.tag.tag-only",
      { hidden: true, title: "与 boss 的宿主同类，只参加会议，不接受私聊" },
      "仅开会",
    ),
  };
  rec.desk = img("l-desk", "desk");
  rec.scene = h(
    "div.scene",
    {},
    img("l-chair", "chair"),
    rec.desk,
    img("l-laptop.l-open", "laptop"),
    img("l-laptop.l-closed", "laptop-closed"),
    rec.lid,
  );
  rec.el = h(
    "button.desk",
    { type: "button" },
    // access and status sit in the top-left corner; the role is the label's last line
    h("span.desk-tags", {}, rec.access, rec.only),
    rec.say,
    rec.scene,
    h("div.label", {}, rec.nm, h("div.cli", {}, rec.cliText), rec.role),
  );
  rec.el.addEventListener("click", () =>
    openScope({ kind: "desk", key: d.key }, rec.el),
  );
  return rec;
}
function updateDesk(rec, d, ctx) {
  let s = deskMood(d, ctx);
  // The backend reports a finished task as idle; a desk that just stopped working cheers briefly.
  const settled = s.mood === "idle" && !s.away;
  if (rec.mood?.mood === "working" && settled) rec.doneUntil = Date.now() + DONE_FOR;
  if (settled && rec.doneUntil > Date.now())
    s = { ...s, mood: "done", say: "搞定" };
  rec.mood = s;
  dress(rec, deskChar(d), rec.scene, rec.desk);
  dressLid(rec, d.provider);
  rec.el.dataset.mood = s.mood;
  setClass(
    rec.el,
    `desk${s.away ? " away" : ""}${d.private_allowed === false ? " only" : ""}${state.drawer.kind === "desk" && state.drawer.key === d.key ? " sel" : ""}`,
  );
  setSay(rec.say, s.say);
  setText(rec.nm, deskName(d));
  setText(rec.cliText, cliName(d.provider));
  setText(rec.role, d.role || "");
  setAttr(rec.role, "title", d.role || null);
  setText(
    rec.access,
    d.access === "read" ? "只读" : d.access === "write" ? "可写" : "",
  );
  rec.access.hidden = !d.access;
  rec.only.hidden = d.private_allowed !== false;
  setAttr(
    rec.el,
    "title",
    `${s.word}\n${handleOf(d)}\n首次出现 ${fullTime(d.first_seen)} · 最近 ${fullTime(d.last_active)}${MODEL_NOTE[d.model_source] || (!d.model && !d.display_model ? MODEL_NOTE.unknown : "")}`,
  );
  setAttr(
    rec.el,
    "aria-label",
    `${deskName(d)}，${cliName(d.provider)}${d.role ? `，${d.role}` : ""}，${s.word}，查看对话`,
  );
}

// ---------------------------------------------------------------- meeting rooms
// A room in session is drawn in full; finished rooms fold into one line each.
const fullRoom = (r) => urgent(r.status);
function roundText(r) {
  const n = r.round;
  if (!n) return pick(GROUP, AN.ongoing(r.status))[0];
  const order = n.order === "serial" ? "依次回复" : "同时回复";
  return `第 ${n.number} 轮 · ${urgent(r.status) ? order : pick(GROUP, r.status)[0]}`;
}
function createRoom(r) {
  const rec = {
    id: r.id,
    title: h("span.zone-title"),
    state: h("span.r"),
    scene: h("div.mscene"),
    full: null,
  };
  rec.el = h(
    "section.zone.room",
    { role: "button", tabindex: "0" },
    h(
      "h2.zone-h",
      {},
      h("span", {}, "会议室 · "),
      rec.title,
      rec.state,
    ),
    rec.scene,
  );
  const open = () => openScope({ kind: "room", id: r.id }, rec.el);
  rec.el.addEventListener("click", open);
  rec.el.addEventListener("contextmenu", (e) => Actions.roomMenu(r.id, e));
  rec.el.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      open();
    }
  });
  return rec;
}
function seatRec(id) {
  let seat = state.seatRecs.get(id);
  if (!seat) {
    seat = {
      say: h("span.say"),
      plate: h("span.plate.tag"),
      nm: h("span.seat-nm"),
      lid: null,
    };
    // the role tag hangs on the table front, the model name sits below the table
    seat.el = h("div.seat", {}, seat.say, img("l-chair", "chair"), seat.plate, seat.nm);
    state.seatRecs.set(id, seat);
  }
  return seat;
}
function seatMood(r, seat, d) {
  const act = d
    ? activities(d).find(
        (a) => a.kind === "meeting" && a.room === r.id && a.seat === seat.seat,
      )
    : null;
  if (act?.status === "running")
    return { mood: "speaking", say: ["作答中"], word: "作答中" };
  if (act?.status === "review")
    return { mood: "idle", say: "已作答", word: "已作答" };
  return { mood: "idle", say: "", word: d || seat.desk ? "" : "缺席" };
}
function updateRoom(rec, r, ctx) {
  const [, tone] = pick(GROUP, AN.ongoing(r.status));
  setClass(
    rec.el,
    `zone room${state.closing.has(r.id) ? " closing" : ""}${state.drawer.kind === "room" && state.drawer.id === r.id ? " sel" : ""}`,
  );
  setText(rec.title, r.title);
  setText(rec.state, roundText(r));
  setClass(rec.state, `r ${tone}`);
  setAttr(
    rec.el,
    "aria-label",
    `会议室 ${r.title}，${roundText(r)}，会议记录 ${r.board ?? 0} 条，查看会议`,
  );
  const nodes = [];
  for (const seat of r.seats) {
    const s = seatRec(`${r.id}|${seat.seat}`);
    const d = ctx.desks.get(seat.desk);
    const m = seatMood(r, seat, d);
    s.el.dataset.mood = m.mood;
    s.el.dataset.key = `${r.id}|${seat.seat}`;
    // a seat always names its desk; one missing from the payload is unknown, not absent
    const absent = !d && !seat.desk;
    setClass(s.el, `seat${absent ? " empty" : ""}`);
    dress(s, d ? deskChar(d) : "default", s.el, s.plate);
    setSay(s.say, m.say);
    const name = d ? deskName(d) : seat.handle?.replace(/^@/, "") || seat.seat;
    // model name and role only; the CLI is in the tooltip
    const tag = d ? d.role || "" : absent ? "缺席" : "";
    setText(s.nm, name);
    setText(s.plate, tag);
    s.plate.hidden = !tag;
    setClass(s.plate, `plate tag${d ? " tag-role" : ""}`);
    setAttr(s.el, "title", d ? handleOf(d) : seat.handle || `席位 ${seat.seat}`);
    nodes.push(s.el);
  }
  reconcile(rec.scene, nodes);
}
// Seats are as large as desk figures, so a walker keeps its size; rows wrap to fit the room.
// The boss's scene takes a desk's size too, so it grows and shrinks with the row.
function layoutRooms() {
  const deskScene = el.desks.querySelector(".desk > .scene");
  if (state.bossRec) state.bossRec.scene.style.width = deskScene ? `${deskScene.offsetWidth}px` : "";
  // Desks scale with the floor, so seats take the size their figures have at the desks.
  const fig = el.desks.querySelector(".desk > .scene > .fig")?.offsetWidth || FIG;
  for (const rec of state.roomRecs.values()) {
    const m = rec.scene;
    if (!m.isConnected) continue;
    const avail = rec.el.clientWidth - 26;
    const slots = m.children.length;
    if (avail <= 0 || !slots) continue;
    const size = Math.min(fig, avail);
    const perRow = Math.min(slots, Math.max(1, Math.floor(avail / size)));
    m.style.setProperty("--S", `${size}px`);
    m.style.width = `${size * perRow}px`;
    // the table texture continues across the seats of one row
    for (const seat of m.querySelectorAll(".seat"))
      seat.style.setProperty("--x", `${-seat.offsetLeft}px`);
  }
}
new ResizeObserver(() => layoutRooms()).observe(el.floor);

// ---------------------------------------------------------------- the office, patched in place
const canAnimate = () =>
  !reduceMotion.matches &&
  !document.hidden &&
  typeof Element.prototype.animate === "function";

function renderOffice(c, ctx, n, newGroup) {
  if (newGroup) {
    for (const map of [
      state.deskRecs,
      state.roomRecs,
      state.seatRecs,
      state.away,
      state.closing,
      state.known,
    ])
      map.clear();
    state.bossRec = null;
    el.desks.replaceChildren();
    el.rooms.replaceChildren();
    el.office.scrollTop = 0;
  }
  if (!state.bossRec) state.bossRec = createBoss();
  updateBoss(state.bossRec, c, n);

  // A room that ends leaves the floor with its seats: remember where each away figure sat
  // so its walk back to the desk starts there.
  const sat = new Map();
  for (const id of state.away.values()) {
    const fig = state.seatRecs.get(id)?.fig;
    if (fig?.isConnected) sat.set(id, fig.getBoundingClientRect());
  }
  // rooms first: desks need to know which rooms are drawn in full
  for (const id of state.roomRecs.keys())
    if (!ctx.rooms.has(id)) state.roomRecs.delete(id);
  const live = new Set();
  // only rooms in session sit on the floor; finished ones are listed beside the boss. A room
  // that has just ended stays while its seats walk back to their desks, then fades out.
  const now = Date.now();
  for (const r of c.rooms) {
    const rec = state.roomRecs.get(r.id);
    if (fullRoom(r)) {
      state.closing.delete(r.id);
      rec?.fold?.cancel();
    } else if (!state.closing.has(r.id) && rec?.el.isConnected && !newGroup && canAnimate()) {
      state.closing.set(r.id, now + LEAVE_MS);
      rec.fold = foldRoom(rec.el);
      setTimeout(() => state.chat && renderAll(state.chat), LEAVE_MS + 50);
    }
  }
  for (const [id, until] of state.closing)
    if (until <= now || !ctx.rooms.has(id)) state.closing.delete(id);
  const rooms = c.rooms.filter((r) => fullRoom(r) || state.closing.has(r.id));
  const roomNodes = rooms.map((r) => {
    let rec = state.roomRecs.get(r.id);
    if (!rec) state.roomRecs.set(r.id, (rec = createRoom(r)));
    updateRoom(rec, r, ctx);
    for (const s of r.seats) live.add(`${r.id}|${s.seat}`);
    return rec.el;
  });
  for (const id of state.seatRecs.keys())
    if (!live.has(id)) state.seatRecs.delete(id);
  reconcile(el.rooms, roomNodes);

  for (const key of state.deskRecs.keys())
    if (!ctx.drawn.has(key)) state.deskRecs.delete(key);
  const deskNodes = c.desks.map((d) => {
    let rec = state.deskRecs.get(d.key);
    if (!rec) state.deskRecs.set(d.key, (rec = createDesk(d)));
    updateDesk(rec, d, ctx);
    return rec.el;
  });
  if (!deskNodes.length)
    deskNodes.push(
      h(
        "div.zone-empty",
        {},
        "还没有工位。boss 派出第一个私聊或会议席位后，工位会出现在这里；超出同时运行上限的空闲工位归入「已优化」。",
      ),
    );
  if (c.optimized?.count) deskNodes.push(optimizedNode(c.optimized, ctx));
  reconcile(el.desks, deskNodes);
  setText(el.deskCount, c.desks.length ? ` · ${c.desks.length}` : "");
  renderSides(state.bossRec, c, ctx);
  layoutRooms();
  walk(!newGroup && state.ready, sat);
}

// After its seats walk back and a pause, an ended room shrinks to nothing with its own gap, so
// the desks below slide up into its place and nothing jumps when it leaves the page.
function foldRoom(node) {
  const style = getComputedStyle(node);
  return node.animate(
    [
      {
        height: `${node.offsetHeight}px`,
        marginTop: style.marginTop,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        borderTopWidth: style.borderTopWidth,
        borderBottomWidth: style.borderBottomWidth,
        opacity: 1,
      },
      {
        height: "0px",
        marginTop: "0px",
        paddingTop: "0px",
        paddingBottom: "0px",
        borderTopWidth: "0px",
        borderBottomWidth: "0px",
        opacity: 0,
      },
    ],
    { duration: FOLD_MS, delay: WALK_MS + HOLD_MS, easing: "cubic-bezier(0.4, 0, 0.2, 1)", fill: "forwards" },
  );
}

// Desks past the running limit are not drawn: a text list, opened on demand, links to each one.
function optimizedNode(opt, ctx) {
  const listed = opt.desks || [];
  const key = JSON.stringify([state.optimizedOpen, opt.count, ART.version(), listed.map((d) => [d.key, d.label, d.status, d.last_active])]);
  let node = state.optimizedEl;
  if (node && node.dataset.key === key) return node;
  const toggle = h(
    "button.optimized-h",
    { type: "button", "aria-expanded": String(Boolean(state.optimizedOpen)) },
    "已优化",
    h("b", {}, `${opt.count} 人`),
    h("span.r", {}, state.optimizedOpen ? "收起" : "展开"),
  );
  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    state.optimizedOpen = !state.optimizedOpen;
    if (state.chat) renderAll(state.chat);
  });
  const rows = [];
  if (state.optimizedOpen) {
    for (const d of listed) {
      const s = deskMood(d, ctx);
      const row = h(
        "button.td-row",
        { type: "button", title: `${d.label}\n${s.word}` },
        h("span.td-heads", {}, ART.head(deskChar(d))),
        h("span.td-main", {}, h("span.td-name", {}, d.label), h("span.td-sub", {}, s.word)),
        h("span.td-time", {}, relTime(d.last_active)),
      );
      row.addEventListener("click", (e) => {
        e.stopPropagation();
        openScope({ kind: "desk", key: d.key }, row);
      });
      rows.push(row);
    }
    if (opt.count > listed.length)
      rows.push(h("div.td-more", {}, `另有 ${opt.count - listed.length} 个更早的工位未列出`));
  }
  node = h(
    "div.optimized",
    { "aria-label": `已优化的工位，${opt.count} 人` },
    toggle,
    rows.length ? h("div.optimized-rows", {}, rows) : null,
  );
  node.dataset.key = key;
  state.optimizedEl = node;
  return node;
}

// A desk that leaves for a meeting walks to its seat (and back): a copy of its figure travels
// between the two places while the real ones fade. Purely decorative.
function walk(animate, sat) {
  for (const [key, rec] of state.deskRecs) {
    const to = rec.mood?.away || null;
    const from = state.away.get(key) ?? null;
    if (to === from) continue;
    if (to) state.away.set(key, to);
    else state.away.delete(key);
    if (!animate || !canAnimate()) continue;
    const seatOf = (id) => state.seatRecs.get(id)?.fig;
    const left = from && seatOf(from);
    const start = from ? (left?.isConnected ? left : sat.get(from)) : rec.fig;
    const end = to ? seatOf(to) : rec.fig;
    travel(start, end, rec.char);
  }
}
// start is a figure, or the last place of one that has already left the page.
function travel(start, end, name) {
  if (!start || !end?.isConnected) return;
  if (start instanceof Element && !start.isConnected) return;
  const base = el.fx.getBoundingClientRect();
  const a = start instanceof Element ? start.getBoundingClientRect() : start,
    b = end.getBoundingClientRect();
  if (!a.width || !b.width) return;
  const walker = ART.figure(name);
  walker.classList.add("walker");
  // With walking frames the figure strolls along a straight line; otherwise the seated figure hops over.
  const walks =
    !walker.classList.contains("no-walk-1") &&
    !walker.classList.contains("no-walk-2");
  if (walks) {
    walker.classList.add("walk");
    // each walker starts on a random foot so a group does not march in step
    walker.style.setProperty("--step", `${-Math.random() * 0.36}s`);
  }
  walker.style.cssText += `left:${a.left - base.left}px;top:${a.top - base.top}px;width:${a.width}px;height:${a.height}px`;
  el.fx.append(walker);
  const scale = b.width / a.width;
  const run = walker.animate(
    [
      { transform: "none", opacity: 0.95 },
      {
        transform: `translate(${(b.left - a.left) / 2}px, ${(b.top - a.top) / 2 - (walks ? 0 : 18)}px) scale(${(1 + scale) / 2})`,
        offset: 0.5,
      },
      {
        transform: `translate(${b.left - a.left}px, ${b.top - a.top}px) scale(${scale})`,
        opacity: 0.95,
      },
    ],
    walks
      ? { duration: WALK_MS, easing: "linear" }
      : { duration: 900, easing: "ease-in-out" },
  );
  // The figure at the destination waits until the walker arrives, so the desk is never shown twice.
  end.style.visibility = "hidden";
  run.onfinish = run.oncancel = () => {
    walker.remove();
    end.style.visibility = "";
  };
}

// An envelope travels between two office elements: dispatch, return, forward.
function fly(kind, from, to, delay) {
  if (!from || !to || !from.isConnected || !to.isConnected) return;
  const base = el.fx.getBoundingClientRect();
  const a = from.getBoundingClientRect(),
    b = to.getBoundingClientRect();
  if (!a.width || !b.width || !base.width) return;
  const dx = b.left + b.width / 2 - a.left - a.width / 2,
    dy = b.top + b.height / 2 - a.top - a.height / 2;
  const token = h(`span.token.token-${kind}`);
  token.style.left = `${a.left + a.width / 2 - base.left}px`;
  token.style.top = `${a.top + a.height / 2 - base.top}px`;
  el.fx.append(token);
  const centre = "translate(-50%, -50%)";
  const run = token.animate(
    [
      { transform: `${centre} scale(.5)`, opacity: 0 },
      { transform: `${centre} scale(1)`, opacity: 1, offset: 0.15 },
      {
        transform: `${centre} translate(${dx}px, ${dy - 30}px) scale(1)`,
        opacity: 1,
        offset: 0.6,
      },
      {
        transform: `${centre} translate(${dx}px, ${dy}px) scale(.6)`,
        opacity: 0,
      },
    ],
    { duration: 1100, delay, easing: "ease-in-out", fill: "backwards" },
  );
  run.onfinish = run.oncancel = () => token.remove();
}
// New messages since the previous poll tell what moved. Typing bubbles are in-place updates, not events.
function ingest(c, ctx, newGroup) {
  const fresh = [];
  for (const m of c.messages) {
    if (state.known.has(m.id)) continue;
    state.known.add(m.id);
    if (m.kind !== "typing") fresh.push(m);
  }
  if (newGroup || !state.ready || !fresh.length) return;
  const spoken = fresh.map((m) => plain(eventText(m, ctx).text));
  if (spoken.length) $("announce").textContent = spoken.join("；");
  if (fresh.length > MAX_FLIGHTS || !canAnimate()) return;
  const deskEl = (key) => state.deskRecs.get(key)?.scene;
  const roomEl = (id) => state.roomRecs.get(id)?.el;
  const boss = state.bossRec?.scene;
  fresh.forEach((m, i) => {
    const delay = i * 160;
    const t = m.thread;
    if (m.kind === "forward") fly("forward", boss, roomEl(t?.id), delay);
    else if (m.speaker?.role === "manager") {
      if (t?.kind === "desk") fly("dispatch", boss, deskEl(m.desk), delay);
      else if (t?.kind === "room") fly("dispatch", boss, roomEl(t.id), delay);
    } else if (m.kind === "reply" || m.kind === "notice") {
      if (t?.kind === "desk") fly("return", deskEl(m.desk), boss, delay);
      // public answers stay in the room; only private ones travel back to the boss
      else if (t?.kind === "room" && m.visibility !== "public")
        fly("return", roomEl(t.id), boss, delay);
    }
  });
}

// ---------------------------------------------------------------- tiny safe Markdown
// Code fences, headings, lists, `code`, **bold**. Everything else is text. No HTML is ever parsed.
function inline(parent, text) {
  let last = 0;
  for (const m of text.matchAll(/`[^`\n]+`|\*\*[^*\n]+\*\*/g)) {
    if (m.index > last) parent.append(text.slice(last, m.index));
    parent.append(
      m[0][0] === "`"
        ? h("code", {}, m[0].slice(1, -1))
        : h("strong", {}, m[0].slice(2, -2)),
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) parent.append(text.slice(last));
  return parent;
}
function renderMarkdown(root, text) {
  const raw = String(text || "").trim();
  const candidate = raw.replace(/^```(?:json)?\s*\n([\s\S]*?)\n```$/i, "$1");
  if (/^[\[{]/.test(candidate)) {
    try {
      JSON.parse(candidate);
      root.replaceChildren(h("pre", {}, h("code", {}, candidate)));
      return;
    } catch {
      /* Incomplete JSON remains ordinary message content. */
    }
  }
  const lines = String(text || "")
    .replace(/\r/g, "")
    .split("\n");
  const out = [];
  let para = [],
    list = null;
  const flush = () => {
    if (para.length) out.push(inline(h("p"), para.join("\n")));
    para = [];
    list = null;
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    let m;
    if (/^\s*```/.test(line)) {
      flush();
      const code = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i]))
        code.push(lines[i]);
      out.push(h("pre", {}, h("code", {}, code.join("\n"))));
    } else if ((m = /^\s{0,3}#{1,6}\s+(.*)$/.exec(line))) {
      flush();
      out.push(inline(h("p.md-h"), m[1]));
    } else if ((m = /^\s*([-*]|\d+\.)\s+(.*)$/.exec(line))) {
      const ordered = /\d/.test(m[1]);
      if (!list || list.ordered !== ordered) {
        flush();
        list = { ordered, node: h(ordered ? "ol" : "ul") };
        out.push(list.node);
      }
      list.node.append(inline(h("li"), m[2]));
    } else if (!line.trim()) flush();
    else {
      if (list) list = null;
      para.push(line);
    }
  }
  flush();
  root.replaceChildren(...out);
}

// ---------------------------------------------------------------- reading a message
// ctx = lookup tables of the payload being rendered (see contextOf).
const isBoss = (m) => m.speaker?.role === "manager";
function speakerDesk(m, ctx) {
  return m.desk && ctx.desks.get(m.desk);
}
function speakerName(m, ctx) {
  const s = m.speaker || {};
  if (s.role === "system") return s.label || "系统";
  if (isBoss(m)) return "boss";
  const d = speakerDesk(m, ctx);
  return d ? deskName(d) : AN.modelName(s.model) || cliName(s.provider) || "组员";
}
// CLI and role after the name, so several "默认模型" desks stay apart.
function speakerSub(m, ctx) {
  const s = m.speaker || {};
  if (s.role === "system" || isBoss(m)) return "";
  const d = speakerDesk(m, ctx);
  const provider = d?.provider || s.provider;
  return [cliName(provider), d?.role].filter(Boolean).join(" · ");
}
function speakerFace(m, ctx) {
  const s = m.speaker || {};
  if (s.role === "system") return null;
  if (isBoss(m)) return hostTile(ctx.c.host, ctx.c.label);
  const d = speakerDesk(m, ctx);
  return ART.head(d ? deskChar(d) : ART.character(s.provider, s.model), "idle");
}
// Recipients as chips. New payloads carry `to: [{key, handle}]`; older ones only `recipients`.
function targets(m, ctx) {
  if (Array.isArray(m.to) && m.to.length && typeof m.to[0] === "object")
    return m.to.map((t) => {
      if (t.key === "@boss") return { kind: "boss" };
      if (t.key === "@all") return { kind: "all" };
      const d = ctx.desks.get(t.key);
      return { kind: "desk", d, handle: t.handle };
    });
  const list = (m.recipients || []).map((r) => {
    const key = r.seat ? ctx.seatDesk.get(`${m.conversation}|${r.seat}`) : null;
    const d =
      (key && ctx.desks.get(key)) ||
      (m.thread?.kind === "desk" ? ctx.desks.get(m.desk) : null);
    return { kind: "desk", d, handle: d ? null : AN.modelName(r.model) || r.label };
  });
  const room = m.thread?.kind === "room" && ctx.rooms.get(m.thread.id);
  if (room && list.length > 1 && list.length === room.seats.length)
    return [{ kind: "all" }];
  return list;
}
function chip(t) {
  if (t.kind === "boss") return h("span.at.boss", {}, "@boss");
  if (t.kind === "all")
    return h("span.at.all", { title: "会议室里的所有人" }, "@all");
  const name = t.d ? deskName(t.d) : (t.handle || "").replace(/^@/, "");
  return h(
    "span.at",
    { title: t.d ? handleOf(t.d) : t.handle || "" },
    t.d ? ART.head(deskChar(t.d)) : null,
    name,
    t.d?.role ? ` · ${t.d.role}` : "",
  );
}
// Like an office chat: what boss sends carries a kind tag (任务, 议题, 私下交代, 转发) and its
// recipients; a reply sits under it with no verb, plus a badge only when it needs attention.
// Returns [tag, recipients, note, badge].
function verbOf(m, ctx) {
  const t = m.thread;
  const room = t?.kind === "room" ? ctx.rooms.get(t.id) : null;
  if (m.kind === "typing") return ["", [], "正在输入…", ""];
  if (m.kind === "instruction") return ["私下交代", targets(m, ctx), "", "仅 TA 可见"];
  if (m.kind === "forward") return ["转发", targets(m, ctx)];
  if (isBoss(m)) {
    if (t?.kind !== "room") return ["任务", targets(m, ctx)];
    const round = m.round != null ? `第 ${m.round} 轮` : "";
    const where = state.drawer.kind === "room" || !room ? round : `「${room.title}」${round}`;
    return ["议题", targets(m, ctx), where];
  }
  // Kept from the other seats in review: a reply only boss sees, never deleted.
  if (m.kept_private) return ["", [], "", "未公开 · 仅 boss 可见"];
  if (m.status === "review") return ["", [], "", "待 boss 审核"];
  // A parallel round shows the answers to the other seats once every seat has answered.
  if (t?.kind === "room" && m.visibility !== "public") return ["", [], "", "本轮结束后公开"];
  return ["", []];
}
function metaOf(m, ctx) {
  if (Array.isArray(m.answering) && m.answering.length)
    return `本轮回答：${m.answering.map((x) => String(x).replace(/^@/, "")).join("、")}`;
  if (m.status === "review" && !m.kept_private)
    return "boss 公开后，会议室里的其他人才看得到";
  return "";
}
// One-line ticker / screen-reader sentence for a message.
function eventText(m, ctx) {
  const who = speakerName(m, ctx);
  const room = m.thread?.kind === "room" ? ctx.rooms.get(m.thread.id) : null;
  const to = targets(m, ctx)
    .map((t) =>
      t.kind === "desk"
        ? t.d
          ? deskName(t.d)
          : (t.handle || "").replace(/^@/, "")
        : t.kind,
    )
    .join("、");
  const text =
    m.kind === "instruction"
      ? `boss 私下交代 ${to}`
      : m.kind === "forward"
        ? `boss 转发了一条消息${room ? `到「${room.title}」` : ""}`
        : isBoss(m)
          ? room
            ? `boss 在「${room.title}」提出议题`
            : `boss 给 ${to || "工位"} 布置了任务`
          : m.kind === "notice"
            ? `${who} ${m.text}`
            : m.status === "review"
              ? `${who} 提交了回答，等 boss 审核`
              : room
                ? `${who} 在「${room.title}」回复了议题`
                : `${who} 回复了任务`;
  return { who, text };
}

// ---------------------------------------------------------------- message nodes
function createMessage(m) {
  const rec = { kind: m.kind, text: null };
  rec.time = h("time");
  rec.ava = h("span.m-ava");
  rec.name = h("b.m-name");
  rec.who = h("span.m-who");
  if (m.kind === "notice") {
    rec.text_ = h("span.note-text");
    rec.detail = h("small.note-detail");
    rec.el = h(
      "div.note",
      {},
      rec.ava,
      rec.name,
      rec.who,
      rec.text_,
      rec.time,
      rec.detail,
    );
    return rec;
  }
  rec.verb = h("span.m-verb");
  rec.to = h("span.m-to");
  rec.verb2 = h("span.m-verb");
  rec.lock = h("span.m-lock");
  rec.fwd = h("button.fwd", { type: "button", hidden: true });
  rec.fwd.addEventListener(
    "click",
    () => rec.fwdId && jumpToMessage(rec.fwdId),
  );
  rec.md = h("div.md");
  rec.dots = h("span.typing", {}, h("i"), h("i"), h("i"));
  rec.tail = h("div.tail");
  rec.status = h("div.typing-status", { hidden: true });
  rec.bub = h(
    "div.bub",
    {},
    m.kind === "typing" ? [rec.tail, rec.status, rec.dots] : [rec.fwd, rec.md],
  );
  rec.meta = h("div.m-meta");
  rec.more = h("button.more", { type: "button", hidden: true }, "展开全文");
  rec.more.addEventListener("click", () => {
    const id = rec.id;
    if (state.expanded.has(id)) state.expanded.delete(id);
    else state.expanded.add(id);
    applyFold(rec);
  });
  rec.el = h(
    "article.msg",
    {},
    h(
      "div.m-h",
      {},
      rec.ava,
      rec.name,
      rec.who,
      rec.verb,
      rec.to,
      rec.verb2,
      rec.lock,
      rec.time,
    ),
    rec.bub,
    rec.meta,
    m.kind === "typing" ? null : rec.more,
  );
  return rec;
}
function setFace(rec, m, ctx) {
  const s = m.speaker || {};
  const key = `${s.role}|${s.provider}|${s.model}|${m.desk}|${isBoss(m) ? ctx.c.host : ""}|${ART.version()}`;
  if (rec.avKey === key) return;
  rec.avKey = key;
  const face = speakerFace(m, ctx);
  rec.ava.replaceChildren(...(face ? [face] : []));
}
// A long run's tail can sit unchanged for minutes: say what the CLI is doing and how long the
// reply has been quiet, so a thinking CLI does not look stuck.
const STALE_TAIL_MS = 60_000;
// Codex names its built-in tools by item type; other CLIs report readable tool names.
const TOOL_WORDS = {
  command_execution: "正在运行命令",
  file_change: "正在修改文件",
  web_search: "正在搜索网页",
};
function typingStatus(m) {
  const d = m.doing || "";
  const tool = d.slice(5);
  const doing = d === "thinking"
    ? "思考中"
    : d === "retrying"
      ? "重试中"
      : d.startsWith("tool:")
        ? TOOL_WORDS[tool] || `正在使用 ${tool || "工具"}`
        : "";
  const quiet = m.text && m.output_at && Date.now() - Date.parse(m.output_at) >= STALE_TAIL_MS;
  return [doing, quiet ? `上次输出 ${relTime(m.output_at)}` : ""].filter(Boolean).join(" · ");
}
function updateMessage(rec, m, ctx, id) {
  rec.id = id;
  const flashing = state.flashId === id ? " flash" : "";
  setAttr(rec.el, "data-id", id);
  setAttr(rec.el, "data-conv", m.conversation);
  setAttr(rec.el, "data-mid", m.message_id);
  setAttr(rec.el, "data-kind", m.kind);
  setFace(rec, m, ctx);
  setText(rec.name, speakerName(m, ctx));
  setText(rec.who, speakerSub(m, ctx));
  setText(rec.time, m.kind === "typing" ? "" : clock(m.time));
  setAttr(rec.time, "title", m.kind === "typing" ? null : fullTime(m.time));
  if (m.kind === "notice") {
    const [, tone] = pick(AN.MEMBER, m.status);
    setClass(rec.el, `note tone-${tone}${flashing}`);
    setText(rec.text_, m.text);
    setText(rec.detail, m.detail || "");
    return;
  }
  const [verb, to, verb2 = "", lock = ""] = verbOf(m, ctx);
  setClass(rec.verb, verb ? "m-verb m-tag" : "m-verb");
  const priv =
    m.kind === "instruction" ||
    (m.thread?.kind === "room" && m.visibility !== "public" && !isBoss(m));
  setClass(
    rec.el,
    `msg${isBoss(m) ? " boss" : ""}${priv ? " private" : ""}${m.status === "review" ? " pending" : ""}${m.reply_to && !isBoss(m) ? " reply" : ""}${m.kind === "typing" ? " typing-msg" : ""}${m.kind === "forward" ? " forward" : ""}${flashing}`,
  );
  setText(rec.verb, verb);
  setText(rec.verb2, verb2);
  setText(rec.lock, lock);
  const toKey = JSON.stringify([ART.version(), ...to.map((t) => [t.kind, t.d?.key, t.d ? deskName(t.d) : t.handle])]);
  if (rec.toKey !== toKey) {
    rec.toKey = toKey;
    rec.to.replaceChildren(...to.map(chip));
  }
  setText(rec.meta, metaOf(m, ctx));
  if (m.kind !== "typing") {
    // A forwarded entry quotes a message by id; the card jumps to it while that message is still listed.
    const f = m.forward;
    rec.fwd.hidden = !f;
    if (f) {
      rec.fwdId = ctx.sources.has(f.message_id) ? f.message_id : null;
      setText(rec.fwd, `↪ 转发自 ${f.source || "其他消息"}`);
      setClass(rec.fwd, rec.fwdId ? "fwd" : "fwd dead");
      setAttr(rec.fwd, "aria-disabled", rec.fwdId ? null : "true");
      setAttr(
        rec.fwd,
        "title",
        rec.fwdId ? "跳到原消息" : "原消息不在当前列表中",
      );
    }
  }
  if (m.kind === "typing") {
    const status = typingStatus(m);
    setText(rec.status, status);
    rec.status.hidden = !status;
  }
  if (rec.text !== m.text) {
    rec.text = m.text;
    if (m.kind === "typing") {
      setText(rec.tail, m.text);
      rec.dots.hidden = !!m.text;
      rec.tail.hidden = !m.text;
    } else {
      renderMarkdown(rec.md, m.text);
      rec.dirty = true; // height changed: measure again
    }
  }
}
// Fold: clamp by CSS, show the button only when the content really overflows. Discarded answers
// start folded whatever their length.
function applyFold(rec) {
  if (!rec.more || !rec.el.isConnected || !rec.el.offsetParent) return;
  const open = state.expanded.has(rec.id);
  rec.bub.classList.toggle("fold", !open);
  const overflows = rec.bub.scrollHeight > FOLD_PX + 6; // scrollHeight ignores the clamp
  rec.more.hidden = !overflows;
  rec.more.textContent = open ? "收起" : "展开全文";
  rec.more.setAttribute("aria-expanded", open ? "true" : "false");
  rec.bub.classList.toggle("fold", !open && overflows);
}
function measureAll() {
  for (const rec of state.nodes.values()) if (rec.more) applyFold(rec);
}

// ---------------------------------------------------------------- conversation column
function openScope(scope, opener) {
  if (opener) state.opener = opener;
  state.drawer = scope;
  state.convOpen = true;
  setCollapsed(false); // picking a desk or room is asking to read it
  if (state.chat) renderAll(state.chat);
  if (overlay.matches) el.tl.focus({ preventScroll: true });
}
function closeConv() {
  state.convOpen = false;
  if (state.chat) renderAll(state.chat);
  const opener = state.opener;
  state.opener = null;
  if (opener?.isConnected) opener.focus({ preventScroll: true });
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && overlay.matches && state.convOpen) closeConv();
});
$("dr-close").addEventListener("click", closeConv);
function setCollapsed(on) {
  if (state.convCollapsed === on) return;
  state.convCollapsed = on;
  store(COLLAPSED_KEY, on ? "1" : null);
}
el.convToggle.addEventListener("click", () => {
  setCollapsed(!state.convCollapsed);
  if (state.chat) renderAll(state.chat);
});

// Drag the borders between the three columns. Each width is a share of the window, kept between
// a minimum and a maximum so the office in the middle never gets squeezed out.
const SPLITS = {
  side: { key: "agentnave.office.side", min: 0.14, max: 0.28, floor: 200, css: "--side-w" },
  conv: { key: "agentnave.office.conv", min: 0.22, max: 0.42, floor: 300, css: "--conv-w" },
};
function applySplit(name, share) {
  const s = SPLITS[name];
  const handle = el.app.querySelector(`[data-split="${name}"]`);
  if (share == null) {
    el.app.style.removeProperty(s.css);
    handle.removeAttribute("aria-valuenow");
    return;
  }
  const clamped = Math.min(s.max, Math.max(s.min, share));
  el.app.style.setProperty(s.css, `max(${s.floor}px, ${(clamped * 100).toFixed(2)}%)`);
  handle.setAttribute("aria-valuenow", String(Math.round(clamped * 100)));
  return clamped;
}
for (const [name, s] of Object.entries(SPLITS)) {
  const handle = el.app.querySelector(`[data-split="${name}"]`);
  handle.setAttribute("aria-valuemin", String(s.min * 100));
  handle.setAttribute("aria-valuemax", String(s.max * 100));
  const saved = Number(stored(s.key));
  if (saved) applySplit(name, saved);
  const shareAt = (x) => {
    const r = el.app.getBoundingClientRect();
    return (name === "side" ? x - r.left : r.right - x) / r.width;
  };
  const current = () => {
    const col = name === "side" ? handle.parentElement : el.drawer;
    return col.getBoundingClientRect().width / el.app.getBoundingClientRect().width;
  };
  const save = (share) => store(s.key, share == null ? null : share.toFixed(4));
  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    el.app.classList.add("resizing");
    let last = null;
    const move = (ev) => { last = applySplit(name, shareAt(ev.clientX)); };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      el.app.classList.remove("resizing");
      if (last != null) save(last);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("lostpointercapture", end, { once: true });
  });
  // Double-click returns to the default width.
  handle.addEventListener("dblclick", () => { applySplit(name, null); save(null); });
  handle.addEventListener("keydown", (e) => {
    const step = { ArrowLeft: -0.01, ArrowRight: 0.01 }[e.key];
    if (!step) return;
    e.preventDefault();
    save(applySplit(name, current() + (name === "side" ? step : -step)));
  });
}
el.drAll.addEventListener("click", () => openScope({ kind: "all" }));

// Messages of the open scope. A desk's conversation is its own messages (private and meeting) plus
// the boss prompts that were addressed to it.
function inScope(m, ctx) {
  const d = state.drawer;
  if (d.kind === "all") return true;
  if (d.kind === "room") return m.conversation === d.id;
  if (m.desk === d.key) return true;
  if (Array.isArray(m.to) && m.to.some((t) => t?.key === d.key)) return true;
  return (m.recipients || []).some(
    (r) => r.seat && ctx.seatDesk.get(`${m.conversation}|${r.seat}`) === d.key,
  );
}
function scopeHead(c, ctx, shown) {
  const d = state.drawer;
  const real = shown.filter((m) => m.kind !== "typing").length;
  if (d.kind === "desk") {
    const desk = ctx.desks.get(d.key);
    if (desk)
      return {
        key: `desk|${d.key}|${deskChar(desk)}`,
        ico: () => ART.head(deskChar(desk)),
        title: deskName(desk),
        sub: `${cliName(desk.provider)}${desk.role ? ` · ${desk.role}` : ""} · ${real} 条消息`,
        note: "boss 布置给 TA 的任务和 TA 的回复只有你们俩看得到；TA 在会议里的回复也列在这里。",
      };
  }
  if (d.kind === "room") {
    const room = ctx.rooms.get(d.id);
    if (room)
      return {
        key: `room|${d.id}`,
        ico: () => h("span", {}, "会"),
        title: room.title,
        sub: `会议室 · ${room.seats.length} 位 · ${roundText(room)}`,
        note: "议题和回复所有参会者都能看到；私下交代只给某一位；待审核的回答要 boss 公开后别人才看得到。",
      };
  }
  return {
    key: `all|${c.host}|${c.label}`,
    ico: () => hostTile(c.host, c.label),
    title: "boss",
    sub: `${c.label} · 全部消息 ${real} 条`,
    note: "",
  };
}
// Like threads in an office chat: each reply sits under the task or agenda item it answers, so
// a meeting's answers never land under an unrelated task. Parents keep their time order; a reply
// whose parent is not listed stays in time order on its own.
function threaded(list) {
  const listed = new Set(list.map((m) => m.id));
  const replies = new Map();
  const top = [];
  for (const m of list) {
    if (m.reply_to && !isBoss(m) && listed.has(m.reply_to)) {
      if (!replies.has(m.reply_to)) replies.set(m.reply_to, []);
      replies.get(m.reply_to).push(m);
    } else top.push(m);
  }
  return top.flatMap((m) => [m, ...(replies.get(m.id) || [])]);
}
function renderConv(c, ctx) {
  el.drawer.hidden = overlay.matches ? !state.convOpen : state.convCollapsed;
  // The fold button lives on the floor's right edge; the overlay has its own close button.
  el.convToggle.hidden = overlay.matches;
  const shut = state.convCollapsed;
  setText(el.convToggle, shut ? "‹" : "›");
  setAttr(el.convToggle, "aria-expanded", shut ? "false" : "true");
  setAttr(el.convToggle, "aria-label", shut ? "展开对话栏" : "收起对话栏");
  setAttr(el.convToggle, "title", shut ? "展开对话栏" : "收起对话栏");
  if (el.drawer.hidden) {
    state.scopeKey = "";
    return;
  }
  // a selection that no longer exists falls back to the whole workspace
  const d = state.drawer;
  if (
    (d.kind === "desk" && !ctx.desks.has(d.key)) ||
    (d.kind === "room" && !ctx.rooms.has(d.id))
  )
    state.drawer = { kind: "all" };
  const scopeKey = c.group + JSON.stringify(state.drawer);
  const reset = state.scopeKey !== scopeKey;
  if (reset) {
    state.scopeKey = scopeKey;
    state.nodes.clear();
    state.seenIds.clear();
    state.unseen = 0;
    state.expanded.clear();
    el.inner.replaceChildren();
    el.tl.scrollTop = 0;
  }
  const shown = threaded(c.messages.filter((m) => inScope(m, ctx)));
  const head = scopeHead(c, ctx, shown);
  if (el.drIco.dataset.key !== head.key) {
    el.drIco.dataset.key = head.key;
    el.drIco.replaceChildren(head.ico());
  }
  setText(el.drTitle, head.title);
  setText(el.drSub, head.sub);
  setText(el.drNote, head.note);
  el.drNote.hidden = !head.note;
  el.drAll.hidden = state.drawer.kind === "all";
  setAttr(el.drawer, "aria-label", `${head.title} 的对话`);
  // Geometry is the truth: a reader who is at the bottom follows new content, anyone else is left alone.
  const follow = reset || nearBottom();
  const order = [];
  let day = null,
    round = null;
  const node = (id, make) => {
    let rec = state.nodes.get(id);
    if (!rec) state.nodes.set(id, (rec = make()));
    order.push(id);
    return rec;
  };
  const separator = (id, text) => {
    const rec = node(id, () => ({ el: h("div.sep", {}, ""), sep: true }));
    setText(rec.el, text);
  };
  if (c.omitted && state.drawer.kind === "all")
    separator("omitted", `更早的 ${c.omitted} 条没有列出`);
  let fresh = 0;
  for (const m of shown) {
    const date = new Date(AN.at(m.time)).toDateString();
    // a thread stays together: only messages that start one open a new day
    const inThread = m.reply_to && !isBoss(m) && order.includes(m.reply_to);
    if (date !== day && !inThread) {
      day = date;
      round = null;
      separator(`day:${m.id}`, dayLabel(m.time));
    }
    if (state.drawer.kind === "room" && m.round != null && m.round !== round) {
      round = m.round;
      separator(`round:${m.conversation}:${m.round}`, `第 ${m.round} 轮`);
    }
    let rec = state.nodes.get(m.id);
    if (rec && rec.kind !== m.kind) {
      state.nodes.delete(m.id);
      rec = null;
    }
    rec = node(m.id, () => createMessage(m));
    updateMessage(rec, m, ctx, m.id);
    if (!state.seenIds.has(m.id)) {
      state.seenIds.add(m.id);
      if (state.ready && !reset && m.kind !== "typing") fresh++;
    }
  }
  if (!shown.length)
    node("empty", () => ({
      el: h("div.dr-empty", {}, "这里还没有消息。"),
      sep: true,
    }));
  const keep = new Set(order);
  for (const id of state.nodes.keys())
    if (!keep.has(id)) state.nodes.delete(id);
  reconcile(
    el.inner,
    [...keep].map((id) => state.nodes.get(id).el),
  );
  for (const rec of state.nodes.values())
    if (rec.dirty) {
      rec.dirty = false;
      applyFold(rec);
    }

  if (follow) {
    el.tl.scrollTop = el.tl.scrollHeight;
    state.unseen = 0;
  } else if (fresh) state.unseen += fresh;
  updateJump();
}
function updateJump() {
  el.jump.hidden = !(state.unseen > 0 && !nearBottom());
  el.jump.textContent = `${state.unseen} 条新消息 ↓`;
}
el.tl.addEventListener(
  "scroll",
  () => {
    if (nearBottom()) state.unseen = 0;
    updateJump();
  },
  { passive: true },
);
el.jump.addEventListener("click", () => {
  el.tl.scrollTo({ top: el.tl.scrollHeight, behavior: "smooth" });
});
window.addEventListener("resize", () => measureAll());
if (document.fonts?.ready) document.fonts.ready.then(measureAll);

// The poll rewrites class names, so the highlight is kept in state until its animation is over.
function flash(target) {
  target.scrollIntoView({
    block: "center",
    behavior: reduceMotion.matches ? "auto" : "smooth",
  });
  state.flashId = target.dataset.id;
  target.classList.remove("flash");
  void target.offsetWidth;
  target.classList.add("flash");
  clearTimeout(state.flashTimer);
  state.flashTimer = setTimeout(() => {
    state.flashId = null;
  }, 1700);
}
// Jump to the message a forward quotes. The original may belong to another desk or room, so widen the
// scope to the whole workspace when it is not in the current one.
function jumpToMessage(mid) {
  const find = () =>
    [...el.inner.querySelectorAll("[data-mid]")]
      .filter((n) => n.dataset.mid === mid && n.dataset.kind !== "forward")
      .pop();
  let target = find();
  if (!target) {
    state.drawer = { kind: "all" };
    renderAll(state.chat);
    target = find();
  }
  if (target) flash(target);
}
// Scroll to the latest stretch of one conversation (links to it use #conv:<id>).
function focusConversation(id) {
  const target = [...el.inner.querySelectorAll("[data-conv]")]
    .filter((n) => n.dataset.conv === id)
    .pop();
  if (target) flash(target);
}

// ---------------------------------------------------------------- ticker
function renderTicker(c, ctx) {
  const last = [...c.messages].reverse().find((m) => m.kind !== "typing");
  el.ticker.hidden = !last;
  if (!last || state.tickId === last.id) return;
  state.tickId = last.id;
  const { text } = eventText(last, ctx);
  el.tick.replaceChildren(h("time", {}, clock(last.time)), text);
  el.tick.classList.remove("new");
  void el.tick.offsetWidth;
  el.tick.classList.add("new");
  setAttr(el.tick, "title", `${fullTime(last.time)} ${text}`);
}

// ---------------------------------------------------------------- body states
function showBlank(kind) {
  const none = !kind;
  el.blank.hidden = none;
  el.office.hidden = !none;
  if (none) return;
  el.head.hidden = true;
  el.drawer.hidden = true;
  el.convToggle.hidden = true;
  el.ticker.hidden = true;
  state.headKey = "";
  state.loaded = null;
  state.drawer = { kind: "all" };
  const text = {
    nogroups: [
      "还没有工作区",
      "在 boss（Claude Code、Codex CLI 等宿主里的 Agent）里派出任务或开始会议，对应的办公室会自动出现在这里。",
    ],
    pick: [
      "选一个工作区开始查看",
      "左侧是每个 boss 会话的工作区，进行中的排在最前面。",
    ],
    missing: ["找不到这个工作区", "它可能已被删除。"],
    loading: ["正在读取办公室…", ""],
    error: ["无法读取办公室", "正在重试，请稍候。"],
  }[kind];
  el.blank.replaceChildren(
    ...[
      h("strong", {}, text[0]),
      h("p", { hidden: kind === "nogroups" && ob.visible }, text[1]),
      kind === "missing" &&
        h("a.blank-link", { href: href("/chat/") }, "回到工作区列表"),
    ].filter(Boolean),
  );
  el.jump.hidden = true;
  placeOb();
}

// ---------------------------------------------------------------- refresh
function contextOf(c) {
  const seatDesk = new Map();
  for (const r of c.rooms)
    for (const s of r.seats) seatDesk.set(`${r.id}|${s.seat}`, s.desk);
  return {
    c,
    seatDesk,
    // Drawn desks plus the listed "optimized" ones, so their messages and links still resolve.
    desks: new Map([...c.desks, ...(c.optimized?.desks || [])].map((d) => [d.key, d])),
    drawn: new Set(c.desks.map((d) => d.key)),
    // a thread desk's earlier task keys, from before it had a native session
    aliases: new Map(
      [...c.desks, ...(c.optimized?.desks || [])].flatMap((d) => (d.aliases || []).map((a) => [a, d.key])),
    ),
    rooms: new Map(c.rooms.map((r) => [r.id, r])),
    fullRooms: new Set(c.rooms.filter(fullRoom).map((r) => r.id)),
    sources: new Set(
      c.messages
        .filter((m) => m.message_id && m.kind !== "forward")
        .map((m) => m.message_id),
    ),
  };
}
// A task desk that gained its native session comes back under its thread key: keep it open where it was.
function followAlias(c, ctx) {
  const d = state.drawer;
  if (d.kind !== "desk" || ctx.desks.has(d.key) || !ctx.aliases.has(d.key)) return;
  const moved = { kind: "desk", key: ctx.aliases.get(d.key) };
  if (state.scopeKey === c.group + JSON.stringify(d)) state.scopeKey = c.group + JSON.stringify(moved);
  state.drawer = moved;
}
function renderAll(c, newGroup = false) {
  const ctx = (state.ctx = contextOf(c));
  followAlias(c, ctx);
  const n = counts(c);
  renderHead(c, n);
  renderOffice(c, ctx, n, newGroup);
  renderConv(c, ctx);
  renderTicker(c, ctx);
}
// #desk:<key> opens a desk's conversation, #conv:<id> a room's transcript (or everything, for a task).
function applyHash(ctx) {
  const key = state.hashKey;
  if (!key || state.focused) return;
  state.focused = true;
  if (key.startsWith("desk:")) {
    // A link made while a task had no native session yet names its earlier task key.
    const desk = ctx.aliases.get(key.slice(5)) ?? key.slice(5);
    if (ctx.desks.has(desk)) openScope({ kind: "desk", key: desk });
  } else if (key.startsWith("conv:")) {
    const id = key.slice(5);
    openScope(ctx.rooms.has(id) ? { kind: "room", id } : { kind: "all" });
    setTimeout(() => focusConversation(id), 0);
  }
}

async function refresh() {
  if (state.refreshing) {
    state.again = true;
    return;
  }
  state.refreshing = true;
  try {
    let chats;
    try {
      chats = await load("/app/chats");
    } catch {
      el.banner.hidden = false;
      if (!state.loaded && !state.groupsData.length) showBlank("error");
      return;
    }
    el.banner.hidden = true;
    state.groupsData = chats.groups;
    placeOb();
    if (
      state.auto &&
      !narrow.matches &&
      !chats.groups.some((g) => g.group === state.selected)
    )
      state.selected = chats.groups[0]?.group ?? null;
    if (!narrow.matches && state.selected) el.app.dataset.view = "chat";
    renderList();
    if (!state.selected) {
      showBlank(chats.groups.length ? "pick" : "nogroups");
      return;
    }
    const selected = state.selected;
    const hashKey = state.hashKey;
    const focus = hashKey?.startsWith("conv:") ? hashKey.slice(5) : null;
    if (state.loaded !== selected) showBlank("loading");
    let chat;
    try {
      chat = await load(
        `/app/chats/${encodeURIComponent(selected)}${focus ? `/focus/${encodeURIComponent(focus)}` : ""}`,
      );
      if (selected !== state.selected || hashKey !== state.hashKey) {
        state.again = true;
        return;
      }
    } catch (e) {
      if (selected !== state.selected || hashKey !== state.hashKey) {
        state.again = true;
        return;
      }
      if (e.status === 404) showBlank("missing");
      else {
        el.banner.hidden = false;
        if (!state.loaded) showBlank("error");
      }
      return;
    }
    const newGroup = state.loaded !== chat.group;
    state.loaded = chat.group;
    if (newGroup) {
      state.drawer = { kind: "all" };
      state.convOpen = false;
      state.opener = null;
      state.scopeKey = "";
      state.tickId = null;
    }
    state.chat = chat;
    showBlank(null);
    renderAll(chat, newGroup);
    ingest(chat, state.ctx, newGroup);
    applyHash(state.ctx);
    renderList(); // selected marker
    state.ready = true;
  } finally {
    state.refreshing = false;
    if (state.again) {
      state.again = false;
      queueMicrotask(refresh);
    }
  }
}

for (const link of document.querySelectorAll("a[href]"))
  setAttr(link, "href", href(link.getAttribute("href")));
sync();
poll(refresh);

ob.start();
