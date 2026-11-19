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
  L,
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
  hostTile,
  deskName,
  deskParts,
  handleOf,
  fullHandle,
  cliOf,
  chatName,
  shownName,
  activities,
} = AN;

const narrow = window.matchMedia("(max-width: 760px)");
const overlay = window.matchMedia("(max-width: 920px)"); // the conversation slides over the floor
const reduceMotion = AN.reducedMotion;
const FOLD_PX = 320; // a bubble taller than this is folded behind "展开全文"
const NEAR_BOTTOM = 64; // within this distance of the bottom the reader "follows" the chat
const MAX_FLIGHTS = 8; // more new messages than this between two polls are not animated
const COLLAPSED_KEY = "agentnave.office.convCollapsed";
const SIDE_COLLAPSED_KEY = "agentnave.office.sideCollapsed";
const FOLD_KEY = "agentnave.office.fold";
// Layout preferences survive restarts; storage may be unavailable, which only loses them.
function stored(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}
function store(key, value) {
  try {
    if (value == null) localStorage.removeItem(key); else localStorage.setItem(key, value);
  } catch { /* storage may be unavailable */ }
}
// Relative times ("just now", "3m ago") and desks that fall asleep move with the clock, not with the
// data: an unchanged poll is drawn again only once per tick.
const TICK_MS = 10e3;
const tickOf = () => Math.floor(Date.now() / TICK_MS);
const FIG = 100; // character size at meeting seats before any desk is drawn, in px (matches --fig in chat.css)
const DONE_FOR = 8e3; // a desk that just finished shows its done frame this long
const WALK_MS = 1600; // a figure's walk between its desk and a meeting seat
const HOLD_MS = 10e3; // an ended room stays empty this long after its seats are back
const FOLD_MS = 500; // then the desks below push it shut
const LEAVE_MS = WALK_MS + HOLD_MS + FOLD_MS;

// Tags stay short; the full text is in the tooltip.
const deskChar = (d) => ART.character(d.provider, d.art_model || d.model);
// How a desk that requested no model got the name it shows.
const UNKNOWN_MODEL_NOTE = L("\n调用时没有指定模型，CLI 也没有回报，无法确认实际使用的模型", "\nNo model was specified and the CLI did not report one; the model used is unknown");

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
  sideToggle: $("side-toggle"),
  drIco: $("dr-ico"),
  drTitle: $("dr-title"),
  drSub: $("dr-sub"),
  drAll: $("dr-all"),
  tl: $("timeline"),
  inner: $("tl-inner"),
  blank: $("blank"),
  jump: $("jump"),
  banner: $("banner"),
};
const state = {
  selected: null, // agent id shown in the middle
  auto: false, // selected by default (no agent in the URL), not by the user
  groupsData: [],
  projects: [], // manually added projects
  order: [],
  items: new Map(),
  sections: {},
  heads: new Map(), // project header buttons by key
  folded: new Map(), // the same keys: whether the reader folded the project
  // A long list shows only its head until the reader asks for more; the amount is not stored.
  moreGroups: 0, // extra projects the reader asked for
  moreItems: new Map(), // "project:key" -> extra entries that project shows
  moreNodes: new Map(), // the "show more" buttons by id, reused across renders
  liveMore: new Set(), // the "show more" buttons this render put in the list
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
  sideCollapsed: stored(SIDE_COLLAPSED_KEY) === "1", // the reader folded the workspace column away
  sidePeek: false, // a folded column laid over the office while the pointer is on it
  convPeek: false,
  opener: null,
  scopeKey: "",
  nodes: new Map(),
  expanded: new Set(),
  seenIds: new Set(),
  unseen: 0,
  flashId: null,
  locate: null,
  locating: false,
  flashTimer: 0,
  ready: false,
  refreshing: false,
  again: false,
};

// ---------------------------------------------------------------- routing
function groupFromLocation() {
  if (new URLSearchParams(location.search).has("group"))
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
  if (path.startsWith("/settings/") && AN.navigate) return AN.navigate(path);
  AN.officeSettings?.close();
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
    // a breakpoint changes which columns can peek: start from the settled state
    state.sidePeek = state.convPeek = false;
    el.app.removeAttribute("data-side-peek");
    el.app.removeAttribute("data-conv-peek");
    sync();
    renderList();
    if (state.chat) renderAll(state.chat);
  });

// ---------------------------------------------------------------- workspace list
function sectionEl(id, text) {
  const node = (state.sections[id] ||= h("div.gl-sec"));
  setText(node, text);
  return node;
}
function createItem(g) {
  const rec = {
    id: g.group,
    tile: h("span.gi-tile"),
    presence: h("i.presence"),
    label: h("b.gi-label"),
    meta: h("span.gi-meta"),
    iso: h("span.pill.tone-idle.act", { role: "button", tabindex: "0" }),
    tileKey: null,
  };
  rec.el = h(
    "a.gi",
    {},
    h("span.gi-host", {}, rec.tile, rec.presence),
    h(
      "span.gi-body",
      {},
      // an agent shows its title; a handover and isolated directories still waiting to be released
      // are the only things it adds
      h("span.gi-top", {}, rec.label),
      rec.meta,
      (rec.bottom = h("span.gi-bottom", {}, rec.iso)),
    ),
  );
  rec.el.addEventListener("click", (e) => {
    if (e.button) return;
    e.preventDefault();
    if (!Actions.pick(rec.id, e)) go(`/chat/${rec.id}`);
  });
  const openIsolated = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (state.loaded === rec.id) return openScope({ kind: "isolated" }, rec.iso);
    state.wantIsolated = rec.id;
    go(`/chat/${rec.id}`);
  };
  rec.iso.addEventListener("click", openIsolated);
  rec.iso.addEventListener("keydown", (e) => (e.key === "Enter" || e.key === " ") && openIsolated(e));
  rec.el.addEventListener("contextmenu", (e) => ChatTerminal.menu(rec.id, e) || Actions.workspaceMenu(rec.id, e));
  return rec;
}
function updateItem(rec, g) {
  // A failure asks nothing of the user, so the list does not call it out.
  const shown = g.status === "failed" ? "idle" : AN.ongoing(g.status);
  const [word, tone] = pick(GROUP, shown);
  const on = g.group === state.selected;
  const name = shownName(g);
  setClass(
    rec.el,
    `gi ${colorClass(g.color)} tone-${tone}${on ? " on" : ""}${urgent(g.status) ? " hot" : ""}${Actions.picked.has(g.group) ? " picked" : ""}`,
  );
  setAttr(rec.el, "href", href(`/chat/${g.group}`));
  setAttr(rec.el, "aria-current", on ? "true" : null);
  setAttr(rec.el, "aria-label", L(`${name}，${handoverWord(g)}boss 在 ${g.label}，${word}`, `${name}, ${handoverWord(g)}boss in ${g.label}, ${word}`));
  const place = itemPlace(g);
  setAttr(rec.el, "title", `${name} · ${place}`);
  const tileKey = `${g.host}|${g.label}|${!!window.Avatars?.custom("host", g.host, "")}`;
  if (rec.tileKey !== tileKey) {
    rec.tileKey = tileKey;
    rec.tile.replaceChildren(hostTile(g.app || g.host, g.label));
  }
  // the dot appears only while something runs
  rec.presence.hidden = !g.live;
  setClass(rec.presence, g.live ? "presence on" : "presence");
  setAttr(rec.presence, "title", g.live ? L("有任务在运行", "Tasks running") : null);
  setText(rec.label, chatName(g));
  // the avatar already names the boss; the line is left for a handover the user should know about
  const handover = g.handover === "pending" || g.handover === "changed";
  rec.meta.hidden = !handover;
  setText(rec.meta, handover ? `${handoverWord(g)}${place}` : "");
  rec.bottom.hidden = rec.iso.hidden = !g.isolated_count;
  setText(rec.iso, isolatedCount(g.isolated_count));
  ChatTerminal.decorate(rec, g);
}
// The boss, then the project when the item is pinned under awaiting handover.
function itemPlace(g) {
  const project = g.handover === "pending" ? g.project_label : "";
  return [g.label, project !== chatName(g) && project].filter(Boolean).join(" · ");
}
// Isolated directories nobody works in any more still wait for boss to release them.
function isolatedCount(n) {
  return L(`${n} 个隔离目录`, `${n} isolated dirs`);
}
// An agent taken over by another boss (ADR-0033).
function handoverWord(g) {
  return g.handover === "changed" ? L("boss 已更换 · ", "boss changed · ") : "";
}
function renderList() {
  const groups = state.groupsData;
  const by = new Map(groups.map((g) => [g.group, g]));
  for (const id of state.items.keys()) if (!by.has(id)) state.items.delete(id);
  state.liveMore.clear();
  const nodes = groupedNodes(groups);
  for (const id of state.moreNodes.keys()) if (!state.liveMore.has(id)) state.moreNodes.delete(id);
  if (!nodes.length) nodes.push(h("div.gl-empty", {}, L("还没有 agent", "No agents yet")));
  reconcile(el.list, nodes);
}
// The cap may never hide a pending handover, work waiting on the reader, or the agent already open.
function mustShow(g) {
  return g.handover === "pending" || urgent(g.status) || g.group === state.selected;
}
// A "show more" button, made once and reused: reconcile matches nodes, so it keeps its place and
// the click handler reads whatever this render decided it opens.
function moreNode(id, text, act, folded) {
  let rec = state.moreNodes.get(id);
  if (!rec) {
    rec = { el: h("button.gl-more", { type: "button" }) };
    rec.el.addEventListener("click", () => rec.act?.());
    state.moreNodes.set(id, rec);
  }
  rec.act = act;
  setText(rec.el, text);
  setAttr(rec.el, "data-folded", folded ? "true" : null);
  state.liveMore.add(id);
  return rec.el;
}
function itemNode(g, folded) {
  let rec = state.items.get(g.group);
  if (!rec) state.items.set(g.group, (rec = createItem(g)));
  updateItem(rec, g);
  setAttr(rec.el, "data-folded", folded ? "true" : null);
  return rec.el;
}
function isFolded(id) {
  if (!state.folded.has(id)) state.folded.set(id, stored(`${FOLD_KEY}.${id}`) === "1");
  return state.folded.get(id);
}
function toggleFold(id) {
  const folded = !isFolded(id);
  state.folded.set(id, folded);
  store(`${FOLD_KEY}.${id}`, folded ? "1" : null);
  renderList();
}
function headNode(section) {
  const id = `project:${section.key}`;
  let rec = state.heads.get(id);
  if (!rec) {
    rec = { name: h("span.gl-grp-name"), hint: h("span.gl-grp-hint"), count: h("span.gl-grp-n") };
    rec.el = h("button.gl-grp", { type: "button" }, h("span.gl-grp-caret", { "aria-hidden": "true" }), rec.name, rec.hint, rec.count);
    rec.el.addEventListener("click", () => toggleFold(id));
    rec.el.addEventListener("contextmenu", (e) => {
      if (e.target.closest?.(".gl-grp-add")) return;
      if (rec.section?.manual && !rec.section.items.length) Actions.projectMenu(rec.section, e);
    });
    state.heads.set(id, rec);
  }
  setAttr(rec.el, "aria-expanded", isFolded(id) ? "false" : "true");
  setAttr(rec.el, "title", section.path || section.title);
  setText(rec.name, section.title);
  setText(rec.hint, section.hint);
  setText(rec.count, String(section.items.length));
  rec.section = section;
  ChatTerminal.headAction(rec);
  return rec.el;
}
// Folded groups stay in the list (the narrow rail has no headers to open them) but are not shown.
// A long list keeps only its first groups and first entries until the reader asks for more.
function groupedNodes(groups) {
  const nodes = [];
  const order = [];
  const live = new Set();
  const sections = ChatGroups.arrange(groups, { projects: state.projects, urgent, L });
  const { sections: kept, hiddenGroups } = ChatGroups.capSections(sections, {
    groupLimit: ChatGroups.caps.groups + state.moreGroups,
    itemLimit: (s) => ChatGroups.caps.items + (state.moreItems.get(`project:${s.key}`) || 0),
    must: mustShow,
  });
  for (const section of kept) {
    const id = `project:${section.key}`;
    live.add(id);
    const folded = isFolded(id);
    nodes.push(headNode(section));
    for (const g of section.shown) {
      // A folded group still shows what needs the reader or is open.
      const hidden = folded && !(urgent(g.status) || g.group === state.selected);
      nodes.push(itemNode(g, hidden));
      if (!hidden) order.push(g.group);
    }
    if (section.hidden)
      nodes.push(
        moreNode(`items:${id}`, L(`再显示 ${ChatGroups.caps.itemStep} 条`, `Show ${ChatGroups.caps.itemStep} more`), () => {
          state.moreItems.set(id, (state.moreItems.get(id) || 0) + ChatGroups.caps.itemStep);
          renderList();
        }, folded),
      );
  }
  if (hiddenGroups)
    nodes.push(
      moreNode(
        "groups:project",
        L("更多项目", "More projects"),
        () => {
          state.moreGroups += ChatGroups.caps.groupStep;
          renderList();
        },
      ),
    );
  for (const id of state.heads.keys()) if (!live.has(id)) state.heads.delete(id);
  state.order = order;
  return nodes;
}
function revealProject(key) {
  if (!key) return;
  const id = `project:${key}`;
  let dirty = false;
  if (isFolded(id)) {
    state.folded.set(id, false);
    store(`${FOLD_KEY}.${id}`, null);
    dirty = true;
  }
  const sections = ChatGroups.arrange(state.groupsData, { projects: state.projects, urgent, L });
  const regularIndex = sections.filter((s) => !s.pinned).findIndex((s) => s.key === key);
  if (regularIndex >= 0 && regularIndex >= ChatGroups.caps.groups + state.moreGroups) {
    const needed = regularIndex + 1 - ChatGroups.caps.groups;
    state.moreGroups += Math.ceil(needed / ChatGroups.caps.groupStep) * ChatGroups.caps.groupStep;
    dirty = true;
  }
  if (dirty || !state.heads.has(id)) renderList();
  const head = state.heads.get(id)?.el;
  if (!head) return;
  head.scrollIntoView?.({ block: "nearest" });
  head.focus?.({ preventScroll: true });
  head.classList?.remove("flash");
  void head.offsetWidth;
  head.classList?.add("flash");
}

// ---------------------------------------------------------------- what everyone is doing
