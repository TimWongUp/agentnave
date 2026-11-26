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
function openScope(scope, opener, options = {}) {
  if (opener) state.opener = opener;
  const officeTop = el.office?.scrollTop ?? 0;
  const floorW = el.floor?.clientWidth ?? 0;
  const anchor = opener && el.office?.contains?.(opener) ? opener : null;
  const anchorTop = anchor?.getBoundingClientRect?.().top ?? null;
  state.drawer = scope;
  state.convOpen = true;
  ChatTerminal.readMessages(Boolean(options.terminal));
  // picking a desk or room is asking to read it; a click inside the peeking column keeps it folded
  if (!state.convPeek) setCollapsed(false);
  if (state.chat) renderAll(state.chat);
  if (el.office && el.floor) {
    if (el.floor.clientWidth === floorW) {
      el.office.scrollTop = officeTop;
    } else if (anchorTop != null && anchor.isConnected) {
      el.office.scrollTop = officeTop + (anchor.getBoundingClientRect().top - anchorTop);
    }
  }
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
// An unfolded column slides in; the class is dropped and set again so a second unfold plays too.
function slideIn(panel) {
  panel.classList.remove("docking");
  void panel.offsetWidth;
  panel.classList.add("docking");
}
el.convToggle.addEventListener("click", () => {
  setCollapsed(!state.convCollapsed);
  if (!state.convCollapsed) slideIn(el.drawer);
  if (state.chat) renderAll(state.chat);
});

// The workspace column folds away the same way; the grid itself is told through data-side.
// Between 761 and 1100px the list is a 64px rail that never folds away: its button lays the full list over
// the office and back instead (the peek stays until a click outside it, Escape, or a choice in it).
const railMode = () => rail.matches && !narrow.matches;
function renderSideToggle() {
  const L = AN.L || ((zh) => zh);
  const rail = railMode();
  const shut = rail ? !state.sidePeek : state.sideCollapsed;
  const label = shut ? L("显示项目列表", "Show project list") : L("隐藏项目列表", "Hide project list");
  const side = rail || !state.sideCollapsed ? "on" : "off";
  if (el.app.dataset.side !== side) el.app.dataset.side = side;
  setAttr(el.sideToggle, "aria-expanded", shut ? "false" : "true");
  setAttr(el.sideToggle, "aria-label", label);
  setAttr(el.sideToggle, "title", label);
}
function toggleRailPeek(on) {
  setPeek("side", on);
  if (on) slideIn(el.app.querySelector(".side"));
  renderSideToggle();
}
el.sideToggle.addEventListener("click", () => {
  if (railMode()) return toggleRailPeek(!state.sidePeek);
  state.sideCollapsed = !state.sideCollapsed;
  store(SIDE_COLLAPSED_KEY, state.sideCollapsed ? "1" : null);
  state.sidePeek = false;
  el.app.removeAttribute("data-side-peek");
  if (!state.sideCollapsed) slideIn(el.app.querySelector(".side"));
  renderSideToggle();
});
renderSideToggle();
const sideColumn = el.app.querySelector(".side");
document.addEventListener(
  "pointerdown",
  (e) => {
    if (!railMode() || !state.sidePeek) return;
    if (!sideColumn.contains(e.target) && !el.sideToggle.contains(e.target)) toggleRailPeek(false);
  },
  true,
);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && railMode() && state.sidePeek) toggleRailPeek(false);
});
sideColumn.addEventListener("click", (e) => {
  if (railMode() && state.sidePeek && e.target.closest?.(".gi")) toggleRailPeek(false);
});
// The conversation button belongs at the bar's right end, beside the theme switch.
document.querySelector("header.bar .bar-end")?.append(el.convToggle);

// A folded column slides over the office while the pointer is on its top-bar button or the window's
// edge, and goes 300ms after the pointer and the focus have both left it. The saved fold is untouched.
const PEEK_EDGE = 6;
const PEEK_DELAY = 300;
const PEEK = {
  side: { panel: el.app.querySelector(".side"), button: el.sideToggle, can: () => state.sideCollapsed && !rail.matches, atEdge: (x) => x <= PEEK_EDGE },
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
  if (name === "side" && railMode()) return; // the rail's list stays until it is dismissed
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
// a minimum and a maximum so the office in the middle never gets squeezed out. The right column
// remembers its width separately for agents with and without a terminal.
const SPLITS = {
  side: { key: "agentnave.office.side", min: 0.14, max: 0.28, floor: 240, css: "--side-w" },
  conv: { key: "agentnave.office.conv", min: 0.22, floor: 300, css: "--conv-w" },
  convTerm: { key: "agentnave.office.conv.term", min: 0.22, floor: 300, css: "--conv-term-w" },
};
function maxShare(name) {
  if (name === "side") return SPLITS.side.max;
  const appW = el.app.getBoundingClientRect().width;
  if (!appW) return 0.72;
  const sideW = state.sideCollapsed ? 0 : (el.app.querySelector(".side")?.getBoundingClientRect().width || 248);
  return Math.max(SPLITS[name].min, (appW - sideW - 350) / appW);
}
function applySplit(name, share) {
  const s = SPLITS[name];
  const handle = el.app.querySelector(`[data-split="${name === "side" ? "side" : "conv"}"]`);
  if (share == null) {
    el.app.style.removeProperty(s.css);
    handle.removeAttribute("aria-valuenow");
    return;
  }
  const clamped = Math.min(maxShare(name), Math.max(s.min, share));
  if (name === "side") {
    el.app.style.setProperty(s.css, `max(${s.floor}px, ${(clamped * 100).toFixed(2)}%)`);
  } else {
    el.app.style.setProperty(s.css, `min(max(${s.floor}px, ${(clamped * 100).toFixed(2)}%), calc(100vw - var(--side-used, 248px) - 350px))`);
  }
  handle.setAttribute("aria-valuenow", String(Math.round(clamped * 100)));
  return clamped;
}
for (const [name, s] of Object.entries(SPLITS)) {
  const saved = Number(stored(s.key));
  if (saved) applySplit(name, saved);
}
for (const slot of ["side", "conv"]) {
  const handle = el.app.querySelector(`[data-split="${slot}"]`);
  const activeName = () => (slot === "side" ? "side" : el.app.dataset.hasTerm === "1" ? "convTerm" : "conv");
  handle.setAttribute("aria-valuemin", String(SPLITS[slot].min * 100));
  handle.setAttribute("aria-valuemax", String(slot === "side" ? SPLITS.side.max * 100 : 75));
  const shareAt = (x) => {
    const r = el.app.getBoundingClientRect();
    return (slot === "side" ? x - r.left : r.right - x) / r.width;
  };
  const current = () => {
    const col = slot === "side" ? handle.parentElement : el.drawer;
    return col.getBoundingClientRect().width / el.app.getBoundingClientRect().width;
  };
  const save = (name, share) => store(SPLITS[name].key, share == null ? null : share.toFixed(4));
  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const name = activeName();
    handle.setPointerCapture(e.pointerId);
    el.app.classList.add("resizing");
    let last = null;
    const move = (ev) => { last = applySplit(name, shareAt(ev.clientX)); };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      el.app.classList.remove("resizing");
      if (last != null) save(name, last);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("lostpointercapture", end, { once: true });
  });
  // Double-click returns to the default width.
  handle.addEventListener("dblclick", () => {
    const name = activeName();
    applySplit(name, null);
    save(name, null);
  });
  handle.addEventListener("keydown", (e) => {
    const step = { ArrowLeft: -0.01, ArrowRight: 0.01 }[e.key];
    if (!step) return;
    e.preventDefault();
    const name = activeName();
    save(name, applySplit(name, current() + (slot === "side" ? step : -step)));
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
function roomIcon() {
  return h("img.dr-ico-img", { src: "/art/office/meeting-icon.webp", alt: "", draggable: false });
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
        ico: roomIcon,
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
function renderConv(c, ctx, forceMeasure = false) {
  const L = AN.L || ((zh) => zh);
  // Beside the floor a folded drawer stays in the page, out to the right, so it can slide; only the overlay hides it.
  const visible = overlay.matches ? state.convOpen : !state.convCollapsed || state.convPeek;
  const wasHidden = Boolean(el.drawer.hidden);
  const wasFolded = Boolean(el.drawer.classList?.contains?.("folded"));
  const wasVisible = Boolean(state.convVisible);
  state.convVisible = visible;
  el.drawer.hidden = overlay.matches && !visible;
  const folded = !overlay.matches && state.convCollapsed;
  el.drawer.classList.toggle("folded", folded);
  // The fold button is in the top bar; the overlay has its own close button.
  el.convToggle.hidden = overlay.matches;
  const shut = state.convCollapsed;
  const hint = shut ? L("显示对话栏", "Show conversation") : L("隐藏对话栏", "Hide conversation");
  setAttr(el.convToggle, "aria-expanded", shut ? "false" : "true");
  setAttr(el.convToggle, "aria-label", hint);
  setAttr(el.convToggle, "title", hint);
  if (!visible) {
    AN.voice?.controls(false, null, ctx);
    // a drawer that is only folded keeps its conversation, scroll position and unread count included
    if (overlay.matches) state.scopeKey = "";
    return;
  }
  // a selection that no longer exists falls back to the whole workspace
  const d = state.drawer;
  if (
    (d.kind === "desk" && !ctx.desks.has(d.key)) ||
    (d.kind === "room" && !ctx.rooms.has(d.id)) ||
    (d.kind === "isolated" && !c.isolated?.length && !(typeof ChatIsolatedCleanup !== "undefined" && ChatIsolatedCleanup.doneCount(c.group)))
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
  setAttr(el.drSub, "title", null);
  el.drAll.hidden = state.drawer.kind === "all";
  setAttr(el.drawer, "aria-label", L(`${head.title} 的对话`, `${head.title} conversation`));
  const becameVisible = forceMeasure || (wasHidden && !el.drawer.hidden) || (wasFolded && !folded) || !wasVisible;
  renderConversationMessages(shown, ctx, reset, becameVisible);
}
function renderConversationMessages(shown, ctx, reset, remeasure = false) {
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
  applyFolds(remeasure ? state.nodes.values() : [...state.nodes.values()].filter((rec) => rec.dirty));

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
  AN.onboarding?.place(kind === "nogroups");
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
      L("还没有 agent", "No agents yet"),
      L("在 boss（Claude Code、Codex CLI 等宿主里的 Agent）里派出任务或开始会议，对应的办公室会自动出现在这里。", "Dispatch a task or start a meeting from boss (your agent in Claude Code, Codex CLI, or another host) and its office will appear here automatically."),
    ],
    pick: [
      L("选一个 agent 开始查看", "Select an agent"),
      L("左侧按项目列出每个 boss 会话的 agent，进行中的排在最前面。", "Agents for each boss session are listed by project on the left, with active ones first."),
    ],
    missing: [L("找不到这个 agent", "Agent not found"), L("它可能已被删除。", "It may have been deleted.")],
    loading: [L("正在读取办公室…", "Loading office…"), ""],
    error: [L("无法读取办公室", "Failed to load office"), L("正在重试，请稍候。", "Retrying, please wait.")],
  }[kind];
  $("blank-note").replaceChildren(
    ...[
      h("strong", {}, text[0]),
      h("p", {}, text[1]),
      kind === "missing" &&
        h("a.blank-link", { href: href("/chat/") }, L("回到项目列表", "Back to projects")),
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
  const wasTerm = Boolean(el.drawer?.classList?.contains?.("term-mode"));
  ChatTerminal.prepare?.(c.group, c.host);
  const leftTerm = wasTerm && !el.drawer?.classList?.contains?.("term-mode");
  renderConv(c, ctx, leftTerm);
  ChatTerminal.column(c.group, c.host, c.app);
  if (typeof syncListSelection === "function") syncListSelection();
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
    if (ctx.desks.has(desk)) {
      const wantTerm = Boolean(ChatTerminal.consumeOpenDesk?.(state.chat?.group, desk));
      openScope({ kind: "desk", key: desk }, null, wantTerm ? { terminal: true } : {});
      if (wantTerm) ChatTerminal.focusFrame?.(ChatTerminal.ofDesk?.(state.chat?.group, desk)?.id);
    }
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
    await ChatTerminal.refresh();
    state.projects = chats.projects || [];
    state.sidebar = typeof ChatGroups !== "undefined" && ChatGroups.normalizeSidebar
      ? ChatGroups.normalizeSidebar(chats.sidebar)
      : (chats.sidebar || null);
    state.groupsData = [...chats.groups, ...ChatTerminal.pending()];
    const derived = ChatTerminal.derivedAll?.() || [];
    // A workspace is found and counts as active through its derived sessions too.
    for (const g of state.groupsData) {
      const own = ChatTerminal.derivedOf?.(g.group) || [];
      g.derived_live = own.some((d) => d.live);
      g.derived_titles = own.map((d) => d.title).join("\n");
    }
    const selectable = [...state.groupsData, ...derived];
    const followed = ChatTerminal.follow(state.selected);
    if (followed !== state.selected) {
      state.selected = followed;
      history.replaceState({}, "", href(`/chat/${followed}${state.hashKey ? `#${encodeURI(state.hashKey)}` : ""}`));
    }
    if (
      state.auto &&
      !narrow.matches &&
      !selectable.some((g) => g.group === state.selected)
    )
      state.selected = state.groupsData[0]?.group ?? null;
    if (!narrow.matches && state.selected) el.app.dataset.view = "chat";
    const openDesk = state.drawer?.kind === "desk" ? state.drawer.key : null;
    const listKey = JSON.stringify([state.groupsData, derived, state.projects, state.sidebar, state.selected, openDesk, tickOf(), document.documentElement.lang]);
    if (listKey !== state.listKey) {
      state.listKey = listKey;
      renderList();
    }
    if (typeof ChatGit !== "undefined") {
      ChatGit.observe?.(state.groupsData, ChatTerminal.all?.() || [], state.selected);
    }
    if (!state.selected) {
      showBlank(chats.groups.length ? "pick" : "nogroups");
      return;
    }
    const selected = state.selected;
    const hashKey = state.hashKey;
    const focus = hashKey?.startsWith("conv:") ? hashKey.slice(5) : null;
    // An agent that has not dispatched has an office with only its boss, drawn here.
    const agent = ChatTerminal.pendingId(selected) != null;
    if (!state.loaded && !agent) showBlank("loading");
    let chat;
    try {
      chat = agent
        ? ChatTerminal.office(selected)
        : await load(
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
    const chatKey = JSON.stringify([selected, hashKey, chat, ChatTerminal.displayState(selected), tickOf(), ART.version(), document.documentElement.lang]);
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
      } else if (ChatTerminal.consumeOpen?.(chat.group)) {
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
  if (state.chat) {
    renderConv(state.chat, state.ctx);
    ChatTerminal.column(state.chat.group, state.chat.host, state.chat.app);
  }
});
sync();
poll(refresh);
