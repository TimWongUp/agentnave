"use strict";
// Derived session desks in the office (ADR-0124): separate floor zone, on-desk state from
// ChatTerminal.ofDesk(), open-to-continue on click, and desk context menu.
const ChatDerived = (() => {
  const { L, setText, reconcile } = AN;

  // kind, 中文, English; "dormant" (its process ended after idling) is only Idle to the reader.
  const STATES = {
    waking: ["working", "正在恢复", "Resuming"],
    starting: ["working", "启动中", "Starting"],
    running: ["working", "运行中", "Running"],
    waiting: ["review", "等待确认", "Needs attention"],
    uncollected: ["review", "待 boss 收取", "Awaiting boss"],
    exited: ["idle", "已结束", "Exited"],
    idle: ["idle", "空闲", "Idle"],
  };
  const groupOf = () => (typeof state !== "undefined" ? state.chat?.group || state.loaded : null);

  function terminalFor(d, group = groupOf()) {
    if (!group || typeof ChatTerminal === "undefined" || !ChatTerminal?.ofDesk) return null;
    return ChatTerminal.ofDesk(group, d.key) || null;
  }

  function mood(d, _ctx, group = groupOf()) {
    const x = terminalFor(d, group);
    const waking = x ? ChatTerminal.isWaking(x.id) : ChatTerminal?.reviving?.(group, d.key);
    // Its terminal is matched by the native session, which a call just sent does not have yet.
    const starting = d.status === "running" && d.key?.startsWith("task:");
    const ds = waking ? "waking" : x ? ChatTerminal.derivedStateOf(x) : starting ? "starting" : "idle";
    const [kind, zh, en] = STATES[ds] || STATES.idle;
    const word = L(zh, en);
    return { mood: kind, say: kind === "working" ? [word] : word, word, away: null };
  }

  function renderZone(derivedDesks, ctx, ops) {
    if (!el?.derivedZone || !el?.derivedDesks) return;
    if (!derivedDesks.length) {
      el.derivedZone.hidden = true;
      reconcile(el.derivedDesks, []);
      if (el.derivedCount) setText(el.derivedCount, "");
      return;
    }
    el.derivedZone.hidden = false;
    const nodes = derivedDesks.map((d) => {
      let rec = state.deskRecs.get(d.key);
      if (!rec) state.deskRecs.set(d.key, (rec = ops.createDesk(d)));
      ops.updateDesk(rec, d, ctx);
      return rec.el;
    });
    reconcile(el.derivedDesks, nodes);
    if (el.derivedCount) setText(el.derivedCount, ` · ${derivedDesks.length}`);
  }

  // Opening a desk always shows its messages; resuming its terminal is an explicit action.
  function openDesk(d, openerEl) {
    const group = groupOf();
    if (!group) return;
    ChatTerminal?.stayOnMessages?.(group, d.key);
    if (typeof openScope === "function") openScope({ kind: "desk", key: d.key }, openerEl);
  }

  function resumeDesk(d, openerEl, x) {
    const group = groupOf();
    if (!group) return;
    openDesk(d, openerEl);
    void ChatTerminal?.revive?.({ id: x?.id, group, deskKey: d.key });
  }

  function deskMenu(d, openerEl, e) {
    if (typeof Actions === "undefined" || !Actions?.openMenu) return;
    const x = terminalFor(d);
    const ds = x ? ChatTerminal.derivedStateOf(x) : "dormant";
    const live = Boolean(x && ds !== "dormant" && ds !== "exited");
    Actions.openMenu(e, [
      { label: L("打开", "Open"), run: () => openDesk(d, openerEl) },
      !live && (x || d.key?.startsWith("thread:")) && { label: L("续接", "Resume"), run: () => resumeDesk(d, openerEl, x) },
      x && { label: L("关闭终端", "Close terminal"), danger: true, run: () => ChatTerminal?.close?.(x.id) },
    ].filter(Boolean));
  }

  return { mood, renderZone, openDesk, deskMenu };
})();
