"use strict";
// The turns a derived session's native log holds beyond the boss's tasks, merged into its desk's
// messages (ADR-0128).
function syncDerivedNotice(text) {
  if (!el.tl || !el.inner) return;
  let note = el.derivedNotice;
  if (!text) {
    if (note) note.hidden = true;
    return;
  }
  if (!note) {
    note = el.derivedNotice = h("div.dr-derived-note", { role: "status" });
    el.tl.insertBefore?.(note, el.inner);
  }
  note.hidden = false;
  setText(note, text);
}
function isDerivedDeskView(c, ctx, deskKey) {
  return Boolean(
    ctx?.desks?.get?.(deskKey)?.derived ||
    (typeof ChatTerminal !== "undefined" && ChatTerminal?.ofDesk?.(c.group, deskKey)),
  );
}
// What a desk's cached turns were read for; a change in it reads them again.
function derivedTurnsVersionKey(c, ctx, deskKey) {
  const desk = ctx?.desks?.get?.(deskKey);
  const term = typeof ChatTerminal !== "undefined" ? ChatTerminal?.ofDesk?.(c.group, deskKey) : null;
  return JSON.stringify([
    c.group,
    deskKey,
    desk?.private?.tasks ?? 0,
    desk?.status ?? "",
    term ? [term.id, term.started, term.derived_state, term.exit != null, term.native] : null,
  ]);
}
// Entering a desk's messages reads its turns again: the session may have worked while nobody looked.
function refreshDerivedTurns(group, deskKey) {
  const entry = state.derivedTurns?.cache.get(`${group}|${deskKey}`);
  if (entry) entry.stale = true;
}
function requestDerivedTurns(c, ctx, deskKey) {
  if (typeof load !== "function" || !state.derivedTurns) return null;
  const cacheId = `${c.group}|${deskKey}`;
  const versionKey = derivedTurnsVersionKey(c, ctx, deskKey);
  const entry = state.derivedTurns.cache.get(cacheId) || null;
  if ((!entry || entry.stale || entry.versionKey !== versionKey) && !state.derivedTurns.pending.has(cacheId)) {
    const req = load(
      `/app/terminal/derived-turns?workspace=${encodeURIComponent(c.group)}&desk=${encodeURIComponent(deskKey)}`,
    )
      .then((res) => {
        state.derivedTurns.cache.set(cacheId, {
          versionKey,
          turns: Array.isArray(res?.turns) ? res.turns : [],
          error: typeof res?.error === "string" ? res.error : "",
        });
      })
      .catch(() => {
        state.derivedTurns.cache.set(cacheId, {
          versionKey,
          turns: [],
          error: AN.L("无法读取终端里的用户消息", "Failed to read user messages from terminal log"),
        });
      })
      .finally(() => {
        state.derivedTurns.pending.delete(cacheId);
        if (state.chat?.group === c.group && state.drawer?.kind === "desk" && state.drawer.key === deskKey) {
          renderConv(state.chat, state.ctx);
        }
      });
    state.derivedTurns.pending.set(cacheId, req);
  }
  return entry;
}
function buildDerivedUserMessages(c, desk, deskKey, turns) {
  const L = AN.L || ((zh) => zh);
  const provider = desk?.provider || "";
  const cliName = AN.cliName?.(provider) || provider;
  const fallbackTime = c.updated || c.created || new Date().toISOString();
  const out = [];
  for (const turn of turns) {
    const promptId = `derived-user:${deskKey}:${turn.index}:prompt`;
    const userTime = turn.user_time || turn.assistant_time || fallbackTime;
    out.push({
      id: promptId,
      conversation: null,
      kind: "prompt",
      speaker: { role: "user", label: L("用户", "User") },
      to: [],
      recipients: [],
      desk: deskKey,
      thread: { kind: "desk", key: deskKey },
      text: turn.user_text,
      time: userTime,
      message_id: null,
      visibility: "private",
      forward: null,
      reply_to: null,
    });
    if (turn.assistant_text) {
      out.push({
        id: `derived-user:${deskKey}:${turn.index}:reply`,
        conversation: null,
        kind: "reply",
        speaker: {
          role: "member",
          provider,
          model: turn.model || desk?.model || "",
          label: cliName,
          identity: desk?.identity || {},
          handle: desk?.identity?.handle || cliName,
        },
        desk: deskKey,
        thread: { kind: "desk", key: deskKey },
        text: turn.assistant_text,
        time: turn.assistant_time || userTime,
        status: turn.status || "succeeded",
        message_id: null,
        visibility: "private",
        forward: null,
        reply_to: promptId,
      });
    } else if (turn.status === "interrupted" || turn.status === "failed") {
      out.push({
        id: `derived-user:${deskKey}:${turn.index}:notice`,
        conversation: null,
        kind: "notice",
        speaker: {
          role: "member",
          provider,
          model: turn.model || desk?.model || "",
          label: cliName,
          identity: desk?.identity || {},
          handle: desk?.identity?.handle || cliName,
        },
        desk: deskKey,
        thread: { kind: "desk", key: deskKey },
        text: turn.status === "interrupted" ? L("已中断", "Interrupted") : L("失败", "Failed"),
        detail: "",
        time: turn.assistant_time || userTime,
        status: turn.status,
        message_id: null,
        visibility: "private",
        forward: null,
        reply_to: promptId,
      });
    }
  }
  return out;
}
function mergeDerivedMessages(streamShown, userMsgs) {
  if (!userMsgs.length) return streamShown;
  return [...streamShown, ...userMsgs]
    .map((m, i) => ({ m, i, at: Number(AN.at?.(m.time)) || 0 }))
    .sort((a, b) => a.at - b.at || a.i - b.i)
    .map((x) => x.m);
}
