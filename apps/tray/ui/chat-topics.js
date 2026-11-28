// The conversation column's reading: one card per meeting or private task, then a chat stream.
// In a desk's stream the other seats of its rounds stay one line each.
const tp = { key: "" };
// The first line that says something: answers often open with a bare heading such as "结论".
function tpFirst(text) {
  const lines = plain(text).split("\n").map((l) => l.trim().replace(/^([-*+]|\d+[.)])\s+/, "")).filter(Boolean);
  return lines.find((l) => l.length > 6) || lines[0] || "";
}
const tpRoundKey = (m) => `${m.conversation}|${m.round}`;
// What a desk's stream lists: its own messages, plus the other seats' answers in its rounds.
function deskStream(list, ctx, key) {
  const own = new Set(list.filter((m) => inDesk(m, key, ctx)).map((m) => m.id));
  const rounds = new Set(
    list.filter((m) => own.has(m.id) && m.thread?.kind === "room" && m.round > 0).map(tpRoundKey),
  );
  const peers = new Set(
    list
      .filter((m) => !own.has(m.id) && !isBoss(m) && m.thread?.kind === "room" && rounds.has(tpRoundKey(m)))
      .map((m) => m.id),
  );
  return { peers, shown: list.filter((m) => own.has(m.id) || peers.has(m.id)) };
}
// Where a message or conversation is read: a room's stream, or the desk that did the task.
function tpScopeOf(m, ctx) {
  if (!m) return { kind: "all" };
  if (m.thread?.kind === "room" && ctx.rooms.has(m.conversation)) return { kind: "room", id: m.conversation };
  return ctx.desks.has(m.desk) ? { kind: "desk", key: m.desk } : { kind: "all" };
}

function tpTopics(list) {
  const topics = [];
  const byKey = new Map();
  const add = (key, topic) => {
    byKey.set(key, topic);
    topics.push(topic);
    return topic;
  };
  for (const m of list) {
    if (m.kind === "approval") continue;
    const room = m.thread?.kind === "room";
    if (room && m.round > 0) {
      const key = tpRoundKey(m);
      const t = byKey.get(key) || add(key, { kind: "round", conversation: m.conversation, round: m.round, prompt: null, replies: [], time: m.time });
      t.time = m.time;
      if (isBoss(m)) t.prompt ||= m.kind === "prompt" ? m : null;
      else t.replies.push(m);
    } else if (!room && m.kind === "prompt") {
      add(m.id, { kind: "task", conversation: m.conversation, prompt: m, replies: [], time: m.time });
    } else if (m.reply_to && byKey.has(m.reply_to)) {
      const t = byKey.get(m.reply_to);
      t.replies.push(m);
      t.time = m.time;
    }
  }
  return topics;
}
function tpSummary(m) {
  if (m.kind === "notice") return tpFirst(m.detail || m.text);
  if (m.kind === "typing") {
    const lines = plain(m.text).split("\n").filter((l) => l.trim());
    return lines.at(-1) || typingStatus(m);
  }
  return tpFirst(m.text);
}
function tpBadge(status) {
  return status ? h(`span.tp-badge.tone-${status[1]}`, {}, status[0]) : null;
}
const tpBad = (m) => m?.kind === "notice" && AN.pick(AN.MEMBER, m.status)[1] === "bad";
function tpOriginCard(origin) {
  const originMsgs = origin.messages || [];
  const lastOrigin = [...originMsgs].reverse().find((m) => m.kind === "reply" || m.kind === "typing" || m.kind === "notice") || originMsgs.at(-1);
  const firstOrigin = originMsgs.find((m) => m.kind === "prompt");
  const runningOrigin = originMsgs.some((m) => m.kind === "typing") || origin.desk?.status === "running";
  return {
    weight: -1,
    time: lastOrigin?.time || origin.desk?.last_active || "",
    tag: AN.L("主对话", "Main"),
    title: AN.L("派生对话", "Derived conversation"),
    status: runningOrigin ? [AN.L("进行中", "Running"), "run"] : null,
    faces: origin.desk ? [ART.head(deskChar(origin.desk))] : [],
    meta: AN.L(`来自 ${origin.parentTitle}`, `From ${origin.parentTitle}`),
    line: lastOrigin ? tpSummary(lastOrigin) : firstOrigin ? tpFirst(firstOrigin.text) : "",
    scope: { kind: "origin" },
  };
}
function tpDirectory(c, ctx) {
  const topics = tpTopics(c.messages);
  const items = [];
  const origin = typeof ChatDerived !== "undefined" ? ChatDerived.originOf?.(c) : null;
  if (origin) items.push(tpOriginCard(origin));
  for (const room of ctx.rooms.values()) {
    const last = topics.filter((t) => t.kind === "round" && t.conversation === room.id).at(-1);
    const failed = room.status === "failed" || last?.replies.some(tpBad);
    const review = room.status === "review" || last?.replies.some((m) => m.status === "review") || room.pending_review > 0;
    const running = room.status === "running";
    const said = last ? [...last.replies].reverse().find((m) => m.kind === "reply") : null;
    const live = running && last ? [...last.replies].reverse().find((m) => m.kind === "typing" && tpSummary(m)) : null;
    const total = last ? Math.max(last.replies.length, last.prompt?.recipients?.length || 0) : 0;
    items.push({
      weight: review ? 0 : running ? 1 : 2,
      time: last?.time || "",
      tag: AN.L("会议", "Meeting"),
      title: room.title,
      status: review ? [AN.L("待审核", "Review"), "review"] : failed ? [AN.L("有失败", "Failed"), "bad"] : running ? [AN.L("进行中", "Running"), "run"] : [AN.L("已结束", "Ended"), "idle"],
      faces: room.seats.map((s) => ctx.desks.get(s.desk)).filter(Boolean).slice(0, 4).map((d) => ART.head(deskChar(d))),
      meta: last ? AN.L(`第 ${last.round} 轮 · ${last.replies.filter((m) => m.kind === "reply").length}/${total} 已答`, `Round ${last.round} · ${last.replies.filter((m) => m.kind === "reply").length}/${total} replied`) : AN.L("还没开始", "Not started"),
      line: said ? tpFirst(said.text) : live ? tpSummary(live) : last?.prompt ? tpFirst(last.prompt.text) : "",
      scope: { kind: "room", id: room.id },
    });
  }
  for (const t of topics.filter((x) => x.kind === "task")) {
    const last = t.replies.at(-1);
    const desk = ctx.desks.get(t.prompt.desk) || ctx.desks.get(last?.desk);
    const named = last?.speaker?.identity
      ? last.speaker
      : (t.prompt.to || t.prompt.recipients || []).find((target) => target.identity) || {};
    const running = t.replies.some((m) => m.kind === "typing");
    const bad = tpBad(last);
    items.push({
      weight: running ? 1 : 2,
      time: t.time,
      tag: desk?.derived ? AN.L("派生", "Derived") : AN.L("任务", "Task"),
      title: tpFirst(t.prompt.text),
      status: bad ? [AN.L("失败", "Failed"), "bad"] : running ? [AN.L("进行中", "Running"), "run"] : last ? [AN.L("已完成", "Done"), "done"] : [AN.L("等待中", "Pending"), "idle"],
      faces: desk ? [ART.head(deskChar(desk))] : [],
      meta: deskName(named),
      line: last ? tpSummary(last) : "",
      scope: desk ? { kind: "desk", key: desk.key } : null,
    });
  }
  // A handover belongs to no meeting or desk, so its note is read here, folded until asked for.
  const handovers = c.messages.filter((x) => x.kind === "handover");
  const pending = c.handover === "pending";
  const explicitPending = handovers.some((x) => x.status === "pending");
  const latestHandover = handovers.reduce((best, x) => (!best || AN.at(x.time) >= AN.at(best.time) ? x : best), null);
  const isPendingCard = (m) => pending && (m.status === "pending" || (!explicitPending && m === latestHandover));
  if (pending && !handovers.some(isPendingCard)) {
    handovers.push({ id: `${c.group}:handover:pending`, status: "pending", time: c.updated || "", text: "" });
  }
  for (const m of handovers) {
    const awaiting = isPendingCard(m);
    items.push({
      weight: awaiting ? 0 : 2,
      time: m.time,
      tag: AN.L("交接", "Handover"),
      title: m.speaker?.label || AN.L("agent 交接", "Agent handover"),
      faces: [],
      meta: "",
      line: m.text,
      next: awaiting ? handoverNext(c) : null,
      note: m.detail ? { id: m.id, text: m.detail } : null,
      scope: null,
    });
  }
  items.sort((x, y) => x.weight - y.weight || AN.at(y.time) - AN.at(x.time));
  const omitted = c.omitted ? [h("div.sep", {}, AN.L(`更早的 ${c.omitted} 条没有列出`, `${c.omitted} earlier items omitted`))] : [];
  if (!items.length) return [...omitted, h("div.dr-empty", {}, AN.L("这里还没有消息。", "No messages yet."))];
  const cards = items.map((item) => {
    const card = h(
      item.scope ? "button.tp-card" : "div.tp-card",
      item.scope ? { type: "button" } : {},
      h("span.tp-card-top", {}, h("span.tp-tag", {}, item.tag), h("b", {}, item.title), tpBadge(item.status)),
      h("span.tp-card-meta", {}, h("span.tp-faces", {}, item.faces), h("span", {}, item.meta), h("time", {}, item.time ? clock(item.time) : "")),
      item.line ? h("span.tp-card-line", {}, item.line) : null,
      item.next,
      item.note ? handoverNote(item.note) : null,
    );
    if (item.scope) card.onclick = () => openScope(item.scope, card);
    if (item.scope?.kind === "room") card.addEventListener?.("contextmenu", (e) => Actions.roomMenu(item.scope.id, e));
    return card;
  });
  return [...cards, ...omitted];
}
function handoverNext(c) {
  const name = AN.chatName(c);
  const cmd = AN.L(`接手 AgentNave 工作区「${name}」（${c.group}）`, `Take over AgentNave workspace "${name}" (${c.group})`);
  const hasConv = Array.isArray(c.conversations) ? c.conversations.length > 0 : Boolean(c.conversations);
  let cancel = null;
  if (hasConv) {
    cancel = h("button.tp-btn", { type: "button" }, AN.L("取消交接", "Cancel handover"));
    cancel.addEventListener("click", () => Actions.cancelHandover(c.group));
  }
  return h(
    "div.tp-handover-next",
    {},
    h("span.tp-handover-hint", {}, AN.L("到任意 boss 的聊天里说：", "In a chat with any boss, say:")),
    h("code.tp-handover-cmd", {}, cmd),
    h("div.tp-handover-actions", {}, AN.copyButton(cmd, "tp-btn"), cancel),
  );
}
// The fold survives the list being drawn again.
function handoverNote(note) {
  const box = h("details.tp-card-note", { open: state.expanded.has(note.id) },
    h("summary", {}, AN.L("交接说明", "Handover notes")), h("span.tp-card-line.whole", {}, note.text));
  box.ontoggle = () => (box.open ? state.expanded.add(note.id) : state.expanded.delete(note.id));
  return box;
}
function renderDirectory(c, ctx) {
  // The stream's nodes are not on screen; picking a card builds them afresh.
  state.scopeKey = "";
  state.nodes.clear();
  const head = `dir|${c.host}|${c.label}`;
  if (el.drIco.dataset.key !== head) {
    el.drIco.dataset.key = head;
    el.drIco.replaceChildren(hostTile(c.app || c.host, c.label));
  }
  setText(el.drTitle, AN.L("全部消息", "All messages"));
  setText(el.drSub, c.label);
  setAttr(el.drSub, "title", null);
  el.drAll.hidden = true;
  el.jump.hidden = true;
  setAttr(el.drawer, "aria-label", AN.L("全部消息", "All messages"));
  const key = JSON.stringify([
    document.documentElement.lang, c.group, c.handover, AN.chatName(c),
    Array.isArray(c.conversations) ? c.conversations.length : Boolean(c.conversations),
    ART.version(),
    c.derived_origin
      ? [
          c.derived_origin.parent_workspace_id,
          c.derived_origin.parent_title,
          c.derived_origin.desk?.status,
          (c.derived_origin.messages || []).map((m) => [m.id, m.status, m.kind === "typing" ? m.text : (m.text || "").length]),
        ]
      : null,
    c.messages.map((m) => [m.id, m.status, m.kind === "typing" ? m.text : (m.text || "").length, m.detail, m.doing, m.speaker?.identity, m.speaker?.effort]),
    [...ctx.rooms.values()].map((r) => [r.id, r.title, r.status, r.pending_review]),
    [...ctx.desks.values()].map((d) => [d.key, deskName(d), Boolean(d.derived)]),
    c.omitted,
  ]);
  if (tp.key === key && el.inner.querySelector(".tp-card, .dr-empty")) return;
  tp.key = key;
  const top = el.tl.scrollTop;
  el.inner.replaceChildren(...tpDirectory(c, ctx));
  el.tl.scrollTop = top;
}

// ---------------------------------------------------------------- isolated directories
// Built when drawn: this file loads before the language helper.
const isolatedStatus = (status) =>
  ({
    using: [L("使用中", "In use"), "tone-run"],
    unfinished: [L("需要处理", "Needs attention"), "tone-review"],
    collapsed: [L("已收起", "Collapsed"), "tone-idle"],
  })[status] || [L("任务已结束", "Task ended"), "tone-idle"];
function isolatedCard(d, ctx) {
  const gitExtra = typeof ChatGit !== "undefined" ? ChatGit.isolatedNodes?.(d, ctx) : null;
  const inUse = d.status === "using" || Boolean(gitExtra?.hasActiveTerminal);
  const [word, tone] = isolatedStatus(inUse ? "using" : d.status);
  const desk = d.task && ctx.desks.get(d.task.desk);
  const who = [desk && deskName(desk), d.task?.text].filter(Boolean).join(" · ");
  const workspace = ctx.c?.group || state?.chat?.group || "";
  const cleanupKept = typeof ChatIsolatedCleanup !== "undefined" ? ChatIsolatedCleanup.keptReason(d.id) : "";
  const del = h(
    "button.iso-del",
    {
      type: "button",
      disabled: inUse,
      title: inUse
        ? L("任务正在使用这个隔离目录，暂时不能删除", "In use by a task; cannot delete right now")
        : L("检查并确认后删除这个隔离目录和受管分支", "Inspect and confirm deleting this isolated directory and managed branch"),
      "aria-label": inUse
        ? L("隔离目录使用中，不能删除", "Isolated directory in use; cannot delete")
        : L(`删除隔离目录 ${d.branch || d.path}`, `Delete isolated directory ${d.branch || d.path}`),
    },
    L("删除", "Delete"),
  );
  if (!inUse) del.addEventListener("click", () => Actions.deleteIsolated(workspace, d, del));
  const extraNodes = gitExtra?.nodes || [];
  const hasUserNode = extraNodes.some((n) => n?.className === "iso-user");
  const whoText = who ? L(`建立者：${who}`, `Created by: ${who}`) : "";
  return h(
    "div.iso-card",
    {},
    h(
      "div.iso-top",
      {},
      h("b.iso-branch", { title: d.branch || d.path }, d.branch || d.path),
      h("span.iso-top-end", {}, h(`span.pill.${tone}`, {}, word), del),
    ),
    ...extraNodes,
    !hasUserNode && whoText ? h("div.iso-line", { title: whoText }, whoText) : null,
    d.created && Number.isFinite(AN.at(d.created)) ? h("div.iso-line", {}, L(`建立于 ${relTime(d.created)}`, `Created ${relTime(d.created)}`)) : null,
    cleanupKept ? h("div.iso-line", {}, L(`未清理：${cleanupKept}`, `Kept: ${cleanupKept}`)) : null,
    d.attention ? h("div.iso-line.iso-warn", {}, L(`上次清理未完成：${d.attention}`, `Last cleanup incomplete: ${d.attention}`)) : null,
    h("div.iso-path", { title: d.path }, d.path),
  );
}
function renderIsolatedDrawer(c, ctx) {
  state.scopeKey = "";
  state.nodes.clear();
  const dirs = c.isolated || [];
  if (el.drIco.dataset.key !== "isolated") {
    el.drIco.dataset.key = "isolated";
    el.drIco.replaceChildren(h("span", {}, L("隔", "I")));
  }
  setText(el.drTitle, L("隔离目录", "Isolated directories"));
  setText(el.drSub, shownName(c));
  setAttr(el.drSub, "title", null);
  el.drAll.hidden = false;
  el.jump.hidden = true;
  setAttr(el.drawer, "aria-label", L("隔离目录", "Isolated directories"));
  const gitVer = typeof ChatGit !== "undefined" ? (ChatGit.version?.() || 0) : 0;
  const cleanup = typeof ChatIsolatedCleanup !== "undefined" ? ChatIsolatedCleanup : null;
  cleanup?.ensureLoaded();
  const derivedKey = typeof ChatTerminal !== "undefined" ? (ChatTerminal.derivedAll?.() || []).map((x) => [x.group, x.isolation_id, x.status]) : [];
  const key = JSON.stringify(["isolated", c.group, dirs, [...ctx.desks.values()].map((d) => [d.key, deskName(d)]), gitVer, derivedKey, cleanup?.version() || 0, Math.floor(Date.now() / 30000)]);
  if (tp.key === key && el.inner.querySelector(".iso-clean")) return;
  tp.key = key;
  el.inner.replaceChildren(...(cleanup ? [cleanup.panel(c.group)] : []), ...dirs.map((d) => isolatedCard(d, ctx)));
}
