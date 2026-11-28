"use strict";
function counts(c) {
  let running = 0;
  for (const d of c.desks)
    running += activities(d).filter((a) => a.status === "running").length;
  const review = c.rooms.reduce((n, r) => n + (r.pending_review || 0), 0);
  return { running, review };
}
// Mood picks the character frame; word is the plain status shown under the desk.
function deskMood(d, ctx) {
  if (d.derived && typeof ChatDerived !== "undefined" && ChatDerived.mood) return ChatDerived.mood(d, ctx);
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
      word: meeting ? L("私聊处理中 · 另一席在开会", "In private chat · also in meeting") : L("私聊处理中", "In private chat"),
      away: null,
    };
  if (meeting) {
    const room = ctx.rooms.get(meeting.room);
    return {
      mood: "idle",
      say: L("去开会了", "In meeting"),
      word: L(`在会议室「${room?.title || "会议"}」`, `In meeting "${room?.title || "Meeting"}"`),
      away: `${meeting.room}|${meeting.seat}`,
    };
  }
  if (d.status === "failed")
    return { mood: "failed", say: L("上次失败", "Failed"), word: L("上次失败", "Last failed"), away: null };
  if (d.status === "pending")
    return { mood: "idle", say: "", word: L("等待中", "Pending"), away: null };
  return AN.dozing(d)
    ? {
        mood: "sleep",
        say: "zz",
        word: L("休息中", "Resting"),
        away: null,
      }
    : { mood: "idle", say: "", word: L("摸鱼", "Slacking off"), away: null };
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
      { href: href("/chat/"), "aria-label": L("返回项目列表", "Back to projects") },
      L("‹ 项目", "‹ Projects"),
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
      iso: h("button.pill.act", { type: "button" }),
    };
    state.head.iso.addEventListener("click", () => openScope({ kind: "isolated" }, state.head.iso));
    el.head.replaceChildren(
      back,
      state.head.title,
      state.head.sub,
      h("span.fh-sum", {}, state.head.iso, state.head.run),
    );
  }
  const t = state.head;
  setText(t.title, shownName(c));
  setText(t.sub, c.label);
  const dirs = c.isolated?.length || 0;
  t.iso.hidden = !dirs;
  setClass(t.iso, "pill act tone-idle");
  setText(t.iso, isolatedCount(dirs));
  t.run.hidden = !n.running && !n.review;
  setText(t.run, n.running ? L(`${n.running} 个在工作`, `${n.running} working`) : L("会议进行中", "In meeting"));
  if (typeof ChatGit !== "undefined") ChatGit.decorateHead?.(t, c);
}

// ---------------------------------------------------------------- boss office
function bossSpeech(c, match) {
  const messages = c.boss_messages || c.messages;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.speaker?.role === "manager" && m.kind === "prompt" && match(m)) return m.speaker;
  }
  return null;
}
// Only the figure waits for the new boss's first dispatch or speech; the office belongs to
// the current host immediately. The release time also distinguishes takeovers by the same
// CLI. A different old host can still speak after release, so time alone is not identity.
function seatHolder(c) {
  const marks = (c.boss_messages || c.messages).filter((m) => m.kind === "handover").map((m) => AN.at(m.time));
  const at = marks.length && c.handover === "changed" ? Math.max(...marks) : null;
  const later = (m) => m.speaker.provider === c.host && (at === null || AN.at(m.time) >= at);
  const earlier = (m) => !later(m);
  const modelOf = (when) => bossSpeech(c, (m) => m.speaker.model && when(m))?.model || "";
  const before = c.previous_host || (c.handover === "changed" && bossSpeech(c, earlier)?.provider) || "";
  // Handover metadata alone is not work: only a dispatch or hosting message earns a portrait.
  const workedBefore = (m) => earlier(m) && m.speaker.provider === before;
  if (!before || !bossSpeech(c, workedBefore)) return { host: c.host, model: modelOf(later), former: null };
  const former = { host: before, model: modelOf((m) => earlier(m) && m.speaker.provider === before) };
  if (!bossSpeech(c, later)) return { ...former, former: null };
  return { host: c.host, model: modelOf(later), former };
}
// `live` only means a server is running work right now; an idle boss dozes off like its desks do.
function recentlyActive(c) {
  const g = state.groupsData?.find?.((x) => x.group === c.group);
  const updated = g?.updated || c.updated || (typeof ChatTerminal !== "undefined" ? ChatTerminal.findDerivedRow?.(c.group)?.updated : null);
  return Boolean(updated) && Date.now() - AN.at(updated) < AN.SLEEP_AFTER;
}
const MUGS = ["mug", "mug-saucer", "mug-cream", "mug-thermos", "mug-glass", "mug-cat"];
const PLANTS = ["plant", "pot-succulent", "pot-cactus", "vase-tulip", "vase-daisy", "pot-pothos"];
const BOSS_SET = {
  claude: "claude",
  "claude-desktop": "claude",
  codex: "codex",
  "codex-desktop": "codex",
  antigravity: "agy",
  "antigravity-desktop": "agy",
  agy: "agy",
  deepseek: "deepseek",
  grok: "grok",
  codebuddy: "codebuddy",
  workbuddy: "codebuddy",
  zcode: "zcode",
  pi: "pi",
  hermes: "hermes",
};
function deskHash(s) {
  let v = 2166136261;
  for (let i = 0; i < s.length; i++) v = Math.imul(v ^ s.charCodeAt(i), 16777619);
  return v >>> 0;
}
function dressBossSet(rec, host) {
  const set = BOSS_SET[host] || "";
  if (rec.bossSet === set) return;
  rec.bossSet = set;
  rec.chair.src = ART.url(`office/${set ? `boss-chair-${set}` : "chair"}.webp`);
  rec.desk.src = ART.url(`office/${set ? `boss-desk-${set}` : "boss-desk"}.webp`);
  rec.laptop.laptop.dress(set);
}
function createBoss() {
  const rec = {
    lid: h("span.lid"),
    scene: null,
    portrait: h("span.boss-portrait", { hidden: true }),
    cli: h("div.cli"),
    st: h("div.st"),
    state: h("span.r"),
  };
  rec.chair = img("l-chair", "chair");
  rec.desk = img("l-desk", "boss-desk");
  rec.laptop = Laptop.create(rec.lid);
  rec.scene = h(
    "div.scene",
    {},
    rec.portrait,
    rec.chair,
    rec.desk,
    rec.laptop,
  );
  // boss sits on the left; meetings and derived sessions sit beside it (or below on narrow floors)
  rec.meetsSec = h("div.side-sec", { hidden: true });
  rec.derivedSec = h("div.side-sec", { hidden: true });
  rec.meets = h("div.boss-side", { hidden: true }, rec.meetsSec, rec.derivedSec);
  // Clicking the boss may open its terminal; this always opens every message of the workspace.
  const allMessages = h("button.boss-all", { type: "button" }, L("全部消息", "All messages"));
  allMessages.addEventListener("click", (e) => {
    e?.stopPropagation?.();
    openScope({ kind: "all" }, el.boss);
  });
  const usage = typeof ChatUsage !== "undefined" ? ChatUsage.mount() : null;
  el.boss.replaceChildren(
    h("h2.zone-h", {}, L("boss 办公室", "boss office"), rec.state, allMessages, AN.voice?.workspaceListen),
    (rec.row = h(
      "div.boss-row",
      {},
      // no speech bubble: the label below already says what the boss is waiting for
      h("div.boss-mid", {}, rec.scene, h("div.label", {}, rec.cli, rec.st), usage),
      rec.meets,
    )),
  );
  el.boss.onclick = () => {
    if (typeof ChatTerminal !== "undefined" && ChatTerminal?.openBoss?.()) return;
    openScope({ kind: "all" }, el.boss);
  };
  return rec;
}
function drawerVisible() {
  if (!el.drawer) return true;
  if (el.drawer.hidden) return false;
  if (el.drawer.classList?.contains?.("folded") && !state.convPeek) return false;
  return true;
}
function activeSelection() {
  if (!drawerVisible()) return null;
  if (typeof ChatTerminal !== "undefined" && ChatTerminal?.showing?.()) return { kind: "boss" };
  if (el.drawer?.classList?.contains?.("term-mode")) return { kind: "boss" };
  if (!state.drawer) return null;
  if (state.drawer.kind === "all" || state.drawer.kind === "origin") return { kind: "boss" };
  if (state.drawer.kind === "desk") return { kind: "desk", key: state.drawer.key };
  if (state.drawer.kind === "room") return { kind: "room", id: state.drawer.id };
  return null;
}
function handoverParts(c) {
  const parts = [];
  for (const item of c.handover_from || []) {
    if (item.status === "linked" && item.group) {
      parts.push({
        text: L(`接手自 ${item.label}「${item.title}」`, `Taken over from ${item.label} "${item.title}"`),
        group: item.group,
      });
    } else if (item.status === "cleaned") {
      parts.push({ text: L("接手自原 agent（已清理）", "Taken over from cleaned agent") });
    }
  }
  for (const item of c.handover_to || []) {
    if (item.status === "claimed") {
      parts.push({ text: L(`已由 ${item.label} 接手`, `Taken over by ${item.label}`) });
    } else if (item.status === "linked" && item.group) {
      parts.push({
        text: L(`已由 ${item.label}「${item.title}」接手`, `Taken over by ${item.label} "${item.title}"`),
        group: item.group,
      });
    } else if (item.status === "cleaned") {
      parts.push({ text: L(`已由 ${item.label} 接手（已清理）`, `Taken over by ${item.label} (cleaned)`) });
    }
  }
  return parts;
}
function renderBossState(rec, parts, tookOver, model) {
  const linkTexts = parts.map((p) => p.text);
  const allTexts = [...linkTexts, tookOver, model].filter(Boolean);
  const full = allTexts.join(" · ");
  if (!parts.length) {
    if (rec.stateKey) {
      rec.state.replaceChildren?.();
      rec.stateKey = "";
    }
    setText(rec.state, full);
  } else {
    const key = JSON.stringify([parts, tookOver || "", model || ""]);
    if (rec.stateKey !== key) {
      rec.stateKey = key;
      const items = [
        ...parts,
        ...(tookOver ? [{ text: tookOver }] : []),
        ...(model ? [{ text: model }] : []),
      ];
      const nodes = [];
      items.forEach((item, index) => {
        if (index > 0) nodes.push(" · ");
        if (item.group) {
          const link = h("a.boss-link", { href: href(`/chat/${encodeURIComponent(item.group)}`) }, item.text);
          link.addEventListener?.("click", (e) => {
            e.preventDefault();
            e.stopPropagation();
            go(`/chat/${encodeURIComponent(item.group)}`);
          });
          nodes.push(link);
        } else {
          nodes.push(item.text);
        }
      });
      rec.state.replaceChildren(...nodes);
    }
  }
  setAttr(rec.state, "title", full || null);
  return [...linkTexts, tookOver].filter(Boolean);
}
function updateBoss(rec, c, n) {
  const derivedOffice = typeof ChatDerived !== "undefined" && Boolean(ChatDerived.isOffice?.(c));
  const origin = derivedOffice ? ChatDerived.originOf?.(c) : null;
  const holder = seatHolder(c);
  const model = holder.model || (derivedOffice ? (origin?.desk?.model || "") : "");
  // Before any dispatch the figure wears the CLI's default model; only a reported model is named.
  const artModel = !holder.model && !holder.former && holder.host === c.host
    ? (origin?.desk?.art_model || c.boss_art_model || model)
    : model;
  dress(rec, ART.character(holder.host, artModel), rec.scene, rec.desk);
  dressBossSet(rec, c.host);
  dressLid(rec, c.app || c.host);
  hangPortrait(rec, c, holder.former);
  let mood;
  let statusText;
  if (derivedOffice && ChatDerived.bossMood) {
    const dm = ChatDerived.bossMood(c, n, recentlyActive(c));
    mood = dm.mood;
    statusText = dm.text;
  } else {
    const inTerminal = typeof ChatTerminal !== "undefined" && Boolean(ChatTerminal?.running?.(c.group));
    const hasBossAct = typeof ChatTerminal !== "undefined" && typeof ChatTerminal?.bossActivity === "function";
    const act = hasBossAct ? ChatTerminal.bossActivity(c.group) : (inTerminal ? "working" : null);
    const blocked = act === "blocked";
    mood = blocked
      ? "review"
      : n.running || n.review || act === "working"
        ? "working"
        : recentlyActive(c)
          ? "idle"
          : "sleep";
    statusText = blocked
      ? L("等待确认", "Needs confirmation")
      : n.running
        ? L(`等 ${n.running} 个回复`, `Waiting on ${n.running}`)
        : n.review
          ? L("会议进行中", "In meeting")
          : act === "working"
            ? L("工作中", "Working")
            : act === "unknown"
              ? L("终端运行中", "Terminal running")
              : L("空闲", "Idle");
  }
  el.boss.dataset.mood = mood;
  setText(rec.cli, c.label);
  setText(rec.st, statusText);
  const originParts = origin
    ? [{ text: L(`派生自 ${origin.parentTitle}`, `Derived from ${origin.parentTitle}`), group: origin.parentGroup }]
    : [];
  const notes = renderBossState(rec, [...originParts, ...handoverParts(c)], c.took_over, model);
  const zhNote = notes.length ? `${notes.join("，")}，` : "";
  const enNote = notes.length ? `${notes.join(", ")}, ` : "";
  const hasTerm = typeof ChatTerminal !== "undefined" && Boolean(ChatTerminal?.bossOpensTerminal?.(c.group));
  setAttr(
    el.boss,
    "aria-label",
    hasTerm
      ? L(`boss 办公室，${zhNote}${rec.st.textContent}，查看终端`, `boss office, ${enNote}${rec.st.textContent}, view terminal`)
      : L(`boss 办公室，${zhNote}${rec.st.textContent}，查看全部消息`, `boss office, ${enNote}${rec.st.textContent}, view all messages`),
  );
  el.boss.classList.toggle("sel", activeSelection()?.kind === "boss");
  if (typeof ChatUsage !== "undefined") ChatUsage.show(c.group);
}

// Once the new boss has spoken, a different former character hangs on the wall to its left.
function hangPortrait(rec, c, former) {
  const face = former ? ART.character(former.host, former.model) : "";
  rec.portrait.hidden = !former || face === rec.char;
  setAttr(rec.portrait, "title", c.took_over || null);
  if (rec.portraitFace === face) return;
  rec.portraitFace = face;
  rec.portrait.replaceChildren(...(face ? [ART.head(face)] : []));
}

// ---------------------------------------------------------------- beside the boss
// Meetings (in session first, ended kept for reopening) and derived sessions beside the boss.
function sideItems(c, ctx) {
  const recent = (a, b) => AN.at(b.last_active) - AN.at(a.last_active);
  const meets = [...c.rooms]
    .sort((a, b) => Number(fullRoom(b)) - Number(fullRoom(a)) || recent(a, b))
    .map((r) => ({
      kind: "room",
      scope: { kind: "room", id: r.id },
      heads: r.seats.map((s) => ctx.desks.get(s.desk)).filter(Boolean),
      name: r.title,
      sub: `${roundText(r)} · ${L(`${r.seats.length} 位参会`, `${r.seats.length} seats`)}`,
      tone: urgent(r.status) ? "run" : r.status === "failed" ? "bad" : "",
      time: r.last_active ? relTime(r.last_active) : "",
    }));
  const derived = typeof ChatDerived !== "undefined" && ChatDerived.sideItems
    ? ChatDerived.sideItems(c).map((item) => ({ ...item, kind: "derived" }))
    : [];
  return { meets, derived };
}
function isSideSelected(item, sel) {
  if (!sel || !item.scope) return false;
  if (item.scope.kind === "room") return sel.kind === "room" && sel.id === item.scope.id;
  if (item.scope.kind === "desk") return sel.kind === "desk" && sel.key === item.scope.key;
  return false;
}
function sideList(node, title, items) {
  const sel = activeSelection();
  // A relative time is patched in place; it never rebuilds the list.
  const key = JSON.stringify([
    title,
    items.map((item) => ({
      id: item.id || item.scope?.id || item.scope?.key || item.name,
      name: item.name,
      sub: item.sub,
      tone: item.tone,
      sel: isSideSelected(item, sel),
      heads: item.heads.slice(0, 3).map((d) => deskChar(d)),
      time: Boolean(item.time),
    })),
    ART.version(),
  ]);
  if (node.dataset.key === key) {
    items.forEach((item, i) => node.times?.[i] && setText(node.times[i], item.time));
    return;
  }
  node.dataset.key = key;
  node.times = [];
  const rows = [];
  for (const item of items) {
    const selected = isSideSelected(item, sel);
    const row = h(
      `button.td-row${item.tone ? `.${item.tone}` : ""}${selected ? ".sel" : ""}`,
      { type: "button", title: `${item.name}\n${item.sub}` },
      h("span.td-heads", {}, item.heads.slice(0, 3).map((d) => ART.head(deskChar(d)))),
      h("span.td-main", {}, h("span.td-name", {}, item.name), h("span.td-sub", {}, item.sub)),
      item.time && (node.times[rows.length] = h("span.td-time", {}, item.time)),
    );
    row.addEventListener("click", (e) => {
      e.stopPropagation();
      if (item.kind === "derived" && typeof ChatDerived !== "undefined" && ChatDerived.openSideItem) {
        ChatDerived.openSideItem(item, row);
      } else if (item.scope) {
        openScope(item.scope, row);
      }
    });
    if (item.kind === "room" && item.scope?.id) {
      row.addEventListener("contextmenu", (e) => Actions.roomMenu(item.scope.id, e));
    } else if (item.kind === "derived" && typeof ChatDerived !== "undefined" && ChatDerived.sideMenu) {
      row.addEventListener("contextmenu", (e) => ChatDerived.sideMenu(item, row, e));
    }
    rows.push(row);
  }
  const list = h("div.side-rows", {}, rows);
  const top = node.querySelector(".side-rows")?.scrollTop || 0;
  node.replaceChildren(
    h("h3.td-h", {}, title, items.length ? h("b", {}, String(items.length)) : null),
    list,
  );
  list.scrollTop = top;
}
function renderSides(rec, c, ctx) {
  const { meets, derived } = sideItems(c, ctx);
  const hasMeets = meets.length > 0;
  const hasDerived = derived.length > 0;
  rec.meets.hidden = !hasMeets && !hasDerived;
  rec.meets.classList?.toggle?.("both", hasMeets && hasDerived);
  rec.meetsSec.hidden = !hasMeets;
  rec.derivedSec.hidden = !hasDerived;
  if (hasMeets) sideList(rec.meetsSec, L("会议", "Meetings"), meets);
  else {
    rec.meetsSec.dataset.key = "";
    rec.meetsSec.replaceChildren?.();
  }
  if (hasDerived) sideList(rec.derivedSec, L("派生", "Derived"), derived);
  else {
    rec.derivedSec.dataset.key = "";
    rec.derivedSec.replaceChildren?.();
  }
  return { meets: meets.length, derived: derived.length };
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
    effort: h("span.tag.tag-effort", { hidden: true, title: L("努力程度", "Effort level") }),
    access: h("span.tag.tag-access", {
      hidden: true,
      title: L("boss 声明的访问等级，只是约定，不改变 CLI 的实际权限", "Access level declared by boss; advisory only, does not change actual CLI permissions"),
    }),
    only: h(
      "span.tag.tag-only",
      { hidden: true, title: L("与 boss 的宿主同类，只参加会议，不接受私聊", "Same host CLI as boss; joins meetings only, no private tasks") },
      L("仅开会", "Meetings only"),
    ),
  };
  const idx = deskHash(d.key || "");
  rec.desk = img("l-desk", "desk");
  rec.mug = img("l-desk", MUGS[idx % MUGS.length]);
  rec.plant = img("l-desk", PLANTS[Math.floor(idx / MUGS.length) % PLANTS.length]);
  rec.scene = h(
    "div.scene",
    {},
    img("l-chair", "chair"),
    rec.desk,
    rec.mug,
    rec.plant,
    Laptop.create(rec.lid),
  );
  rec.el = h(
    "button.desk",
    { type: "button" },
    // effort, access and status sit in the top-left corner; the role is the label's last line
    h("span.desk-tags", {}, rec.effort, rec.access, rec.only),
    rec.say,
    rec.scene,
    h("div.label", {}, rec.nm, h("div.cli", {}, rec.cliText), rec.role),
  );
  rec.d = d;
  rec.el.addEventListener("click", () => {
    const current = rec.d || d;
    openScope({ kind: "desk", key: current.key }, rec.el);
  });
  return rec;
}
function updateDesk(rec, d, ctx) {
  rec.d = d;
  let s = deskMood(d, ctx);
  // The backend reports a finished task as idle; a desk that just stopped working cheers briefly.
  const settled = !d.derived && s.mood === "idle" && !s.away;
  if (rec.mood?.mood === "working" && settled) rec.doneUntil = Date.now() + DONE_FOR;
  if (settled && rec.doneUntil > Date.now())
    s = { ...s, mood: "done", say: L("搞定", "Done") };
  rec.mood = s;
  dress(rec, deskChar(d), rec.scene, rec.desk);
  dressLid(rec, d.provider);
  rec.el.dataset.mood = s.mood;
  const sel = activeSelection();
  setClass(
    rec.el,
    `desk${s.away ? " away" : ""}${d.private_allowed === false ? " only" : ""}${sel?.kind === "desk" && sel.key === d.key ? " sel" : ""}`,
  );
  setSay(rec.say, s.say);
  const { name: bare, effort } = deskParts(d);
  setText(rec.nm, bare);
  setText(rec.effort, effort);
  setAttr(rec.effort, "data-effort", effort || null);
  rec.effort.hidden = !effort;
  setText(rec.cliText, cliOf(d));
  setText(rec.role, d.role || "");
  setAttr(rec.role, "title", d.role || null);
  setText(
    rec.access,
    d.access === "read" ? L("只读", "Read-only") : d.access === "write" ? L("可写", "Writable") : "",
  );
  setAttr(rec.access, "data-access", d.access || null);
  rec.access.hidden = !d.access;
  rec.only.hidden = d.private_allowed !== false;
  setAttr(
    rec.el,
    "title",
    `${s.word}\n${fullHandle(d)}\n${L(`首次出现 ${fullTime(d.first_seen)} · 最近 ${fullTime(d.last_active)}`, `First seen ${fullTime(d.first_seen)} · Last active ${fullTime(d.last_active)}`)}${d.identity?.model_source === "unknown" ? UNKNOWN_MODEL_NOTE : ""}`,
  );
  setAttr(
    rec.el,
    "aria-label",
    L(`${deskName(d)}，${cliOf(d)}${d.role ? `，${d.role}` : ""}，${s.word}，查看对话`, `${deskName(d)}, ${cliOf(d)}${d.role ? `, ${d.role}` : ""}, ${s.word}, view conversation`),
  );
}

// ---------------------------------------------------------------- meeting rooms
// A room in session is drawn in full; finished rooms fold into one line each.
const fullRoom = (r) => urgent(r.status);
function roundText(r) {
  const n = r.round;
  if (!n) return pick(GROUP, AN.ongoing(r.status))[0];
  const order = n.order === "serial" ? L("依次回复", "In turn") : L("同时回复", "In parallel");
  return L(`第 ${n.number} 轮 · ${urgent(r.status) ? order : pick(GROUP, r.status)[0]}`, `Round ${n.number} · ${urgent(r.status) ? order : pick(GROUP, r.status)[0]}`);
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
      h("span", {}, L("会议室 · ", "Meeting room · ")),
      rec.title,
      rec.state,
    ),
    rec.scene,
  );
  const open = () => openScope({ kind: "room", id: r.id }, rec.el);
  rec.el.addEventListener("click", open);
  rec.el.addEventListener("contextmenu", (e) => Actions.roomMenu(r.id, e));
  rec.el.addEventListener("keydown", (e) => {
    const tag = String(e.target?.tagName || "").toUpperCase();
    if (e.target?.isContentEditable || tag === "TEXTAREA" || tag === "INPUT" || tag === "SELECT") return;
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
    return { mood: "speaking", say: [L("作答中", "Replying")], word: L("作答中", "Replying") };
  if (act?.status === "review")
    return { mood: "idle", say: L("已作答", "Replied"), word: L("已作答", "Replied") };
  return { mood: "idle", say: "", word: d || seat.desk ? "" : L("缺席", "Absent") };
}
function updateRoom(rec, r, ctx) {
  const [, tone] = pick(GROUP, AN.ongoing(r.status));
  const sel = activeSelection();
  setClass(
    rec.el,
    `zone room${state.closing.has(r.id) ? " closing" : ""}${sel?.kind === "room" && sel.id === r.id ? " sel" : ""}`,
  );
  setText(rec.title, r.title);
  setText(rec.state, roundText(r));
  setClass(rec.state, `r ${tone}`);
  setAttr(
    rec.el,
    "aria-label",
    L(`会议室 ${r.title}，${roundText(r)}，会议记录 ${r.board ?? 0} 条，查看会议`, `Meeting room ${r.title}, ${roundText(r)}, ${r.board ?? 0} notes, view meeting`),
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
    const name = d ? deskName(d) : seat.identity ? handleOf(seat).replace(/^@/, "") : seat.seat;
    // model name and role only; the CLI is in the tooltip
    const tag = d ? d.role || "" : absent ? L("缺席", "Absent") : "";
    setText(s.nm, name);
    setText(s.plate, tag);
    s.plate.hidden = !tag;
    setClass(s.plate, `plate tag${d ? " tag-role" : ""}`);
    setAttr(s.el, "title", d || seat.identity ? fullHandle(d || seat) : L(`席位 ${seat.seat}`, `Seat ${seat.seat}`));
    nodes.push(s.el);
  }
  reconcile(rec.scene, nodes);
}
// What one desk's scene would measure if nothing held it back: read off a desk's card (the cap set
// below narrows only the scene inside it, so it never feeds back), or, with none open, worked out
// from the grid the next desk would join (--cols follows the floor's width in CSS).
const DESK_FRAME = 18; // a desk's side padding and border around its scene
const DESK_PAD = 16; // the same padding alone, inside the card's border
function deskSceneWidth() {
  const desk = el.desks.querySelector(".desk");
  if (desk) return desk.clientWidth - DESK_PAD;
  const grid = getComputedStyle(el.desks);
  const cols = parseInt(grid.getPropertyValue("--cols"), 10) || 1;
  const gap = parseFloat(grid.columnGap) || 0;
  return (el.desks.clientWidth - gap * (cols - 1)) / cols - DESK_FRAME;
}
// The boss is always this many desks wide, so boss and staff keep one scale on every floor.
const BOSS_SCALE = 1.3;
const MEETINGS_MIN = 225; // the meetings panel's narrowest width beside the boss, as in chat.css
// How wide the boss's scene may grow: its whole row, less the meetings panel when that sits beside
// it. When the boss would have to shrink to keep the panel beside it, the panel moves below instead.
function bossRoom(natural) {
  const row = state.bossRec?.row;
  if (!row?.isConnected) return 0;
  const width = row.clientWidth;
  const gap = parseFloat(getComputedStyle(row).columnGap) || 0;
  const meets = !state.bossRec.meets.hidden;
  const fits = width - gap - MEETINGS_MIN >= natural * BOSS_SCALE;
  row.classList.toggle("stack", meets && !fits);
  const beside = meets && fits && getComputedStyle(row).gridTemplateColumns.trim().split(/\s+/).length > 1;
  return width - (beside ? gap + MEETINGS_MIN : 0);
}
// Seats are as large as desk figures, so a walker keeps its size; rows wrap to fit the room.
// The boss's scene is BOSS_SCALE desks; on a floor too narrow for that the desks give way instead.
function layoutRooms() {
  let complete = true;
  const natural = deskSceneWidth();
  const room = bossRoom(natural);
  const deskWidth = natural > 0 && room > 0 ? Math.min(natural, room / BOSS_SCALE) : natural;
  const cap = deskWidth > 0 && deskWidth < natural ? `${Math.floor(deskWidth)}px` : "";
  if ((state.deskCap ?? "") !== cap) {
    state.deskCap = cap;
    el.desks.style.setProperty("--desk-scene", cap);
  }
  const bossWidth = deskWidth > 0 ? `${Math.round(deskWidth * BOSS_SCALE)}px` : "";
  if (state.bossRec && state.bossRec.scene.style.width !== bossWidth)
    state.bossRec.scene.style.width = bossWidth;
  // Desks scale with the floor, so seats take the size their figures have at the desks.
  const fig = el.desks.querySelector(".desk > .scene > .fig")?.offsetWidth || FIG;
  for (const rec of state.roomRecs.values()) {
    const m = rec.scene;
    if (!m.isConnected) continue;
    const avail = rec.el.clientWidth - 26;
    const slots = m.children.length;
    if (avail <= 0) complete = false;
    if (avail <= 0 || !slots) continue;
    const size = Math.min(fig, avail);
    const perRow = Math.min(slots, Math.max(1, Math.floor(avail / size)));
    m.style.setProperty("--S", `${size}px`);
    m.style.width = `${size * perRow}px`;
    // the table texture continues across the seats of one row
    for (const seat of m.querySelectorAll(".seat"))
      seat.style.setProperty("--x", `${-seat.offsetLeft}px`);
  }
  return complete;
}
// Layout reads sizes, so it runs when the number of desks or seats changes (the floor's own width
// is watched below), not on every poll.
let laidOutFor = "";
function layoutIfChanged(key) {
  if (key === laidOutFor) return;
  laidOutFor = layoutRooms() ? key : "";
}
// A desk holds its done frame for a while without any new data, and needs one more pass once it ends.
const timeDriven = () => [...state.deskRecs.values()].some((rec) => rec.doneUntil > (state.drawnAt || 0));
// Scene sizes depend on the floor's width. Their height changes must not feed back into
// this observer, and writes belong outside resize delivery to avoid a ResizeObserver loop.
let observedFloorWidth = null;
new ResizeObserver(([entry]) => {
  const width = entry.contentRect.width;
  if (width <= 0 || width === observedFloorWidth) return;
  observedFloorWidth = width;
  requestAnimationFrame(layoutRooms);
}).observe(el.floor);

// ---------------------------------------------------------------- the office, patched in place
const blurred = () => document.documentElement.classList.contains("blurred");
const canAnimate = () =>
  !reduceMotion.matches &&
  !AN.isHidden() &&
  !blurred() &&
  typeof Element.prototype.animate === "function";
// A walk or flight already under way stands still with the window's pause and goes on from there.
const moving = new Set();
function moves(run) {
  moving.add(run);
  const done = () => moving.delete(run);
  run.finished.then(done, done);
  return run;
}
window.addEventListener("agentnave:pause", () => {
  const still = blurred();
  for (const run of moving) still ? run.pause() : run.play();
});

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
    laidOutFor = "";
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
  const removedRoom = !newGroup && (el.rooms.children?.length || 0) > roomNodes.length;
  if (removedRoom && globalThis.ANMotion?.flipY) {
    globalThis.ANMotion.flipY([...roomNodes, el.desksZone], () => reconcile(el.rooms, roomNodes));
  } else {
    reconcile(el.rooms, roomNodes);
  }

  for (const key of state.deskRecs.keys())
    if (!ctx.drawn.has(key)) state.deskRecs.delete(key);
  const regularDesks = c.desks.filter((d) => !d.derived);
  const deskNodes = regularDesks.map((d) => {
    let rec = state.deskRecs.get(d.key);
    if (!rec) state.deskRecs.set(d.key, (rec = createDesk(d)));
    updateDesk(rec, d, ctx);
    return rec.el;
  });
  const sideCounts = renderSides(state.bossRec, c, ctx);
  // With only derived sessions beside the boss there is nothing to put in open desks, so hide the empty zone.
  const onlyDerived = sideCounts.derived > 0 && !regularDesks.length && !c.optimized?.count;
  if (el.desksZone) el.desksZone.hidden = onlyDerived;
  if (!deskNodes.length)
    deskNodes.push(h("div.zone-empty", {}, L("boss 派活后，工位出现在这里", "Desks appear here once boss dispatches work")));
  if (c.optimized?.count) deskNodes.push(optimizedNode(c.optimized, ctx));
  reconcile(el.desks, deskNodes);
  setText(el.deskCount, regularDesks.length ? ` · ${regularDesks.length}` : "");
  layoutIfChanged(JSON.stringify([regularDesks.length, sideCounts.meets, sideCounts.derived, !state.bossRec.meets.hidden, rooms.map((r) => [r.id, r.seats.length])]));
  walk(!newGroup && state.ready, sat);
}

// After its seats walk back and a pause, an ended room fades out on the compositor (zero per-frame
// layout), then surviving zones below FLIP smoothly upward when reconcile removes the room.
function foldRoom(node) {
  void node.offsetHeight;
  return node.animate(
    [
      { transform: "translate3d(0, 0, 0) scale(1)", opacity: 1 },
      { transform: "translate3d(0, -10px, 0) scale(0.97)", opacity: 0 },
    ],
    { duration: FOLD_MS, delay: WALK_MS + HOLD_MS, easing: "cubic-bezier(0.4, 0, 1, 1)", fill: "forwards" },
  );
}

// Desks past the running limit are not drawn: a text list, opened on demand, links to each one.
function optimizedNode(opt, ctx) {
  const listed = opt.desks || [];
  const key = JSON.stringify([state.optimizedOpen, opt.count, ART.version(), listed.map((d) => [d.key, handleOf(d), d.status, d.last_active])]);
  let node = state.optimizedEl;
  if (node && node.dataset.key === key) return node;
  const toggle = h(
    "button.optimized-h",
    { type: "button", "aria-expanded": String(Boolean(state.optimizedOpen)) },
    L("已优化", "Optimized"),
    h("b", {}, L(`${opt.count} 人`, `${opt.count}`)),
    h("span.r", {}, state.optimizedOpen ? L("收起", "Collapse") : L("展开", "Expand")),
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
        { type: "button", title: `${fullHandle(d)}\n${s.word}` },
        h("span.td-heads", {}, ART.head(deskChar(d))),
        h("span.td-main", {}, h("span.td-name", {}, handleOf(d)), h("span.td-sub", {}, s.word)),
        h("span.td-time", {}, relTime(d.last_active)),
      );
      row.addEventListener("click", (e) => {
        e.stopPropagation();
        openScope({ kind: "desk", key: d.key }, row);
      });
      rows.push(row);
    }
    if (opt.count > listed.length)
      rows.push(h("div.td-more", {}, L(`另有 ${opt.count - listed.length} 个更早的工位未列出`, `${opt.count - listed.length} earlier desks omitted`)));
  }
  node = h(
    "div.optimized",
    { "aria-label": L(`已优化的工位，${opt.count} 人`, `Optimized desks, ${opt.count}`) },
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
const TELEPORT_GAP_MS = 900;
const activeWalks = new WeakMap(); // destination figure -> latest walk token
// start is a figure, or the last place of one that has already left the page.
function travel(start, end, name) {
  if (!start || !end?.isConnected) return;
  if (start instanceof Element && !start.isConnected) return;
  const base = el.fx.getBoundingClientRect();
  const a = start instanceof Element ? start.getBoundingClientRect() : start,
    b = end.getBoundingClientRect();
  if (!a.width || !b.width) return;
  if (ART.canWarp(name)) return teleport(start, end, name, a, base);
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
  const run = moves(walker.animate(
    [
      { transform: "translate3d(0, 0, 0)", opacity: 0.96 },
      {
        transform: `translate3d(${(b.left - a.left) / 2}px, ${(b.top - a.top) / 2 - (walks ? 0 : 18)}px, 0) scale(${(1 + scale) / 2})`,
        opacity: 1,
        offset: 0.5,
      },
      {
        transform: `translate3d(${b.left - a.left}px, ${b.top - a.top}px, 0) scale(${scale})`,
        opacity: 0.96,
        offset: 0.9,
      },
      {
        transform: `translate3d(${b.left - a.left}px, ${b.top - a.top}px, 0) scale(${scale})`,
        opacity: 0,
      },
    ],
    walks
      ? { duration: WALK_MS, easing: "linear" }
      : { duration: 900, easing: "cubic-bezier(0.4, 0, 0.2, 1)" },
  ));
  const token = {};
  activeWalks.set(end, token);
  // The figure at the destination waits until the walker arrives, then crossfades smoothly into place.
  end.style.visibility = "hidden";
  const settle = (arrived) => {
    walker.remove();
    if (activeWalks.get(end) !== token) return;
    activeWalks.delete(end);
    end.style.visibility = "";
    if (arrived && end.isConnected && typeof end.animate === "function" && canAnimate()) {
      end.animate(
        [{ opacity: 0.2, transform: "translate3d(0, -2px, 0)" }, { opacity: 1, transform: "translate3d(0, 0, 0)" }],
        { duration: 120, easing: "cubic-bezier(0.2, 0, 0, 1)" },
      );
    }
  };
  run.onfinish = () => settle(true);
  run.oncancel = () => settle(false);
}

// A character with a teleport vanishes where it was and appears at its destination; nothing crosses the room.
function teleport(start, end, name, a, base) {
  // a figure that has already left the page plays its exit where it last stood; the boss's figure is someone else
  if (!(start instanceof Element)) {
    const ghost = ART.figure(name);
    ghost.classList.add("walker");
    ghost.style.cssText += `left:${a.left - base.left}px;top:${a.top - base.top}px;width:${a.width}px;height:${a.height}px`;
    el.fx.append(ghost);
    ART.warp(ghost, "out").finally(() => ghost.remove());
  } else if (start.dataset.char === name) ART.warp(start, "out");
  if (end.dataset.char !== name) return;
  end.style.visibility = "hidden";
  setTimeout(() => {
    end.style.visibility = "";
    if (end.isConnected && canAnimate()) ART.warp(end, "in");
  }, TELEPORT_GAP_MS);
}

// An envelope travels between two office elements along a smooth parabolic arc with a subtle arrival receipt.
function fly(kind, from, to, delay) {
  if (!from || !to || !from.isConnected || !to.isConnected) return;
  const base = el.fx.getBoundingClientRect();
  const a = from.getBoundingClientRect(),
    b = to.getBoundingClientRect();
  if (!a.width || !b.width || !base.width) return;
  const dx = b.left + b.width / 2 - a.left - a.width / 2,
    dy = b.top + b.height / 2 - a.top - a.height / 2;
  const dist = Math.hypot(dx, dy);
  const duration = Math.round(Math.max(540, Math.min(900, 440 + dist * 0.65)));
  const arc = Math.min(84, Math.max(32, dist * 0.22));
  const dir = dx >= 0 ? 1 : -1;
  const token = h(`span.token.token-${kind}`);
  token.style.left = `${a.left + a.width / 2 - base.left}px`;
  token.style.top = `${a.top + a.height / 2 - base.top}px`;
  el.fx.append(token);
  const centre = "translate(-50%, -50%)";
  const frames = [0, 0.14, 0.28, 0.44, 0.6, 0.76, 0.9, 1].map((t) => {
    const px = dx * t;
    const py = dy * t - 4 * arc * t * (1 - t);
    const rot = dir * (1 - 2 * t) * 10;
    const sc = t === 0 ? 0.55 : t < 0.2 ? 1.06 : t > 0.85 ? 0.68 : 1;
    const op = t === 0 ? 0 : t < 0.2 ? 1 : t > 0.85 ? 0 : 1;
    return {
      transform: `${centre} translate3d(${px.toFixed(1)}px, ${py.toFixed(1)}px, 0) rotate(${rot.toFixed(1)}deg) scale(${sc})`,
      opacity: op,
      offset: t,
    };
  });
  const run = moves(token.animate(frames, { duration, delay, easing: "cubic-bezier(0.22, 0.08, 0.24, 1)", fill: "backwards" }));
  const done = (arrived) => {
    token.remove();
    if (arrived && to.isConnected && to.classList?.contains?.("scene") && typeof to.animate === "function" && canAnimate()) {
      to.animate(
        [{ transform: "scale(1)" }, { transform: "scale(1.018)", offset: 0.4 }, { transform: "scale(1)" }],
        { duration: 160, easing: "cubic-bezier(0.2, 0, 0, 1)" },
      );
    }
  };
  run.onfinish = () => done(true);
  run.oncancel = () => done(false);
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
