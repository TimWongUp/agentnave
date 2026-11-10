"use strict";
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
function speakerName(m) {
  const s = m.speaker || {};
  if (s.role === "system") return s.label || L("系统", "System");
  if (isBoss(m)) return "boss";
  // A message signs with the name it was said under, not what its desk goes by now.
  return AN.deskName(s);
}
// The role leads the name and the CLI follows it, so several "默认模型" desks stay apart.
function speakerRole(m) {
  return isBoss(m) ? "" : m.speaker?.identity?.role || "";
}
function speakerSub(m) {
  const s = m.speaker || {};
  if (s.role === "system" || isBoss(m)) return "";
  return AN.cliOf(s);
}
function speakerFace(m, ctx) {
  const s = m.speaker || {};
  if (s.role === "system") return null;
  // A boss wears the character of the model it ran when it spoke, a former boss included.
  if (isBoss(m)) return ART.head(ART.character(s.provider || ctx.c.host, s.model), "idle");
  const d = speakerDesk(m, ctx);
  return ART.head(d ? deskChar(d) : ART.character(s.provider, s.model), "idle");
}
// Recipients as chips. New payloads carry `to: [{key, handle, identity}]`; older ones only `recipients`.
function targets(m, ctx) {
  if (Array.isArray(m.to) && m.to.length && typeof m.to[0] === "object")
    return m.to.map((t) => {
      if (t.key === "@boss") return { kind: "boss" };
      if (t.key === "@all") return { kind: "all" };
      const d = ctx.desks.get(t.key);
      return { kind: "desk", d, who: t };
    });
  const list = (m.recipients || []).map((r) => {
    const key = r.seat ? ctx.seatDesk.get(`${m.conversation}|${r.seat}`) : null;
    const d =
      (key && ctx.desks.get(key)) ||
      (m.thread?.kind === "desk" ? ctx.desks.get(m.desk) : null);
    return { kind: "desk", d, who: r };
  });
  const room = m.thread?.kind === "room" && ctx.rooms.get(m.thread.id);
  if (room && list.length > 1 && list.length === room.seats.length)
    return [{ kind: "all" }];
  return list;
}
// A historical recipient only carries the identity recorded on that message.
const targetWho = (t) => t.who || {};
function chip(t) {
  if (t.kind === "boss") return h("span.at.boss", {}, "@boss");
  if (t.kind === "all")
    return h("span.at.all", { title: L("会议室里的所有人", "Everyone in the meeting room") }, "@all");
  const who = targetWho(t);
  const role = who.identity?.role;
  // One run of inline text, so a long name wraps back under the @ instead of beside the head.
  return h(
    "span.at.who",
    { title: AN.fullHandle(who) },
    h(
      "span.at-name",
      {},
      "@",
      t.d ? ART.head(deskChar(t.d)) : null,
      role ? h("span.at-role", {}, role) : null,
      AN.deskName(who),
    ),
  );
}
// Like an office chat: what boss sends carries a kind tag (任务, 议题, 转发) and its
// recipients; a reply sits under it with no verb, plus a badge only when it needs attention.
// Returns [tag, recipients, note, badge].
function verbOf(m, ctx) {
  const t = m.thread;
  const room = t?.kind === "room" ? ctx.rooms.get(t.id) : null;
  if (m.kind === "handover") return ["", []];
  if (m.kind === "typing") return ["", [], L("正在输入…", "Typing…"), ""];
  if (m.kind === "instruction") return ["", targets(m, ctx), "", L("仅 TA 可见", "Private")];
  // A private task has one recipient, so only a meeting names who a message is for.
  const to = t?.kind === "room" ? targets(m, ctx) : [];
  if (m.kind === "forward") return [L("转发", "Forward"), to];
  if (isBoss(m)) {
    if (t?.kind !== "room") return [L("任务", "Task"), to];
    const round = m.round > 0 ? L(`第 ${m.round} 轮`, `Round ${m.round}`) : "";
    const where = state.drawer.kind === "room" || !room ? round : L(`「${room.title}」${round}`, `"${room.title}"${round ? ` · ${round}` : ""}`);
    return [m.round > 0 ? L("议题", "Topic") : L("发言", "Note"), to, where];
  }
  // Kept from the other seats in review: a reply only boss sees, never deleted.
  if (m.kept_private) return ["", [], "", L("未公开 · 仅 boss 可见", "Unpublished · boss only")];
  if (m.status === "review") return ["", [], "", L("待 boss 审核", "Awaiting boss review")];
  // A parallel round shows the answers to the other seats once every seat has answered.
  if (t?.kind === "room" && m.visibility !== "public") return ["", [], "", L("本轮结束后公开", "Visible after round")];
  return ["", []];
}
// Who answers this round, shown by face; the answers themselves follow right below.
function answeringDesks(m, ctx) {
  return (m.answering || []).map((key) => ctx.desks.get(key)).filter(Boolean);
}
function metaOf(m, ctx) {
  if (Array.isArray(m.answering) && m.answering.length)
    return L(`${m.answering.length} 位回答`, `${m.answering.length} answering`);
  if (m.status === "review" && !m.kept_private)
    return L("boss 公开后，会议室里的其他人才看得到", "Visible to others in the room once boss publishes it");
  return "";
}
// Native prompt fields are subagent data: format their words without interpreting markup.
function approvalText(m) {
  const p = m.approval || {}, a = p.answer || {};
  const question = p.kind === "question";
  const tool = p.tool_name || L("工具", "tool");
  if (p.tool_name === "ExitPlanMode") return planText(p, a);
  let text;
  if (a.actor === "service_cancel")
    text = question ? L("取消时自动拒绝了提问", "Automatically denied the question on cancellation")
      : L(`取消时自动拒绝了 ${tool}`, `Automatically denied ${tool} on cancellation`);
  else if (a.actor === "boss") {
    if (question) {
      const answers = Object.entries(a.answers || {}).map(([q, answer]) => `${q} → ${answer}`).join(L("；", "; "));
      text = L("boss 回答了提问", "boss answered the question") + (answers ? L(`：${answers}`, `: ${answers}`) : "");
    } else if (a.decision === "allow") text = L(`boss 允许了 ${tool}`, `boss allowed ${tool}`);
    else text = L(`boss 拒绝了 ${tool}`, `boss denied ${tool}`) + (a.message ? L(`：${a.message}`, `: ${a.message}`) : "");
  } else if (p.status === "closed")
    text = question ? L("提问已结束，未回答", "Question closed without an answer")
      : L(`${tool} 的请求已结束，未回答`, `${tool} request closed without an answer`);
  else text = question ? L("等 boss 回答提问", "Waiting for boss to answer the question")
    : L(`等 boss 决定：${tool}`, `Waiting for boss to decide: ${tool}`);
  if (p.status === "closed" && a.actor) text += L("（答复未送达）", " (answer not delivered)");
  return text;
}
// Approving a plan is what the user sees in Claude Code, not a tool name.
function planText(p, a) {
  let text;
  if (a.actor === "service_cancel") text = L("取消时自动退回了计划", "Automatically sent the plan back on cancellation");
  else if (a.actor === "boss")
    text = a.decision === "allow" ? L("boss 批准了计划", "boss approved the plan")
      : L("boss 退回了计划", "boss sent the plan back") + (a.message ? L(`：${a.message}`, `: ${a.message}`) : "");
  else if (p.status === "closed") text = L("计划确认已结束，未回答", "Plan approval closed without an answer");
  else text = L("等 boss 批准计划", "Waiting for boss to approve the plan");
  if (p.status === "closed" && a.actor) text += L("（答复未送达）", " (answer not delivered)");
  return text;
}
function approvalInput(m) {
  let input = m.approval?.tool_input || "";
  try {
    const data = JSON.parse(input);
    input = data?.file_path || data?.path || data?.command || data?.plan || input;
  } catch { /* A non-JSON input is still plain text. */ }
  const chars = Array.from(String(input).replace(/\s+/gu, " ").trim());
  return chars.length > 80 ? chars.slice(0, 79).join("") + "…" : chars.join("");
}
// One-line ticker / screen-reader sentence for a message.
function eventText(m, ctx) {
  const who = speakerName(m);
  if (m.kind === "approval") return { who, text: who + L("：", ": ") + approvalText(m) };
  const room = m.thread?.kind === "room" ? ctx.rooms.get(m.thread.id) : null;
  const to = targets(m, ctx)
    .map((t) =>
      t.kind === "desk" ? AN.deskName(targetWho(t)) : t.kind,
    )
    .join(L("、", ", "));
  const text =
    m.kind === "handover"
      ? m.text
      : m.kind === "instruction"
      ? `boss @${to}`
      : m.kind === "forward"
        ? L(`boss 转发了一条消息${room ? `到「${room.title}」` : ""}`, `boss forwarded a message${room ? ` to "${room.title}"` : ""}`)
        : isBoss(m)
          ? room
            ? L(`boss 在「${room.title}」提出议题`, `boss raised a topic in "${room.title}"`)
            : L(`boss 给 ${to || "工位"} 布置了任务`, `boss assigned a task to ${to || "desk"}`)
          : m.kind === "notice"
            ? `${who} ${m.text}`
            : m.status === "review"
              ? L(`${who} 提交了回答，等 boss 审核`, `${who} submitted a reply, awaiting boss review`)
              : room
                ? L(`${who} 在「${room.title}」回复了议题`, `${who} replied in "${room.title}"`)
                : L(`${who} 回复了任务`, `${who} replied to the task`);
  return { who, text };
}

// ---------------------------------------------------------------- message nodes
function createMessage(m) {
  const rec = { kind: m.kind, text: null };
  rec.time = h("time");
  rec.ava = h("span.m-ava");
  rec.role = h("span.m-role");
  rec.name = h("b.m-name");
  rec.who = h("span.m-who");
  if (m.kind === "notice" || m.kind === "approval") {
    rec.text_ = h("span.note-text");
    rec.detail = h("small.note-detail");
    rec.el = h(
      "div.note",
      {},
      rec.ava,
      rec.role,
      rec.name,
      rec.who,
      rec.text_,
      rec.time,
      rec.detail,
    );
    return rec;
  }
  rec.verb = h("span.m-verb");
  rec.to = h("div.m-to");
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
    m.kind === "typing" ? [rec.tail, rec.status, rec.dots] : [rec.fwd, rec.to, rec.md],
  );
  rec.metaFaces = h("span.tp-faces");
  rec.metaText = h("span");
  rec.meta = h("div.m-meta", { hidden: true }, rec.metaFaces, rec.metaText);
  rec.more = h("button.more", { type: "button", hidden: true }, L("展开全文", "Show more"));
  rec.more.addEventListener("click", () => {
    const id = rec.id;
    if (state.expanded.has(id)) state.expanded.delete(id);
    else state.expanded.add(id);
    // A peer's or handover's collapsed bubble holds one line, not its answer: draw it again.
    if (rec.el.dataset.peer !== undefined || rec.el.dataset.brief !== undefined) renderAll(state.chat);
    applyFolds([rec]);
  });
  rec.el = h(
    "article.msg",
    {},
    h(
      "div.m-h",
      {},
      rec.ava,
      rec.role,
      rec.name,
      rec.who,
      rec.verb,
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
  const key = `${s.role}|${s.provider}|${s.model}|${m.desk}|${isBoss(m) ? `${ctx.c.host}|${ctx.c.app || ""}` : ""}|${ART.version()}`;
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
  command_execution: L("正在运行命令", "Running command"),
  file_change: L("正在修改文件", "Editing files"),
  web_search: L("正在搜索网页", "Searching web"),
  read_file: L("正在读取文件", "Reading file"),
};
// Antigravity's own tool names, as its transcript reports them.
for (const [names, word] of [
  [["run_command"], TOOL_WORDS.command_execution],
  [["view_file", "list_dir", "grep_search", "find_by_name"], TOOL_WORDS.read_file],
  [["edit_file", "write_file", "write_to_file", "replace_file_content", "multi_replace_file_content"], TOOL_WORDS.file_change],
  [["search_web", "read_url_content"], TOOL_WORDS.web_search],
]) for (const name of names) TOOL_WORDS[name] = word;
function activityDuration(start) {
  const at = Date.parse(start);
  if (!Number.isFinite(at)) return "";
  const seconds = Math.max(0, Math.floor((Date.now() - at) / 1000));
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, "0")}s`;
}
function typingStatus(m) {
  const d = m.doing || "";
  const tool = d.slice(5);
  const doing = d === "thinking"
    ? L("思考中", "Thinking")
    : d === "retrying"
      ? L("重试中", "Retrying")
      : d.startsWith("tool:")
        ? TOOL_WORDS[tool] || L(`正在使用 ${tool || "工具"}`, `Using ${tool || "tool"}`)
        : "";
  const quiet = m.text && m.output_at && Date.now() - Date.parse(m.output_at) >= STALE_TAIL_MS;
  const duration = doing && m.doing_since ? activityDuration(m.doing_since) : "";
  return [doing, duration, quiet ? L(`上次输出 ${relTime(m.output_at)}`, `Last output ${relTime(m.output_at)}`) : ""].filter(Boolean).join(" · ");
}
const SOURCE_STAMP = / · (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))$/;
function localizeSourceTime(text) {
  return text.replace(SOURCE_STAMP, (suffix, iso) => {
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return suffix;
    const shown = date.getFullYear() === new Date().getFullYear()
      ? `${dayLabel(iso)} ${clock(iso)}` : fullTime(iso);
    return ` · ${shown}`;
  });
}
function updateMessage(rec, m, ctx, id) {
  rec.id = id;
  const flashing = state.flashId === id ? " flash" : "";
  setAttr(rec.el, "data-id", id);
  setAttr(rec.el, "data-conv", m.conversation);
  setAttr(rec.el, "data-mid", m.message_id);
  setAttr(rec.el, "data-kind", m.kind);
  const peer = state.peers?.has(id);
  setAttr(rec.el, "data-peer", peer ? m.desk : null);
  setFace(rec, m, ctx);
  setText(rec.role, speakerRole(m));
  setText(rec.name, speakerName(m));
  setText(rec.who, speakerSub(m));
  setText(rec.time, m.kind === "typing" ? "" : clock(m.time));
  setAttr(rec.time, "title", m.kind === "typing" ? null : fullTime(m.time));
  if (m.kind === "notice" || m.kind === "approval") {
    const approval = m.kind === "approval";
    const tone = approval ? "idle" : pick(AN.MEMBER, m.status)[1];
    setClass(rec.el, `note tone-${tone}${flashing}`);
    setText(rec.text_, approval ? approvalText(m) : m.text);
    setText(rec.detail, approval ? approvalInput(m) : m.detail || "");
    return;
  }
  AN.voice?.decorate(rec, m, ctx);
  const [verb, to, verb2 = "", lock = ""] = verbOf(m, ctx);
  setClass(rec.verb, verb ? "m-verb m-tag" : "m-verb");
  const priv =
    m.kind === "instruction" ||
    (m.thread?.kind === "room" && m.visibility !== "public" && !isBoss(m));
  setClass(
    rec.el,
    `msg${isBoss(m) ? " boss" : ""}${priv ? " private" : ""}${m.status === "review" ? " pending" : ""}${m.reply_to ? " reply" : ""}${m.kind === "typing" ? " typing-msg" : ""}${m.kind === "forward" ? " forward" : ""}${flashing}`,
  );
  setText(rec.verb, verb);
  setText(rec.verb2, verb2);
  setText(rec.lock, lock);
  const toKey = JSON.stringify([ART.version(), ...to.map((t) => [t.kind, t.d?.key, t.kind === "desk" && [AN.deskName(targetWho(t)), AN.fullHandle(targetWho(t))]])]);
  if (rec.toKey !== toKey) {
    rec.toKey = toKey;
    rec.to.replaceChildren(...to.map(chip));
  }
  const meta = metaOf(m, ctx);
  rec.meta.hidden = !meta;
  setClass(rec.meta, m.answering?.length ? "m-meta m-round" : "m-meta");
  setText(rec.metaText, meta);
  const faces = answeringDesks(m, ctx);
  setAttr(rec.meta, "title", faces.length ? faces.map(AN.fullHandle).join("\n") : null);
  const facesKey = JSON.stringify([ART.version(), ...faces.map((d) => d.key)]);
  if (rec.facesKey !== facesKey) {
    rec.facesKey = facesKey;
    rec.metaFaces.replaceChildren(...faces.map((d) => ART.head(deskChar(d))));
  }
  if (m.kind !== "typing") {
    // A forwarded entry quotes a message by id; the card jumps to it while that message is still listed.
    const f = m.forward;
    rec.fwd.hidden = !f;
    if (f) {
      rec.fwdId = ctx.sources.has(f.message_id) ? f.message_id : null;
      const source = f.source && localizeSourceTime(f.source);
      setText(rec.fwd, L(`↪ 转发自 ${source || "其他消息"}`, `↪ Forwarded from ${source || "another message"}`));
      setClass(rec.fwd, rec.fwdId ? "fwd" : "fwd dead");
      setAttr(rec.fwd, "aria-disabled", rec.fwdId ? null : "true");
      setAttr(
        rec.fwd,
        "title",
        rec.fwdId ? L("跳到原消息", "Jump to original message") : L("原消息不在当前列表中", "Original message not in current view"),
      );
    }
  }
  if (m.kind === "typing") {
    rec.statusMessage = m;
    const status = typingStatus(m);
    setText(rec.status, status);
    rec.status.hidden = !status;
  }
  const summary = peer && m.kind !== "typing" && !state.expanded.has(id);
  // A handover note is long: it opens on request, below the one line saying who took over.
  const brief = m.kind === "handover" && !!m.detail;
  rec.el.toggleAttribute("data-brief", brief);
  const note = brief && state.expanded.has(id) ? m.detail : "";
  const body = m.kind === "handover" ? [m.text, note].filter(Boolean).join("\n\n") : summary ? tpFirst(m.text) : m.text;
  if (rec.text !== body) {
    rec.text = body;
    if (m.kind === "typing") {
      setText(rec.tail, m.text);
      rec.dots.hidden = !!m.text;
      rec.tail.hidden = !m.text;
    } else if (m.kind === "handover") {
      setClass(rec.md, "md handover-text");
      setText(rec.md, body);
      rec.dirty = true;
    } else if (summary) {
      setText(rec.md, body);
      rec.dirty = true;
    } else {
      renderMarkdown(rec.md, m.text);
      rec.dirty = true; // height changed: measure again
    }
  }
}
// Fold: clamp by CSS, show the button only when the content really overflows. Discarded answers
// start folded whatever their length.
