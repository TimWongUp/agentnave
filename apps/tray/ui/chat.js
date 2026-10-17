"use strict";
// App group chat: one group per manager session. Left: group list (running pinned). Right:
// the selected group's timeline as a read-only group chat. Everything is patched in place:
// list items keyed by group id, timeline nodes keyed by message id (typing bubbles by member
// key), so a 1.5 s poll never rebuilds the list, never flickers and never moves the scroll
// position of a reader who scrolled up.
const {
  h, $, load, href, GROUP, CONV, MEMBER, pick, urgent, avatar, hostAvatar, colorClass,
  relTime, clock, fullTime, dayLabel, setText, setAttr, setClass, reconcile, stickyOrder, poll, plain,
} = AN;

const mockOn = !!(window.AgentNaveMock && window.AgentNaveMock.enabled());
const narrow = window.matchMedia("(max-width: 760px)");
const FOLD_PX = 280; // a bubble taller than this is folded behind "展开全文"
const NEAR_BOTTOM = 64; // within this distance of the bottom the reader "follows" the chat

const nearBottom = () => el.tl.scrollHeight - el.tl.scrollTop - el.tl.clientHeight < NEAR_BOTTOM;
const el = { app: $("app"), list: $("list"), head: $("head"), tl: $("timeline"), inner: $("tl-inner"), blank: $("blank"), jump: $("jump"), ro: $("ro"), banner: $("banner"), count: $("side-count") };
const state = {
  selected: null, // group id shown on the right
  auto: false, // selected by default (no group in the URL), not by the user
  groupsData: [], order: [], items: new Map(), sections: {},
  chat: null, // last timeline payload
  loaded: null, // group id whose nodes are currently in the timeline
  nodes: new Map(), expanded: new Set(), seenIds: new Set(),
  unseen: 0, ready: false, hashKey: null, headKey: "",
  chips: new Map(), convChips: new Map(), refreshing: false, again: false,
};

const ob = AN.onboarding({ variant: "compact", onChange: () => placeOb() });
function placeOb() {
  const none = state.groupsData.length === 0;
  ob.setVariant(none ? "full" : "compact");
  const slot = none ? (el.blank.hidden ? null : el.blank) : $("onboard-side");
  const description = el.blank.querySelector(":scope > p");
  if (description && none) description.hidden = ob.visible;
  if (!ob.visible || !slot) { ob.el.remove(); return; }
  if (ob.el.parentNode !== slot) slot.append(ob.el);
}


// ---------------------------------------------------------------- routing
function groupFromLocation() {
  if (mockOn && new URLSearchParams(location.search).has("group")) return new URLSearchParams(location.search).get("group");
  const m = /^\/chat\/([^/]+)/.exec(location.pathname);
  try { return m ? decodeURIComponent(m[1]) : null; } catch { return null; }
}
const keyFromHash = () => {
  try { return location.hash.length > 1 ? decodeURIComponent(location.hash.slice(1)) : null; } catch { return null; }
};
function go(path, replace) {
  history[replace ? "replaceState" : "pushState"]({}, "", href(path));
  sync();
  refresh();
}
function sync() {
  const fromUrl = groupFromLocation();
  state.hashKey = keyFromHash();
  if (fromUrl) { state.selected = fromUrl; state.auto = false; }
  else if (narrow.matches) { state.selected = null; state.auto = false; }
  else state.auto = true; // wide window without a group: first group is picked in refresh()
  el.app.dataset.view = fromUrl || (!narrow.matches && state.selected) ? "chat" : "list";
}
window.addEventListener("popstate", () => { sync(); refresh(); });
window.addEventListener("hashchange", () => {
  sync();
  if (state.loaded === state.selected && state.hashKey) focusMember(state.hashKey);
  else refresh();
});
narrow.addEventListener("change", () => { sync(); renderList(); measureAll(); });

// ---------------------------------------------------------------- group list
function sectionEl(id, text) {
  return (state.sections[id] ||= h("div.gl-sec", {}, text));
}
function createItem(g) {
  const presence = h("i.presence");
  const rec = {
    id: g.group, presence,
    label: h("b.gi-label"), meta: h("span.gi-meta"), time: h("time"),
    title: h("span.gi-title"), prev: h("span.gi-prev"), pill: h("span.pill"),
  };
  rec.el = h("a.gi", {}, h("span.gi-ava", {}, hostAvatar(g.host, g.label), presence),
    h("span.gi-body", {}, h("span.gi-top", {}, rec.label, rec.meta, rec.time), rec.title, h("span.gi-bottom", {}, rec.prev, rec.pill)));
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
  setClass(rec.el, `gi ${colorClass(g.color)} tone-${tone}${on ? " on" : ""}${urgent(g.status) ? " hot" : ""}`);
  setAttr(rec.el, "href", href(`/chat/${g.group}`));
  setAttr(rec.el, "aria-current", on ? "true" : null);
  setAttr(rec.el, "aria-label", `${g.label}，${word}，${g.title}`);
  setClass(rec.presence, g.live ? "presence on" : "presence");
  setText(rec.label, g.label);
  setText(rec.meta, `${g.conversations} 个会话`);
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
  state.order = stickyOrder(state.order, groups.map((g) => ({ id: g.group, rank: urgent(g.status) ? 0 : 1 })));
  for (const id of state.items.keys()) if (!by.has(id)) state.items.delete(id);
  const nodes = [];
  let section = -1;
  for (const id of state.order) {
    const g = by.get(id);
    const rank = urgent(g.status) ? 0 : 1;
    if (rank !== section) {
      section = rank;
      nodes.push(rank === 0 ? sectionEl("hot", "进行中") : sectionEl("rest", "最近"));
    }
    let rec = state.items.get(id);
    if (!rec) state.items.set(id, (rec = createItem(g)));
    updateItem(rec, g);
    nodes.push(rec.el);
  }
  if (!groups.length) nodes.push(h("div.gl-empty", {}, "还没有群聊"));
  reconcile(el.list, nodes);
}

// ---------------------------------------------------------------- group head
function renderHead(c) {
  const [word, tone] = pick(GROUP, c.status);
  el.head.hidden = false;
  if (state.headKey !== c.group) {
    state.headKey = c.group;
    state.chips.clear();
    state.convChips.clear();
    const back = h("a.back", { href: href("/chat/"), "aria-label": "返回群列表" }, "‹ 群聊");
    back.addEventListener("click", (e) => { e.preventDefault(); state.selected = null; go("/chat/"); });
    state.head = {
      back, ava: h("span.gh-ava", {}, hostAvatar(c.host, c.label), h("i.presence")), label: h("b.gh-label"), pill: h("span.pill"), sub: h("span.gh-sub"),
      members: h("div.gh-members", { "aria-label": "组员" }), convs: h("div.gh-convs", { "aria-label": "本群的会话" }),
    };
    el.head.replaceChildren(h("div.gh-row", {}, state.head.back, state.head.ava, h("div.gh-id", {}, h("span.gh-line", {}, state.head.label, state.head.pill), state.head.sub), state.head.members), state.head.convs);
  }
  const t = state.head;
  setClass(el.head, `gh ${colorClass(c.color)}`);
  setClass(t.ava.lastChild, c.live ? "presence on" : "presence");
  setText(t.label, c.label);
  setClass(t.pill, `pill tone-${tone}`);
  setText(t.pill, word);
  setText(t.sub, `${c.live ? "主控在线" : "主控离线"} · ${c.conversations.length} 个会话 · ${c.members.length} 位组员`);

  const chipNodes = c.members.map((m) => {
    let chip = state.chips.get(m.key);
    if (!chip) {
      chip = h("button.mchip", { type: "button" }, avatar(m.provider, m.model), h("i.dot"));
      chip.addEventListener("click", () => focusMember(m.key));
      state.chips.set(m.key, chip);
    }
    const [label, tone2] = pick(MEMBER, m.status);
    setClass(chip, `mchip tone-${tone2}`);
    setAttr(chip, "title", `${m.model} · ${m.provider} · ${label}\n${m.line}`);
    setAttr(chip, "aria-label", `${m.model}，${label}，跳到最近一条消息`);
    return chip;
  });
  for (const k of state.chips.keys()) if (!c.members.some((m) => m.key === k)) state.chips.delete(k);
  reconcile(t.members, chipNodes);

  const convNodes = c.conversations.map((cv) => {
    let chip = state.convChips.get(cv.id);
    if (!chip) {
      chip = h("a.cchip", {}, h("span.glyph"), h("span.cchip-title"), h("i.dot"));
      state.convChips.set(cv.id, chip);
    }
    const [label, tone2] = pick(CONV, cv.status);
    setClass(chip, `cchip tone-${tone2}`);
    setAttr(chip, "href", href(cv.director_url));
    setAttr(chip, "title", `${cv.title}（${label}）在工作台打开`);
    setText(chip.firstChild, cv.mode === "task" ? "任" : "节");
    setClass(chip.firstChild, "glyph " + cv.mode);
    setText(chip.children[1], cv.title);
    return chip;
  });
  reconcile(t.convs, convNodes);
}

// ---------------------------------------------------------------- tiny safe Markdown
// Code fences, headings, lists, `code`, **bold**. Everything else is text. No HTML is ever parsed.
function inline(parent, text) {
  let last = 0;
  for (const m of text.matchAll(/`[^`\n]+`|\*\*[^*\n]+\*\*/g)) {
    if (m.index > last) parent.append(text.slice(last, m.index));
    parent.append(m[0][0] === "`" ? h("code", {}, m[0].slice(1, -1)) : h("strong", {}, m[0].slice(2, -2)));
    last = m.index + m[0].length;
  }
  if (last < text.length) parent.append(text.slice(last));
  return parent;
}
function renderMarkdown(root, text) {
  const lines = String(text || "").replace(/\r/g, "").split("\n");
  const out = [];
  let para = [], list = null;
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
      while (++i < lines.length && !/^\s*```/.test(lines[i])) code.push(lines[i]);
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
    else { if (list) list = null; para.push(line); }
  }
  flush();
  root.replaceChildren(...out);
}

// ---------------------------------------------------------------- timeline nodes
function who(m, c) {
  const s = m.speaker;
  if (s.role === "manager") return { name: s.label || c.label, sub: "主控" };
  return { name: s.label || s.model, sub: s.label && s.label !== s.model ? `${s.model} · ${s.provider}` : s.provider };
}
function recipients(m, c) {
  const names = (m.to || []).map((k) => c.members.find((x) => x.key === k)?.model || k);
  return names.length > 3 ? `→ ${names.length} 位组员` : names.length ? "→ " + names.join("、") : "";
}
function speakerAvatar(m, c) {
  const s = m.speaker;
  return s.role === "manager" ? hostAvatar(c.host, c.label) : avatar(s.provider, s.model);
}

function createMessage(m, c) {
  const rec = { kind: m.kind, text: null };
  if (m.kind === "notice") {
    rec.ava = h("span.note-ava");
    rec.name = h("b");
    rec.text_ = h("span.note-text");
    rec.detail = h("small.note-detail");
    rec.time = h("time");
    rec.el = h("div.note", {}, rec.ava, h("span.note-main", {}, h("span.note-line", {}, rec.name, rec.text_, rec.time), rec.detail));
    return rec;
  }
  rec.avaSlot = h("span.m-ava");
  rec.name = h("b.m-name");
  rec.sub = h("span.m-sub");
  rec.to = h("span.m-to");
  rec.time = h("time");
  rec.md = h("div.md");
  rec.dots = h("span.typing", {}, h("i"), h("i"), h("i"));
  rec.tail = h("div.tail");
  rec.bub = h("div.bub", {}, m.kind === "typing" ? [rec.tail, rec.dots] : rec.md);
  rec.more = h("button.more", { type: "button", hidden: true }, "展开全文");
  rec.more.addEventListener("click", () => {
    const id = rec.id;
    if (state.expanded.has(id)) state.expanded.delete(id);
    else state.expanded.add(id);
    applyFold(rec);
  });
  rec.el = h("article.msg", {}, rec.avaSlot, h("div.m-col", {}, h("div.m-head", {}, rec.name, rec.sub, rec.to, rec.time), rec.bub, m.kind === "typing" ? null : rec.more));
  return rec;
}
function updateMessage(rec, m, c, id) {
  rec.id = id;
  const s = m.speaker;
  const [label, tone] = pick(MEMBER, m.status);
  setAttr(rec.el, "data-key", s.role === "member" ? `${s.provider}/${s.model}` : null);
  setAttr(rec.el, "data-id", id);
  if (m.kind === "notice") {
    const key = s.provider + s.model;
    if (rec.avKey !== key) { rec.avKey = key; rec.ava.replaceChildren(s.role === "system" ? "" : avatar(s.provider, s.model, "sm")); }
    setClass(rec.el, `note tone-${tone}`);
    setText(rec.name, who(m, c).name);
    setText(rec.text_, m.text);
    setText(rec.detail, m.detail || "");
    rec.detail.hidden = !m.detail;
    setText(rec.time, clock(m.time));
    setAttr(rec.time, "title", fullTime(m.time));
    return;
  }
  const w = who(m, c);
  const mine = s.role === "manager";
  setClass(rec.el, `msg ${mine ? "mgr" : "mem"}${m.kind === "typing" ? " typing-msg" : ""}`);
  const key = (mine ? c.host : s.provider) + "|" + s.model + "|" + (mine ? c.group : "");
  if (rec.avKey !== key) { rec.avKey = key; rec.avaSlot.replaceChildren(speakerAvatar(m, c)); }
  setText(rec.name, w.name);
  setText(rec.sub, m.kind === "typing" ? `${w.sub} · 正在输入` : w.sub);
  setText(rec.to, mine ? recipients(m, c) : "");
  setText(rec.time, m.kind === "typing" ? "" : clock(m.time));
  setAttr(rec.time, "title", m.kind === "typing" ? null : fullTime(m.time));
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

function sepDay(text) {
  return h("div.sep.day", {}, h("span", {}, text));
}
function sepConv(cv) {
  return h("a.sep.conv", {}, h("span.glyph"), h("span.sep-title"), h("span.sep-link", {}, "在工作台打开"));
}
function updateSep(node, kind, data) {
  if (kind === "day") { setText(node.firstChild, data); return; }
  const cv = data;
  setAttr(node, "href", cv ? href(cv.director_url) : null);
  setText(node.firstChild, cv?.mode === "show" ? "节" : "任");
  setClass(node.firstChild, "glyph " + (cv?.mode || "task"));
  setText(node.children[1], cv ? cv.title : "会话");
}

function renderTimeline(c) {
  const newGroup = state.loaded !== c.group;
  setClass(el.tl, `tl ${colorClass(c.color)}`); // gives the manager bubbles their colour
  if (newGroup) {
    state.loaded = c.group;
    state.nodes.clear();
    state.seenIds.clear();
    state.unseen = 0;
    state.expanded.clear();
    el.inner.replaceChildren();
    el.tl.scrollTop = 0;
  }
  // Geometry is the truth: a reader who is at the bottom follows new content, anyone else is left alone.
  const follow = newGroup || nearBottom();
  const convs = new Map(c.conversations.map((x) => [x.id, x]));
  const order = [];
  const fresh = [];
  const spoken = [];
  let day = null, conv = null;
  const node = (id, make) => {
    let rec = state.nodes.get(id);
    if (!rec) state.nodes.set(id, (rec = make()));
    order.push(id);
    return rec;
  };
  for (const m of c.messages) {
    const d = dayLabel(m.time);
    const date = new Date(AN.at(m.time)).toDateString();
    if (date !== day) {
      day = date;
      const rec = node(`day:${m.id}`, () => ({ el: sepDay(d), sep: true }));
      updateSep(rec.el, "day", d);
    }
    if (m.conversation !== conv) {
      conv = m.conversation;
      const rec = node(`conv:${m.conversation}:${m.id}`, () => ({ el: sepConv(), sep: true }));
      updateSep(rec.el, "conv", convs.get(m.conversation));
    }
    let rec = state.nodes.get(m.id);
    if (rec && rec.kind !== m.kind) { state.nodes.delete(m.id); rec = null; }
    rec = node(m.id, () => createMessage(m, c));
    updateMessage(rec, m, c, m.id);
    if (!state.seenIds.has(m.id)) {
      state.seenIds.add(m.id);
      if (state.ready && !newGroup) { fresh.push(m); if (m.kind !== "typing" && m.kind !== "prompt") spoken.push(`${who(m, c).name}：${plain(m.text).split("\n")[0].slice(0, 60)}`); }
    }
  }
  const keep = new Set(order);
  for (const id of state.nodes.keys()) if (!keep.has(id)) state.nodes.delete(id);
  reconcile(el.inner, order.map((id) => state.nodes.get(id).el));
  for (const rec of state.nodes.values()) if (rec.dirty) { rec.dirty = false; applyFold(rec); }

  if (follow) {
    el.tl.scrollTop = el.tl.scrollHeight;
    state.unseen = 0;
  } else if (fresh.length) state.unseen += fresh.length;
  if (newGroup && state.hashKey) setTimeout(() => focusMember(state.hashKey), 0);
  updateJump();
  if (spoken.length) $("announce").textContent = spoken.join("；");
}
function updateJump() {
  el.jump.hidden = !(state.unseen > 0 && !nearBottom());
  el.jump.textContent = `${state.unseen} 条新消息 ↓`;
}
el.tl.addEventListener("scroll", () => {
  if (nearBottom()) state.unseen = 0;
  updateJump();
}, { passive: true });
el.jump.addEventListener("click", () => {
  el.tl.scrollTo({ top: el.tl.scrollHeight, behavior: "smooth" });
});
window.addEventListener("resize", () => measureAll());
if (document.fonts?.ready) document.fonts.ready.then(measureAll);

// Scroll to a member's latest message and flash it. Used by the head chips and by #<key> links.
function focusMember(key) {
  const all = el.inner.querySelectorAll(".msg[data-key], .note[data-key]");
  const target = [...all].filter((n) => n.dataset.key === key).pop();
  if (!target) return;
  target.scrollIntoView({ block: "center", behavior: "smooth" });
  target.classList.remove("flash");
  void target.offsetWidth;
  target.classList.add("flash");
}

// ---------------------------------------------------------------- body states
function showBlank(kind) {
  const none = !kind;
  el.blank.hidden = none;
  el.tl.hidden = !none;
  el.ro.hidden = !none;
  if (none) return;
  el.head.hidden = true;
  state.headKey = "";
  state.loaded = null;
  const text = {
    nogroups: ["还没有群聊", "在主控 Agent（Claude Code、Codex CLI 等）里派出任务或开始节目，对应的群会自动出现在这里。"],
    pick: ["选一个群开始查看", "左侧是每个主控会话的群，进行中的排在最前面。"],
    missing: ["找不到这个群", "它可能已被归档或删除。"],
    loading: ["正在读取群聊…", ""],
    error: ["无法读取群聊", "正在重试，请稍候。"],
  }[kind];
  el.blank.replaceChildren(...[h("strong", {}, text[0]), h("p", { hidden: kind === "nogroups" && ob.visible }, text[1]), kind === "missing" && h("a.blank-link", { href: href("/chat/") }, "回到群列表")].filter(Boolean));
  el.jump.hidden = true;
  placeOb();
}

// ---------------------------------------------------------------- refresh
async function refresh() {
  if (state.refreshing) { state.again = true; return; }
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
    if (state.auto && !narrow.matches && !chats.groups.some((g) => g.group === state.selected))
      state.selected = chats.groups[0]?.group ?? null;
    if (!narrow.matches && state.selected) el.app.dataset.view = "chat";
    renderList();
    if (!state.selected) {
      showBlank(chats.groups.length ? "pick" : "nogroups");
      return;
    }
    const selected = state.selected;
    if (state.loaded !== selected) showBlank("loading");
    let chat;
    try {
      chat = await load(`/app/chats/${encodeURIComponent(selected)}`);
      if (selected !== state.selected) { state.again = true; return; }
    } catch (e) {
      if (selected !== state.selected) { state.again = true; return; }
      if (e.status === 404) showBlank("missing");
      else {
        el.banner.hidden = false;
        if (!state.loaded) showBlank("error");
      }
      return;
    }
    state.chat = chat;
    showBlank(null);
    renderHead(chat);
    renderTimeline(chat);
    renderList(); // selected marker
    state.ready = true;
  } finally {
    state.refreshing = false;
    if (state.again) { state.again = false; queueMicrotask(refresh); }
  }
}

for (const link of document.querySelectorAll("a[href]")) setAttr(link, "href", href(link.getAttribute("href")));
sync();
poll(refresh);

ob.start();
