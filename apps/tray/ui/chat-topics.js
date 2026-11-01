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
function deskStream(list, ctx) {
  const own = new Set(list.filter((m) => inScope(m, ctx)).map((m) => m.id));
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
function tpDirectory(c, ctx) {
  const topics = tpTopics(c.messages);
  const items = [];
  for (const room of ctx.rooms.values()) {
    const last = topics.filter((t) => t.kind === "round" && t.conversation === room.id).at(-1);
    const failed = room.status === "failed" || last?.replies.some(tpBad);
    const review = room.status === "review" || last?.replies.some((m) => m.status === "review") || room.pending_review > 0;
    const running = room.status === "running";
    const said = last ? [...last.replies].reverse().find((m) => m.kind === "reply") : null;
    const total = last ? Math.max(last.replies.length, last.prompt?.recipients?.length || 0) : 0;
    items.push({
      weight: review || failed ? 0 : running ? 1 : 2,
      time: last?.time || "",
      tag: "会议",
      title: room.title,
      status: review ? ["待审核", "review"] : failed ? ["有失败", "bad"] : running ? ["进行中", "run"] : ["已结束", "idle"],
      faces: room.seats.map((s) => ctx.desks.get(s.desk)).filter(Boolean).slice(0, 4).map((d) => ART.head(deskChar(d))),
      meta: last ? `第 ${last.round} 轮 · ${last.replies.filter((m) => m.kind === "reply").length}/${total} 已答` : "还没开始",
      line: said ? tpFirst(said.text) : last?.prompt ? tpFirst(last.prompt.text) : "",
      scope: { kind: "room", id: room.id },
    });
  }
  for (const t of topics.filter((x) => x.kind === "task")) {
    const last = t.replies.at(-1);
    const desk = ctx.desks.get(t.prompt.desk) || ctx.desks.get(last?.desk);
    const running = t.replies.some((m) => m.kind === "typing");
    const bad = tpBad(last);
    items.push({
      weight: bad ? 0 : running ? 1 : 2,
      time: t.time,
      tag: "任务",
      title: tpFirst(t.prompt.text),
      status: bad ? ["失败", "bad"] : running ? ["进行中", "run"] : last ? ["已完成", "done"] : ["等待中", "idle"],
      faces: desk ? [ART.head(deskChar(desk))] : [],
      meta: desk ? deskName(desk) : "",
      line: last ? tpSummary(last) : "",
      scope: desk ? { kind: "desk", key: desk.key } : null,
    });
  }
  // A handover belongs to no meeting or desk, so its whole note is read here.
  for (const m of c.messages.filter((x) => x.kind === "handover")) {
    items.push({
      weight: 2,
      time: m.time,
      tag: "交接",
      title: m.speaker?.label || "工作区交接",
      faces: [],
      meta: "",
      line: [m.text, m.detail].filter(Boolean).join("\n\n"),
      whole: true,
      scope: null,
    });
  }
  items.sort((x, y) => x.weight - y.weight || AN.at(y.time) - AN.at(x.time));
  const omitted = c.omitted ? [h("div.sep", {}, `更早的 ${c.omitted} 条没有列出`)] : [];
  if (!items.length) return [...omitted, h("div.dr-empty", {}, "这里还没有消息。")];
  const cards = items.map((item) => {
    const card = h(
      item.scope ? "button.tp-card" : "div.tp-card",
      item.scope ? { type: "button" } : {},
      h("span.tp-card-top", {}, h("span.tp-tag", {}, item.tag), h("b", {}, item.title), tpBadge(item.status)),
      h("span.tp-card-meta", {}, h("span.tp-faces", {}, item.faces), h("span", {}, item.meta), h("time", {}, item.time ? clock(item.time) : "")),
      item.line ? h(`span.tp-card-line${item.whole ? ".whole" : ""}`, {}, item.line) : null,
    );
    if (item.scope) card.onclick = () => openScope(item.scope, card);
    return card;
  });
  return [...cards, ...omitted];
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
  setText(el.drTitle, "boss");
  setText(el.drSub, c.label);
  el.drAll.hidden = true;
  el.jump.hidden = true;
  setAttr(el.drawer, "aria-label", "boss 的对话");
  const key = JSON.stringify([
    c.group, ART.version(),
    c.messages.map((m) => [m.id, m.status, m.kind === "typing" ? m.text : (m.text || "").length, m.detail, m.doing]),
    [...ctx.rooms.values()].map((r) => [r.id, r.title, r.status, r.pending_review]),
    [...ctx.desks.values()].map((d) => [d.key, deskName(d)]),
    c.omitted,
  ]);
  if (tp.key === key && el.inner.querySelector(".tp-card, .dr-empty")) return;
  tp.key = key;
  const top = el.tl.scrollTop;
  el.inner.replaceChildren(...tpDirectory(c, ctx));
  el.tl.scrollTop = top;
}
