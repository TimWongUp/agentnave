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
const rail = window.matchMedia("(max-width: 1100px)"); // the workspace list shrinks to a 64px rail
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
  desksZone: $("desks")?.closest(".desks-zone"),
  derivedZone: $("derived-zone"),
  derivedDesks: $("derived-desks"),
  derivedCount: $("derived-count"),
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
  sidebar: { project_aliases: {}, pinned_projects: [], pinned_groups: [], project_order: [] },
  order: [],
  items: new Map(),
  sections: {},
  heads: new Map(), // project header buttons by key
  folded: new Map(), // the same keys: whether the reader folded the project
  // A long list shows only its head until the reader asks for more; the amount is not stored.
  moreGroups: 0, // extra projects the reader asked for
  moreItems: new Map(), // "project:key" -> extra entries that project shows
  moreFlat: 0, // extra items shown in flat view
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
  derivedTurns: { cache: new Map(), pending: new Map() },
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
for (const query of [narrow, overlay, rail])
  query.addEventListener("change", () => {
    // a breakpoint changes which columns can peek: start from the settled state
    state.sidePeek = state.convPeek = false;
    el.app.removeAttribute("data-side-peek");
    el.app.removeAttribute("data-conv-peek");
    if (typeof renderSideToggle === "function") renderSideToggle(); // the 761–1100px rail never folds away
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
function sidebarData() {
  return state.sidebar || { project_aliases: {}, pinned_projects: [], pinned_groups: [], project_order: [] };
}
function isGroupPinned(id) {
  return Boolean(sidebarData().pinned_groups?.includes(id));
}
function projectTitleOf(g) {
  if (!g?.project_key || g.project_key === ChatGroups.other) return "";
  const alias = sidebarData().project_aliases?.[g.project_key]?.trim();
  if (alias) return alias;
  return g.project_label || state.projects?.find((p) => p.key === g.project_key)?.label || "";
}
// The ⋯ on a row opens that row's context menu just below itself, for the keyboard and for anyone who
// never right-clicks. It sits inside the row, so its click must not reach the row's own.
function moreButton(cls, open) {
  const label = L("更多操作", "More actions");
  const button = h(`span.${cls}`, { role: "button", tabindex: "0", title: label, "aria-label": label, "aria-haspopup": "menu" }, "⋯");
  const run = (e) => {
    e.preventDefault();
    e.stopPropagation();
    const box = button.getBoundingClientRect();
    open({ clientX: box.left, clientY: box.bottom + 4, preventDefault() {}, stopPropagation() {} });
  };
  button.addEventListener("click", run);
  button.addEventListener("keydown", (e) => {
    if ((e.key === "Enter" || e.key === " ") && !e.isComposing) run(e);
  });
  return button;
}
function createItem(g) {
  const rec = {
    id: g.group,
    tile: h("span.gi-tile"),
    presence: h("i.presence"),
    label: h("b.gi-label"),
    pin: h("span.gi-pin", { "aria-hidden": "true", hidden: true }),
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
      (rec.top = h("span.gi-top", {}, rec.label, rec.pin)),
      rec.meta,
      (rec.bottom = h("span.gi-bottom", {}, rec.iso)),
    ),
    moreButton("gi-more", (e) => ChatTerminal.menu(rec.id, e) || Actions.workspaceMenu(rec.id, e)),
  );
  rec.el.addEventListener("click", (e) => {
    if (e.button) return;
    e.preventDefault();
    if (!Actions.pick(rec.id, e)) {
      if (ChatTerminal.openDerived?.(rec.id)) return;
      // The boss row of the office whose derived desk is open leads back to boss.
      const back = state.loaded === rec.id && selectedDerivedDeskKey();
      go(`/chat/${rec.id}`);
      if (back && !ChatTerminal.openBoss?.()) openScope({ kind: "all" });
    }
  });
  const openIsolated = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (state.loaded === rec.id) return openScope({ kind: "isolated" }, rec.iso);
    state.wantIsolated = rec.id;
    go(`/chat/${rec.id}`);
  };
  rec.iso.addEventListener("click", openIsolated);
  rec.iso.addEventListener("keydown", (e) => {
    const tag = String(e.target?.tagName || "").toUpperCase();
    if (e.target?.isContentEditable || tag === "TEXTAREA" || tag === "INPUT" || tag === "SELECT") return;
    if (e.key === "Enter" || e.key === " ") openIsolated(e);
  });
  rec.el.addEventListener("contextmenu", (e) => ChatTerminal.menu(rec.id, e) || Actions.workspaceMenu(rec.id, e));
  return rec;
}
function selectedDerivedDeskKey() {
  if (typeof state === "undefined" || state.drawer?.kind !== "desk" || !state.drawer.key) return null;
  if (typeof el !== "undefined") {
    if (el.drawer?.hidden) return null;
    if (el.drawer?.classList?.contains?.("folded") && !state.convPeek) return null;
  }
  const key = state.drawer.key;
  if (state.ctx?.desks?.get?.(key)?.derived) return key;
  if (typeof ChatTerminal !== "undefined" && ChatTerminal?.derivedOf?.(state.selected)?.some((d) => d.desk_key === key)) {
    return key;
  }
  return null;
}
const isDerivedItem = (g) => g?.derived === true;
function isItemOn(g, deskKey = selectedDerivedDeskKey()) {
  if (isDerivedItem(g)) {
    return Boolean(deskKey && g.parent_workspace_id === state.selected && g.desk_key === deskKey) || g.group === state.selected;
  }
  return g.group === state.selected && !deskKey;
}
function syncListSelection() {
  if (typeof state === "undefined" || !state.items) return;
  const deskKey = selectedDerivedDeskKey();
  for (const rec of state.items.values()) {
    if (!rec.g) continue;
    const on = isItemOn(rec.g, deskKey);
    rec.el.classList?.toggle?.("on", on);
    setAttr(rec.el, "aria-current", on ? "true" : null);
  }
}
function updateItem(rec, g, flat = false, level = 0) {
  rec.g = g;
  const isChild = isDerivedItem(g);
  const isGrandchild = !isChild && level === 2;
  // A failure asks nothing of the user, so the list does not call it out.
  const dMeta = isChild ? ChatTerminal.derivedMeta(g) : null;
  const shown = g.status === "failed" ? "idle" : AN.ongoing(g.status);
  const [word, groupTone] = pick(GROUP, shown);
  const tone = dMeta ? dMeta.tone : groupTone;
  const on = isItemOn(g);
  const pinned = isGroupPinned(g.group);
  const name = shownName(g);
  setClass(
    rec.el,
    `gi ${colorClass(g.color)} tone-${tone}${isChild || isGrandchild ? " gi-child" : ""}${isGrandchild ? " gi-grandchild" : ""}${on ? " on" : ""}${urgent(g.status) ? " hot" : ""}${pinned ? " pinned" : ""}${Actions.picked.has(g.group) ? " picked" : ""}`,
  );
  setAttr(rec.el, "href", href(isChild ? ChatTerminal.rowPath(g) : `/chat/${g.group}`));
  setAttr(rec.el, "aria-current", on ? "true" : null);
  setAttr(
    rec.el,
    "aria-label",
    isChild
      ? L(`${name}，派生终端 ${g.label}，${dMeta.text}`, `${name}, derived terminal in ${g.label}, ${dMeta.text}`)
      : L(`${name}，${handoverWord(g)}boss 在 ${g.label}，${word}`, `${name}, ${handoverWord(g)}boss in ${g.label}, ${word}`),
  );
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
  rec.pin.hidden = !pinned;
  setAttr(rec.pin, "title", pinned ? L("已置顶", "Pinned") : null);
  // In flat view, .gi-meta also displays the owning project name when not "other".
  if (isChild) {
    const project = flat ? projectTitleOf(g) : "";
    const metaText = [dMeta.text, project].filter(Boolean).join(" · ");
    rec.meta.hidden = false;
    setClass(rec.meta, `gi-meta tone-${tone}`);
    setText(rec.meta, metaText);
  } else {
    const handover = g.handover === "pending" || g.handover === "changed";
    const summary = ChatTerminal.derivedSummary?.(g.group);
    setClass(rec.meta, "gi-meta");
    if (flat) {
      const project = projectTitleOf(g);
      const parts = [];
      if (g.handover === "pending") parts.push(L("待接手", "Awaiting handover"));
      else if (g.handover === "changed") parts.push(L("boss 已更换", "boss changed"));
      if (handover) parts.push(g.label);
      if (summary) parts.push(summary.text);
      if (project) parts.push(project);
      const metaText = parts.filter(Boolean).join(" · ");
      rec.meta.hidden = !metaText;
      setText(rec.meta, metaText);
    } else {
      const parts = [];
      if (handover) parts.push(`${handoverWord(g)}${place}`);
      if (summary) parts.push(summary.text);
      const metaText = parts.join(" · ");
      rec.meta.hidden = !metaText;
      setText(rec.meta, metaText);
    }
  }
  const cleaned = typeof ChatIsolatedCleanup !== "undefined" ? ChatIsolatedCleanup.doneCount(g.group) : 0;
  rec.bottom.hidden = rec.iso.hidden = !g.isolated_count && !cleaned;
  setText(rec.iso, g.isolated_count ? isolatedCount(g.isolated_count) : L(`已清理 ${cleaned} 个隔离目录`, `${cleaned} isolated dirs cleaned`));
  ChatTerminal.decorate(rec, g);
  if (typeof ChatGit !== "undefined") ChatGit.decorateItem?.(rec, g);
}
// The boss, then the project when the item is pinned under awaiting handover.
function itemPlace(g) {
  const project = g.handover === "pending" ? projectTitleOf(g) : "";
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
function sidebarPrefs() {
  return typeof ChatSidebar !== "undefined" ? ChatSidebar.prefs() : (ChatGroups.defaultPrefs || { view: "project" });
}
function sidebarQuery() {
  return typeof ChatSidebar !== "undefined" ? ChatSidebar.query() : "";
}
function renderList() {
  const groups = state.groupsData;
  // derived terminals have rows here without being groups of their own
  const listed = new Set([...groups, ...(ChatTerminal.derivedAll?.() || [])].map((g) => g.group));
  for (const id of state.items.keys()) if (!listed.has(id)) state.items.delete(id);
  state.liveMore.clear();
  const prefs = sidebarPrefs();
  const query = sidebarQuery();
  const nodes = prefs.view === "flat" && typeof ChatGroups.arrangeFlat === "function"
    ? flatNodes(groups, { prefs, query })
    : groupedNodes(groups, { prefs, query });
  for (const id of state.moreNodes.keys()) if (!state.liveMore.has(id)) state.moreNodes.delete(id);
  if (!nodes.length) {
    const filtered = Boolean(query.trim()) || prefs.filter === "active" || (prefs.hideEmpty && (groups.length || state.projects.length));
    nodes.push(h("div.gl-empty", {}, filtered ? L("无匹配项", "No matches") : L("还没有 agent", "No agents yet")));
  }
  reconcile(el.list, nodes);
  if (typeof ChatSidebar !== "undefined") {
    ChatSidebar.syncUI?.();
    ChatSidebar.syncActiveHighlight?.();
  }
}
// The cap may never hide a pending handover, work waiting on the reader, a pinned agent, the agent
// already open, or an agent with a derived terminal still open under it.
function mustShowOwn(g) {
  return (
    g.handover === "pending" ||
    urgent(g.status) ||
    g.group === state.selected ||
    isGroupPinned(g.group)
  );
}
function mustShow(g) {
  if (mustShowOwn(g)) return true;
  const derived = ChatTerminal.derivedOf?.(g.group) || [];
  if (derived.some((d) => d.terminal != null)) return true;
  return derived.some((d) =>
    (ChatTerminal.derivedChildrenOf?.(g.group, d.desk_key) || []).some(mustShowOwn),
  );
}
// A folded project keeps only pending handovers, urgent work, and the open agent visible.
function stayVisibleWhenFolded(g) {
  return g.handover === "pending" || urgent(g.status) || g.group === state.selected;
}
function itemNode(g, folded, flat = false, level = 0) {
  let rec = state.items.get(g.group);
  if (!rec) state.items.set(g.group, (rec = createItem(g)));
  updateItem(rec, g, flat, level);
  setAttr(rec.el, "data-folded", folded ? "true" : null);
  return rec.el;
}
function partitionNested(groups) {
  const present = new Set(groups.map((g) => g.group));
  const children = new Map();
  const nestedIds = new Set();
  for (const parent of groups) {
    if (parent.handover === "pending") continue;
    for (const d of ChatTerminal.derivedOf?.(parent.group) || []) {
      const matched = (ChatTerminal.derivedChildrenOf?.(parent.group, d.desk_key) || []).filter((child) =>
        present.has(child.group),
      );
      if (matched.length) {
        children.set(`${parent.group}|${d.desk_key}`, matched);
        for (const child of matched) nestedIds.add(child.group);
      }
    }
  }
  return {
    topGroups: nestedIds.size ? groups.filter((g) => !nestedIds.has(g.group)) : groups,
    children,
  };
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
function setAllFolded(folded) {
  const sections = ChatGroups.arrange(state.groupsData, {
    projects: state.projects,
    urgent,
    L,
    sidebar: sidebarData(),
    prefs: sidebarPrefs(),
    query: sidebarQuery(),
  });
  for (const s of sections) {
    const id = `project:${s.key}`;
    state.folded.set(id, folded);
    store(`${FOLD_KEY}.${id}`, folded ? "1" : null);
  }
  if (!folded) {
    for (const id of state.folded.keys()) {
      state.folded.set(id, false);
      store(`${FOLD_KEY}.${id}`, null);
    }
  }
  renderList();
}
function headNode(section, folded = false) {
  const id = `project:${section.key}`;
  let rec = state.heads.get(id);
  if (!rec) {
    rec = {
      name: h("span.gl-grp-name"),
      hint: h("span.gl-grp-hint"),
      pin: h("span.gl-grp-pin", { "aria-hidden": "true", hidden: true }),
      dot: h("i.gl-grp-dot", { "aria-hidden": "true", hidden: true }),
      count: h("span.gl-grp-n"),
    };
    rec.el = h(
      "button.gl-grp",
      { type: "button" },
      h("span.gl-grp-caret", { "aria-hidden": "true" }),
      rec.name,
      rec.hint,
      rec.pin,
      rec.dot,
      rec.count,
      (rec.more = moreButton("gl-grp-more", (e) => rec.section && Actions.projectMenu(rec.section, e))),
    );
    rec.el.addEventListener("click", () => toggleFold(id));
    rec.el.addEventListener("contextmenu", (e) => {
      if (e.target.closest?.(".gl-grp-add")) return;
      if (rec.section && !rec.section.pinned && rec.section.key !== ChatGroups.handover) Actions.projectMenu(rec.section, e);
    });
    state.heads.set(id, rec);
  }
  setAttr(rec.el, "aria-expanded", folded ? "false" : "true");
  setAttr(rec.el, "data-kind", section.pinned ? "handover" : section.other ? "other" : null);
  setAttr(rec.el, "title", section.path || section.title);
  setText(rec.name, section.title);
  setText(rec.hint, section.hint);
  rec.pin.hidden = !section.pinnedProject;
  rec.el.classList?.toggle?.("pinned", Boolean(section.pinnedProject));
  const tone = folded && ChatGroups.foldedTone ? ChatGroups.foldedTone(section.items) : "";
  rec.dot.hidden = !tone;
  setAttr(rec.dot, "data-tone", tone || null);
  const dotTitle = tone === "review"
    ? L("待审核/待接手", "Awaiting review or handover")
    : tone === "bad"
      ? L("有失败", "Has failed tasks")
      : tone === "run"
        ? L("运行中", "Running")
        : null;
  setAttr(rec.dot, "title", dotTitle);
  setText(rec.count, String(section.items.length));
  rec.more.hidden = Boolean(section.pinned || section.key === ChatGroups.handover);
  rec.section = section;
  ChatTerminal.headAction(rec);
  if (typeof ChatSidebar !== "undefined") ChatSidebar.attachDrag?.(rec);
  return rec.el;
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
function flatNodes(groups, { prefs, query }) {
  const nodes = [];
  const order = [];
  const searching = Boolean(query && query.trim());
  const arranged = ChatGroups.arrangeFlat(groups, { projects: state.projects, urgent, L, sidebar: sidebarData(), prefs, query });
  const { topGroups: items, children: childMap } = partitionNested(arranged);
  const limit = searching ? Infinity : ChatGroups.caps.groups * ChatGroups.caps.items + state.moreFlat;
  const shown = ChatGroups.capped ? ChatGroups.capped(items, limit, mustShow) : items;
  for (const g of shown) {
    nodes.push(itemNode(g, false, true));
    order.push(g.group);
    for (const child of ChatTerminal.derivedOf?.(g.group) || []) {
      nodes.push(itemNode(child, false, true, 1));
      order.push(child.group);
      for (const gk of childMap.get(`${g.group}|${child.desk_key}`) || []) {
        nodes.push(itemNode(gk, false, true, 2));
        order.push(gk.group);
      }
    }
  }
  if (items.length > shown.length) {
    nodes.push(
      moreNode("flat:items", L("显示更多", "Show more"), () => {
        state.moreFlat += ChatGroups.caps.groups * ChatGroups.caps.itemStep;
        renderList();
      }),
    );
  }
  state.heads.clear();
  state.order = order;
  return nodes;
}
// Folded groups stay in the list (the narrow rail has no headers to open them) but are not shown.
// A long list keeps only its first groups and first entries until the reader asks for more.
function groupedNodes(groups, { prefs, query } = {}) {
  const nodes = [];
  const order = [];
  const live = new Set();
  const searching = Boolean(query && query.trim());
  const { topGroups, children: fullChildMap } = searching ? { topGroups: groups, children: new Map() } : partitionNested(groups);
  const sections = ChatGroups.arrange(topGroups, { projects: state.projects, urgent, L, sidebar: sidebarData(), prefs, query });
  const { sections: kept, hiddenGroups } = ChatGroups.capSections(sections, {
    groupLimit: searching ? Infinity : ChatGroups.caps.groups + state.moreGroups,
    itemLimit: (s) => (searching ? Infinity : ChatGroups.caps.items + (state.moreItems.get(`project:${s.key}`) || 0)),
    must: mustShow,
  });
  for (const section of kept) {
    const id = `project:${section.key}`;
    live.add(id);
    const folded = searching ? false : isFolded(id);
    nodes.push(headNode(section, folded));
    const searchChildMap = searching ? partitionNested(section.shown).children : null;
    const childMap = searchChildMap || fullChildMap;
    const nestedInSection = searching
      ? new Set([...childMap.values()].flat().map((c) => c.group))
      : null;
    for (const g of section.shown) {
      if (nestedInSection?.has(g.group)) continue;
      const children = ChatTerminal.derivedOf?.(g.group) || [];
      const hasTermChildren = children.some((d) => d.terminal != null);
      const hasVisibleGrandchild = children.some((d) =>
        (childMap.get(`${g.group}|${d.desk_key}`) || []).some(stayVisibleWhenFolded),
      );
      // A folded group still shows what needs the reader, is open, or has a derived terminal.
      const hidden = folded && !(stayVisibleWhenFolded(g) || hasTermChildren || hasVisibleGrandchild);
      nodes.push(itemNode(g, hidden, false));
      if (!hidden) order.push(g.group);
      const deskKey = selectedDerivedDeskKey();
      for (const child of children) {
        const gks = childMap.get(`${g.group}|${child.desk_key}`) || [];
        const childHasVisibleGk = gks.some(stayVisibleWhenFolded);
        const childHidden = folded && child.terminal == null && !isItemOn(child, deskKey) && !childHasVisibleGk;
        nodes.push(itemNode(child, childHidden, false, 1));
        if (!childHidden) order.push(child.group);
        for (const gk of gks) {
          const gkHidden = folded && !stayVisibleWhenFolded(gk);
          nodes.push(itemNode(gk, gkHidden, false, 2));
          if (!gkHidden) order.push(gk.group);
        }
      }
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
  const sections = ChatGroups.arrange(state.groupsData, { projects: state.projects, urgent, L, sidebar: sidebarData(), prefs: sidebarPrefs() });
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
