"use strict";
// App office: one workspace per manager session. Left: workspace list (running pinned). Right: the
// selected workspace as a read-only office -- the manager's desk, one desk per (CLI, model, role) and
// one meeting room per saved group. A drawer shows messages: the whole workspace's fused timeline, one
// desk's history or one room's transcript. Everything is patched in place (list items keyed by group id,
// desks by desk key, rooms by room id, message nodes by message id) so a 1.5 s poll never rebuilds the
// DOM, never flickers and never moves the scroll position of a reader who scrolled up. Movement
// (dispatch, return, forward, meeting seats) is inferred from the difference between two polls.
const {
  h,
  $,
  load,
  href,
  GROUP,
  pick,
  urgent,
  deskState,
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
} = AN;

const mockOn = !!(window.AgentNaveMock && window.AgentNaveMock.enabled());
const narrow = window.matchMedia("(max-width: 760px)");
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const FOLD_PX = 280; // a bubble taller than this is folded behind "展开全文"
const NEAR_BOTTOM = 64; // within this distance of the bottom the reader "follows" the chat
const MAX_FLIGHTS = 8; // more new messages than this between two polls are not animated

const nearBottom = () =>
  el.tl.scrollHeight - el.tl.scrollTop - el.tl.clientHeight < NEAR_BOTTOM;
const el = {
  app: $("app"),
  list: $("list"),
  head: $("head"),
  office: $("office"),
  mgrDesk: $("mgr-desk"),
  desks: $("desks"),
  rooms: $("rooms"),
  fx: $("fx"),
  deskCount: $("desks-count"),
  roomCount: $("rooms-count"),
  drawer: $("drawer"),
  drTitle: $("dr-title"),
  drSub: $("dr-sub"),
  tl: $("timeline"),
  inner: $("tl-inner"),
  blank: $("blank"),
  jump: $("jump"),
  ro: $("ro"),
  banner: $("banner"),
  count: $("side-count"),
};
const state = {
  selected: null, // workspace id shown on the right
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
  deskRecs: new Map(),
  roomRecs: new Map(),
  seatRecs: new Map(),
  pawns: new Map(),
  known: new Set(),
  mgr: null,
  // drawer: null | {kind:"all"} | {kind:"desk", key} | {kind:"room", id}
  drawer: null,
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
narrow.addEventListener("change", () => {
  sync();
  renderList();
  placeOb();
  measureAll();
});

// ---------------------------------------------------------------- workspace list
function sectionEl(id, text) {
  return (state.sections[id] ||= h("div.gl-sec", {}, text));
}
function createItem(g) {
  const presence = h("i.presence");
  const rec = {
    id: g.group,
    presence,
    label: h("b.gi-label"),
    meta: h("span.gi-meta"),
    time: h("time"),
    title: h("span.gi-title"),
    prev: h("span.gi-prev"),
    pill: h("span.pill"),
  };
  rec.el = h(
    "a.gi",
    {},
    h("span.gi-ava", {}, presence),
    h(
      "span.gi-body",
      {},
      h("span.gi-top", {}, rec.label, rec.meta, rec.time),
      rec.title,
      h("span.gi-bottom", {}, rec.prev, rec.pill),
    ),
  );
  rec.el.addEventListener("click", (e) => {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button) return;
    e.preventDefault();
    go(`/chat/${rec.id}`);
  });
  return rec;
}
function updateItem(rec, g) {
  const [word, tone] = pick(GROUP, g.status);
  const on = g.group === state.selected;
  setClass(
    rec.el,
    `gi ${colorClass(g.color)} tone-${tone}${on ? " on" : ""}${urgent(g.status) ? " hot" : ""}`,
  );
  setAttr(rec.el, "href", href(`/chat/${g.group}`));
  setAttr(rec.el, "aria-current", on ? "true" : null);
  setAttr(rec.el, "aria-label", `${g.label}，${word}，${g.title}`);
  setClass(rec.presence, g.live ? "presence on" : "presence");
  setText(rec.label, g.label);
  setText(
    rec.meta,
    `${g.desks_count ?? 0} 个工位 · ${g.rooms_count ?? 0} 间会议室`,
  );
  setText(rec.time, relTime(g.updated));
  setText(rec.title, g.title);
  setText(rec.prev, g.preview);
  rec.pill.hidden = g.status === "idle";
  setClass(rec.pill, `pill tone-${tone}`);
  setText(rec.pill, word);
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

// ---------------------------------------------------------------- workspace head
function renderHead(c) {
  const [word, tone] = pick(GROUP, c.status);
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
    const all = h(
      "button.all-btn",
      { type: "button", "aria-expanded": "false" },
      h("span", {}, "消息列表"),
      h("b.all-count"),
    );
    all.addEventListener("click", () => {
      if (state.drawer?.kind === "all") closeDrawer();
      else openDrawer({ kind: "all" }, all);
    });
    state.head = {
      back,
      ava: h("span.gh-ava", {}, h("i.presence")),
      label: h("b.gh-label"),
      pill: h("span.pill"),
      sub: h("span.gh-sub"),
      all,
    };
    el.head.replaceChildren(
      h(
        "div.gh-row",
        {},
        state.head.back,
        state.head.ava,
        h(
          "div.gh-id",
          {},
          h("span.gh-line", {}, state.head.label, state.head.pill),
          state.head.sub,
        ),
        all,
      ),
    );
  }
  const t = state.head;
  setClass(el.head, `gh ${colorClass(c.color)}`);
  setClass(t.ava.lastChild, c.live ? "presence on" : "presence");
  setText(t.label, c.label);
  setClass(t.pill, `pill tone-${tone}`);
  setText(t.pill, word);
  setText(
    t.sub,
    `${c.live ? "主控在线" : "主控离线"} · ${c.desks.length} 个工位 · ${c.rooms.length} 间会议室`,
  );
  setText(
    t.all.lastChild,
    String(c.messages.filter((m) => m.kind !== "typing").length),
  );
  setAttr(
    t.all,
    "aria-expanded",
    state.drawer?.kind === "all" ? "true" : "false",
  );
}

// ---------------------------------------------------------------- office
const canAnimate = () =>
  !reduceMotion.matches &&
  !document.hidden &&
  typeof Element.prototype.animate === "function";

function createManagerDesk() {
  const rec = {
    tile: h("span.md-tile"),
    presence: h("i.presence"),
    name: h("b.md-name"),
    sub: h("span.md-sub"),
    papers: h("span.papers", { "aria-hidden": "true" }),
    review: h("span.md-review"),
    hostKey: null,
  };
  rec.el = h(
    "button.mgr-desk",
    { type: "button" },
    h("span.md-ava", {}, rec.tile, rec.presence),
    h("span.md-body", {}, h("span.md-line", {}, rec.name, rec.sub), rec.review),
    rec.papers,
  );
  rec.el.addEventListener("click", () => openDrawer({ kind: "all" }, rec.el));
  return rec;
}
function updateManagerDesk(rec, c) {
  const pending = c.rooms.reduce((n, r) => n + (r.pending_review || 0), 0);
  const [word] = pick(GROUP, c.status);
  setClass(
    rec.el,
    `mgr-desk ${colorClass(c.color)}${pending ? " has-review" : ""}`,
  );
  const custom = !!(c.host && window.Avatars?.custom("host", c.host, ""));
  const key = `${c.host}|${custom}`;
  if (rec.hostKey !== key) {
    rec.hostKey = key;
    rec.tile.replaceChildren(
      custom
        ? Avatars.create("host", c.host, "")
        : (c.label || "?").slice(0, 1),
    );
  }
  setClass(rec.presence, c.live ? "presence on" : "presence");
  setText(rec.name, c.label);
  setText(rec.sub, `主控办公室 · ${c.live ? "在线" : "离线"} · ${word}`);
  setText(rec.review, pending ? `待审 ${pending}` : "没有待审");
  setClass(rec.review, pending ? "md-review on" : "md-review");
  const sheets = Math.min(pending, 5);
  if (rec.papers.children.length !== sheets)
    rec.papers.replaceChildren(...Array.from({ length: sheets }, () => h("i")));
  setAttr(
    rec.el,
    "aria-label",
    `${c.label}的办公桌，${pending ? `${pending} 条待审` : "没有待审"}，打开消息列表`,
  );
}

function createDesk(d) {
  const rec = {
    key: d.key,
    slot: h("span.slot"),
    name: h("b.d-name"),
    dot: h("i.dot"),
    word: h("span"),
    only: h(
      "span.tag.tag-only",
      { title: "与主控宿主同类，只参加会议，不接受私聊", hidden: true },
      "仅会议",
    ),
    access: h(
      "span.tag.tag-access",
      { title: "主控声明的访问等级，只是约定，不改变 CLI 的实际权限", hidden: true },
      "",
    ),
    priv: h(
      "span.tag.tag-priv",
      { title: "这个工位正在会议中，同时还有一条私聊在运行", hidden: true },
      "私聊进行中",
    ),
    meta: h("span.d-meta"),
  };
  rec.el = h(
    "button.desk",
    { type: "button" },
    h("span.chair", {}, rec.slot),
    h(
      "span.d-body",
      {},
      rec.name,
      h("span.d-status", {}, rec.dot, rec.word),
      h("span.d-tags", {}, rec.access, rec.only, rec.priv),
      rec.meta,
    ),
  );
  rec.el.addEventListener("click", () =>
    openDrawer({ kind: "desk", key: d.key }, rec.el),
  );
  return rec;
}
function updateDesk(rec, d, rooms) {
  const room = d.meeting.room ? rooms.get(d.meeting.room) : null;
  const s = deskState(d, room);
  setClass(
    rec.el,
    `desk tone-${s.tone}${s.working ? " working" : ""}${room ? " away" : ""}${d.private_allowed ? "" : " only"}`,
  );
  setText(rec.name, d.label);
  setText(rec.word, s.word);
  rec.only.hidden = d.private_allowed;
  setText(rec.access, d.access === "read" ? "只读" : d.access === "write" ? "可写" : "");
  rec.access.hidden = !d.access;
  rec.priv.hidden = !(room && d.private.running > 0);
  setText(
    rec.meta,
    [
      d.private.threads ? `私聊 ${d.private.threads} 条线程` : "",
      relTime(d.last_active),
    ]
      .filter(Boolean)
      .join(" · "),
  );
  setAttr(rec.el, "title", `${d.label}\n首次出现 ${fullTime(d.first_seen)}`);
  setAttr(
    rec.el,
    "aria-label",
    `${d.label}，${s.word}${d.private_allowed ? "" : "，仅会议"}${rec.priv.hidden ? "" : "，私聊进行中"}，查看历史消息`,
  );
}

function createRoom(r) {
  const rec = {
    id: r.id,
    title: h("b.r-title"),
    pill: h("span.pill"),
    seats: h("div.seats"),
    boardText: h("b.wb-count"),
    review: h("span.tag.tag-review", { hidden: true }),
  };
  rec.open = h("button.r-open", { type: "button" }, rec.title);
  rec.board = h(
    "button.whiteboard",
    { type: "button" },
    h("i.wb-ico", { "aria-hidden": "true" }),
    rec.boardText,
    rec.review,
  );
  rec.el = h(
    "article.room",
    {},
    h("header.r-head", {}, rec.open, rec.pill),
    rec.board,
    rec.seats,
  );
  for (const button of [rec.open, rec.board])
    button.addEventListener("click", () =>
      openDrawer({ kind: "room", id: r.id }, button),
    );
  return rec;
}
function seatRec(id) {
  let seat = state.seatRecs.get(id);
  if (!seat) {
    seat = { slot: h("span.slot"), cap: h("span.seat-cap") };
    seat.el = h("div.seat", {}, seat.slot, seat.cap);
    state.seatRecs.set(id, seat);
  }
  return seat;
}
function updateRoom(rec, r, desks) {
  const [word, tone] = pick(GROUP, r.status);
  setClass(rec.el, `room tone-${tone}${r.status === "running" ? " live" : ""}`);
  setText(rec.title, r.title);
  setAttr(rec.open, "title", r.title);
  setClass(rec.pill, `pill tone-${tone}`);
  setText(rec.pill, word);
  setText(rec.boardText, `白板 ${r.board} 条`);
  rec.review.hidden = !r.pending_review;
  setText(rec.review, `待审 ${r.pending_review}`);
  setAttr(
    rec.open,
    "aria-label",
    `${r.title}，${word}，白板 ${r.board} 条${r.pending_review ? `，待审 ${r.pending_review}` : ""}，查看会议室消息`,
  );
  reconcile(
    rec.seats,
    r.seats.map((seat) => {
      const s = seatRec(`${r.id}|${seat.seat}`);
      const d = desks.get(seat.desk);
      setText(s.cap, d ? d.role || d.model || d.provider : "—");
      setAttr(s.el, "title", `${d ? d.label : "—"} · 席位 ${seat.seat}`);
      return s.el;
    }),
  );
}

function pawnFor(d) {
  let pawn = state.pawns.get(d.key);
  if (!pawn)
    state.pawns.set(
      d.key,
      (pawn = { el: h("span.pawn", {}, avatar(d.provider, d.model)) }),
    );
  return pawn;
}
// Seat the avatars: in the meeting room's seat while the desk is in a meeting, otherwise at its own desk.
function seatPawns(desks, animate) {
  for (const d of desks) {
    const pawn = pawnFor(d);
    const seat =
      d.meeting.room &&
      state.seatRecs.get(`${d.meeting.room}|${d.meeting.seat}`);
    const target = seat ? seat.slot : state.deskRecs.get(d.key).slot;
    const from = pawn.el.parentNode;
    if (from === target) continue;
    const before =
      from && animate && canAnimate() ? pawn.el.getBoundingClientRect() : null;
    target.append(pawn.el);
    if (!before || !before.width) continue;
    const after = pawn.el.getBoundingClientRect();
    pawn.el.animate(
      [
        {
          transform: `translate(${before.left - after.left}px, ${before.top - after.top}px)`,
        },
        { transform: "none" },
      ],
      { duration: 700, easing: "cubic-bezier(.3,.7,.3,1)" },
    );
  }
  for (const d of desks)
    state.pawns
      .get(d.key)
      .el.classList.toggle(
        "working",
        state.deskRecs.get(d.key).el.classList.contains("working"),
      );
}

function renderOffice(c, newGroup) {
  const desks = new Map(c.desks.map((d) => [d.key, d]));
  const rooms = new Map(c.rooms.map((r) => [r.id, r]));
  if (newGroup) {
    for (const map of [
      state.deskRecs,
      state.roomRecs,
      state.seatRecs,
      state.pawns,
      state.known,
    ])
      map.clear();
    state.mgr = null;
    el.desks.replaceChildren();
    el.rooms.replaceChildren();
    el.mgrDesk.replaceChildren();
    el.office.scrollTop = 0;
  }
  if (!state.mgr) {
    state.mgr = createManagerDesk();
    el.mgrDesk.append(state.mgr.el);
  }
  updateManagerDesk(state.mgr, c);

  for (const key of state.deskRecs.keys())
    if (!desks.has(key)) {
      state.deskRecs.delete(key);
      state.pawns.delete(key);
    }
  const deskNodes = c.desks.map((d) => {
    let rec = state.deskRecs.get(d.key);
    if (!rec) state.deskRecs.set(d.key, (rec = createDesk(d)));
    updateDesk(rec, d, rooms);
    return rec.el;
  });
  if (!deskNodes.length)
    deskNodes.push(
      h(
        "div.zone-empty",
        {},
        "还没有工位。主控派出第一个私聊或会议席位后，工位会出现在这里并一直保留。",
      ),
    );
  reconcile(el.desks, deskNodes);
  setText(el.deskCount, c.desks.length ? String(c.desks.length) : "");

  for (const id of state.roomRecs.keys())
    if (!rooms.has(id)) state.roomRecs.delete(id);
  const seated = new Set();
  const roomNodes = c.rooms.map((r) => {
    let rec = state.roomRecs.get(r.id);
    if (!rec) state.roomRecs.set(r.id, (rec = createRoom(r)));
    updateRoom(rec, r, desks);
    for (const seat of r.seats) seated.add(`${r.id}|${seat.seat}`);
    return rec.el;
  });
  for (const id of state.seatRecs.keys())
    if (!seated.has(id)) state.seatRecs.delete(id);
  if (!roomNodes.length)
    roomNodes.push(
      h(
        "div.zone-empty",
        {},
        "还没有会议室。主控开始会议后，每个协作组各占一间。",
      ),
    );
  reconcile(el.rooms, roomNodes);
  setText(el.roomCount, c.rooms.length ? String(c.rooms.length) : "");

  seatPawns(c.desks, !newGroup && state.ready);
}

const GLYPH = { dispatch: "✉", return: "↩", forward: "↪", board: "✎" };
const FLIGHT_LABEL = {
  dispatch: "派单",
  return: "交回",
  forward: "转发",
  board: "写上白板",
};
// A small token travels between two office elements. Purely decorative: it takes no focus.
function fly(kind, from, to, delay) {
  if (!from || !to || !from.isConnected || !to.isConnected) return;
  const base = el.fx.getBoundingClientRect();
  const a = from.getBoundingClientRect(),
    b = to.getBoundingClientRect();
  if (!a.width || !b.width || !base.width) return;
  const dx = b.left + b.width / 2 - a.left - a.width / 2,
    dy = b.top + b.height / 2 - a.top - a.height / 2;
  const token = h(
    `span.token.token-${kind}`,
    { title: FLIGHT_LABEL[kind] },
    GLYPH[kind],
  );
  token.style.left = `${a.left + a.width / 2 - base.left}px`;
  token.style.top = `${a.top + a.height / 2 - base.top}px`;
  el.fx.append(token);
  const centre = "translate(-50%, -50%)";
  const run = token.animate(
    [
      { transform: `${centre} scale(.5)`, opacity: 0 },
      { transform: `${centre} scale(1)`, opacity: 1, offset: 0.15 },
      {
        transform: `${centre} translate(${dx}px, ${dy}px) scale(1)`,
        opacity: 1,
        offset: 0.85,
      },
      {
        transform: `${centre} translate(${dx}px, ${dy}px) scale(.6)`,
        opacity: 0,
      },
    ],
    { duration: 1200, delay, easing: "ease-in-out", fill: "backwards" },
  );
  run.onfinish = run.oncancel = () => token.remove();
}
// New messages since the previous poll tell what moved. Typing bubbles are in-place updates, not events.
function ingest(c, newGroup) {
  const fresh = [];
  for (const m of c.messages) {
    if (state.known.has(m.id)) continue;
    state.known.add(m.id);
    if (m.kind !== "typing") fresh.push(m);
  }
  if (newGroup || !state.ready || !fresh.length) return;
  const spoken = fresh
    .filter((m) => m.kind !== "prompt")
    .map(
      (m) =>
        `${who(m, state.ctx).name}：${plain(m.text).split("\n")[0].slice(0, 60)}`,
    );
  if (spoken.length) $("announce").textContent = spoken.join("；");
  if (fresh.length > MAX_FLIGHTS || !canAnimate()) return;
  const deskEl = (key) => state.deskRecs.get(key)?.el;
  const roomEl = (id) => state.roomRecs.get(id)?.el;
  const pawnEl = (key) => state.pawns.get(key)?.el;
  const mgr = state.mgr.el;
  fresh.forEach((m, i) => {
    const delay = i * 160;
    const t = m.thread;
    if (m.kind === "forward") fly("forward", mgr, roomEl(t?.id), delay);
    else if (m.kind === "prompt" && m.speaker.role === "manager") {
      if (t?.kind === "desk") fly("dispatch", mgr, deskEl(m.desk), delay);
      else if (t?.kind === "room") fly("dispatch", mgr, roomEl(t.id), delay);
    } else if (m.kind === "reply" || m.kind === "notice") {
      if (t?.kind === "desk") fly("return", deskEl(m.desk), mgr, delay);
      else if (t?.kind === "room") {
        if (m.visibility === "public")
          fly("board", pawnEl(m.desk), state.roomRecs.get(t.id)?.board, delay);
        else fly("return", pawnEl(m.desk), mgr, delay);
      }
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

// ---------------------------------------------------------------- message nodes
// ctx = lookup tables of the payload being rendered (see contextOf).
function who(m, ctx) {
  const s = m.speaker;
  if (s.role === "system") return { name: s.label || "系统", sub: "" };
  if (s.role === "manager")
    return {
      name: m.kind === "forward" ? s.label || "主控转发" : ctx.c.label,
      sub: "主控",
    };
  const desk = m.desk && ctx.desks.get(m.desk);
  return {
    name: desk ? desk.label : s.model || s.label || s.provider,
    sub: "",
  };
}
// Which thread a message belongs to: a private desk chat, or a room (public once committed to the board).
function threadTag(m, ctx) {
  if (m.private)
    return {
      text: "🔒 私密",
      title: "这条消息不会向会议内其他成员公开。",
      cls: "priv",
    };
  const t = m.thread;
  if (t?.kind === "desk")
    return {
      text: "🔒 私聊",
      title: "主控与这个工位的一对一私聊，不会向会议公开。",
      cls: "priv",
    };
  if (t?.kind === "room") {
    const title = ctx.rooms.get(t.id)?.title || "会议室";
    return m.visibility === "public"
      ? { text: title, title: `会议室：${title}`, cls: "room" }
      : {
          text: `🔒 ${title} · 未公开`,
          title: "运行中、待审或隔离席位的内容，尚未向会议公开。",
          cls: "priv",
        };
  }
  return null;
}
function createMessage(m) {
  const rec = { kind: m.kind, text: null };
  rec.thread = h("span.m-thread");
  rec.time = h("time");
  if (m.kind === "notice") {
    rec.ava = h("span.note-ava");
    rec.name = h("b");
    rec.text_ = h("span.note-text");
    rec.detail = h("small.note-detail");
    rec.el = h(
      "div.note",
      {},
      rec.ava,
      h(
        "span.note-main",
        {},
        h("span.note-line", {}, rec.name, rec.text_, rec.thread, rec.time),
        rec.detail,
      ),
    );
    return rec;
  }
  rec.avaSlot = h("span.m-ava");
  rec.name = h("b.m-name");
  rec.sub = h("span.m-sub");
  rec.to = h("div.m-to", { "aria-label": "消息收件人" });
  rec.fwd = h("button.fwd", { type: "button", hidden: true });
  rec.fwd.addEventListener(
    "click",
    () => rec.fwdId && jumpToMessage(rec.fwdId),
  );
  rec.md = h("div.md");
  rec.dots = h("span.typing", {}, h("i"), h("i"), h("i"));
  rec.tail = h("div.tail");
  rec.bub = h(
    "div.bub",
    {},
    m.kind === "typing" ? [rec.tail, rec.dots] : [rec.fwd, rec.md],
  );
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
    rec.avaSlot,
    h(
      "div.m-col",
      {},
      h("div.m-head", {}, rec.name, rec.sub, rec.thread, rec.time),
      rec.to,
      rec.bub,
      m.kind === "typing" ? null : rec.more,
    ),
  );
  return rec;
}
function updateMessage(rec, m, ctx, id) {
  rec.id = id;
  const s = m.speaker;
  const [, tone] = pick(AN.MEMBER, m.status);
  const tag = threadTag(m, ctx);
  setAttr(rec.el, "data-id", id);
  setAttr(rec.el, "data-conv", m.conversation);
  setAttr(rec.el, "data-mid", m.message_id);
  setAttr(rec.el, "data-kind", m.kind);
  setText(rec.thread, tag ? tag.text : "");
  setClass(rec.thread, `m-thread${tag ? ` ${tag.cls}` : ""}`);
  setAttr(rec.thread, "title", tag ? tag.title : null);
  rec.thread.hidden = !tag;
  const priv = tag?.cls === "priv" ? " thread-private" : "";
  const flashing = state.flashId === id ? " flash" : "";
  if (m.kind === "notice") {
    const key = s.provider + s.model;
    if (rec.avKey !== key) {
      rec.avKey = key;
      rec.ava.replaceChildren(
        s.role === "system" ? "" : avatar(s.provider, s.model, "sm"),
      );
    }
    setClass(rec.el, `note tone-${tone}${priv}${flashing}`);
    setText(rec.name, who(m, ctx).name);
    setText(rec.text_, m.text);
    setText(rec.detail, m.detail || "");
    rec.detail.hidden = !m.detail;
    setText(rec.time, clock(m.time));
    setAttr(rec.time, "title", fullTime(m.time));
    return;
  }
  const w = who(m, ctx);
  const mine = s.role === "manager";
  setClass(
    rec.el,
    `msg ${mine ? "mgr" : "mem"}${m.kind === "typing" ? " typing-msg" : ""}${m.kind === "forward" ? " forward" : ""}${priv}${flashing}`,
  );
  const key =
    (mine ? ctx.c.host : s.provider) +
    "|" +
    s.model +
    "|" +
    (mine ? ctx.c.group : "");
  if (rec.avKey !== key) {
    rec.avKey = key;
    rec.avaSlot.replaceChildren(
      ...[mine && !s.model ? null : avatar(s.provider, s.model)].filter(
        Boolean,
      ),
    );
  }
  setText(rec.name, w.name);
  setText(
    rec.sub,
    m.kind === "typing" ? `${w.sub ? `${w.sub} · ` : ""}正在输入` : w.sub,
  );
  const targets = mine ? m.recipients || [] : [];
  const toKey = JSON.stringify(targets);
  if (rec.toKey !== toKey) {
    rec.toKey = toKey;
    rec.to.replaceChildren(
      ...targets.map((target) => {
        const name = target.model || target.label || target.provider;
        return h(
          "span.recipient",
          { title: `发给 ${name}${target.seat ? ` · ${target.seat}` : ""}` },
          avatar(target.provider, target.model, "sm"),
        );
      }),
    );
  }
  setAttr(
    rec.name,
    "title",
    s.model
      ? s.model_source === "native"
        ? "CLI 回报的模型"
        : mine
          ? "主控记录的模型"
          : "调用时指定的模型"
      : "此条记录没有可靠的模型信息",
  );
  setText(rec.time, m.kind === "typing" ? "" : clock(m.time));
  setAttr(rec.time, "title", m.kind === "typing" ? null : fullTime(m.time));
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
// Fold: clamp by CSS, show the button only when the content really overflows.
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

// ---------------------------------------------------------------- drawer: fused timeline, desk history, room transcript
function openDrawer(scope, opener) {
  if (opener) state.opener = opener;
  state.drawer = scope;
  if (state.chat) renderAll(state.chat);
  el.tl.focus({ preventScroll: true });
}
function closeDrawer() {
  state.drawer = null;
  if (state.chat) renderAll(state.chat);
  const opener = state.opener;
  state.opener = null;
  if (opener?.isConnected) opener.focus({ preventScroll: true });
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.drawer) closeDrawer();
});
$("dr-close").addEventListener("click", closeDrawer);

// Messages of the open scope. A desk's history is its own messages (private and meeting) plus the
// manager prompts that were addressed to one of its seats.
function inScope(m, ctx) {
  const d = state.drawer;
  if (d.kind === "all") return true;
  if (d.kind === "room") return m.conversation === d.id;
  if (m.desk === d.key) return true;
  return (m.recipients || []).some(
    (r) => r.seat && ctx.seatDesk.get(`${m.conversation}|${r.seat}`) === d.key,
  );
}
function drawerTitle(ctx, shown) {
  const d = state.drawer;
  const real = shown.filter((m) => m.kind !== "typing");
  const counts = `${real.length} 条 · 公开 ${real.filter((m) => m.visibility === "public").length} · 私聊 ${real.filter((m) => m.thread?.kind === "desk").length}`;
  if (d.kind === "desk")
    return [ctx.desks.get(d.key)?.label || "工位", `工位历史 · ${counts}`];
  if (d.kind === "room")
    return [ctx.rooms.get(d.id)?.title || "会议室", `会议室 · ${counts}`];
  return ["消息列表", `整个工作区 · ${counts}`];
}

function renderDrawer(c, ctx) {
  el.drawer.hidden = !state.drawer;
  if (!state.drawer) {
    state.scopeKey = "";
    return;
  }
  const scopeKey = c.group + JSON.stringify(state.drawer);
  const reset = state.scopeKey !== scopeKey;
  setClass(el.tl, `tl ${colorClass(c.color)}`); // gives the manager bubbles their colour
  if (reset) {
    state.scopeKey = scopeKey;
    state.nodes.clear();
    state.seenIds.clear();
    state.unseen = 0;
    state.expanded.clear();
    el.inner.replaceChildren();
    el.tl.scrollTop = 0;
  }
  const shown = c.messages.filter((m) => inScope(m, ctx));
  const [title, sub] = drawerTitle(ctx, shown);
  setText(el.drTitle, title);
  setText(el.drSub, sub);
  setAttr(el.drawer, "aria-label", title);
  // Geometry is the truth: a reader who is at the bottom follows new content, anyone else is left alone.
  const follow = reset || nearBottom();
  const order = [];
  let day = null;
  const node = (id, make) => {
    let rec = state.nodes.get(id);
    if (!rec) state.nodes.set(id, (rec = make()));
    order.push(id);
    return rec;
  };
  let fresh = 0;
  for (const m of shown) {
    const date = new Date(AN.at(m.time)).toDateString();
    if (date !== day) {
      day = date;
      const label = dayLabel(m.time);
      const rec = node(`day:${m.id}`, () => ({
        el: h("div.sep.day", {}, h("span")),
        sep: true,
      }));
      setText(rec.el.firstChild, label);
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
    order.map((id) => state.nodes.get(id).el),
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
  el.jump.hidden = !(state.drawer && state.unseen > 0 && !nearBottom());
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
  let target = state.drawer ? find() : null;
  if (!target) {
    state.drawer = { kind: "all" };
    renderAll(state.chat);
    target = find();
  }
  if (target) flash(target);
}
// Scroll to the latest stretch of one conversation (links from 管理会话 use #conv:<id>).
function focusConversation(id) {
  const target = [...el.inner.querySelectorAll("[data-conv]")]
    .filter((n) => n.dataset.conv === id)
    .pop();
  if (target) flash(target);
}

// ---------------------------------------------------------------- body states
function showBlank(kind) {
  const none = !kind;
  el.blank.hidden = none;
  el.office.hidden = !none;
  el.ro.hidden = !none;
  if (none) return;
  el.head.hidden = true;
  el.drawer.hidden = true;
  state.headKey = "";
  state.loaded = null;
  state.drawer = null;
  const text = {
    nogroups: [
      "还没有工作区",
      "在主控 Agent（Claude Code、Codex CLI 等）里派出任务或开始会议，对应的办公室会自动出现在这里。",
    ],
    pick: [
      "选一个工作区开始查看",
      "左侧是每个主控会话的工作区，进行中的排在最前面。",
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
    desks: new Map(c.desks.map((d) => [d.key, d])),
    rooms: new Map(c.rooms.map((r) => [r.id, r])),
    sources: new Set(
      c.messages
        .filter((m) => m.message_id && m.kind !== "forward")
        .map((m) => m.message_id),
    ),
  };
}
function renderAll(c, newGroup = false) {
  const ctx = (state.ctx = contextOf(c));
  renderHead(c);
  renderOffice(c, newGroup);
  renderDrawer(c, ctx);
}
// #desk:<key> opens a desk's history, #conv:<id> a room's transcript (or the whole list, for a task).
function applyHash(ctx) {
  const key = state.hashKey;
  if (!key || state.focused) return;
  state.focused = true;
  if (key.startsWith("desk:")) {
    if (ctx.desks.has(key.slice(5)))
      openDrawer({ kind: "desk", key: key.slice(5) });
  } else if (key.startsWith("conv:")) {
    const id = key.slice(5);
    openDrawer(ctx.rooms.has(id) ? { kind: "room", id } : { kind: "all" });
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
      state.drawer = null;
      state.opener = null;
      state.scopeKey = "";
    }
    state.chat = chat;
    showBlank(null);
    renderAll(chat, newGroup);
    ingest(chat, newGroup);
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
