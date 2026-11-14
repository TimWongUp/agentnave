"use strict";
setInterval(() => {
  if (document.hidden) return;
  for (const rec of state.nodes.values()) {
    if (rec.kind !== "typing" || !rec.statusMessage || !rec.status?.isConnected) continue;
    const status = typingStatus(rec.statusMessage);
    setText(rec.status, status);
    rec.status.hidden = !status;
  }
}, 1000);
function applyFolds(records) {
  // Keep geometry reads together: alternating styles and scrollHeight for each message
  // forces the browser to lay out the whole conversation again for every bubble.
  const visible = [...records]
    .filter((rec) => rec.more && rec.el.isConnected && rec.el.offsetParent)
    .map((rec) => ({ rec, open: state.expanded.has(rec.id) }));
  for (const { rec, open } of visible)
    rec.bub.classList.toggle("fold", !open);
  const measured = visible.map(({ rec, open }) => ({
    rec,
    open,
    // A peer always has more to show than its one line.
    overflows: rec.el.dataset.peer !== undefined || rec.el.dataset.brief !== undefined || rec.bub.scrollHeight > FOLD_PX + 6, // ignores the clamp
  }));
  for (const { rec, open, overflows } of measured) {
    rec.more.hidden = !overflows;
    rec.more.textContent = open ? L("收起", "Show less") : L("展开全文", "Show more");
    rec.more.setAttribute("aria-expanded", open ? "true" : "false");
    rec.bub.classList.toggle("fold", !open && overflows);
    rec.dirty = false;
  }
}
function measureAll() {
  applyFolds(state.nodes.values());
}

// ---------------------------------------------------------------- conversation column
function openScope(scope, opener) {
  if (opener) state.opener = opener;
  state.drawer = scope;
  state.convOpen = true;
  // picking a desk or room is asking to read it; a click inside the peeking column keeps it folded
  if (!state.convPeek) setCollapsed(false);
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
  state.convPeek = false; // a docked or newly folded column is not peeking
  el.app.removeAttribute("data-conv-peek");
  store(COLLAPSED_KEY, on ? "1" : null);
}
el.convToggle.addEventListener("click", () => {
  setCollapsed(!state.convCollapsed);
  if (state.chat) renderAll(state.chat);
});

// The workspace column folds away the same way; the grid itself is told through data-side.
function renderSideToggle() {
  const L = AN.L || ((zh) => zh);
  const shut = state.sideCollapsed;
  const label = shut ? L("显示工作区列表", "Show workspace list") : L("隐藏工作区列表", "Hide workspace list");
  if (el.app.dataset.side !== (shut ? "off" : "on")) el.app.dataset.side = shut ? "off" : "on";
  setAttr(el.sideToggle, "aria-expanded", shut ? "false" : "true");
  setAttr(el.sideToggle, "aria-label", label);
  setAttr(el.sideToggle, "title", label);
}
el.sideToggle.addEventListener("click", () => {
  state.sideCollapsed = !state.sideCollapsed;
  store(SIDE_COLLAPSED_KEY, state.sideCollapsed ? "1" : null);
  state.sidePeek = false;
  el.app.removeAttribute("data-side-peek");
  renderSideToggle();
});
renderSideToggle();
// The conversation button belongs at the bar's right end, beside the theme switch.
document.querySelector("header.bar .bar-end")?.append(el.convToggle);

// A folded column slides over the office while the pointer is on its top-bar button or the window's
// edge, and goes 300ms after the pointer and the focus have both left it. The saved fold is untouched.
const PEEK_EDGE = 6;
const PEEK_DELAY = 300;
const PEEK = {
  side: { panel: el.app.querySelector(".side"), button: el.sideToggle, can: () => state.sideCollapsed && !narrow.matches, atEdge: (x) => x <= PEEK_EDGE },
  conv: { panel: el.drawer, button: el.convToggle, can: () => state.convCollapsed && !overlay.matches, atEdge: (x) => x >= innerWidth - PEEK_EDGE },
};
const peekTimers = {};
function setPeek(name, on) {
  if (state[`${name}Peek`] === on) return;
  state[`${name}Peek`] = on;
  el.app.toggleAttribute(`data-${name}-peek`, on);
  if (name === "conv" && state.chat) renderAll(state.chat); // the drawer is hidden while folded
}
function showPeek(name) {
  clearTimeout(peekTimers[name]);
  if (PEEK[name].can()) setPeek(name, true);
}
function hidePeekSoon(name) {
  const { panel, button } = PEEK[name];
  clearTimeout(peekTimers[name]);
  peekTimers[name] = setTimeout(() => {
    // only keyboard focus holds it open; a click leaves focus behind that the pointer has already left
    if (panel.matches(":hover") || button.matches(":hover") || panel.querySelector(":focus-visible")) return;
    setPeek(name, false);
  }, PEEK_DELAY);
}
for (const [name, { panel, button }] of Object.entries(PEEK)) {
  button.addEventListener("pointerenter", () => showPeek(name));
  button.addEventListener("pointerleave", () => hidePeekSoon(name));
  panel.addEventListener("pointerleave", () => hidePeekSoon(name));
  panel.addEventListener("focusout", () => hidePeekSoon(name));
}
// Listening on the whole document leaves nothing at the edge to cover the office or its scrollbar.
document.addEventListener("pointermove", (e) => {
  if (e.buttons) return;
  for (const [name, { atEdge }] of Object.entries(PEEK))
    if (atEdge(e.clientX) && e.clientY >= el.app.getBoundingClientRect().top) showPeek(name);
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
// A collapsed peer opens from anywhere on its bubble, as its button says.
el.inner.addEventListener("click", (event) => {
  if (event.target.closest(".more")) return;
  event.target.closest("[data-peer] .bub.fold")?.parentElement.querySelector(".more")?.click();
});

// Messages of the open scope. A desk's conversation is its own messages (private and meeting) plus
// the boss prompts that were addressed to it.
function inScope(m, ctx) {
  const d = state.drawer;
  if (d.kind === "all") return true;
  if (d.kind === "room") return m.conversation === d.id;
  return inDesk(m, d.key, ctx);
}
function inDesk(m, key, ctx) {
  if (m.desk === key) return true;
  if (Array.isArray(m.to) && m.to.some((t) => t?.key === key)) return true;
  return (m.recipients || []).some(
    (r) => r.seat && ctx.seatDesk.get(`${m.conversation}|${r.seat}`) === key,
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
        sub: `${cliOf(desk)}${desk.role ? ` · ${desk.role}` : ""} · ${L(`${real} 条消息`, `${real} messages`)}`,
      };
  }
  if (d.kind === "room") {
    const room = ctx.rooms.get(d.id);
    if (room)
      return {
        key: `room|${d.id}`,
        ico: () => h("span", {}, L("会", "M")),
        title: room.title,
        sub: L(`会议室 · ${room.seats.length} 位 · ${roundText(room)}`, `Meeting room · ${room.seats.length} seats · ${roundText(room)}`),
      };
  }
  return {
    key: `all|${c.host}|${c.label}`,
    ico: () => hostTile(c.app || c.host, c.label),
    title: "boss",
    sub: L(`${c.label} · 全部消息 ${real} 条`, `${c.label} · ${real} messages`),
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
    if (m.reply_to && listed.has(m.reply_to)) {
      if (!replies.has(m.reply_to)) replies.set(m.reply_to, []);
      replies.get(m.reply_to).push(m);
    } else top.push(m);
  }
  return top.flatMap((m) => [m, ...instructionsFirst(replies.get(m.id) || [])]);
}
// A private instruction sits right above the answer it asked for; one whose seat has not answered
// yet ends the thread.
function instructionsFirst(replies) {
  const waiting = new Map();
  for (const m of replies)
    if (m.kind === "instruction") {
      const seat = m.recipients?.[0]?.focus;
      waiting.set(seat, [...(waiting.get(seat) || []), m]);
    }
  const ordered = replies
    .filter((m) => m.kind !== "instruction")
    .flatMap((m) => {
      if (m.kind === "approval") return [m];
      const asked = waiting.get(m.focus) || [];
      waiting.delete(m.focus);
      return [...asked, m];
    });
  return [...ordered, ...[...waiting.values()].flat()];
}
function renderConv(c, ctx) {
  const L = AN.L || ((zh) => zh);
  el.drawer.hidden = overlay.matches ? !state.convOpen : state.convCollapsed && !state.convPeek;
  // The fold button is in the top bar; the overlay has its own close button.
  el.convToggle.hidden = overlay.matches;
  const shut = state.convCollapsed;
  const hint = shut ? L("显示对话栏", "Show conversation") : L("隐藏对话栏", "Hide conversation");
  setAttr(el.convToggle, "aria-expanded", shut ? "false" : "true");
  setAttr(el.convToggle, "aria-label", hint);
  setAttr(el.convToggle, "title", hint);
  if (el.drawer.hidden) {
    AN.voice?.controls(false, null, ctx);
    state.scopeKey = "";
    return;
  }
  // a selection that no longer exists falls back to the whole workspace
  const d = state.drawer;
  if (
    (d.kind === "desk" && !ctx.desks.has(d.key)) ||
    (d.kind === "room" && !ctx.rooms.has(d.id)) ||
    (d.kind === "isolated" && !c.isolated?.length)
  )
    state.drawer = { kind: "all" };
  el.drawer.classList.toggle("tp-room", state.drawer.kind === "room");
  const desk = state.drawer.kind === "desk" ? state.drawer.key : null;
  // Listening to a seat excludes the folded replies from its peers.
  AN.voice?.controls(true, state.drawer.kind === "room" ? { group: c.group, id: state.drawer.id } : null, ctx,
    desk, desk ? chat => deskStream(chat.messages, contextOf(chat), desk).shown : null,
    desk ? chat => { const next = contextOf(chat); return chat.messages.filter(m => inDesk(m, desk, next)); } : null);
  if (state.drawer.kind === "all") {
    renderDirectory(c, ctx);
    return;
  }
  if (state.drawer.kind === "isolated") {
    renderIsolatedDrawer(c, ctx);
    return;
  }
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
  const stream = state.drawer.kind === "desk" ? deskStream(c.messages, ctx, state.drawer.key) : null;
  state.peers = stream?.peers || new Set();
  const shown = threaded(stream ? stream.shown : c.messages.filter((m) => inScope(m, ctx)));
  const head = scopeHead(c, ctx, shown);
  if (el.drIco.dataset.key !== head.key) {
    el.drIco.dataset.key = head.key;
    el.drIco.replaceChildren(head.ico());
  }
  setText(el.drTitle, head.title);
  setText(el.drSub, head.sub);
  el.drAll.hidden = state.drawer.kind === "all";
  setAttr(el.drawer, "aria-label", L(`${head.title} 的对话`, `${head.title} conversation`));
  renderConversationMessages(shown, ctx, reset);
}
function renderConversationMessages(shown, ctx, reset) {
  const L = AN.L || ((zh) => zh);
  // Geometry is the truth: a reader who is at the bottom follows new content, anyone else is left alone.
  const follow = reset || nearBottom();
  const order = [];
  const ordered = new Set();
  let day = null,
    round = null,
    stamped = null;
  const node = (id, make) => {
    let rec = state.nodes.get(id);
    if (!rec) state.nodes.set(id, (rec = make()));
    order.push(id);
    ordered.add(id);
    return rec;
  };
  const separator = (id, text) => {
    const rec = node(id, () => ({ el: h("div.sep", {}, ""), sep: true }));
    setText(rec.el, text);
  };
  let fresh = 0;
  for (const m of shown) {
    const date = new Date(AN.at(m.time)).toDateString();
    // a thread stays together: only messages that start one open a new day
    const inThread = m.reply_to && ordered.has(m.reply_to);
    if (date !== day && !inThread) {
      day = date;
      round = null;
      separator(`day:${m.id}`, dayLabel(m.time));
    }
    if (state.drawer.kind === "room" && m.round > 0 && m.round !== round) {
      round = m.round;
      separator(`round:${m.conversation}:${m.round}`, L(`第 ${m.round} 轮`, `Round ${m.round}`));
    }
    // Like a messenger: a time line opens each stretch instead of a time on every message.
    if (m.kind !== "typing") {
      const at = AN.at(m.time);
      if (stamped === null || Math.abs(at - stamped) >= 5 * 60e3) {
        stamped = at;
        separator(`time:${m.id}`, clock(m.time));
      }
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
      el: h("div.dr-empty", {}, L("这里还没有消息。", "No messages yet.")),
      sep: true,
    }));
  const keep = new Set(order);
  for (const id of state.nodes.keys())
    if (!keep.has(id)) state.nodes.delete(id);
  reconcile(
    el.inner,
    [...keep].map((id) => state.nodes.get(id).el),
  );
  applyFolds([...state.nodes.values()].filter((rec) => rec.dirty));

  if (follow) {
    el.tl.scrollTop = el.tl.scrollHeight;
    state.unseen = 0;
  } else if (fresh) state.unseen += fresh;
  updateJump();
}

function updateJump() {
  el.jump.hidden = !(state.unseen > 0 && !nearBottom());
  el.jump.textContent = L(`${state.unseen} 条新消息 ↓`, `${state.unseen} new messages ↓`);
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
      .filter((n) => n.dataset.mid === mid)
      .pop();
  let target = find();
  if (!target) {
    const message = state.chat.messages.find((m) => m.message_id === mid);
    state.drawer = tpScopeOf(message, state.ctx);
    renderAll(state.chat);
    target = find();
  }
  if (target) flash(target);
}
// Scroll to the message playing, once its workspace and conversation are on screen.
function playingMessage() {
  const job = state.locate;
  return job && job.group === state.chat?.group && state.chat.messages.find((m) => m.id === job.id);
}
function locatePlaying() {
  if (!playingMessage()) return false;
  state.focused = true; // this message, not the end of the conversation the address names
  if (state.locating) return true;
  state.locating = true;
  // Two frames on: scrolling in the frame that drops the settings overlay leaves the list unpainted.
  requestAnimationFrame(() => requestAnimationFrame(() => {
    state.locating = false;
    const message = playingMessage();
    state.locate = null;
    if (!message) return;
    openScope(tpScopeOf(message, state.ctx));
    const target = state.nodes.get(message.id)?.el;
    if (!target) return;
    // At once: the poll that follows the window coming up would take a smooth scroll back to the end.
    target.scrollIntoView({ block: "center" });
    flash(target);
  }));
  return true;
}
addEventListener("agentnave:voice-locate", ({ detail }) => {
  state.locate = detail;
  if (locatePlaying()) return;
  // Not loaded: another workspace, or too far back for the latest messages.
  const key = detail.conversation ? `conv:${detail.conversation}` : detail.album?.link;
  go(`/chat/${encodeURIComponent(detail.group)}${key ? `#${encodeURIComponent(key)}` : ""}`);
});
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
      L("还没有工作区", "No workspaces yet"),
      L("在 boss（Claude Code、Codex CLI 等宿主里的 Agent）里派出任务或开始会议，对应的办公室会自动出现在这里。", "Dispatch a task or start a meeting from boss (your agent in Claude Code, Codex CLI, or another host) and its office will appear here automatically."),
    ],
    pick: [
      L("选一个工作区开始查看", "Select a workspace"),
      L("左侧是每个 boss 会话的工作区，进行中的排在最前面。", "Workspaces for each boss session are listed on the left, with active ones first."),
    ],
    missing: [L("找不到这个工作区", "Workspace not found"), L("它可能已被删除。", "It may have been deleted.")],
    loading: [L("正在读取办公室…", "Loading office…"), ""],
    error: [L("无法读取办公室", "Failed to load office"), L("正在重试，请稍候。", "Retrying, please wait.")],
  }[kind];
  el.blank.replaceChildren(
    ...[
      h("strong", {}, text[0]),
      h("p", {}, text[1]),
      kind === "missing" &&
        h("a.blank-link", { href: href("/chat/") }, L("回到工作区列表", "Back to workspaces")),
    ].filter(Boolean),
  );
  el.jump.hidden = true;
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
        .filter((m) => m.message_id)
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
  renderConv(c, ctx);
  renderSideToggle();
  renderTicker(c, ctx);
  renderOffice(c, ctx, n, newGroup);
  if (state.locate) locatePlaying();
}
// #desk:<key> opens a desk's conversation, #conv:<id> a room's transcript (or everything, for a task).
function applyHash(ctx) {
  const key = state.hashKey;
  // Loaded and not there: returning here later must not jump to it.
  if (!key && !state.locating && state.locate?.group === state.chat?.group) state.locate = null;
  if (!key || state.focused) return;
  state.focused = true;
  state.locate = null; // its conversation is loaded and it is not there
  if (key.startsWith("desk:")) {
    // A link made while a task had no native session yet names its earlier task key.
    const desk = ctx.aliases.get(key.slice(5)) ?? key.slice(5);
    if (ctx.desks.has(desk)) openScope({ kind: "desk", key: desk });
  } else if (key.startsWith("conv:")) {
    const id = key.slice(5);
    const last = state.chat?.messages.findLast((m) => m.conversation === id);
    openScope(ctx.rooms.has(id) ? { kind: "room", id } : tpScopeOf(last, ctx));
    setTimeout(() => focusConversation(id), 0);
  }
}

async function refresh() {
  void AN.providerHealth?.refresh(document.getElementById("provider-health-alert"));
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
      setText(el.banner, L("读取中断，正在重试…", "Connection interrupted, retrying…"));
      el.banner.hidden = false;
      state.chatKey = null;
      if (!state.loaded && !state.groupsData.length) showBlank("error");
      return;
    }
    el.banner.hidden = true;
    state.groupsData = chats.groups;
    if (
      state.auto &&
      !narrow.matches &&
      !chats.groups.some((g) => g.group === state.selected)
    )
      state.selected = chats.groups[0]?.group ?? null;
    if (!narrow.matches && state.selected) el.app.dataset.view = "chat";
    const listKey = JSON.stringify([chats.groups, state.selected, tickOf(), document.documentElement.lang]);
    if (listKey !== state.listKey) {
      state.listKey = listKey;
      renderList();
    }
    if (!state.selected) {
      showBlank(chats.groups.length ? "pick" : "nogroups");
      return;
    }
    const selected = state.selected;
    const hashKey = state.hashKey;
    const focus = hashKey?.startsWith("conv:") ? hashKey.slice(5) : null;
    if (!state.loaded) showBlank("loading");
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
      state.chatKey = null;
      if (e.status === 404) showBlank("missing");
      else {
        setText(el.banner, L("读取中断，正在重试…", "Connection interrupted, retrying…"));
        el.banner.hidden = false;
        if (!state.loaded) showBlank("error");
      }
      return;
    }
    const newGroup = state.loaded !== chat.group;
    state.loaded = chat.group;
    // The same payload within the same tick draws the same office; only things that move with
    // the clock or wait for this load need a fresh pass.
    const chatKey = JSON.stringify([selected, hashKey, chat, tickOf(), ART.version(), document.documentElement.lang]);
    const unchanged = state.ready && !newGroup && chatKey === state.chatKey && !state.locate
      && !(state.hashKey && !state.focused) && !timeDriven();
    // The voice player keeps following the workspace and reloads its own settings sparingly.
    if (unchanged) {
      AN.voice?.ingest(chat, { whole: !focus });
      return;
    }
    if (newGroup) {
      state.drawer = { kind: "all" };
      state.convOpen = false;
      state.opener = null;
      state.scopeKey = "";
      state.tickId = null;
      if (state.wantIsolated === chat.group) {
        state.drawer = { kind: "isolated" };
        state.convOpen = true;
        setCollapsed(false);
      }
    }
    state.wantIsolated = null;
    state.chat = chat;
    showBlank(null);
    // Office polls also refresh voice settings.
    AN.voice?.ingest(chat, { whole: !focus });
    renderAll(chat, newGroup);
    ingest(chat, state.ctx, newGroup);
    applyHash(state.ctx);
    state.chatKey = chatKey;
    state.drawnAt = Date.now();
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
// A model-name choice changes names in every list and message; redraw them now instead of at the next change.
addEventListener("agentnave:model-names", () => {
  state.listKey = null;
  state.chatKey = null;
  refresh();
});
addEventListener("agentnave:voice", () => {
  if (state.chat) renderConv(state.chat, state.ctx);
});
sync();
poll(refresh);
