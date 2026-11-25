"use strict";
// Derived session desks in the office (ADR-0124): separate floor zone, on-desk state from
// ChatTerminal.ofDesk(), dormant dimming, wake-on-click, and desk context menu.
const ChatDerived = (() => {
  const { L, setText, reconcile } = AN;

  function terminalFor(d, groupOverride) {
    const group = groupOverride || (typeof state !== "undefined" ? state.chat?.group || state.loaded : null);
    if (!group || typeof ChatTerminal === "undefined" || !ChatTerminal?.ofDesk) return null;
    return ChatTerminal.ofDesk(group, d.key) || null;
  }

  function mood(d, _ctx, groupOverride) {
    const x = terminalFor(d, groupOverride);
    if (x && ChatTerminal?.isWaking?.(x.id)) {
      const word = L("正在恢复", "Resuming");
      return { mood: "working", say: [word], word, away: null, dormant: false, waking: true };
    }
    if (!x) {
      const word = L("已收起", "Dormant");
      return { mood: "sleep", say: word, word, away: null, dormant: true, waking: false };
    }
    const ds = ChatTerminal?.derivedStateOf ? ChatTerminal.derivedStateOf(x) : "idle";
    switch (ds) {
      case "starting": {
        const word = L("启动中", "Starting");
        return { mood: "working", say: [word], word, away: null, dormant: false, waking: false };
      }
      case "running": {
        const word = L("运行中", "Running");
        return { mood: "working", say: [word], word, away: null, dormant: false, waking: false };
      }
      case "waiting": {
        const word = L("等待确认", "Needs attention");
        return { mood: "review", say: word, word, away: null, dormant: false, waking: false };
      }
      case "uncollected": {
        const word = L("待 boss 收取", "Awaiting boss");
        return { mood: "review", say: word, word, away: null, dormant: false, waking: false };
      }
      case "dormant": {
        const word = L("已收起", "Dormant");
        return { mood: "sleep", say: word, word, away: null, dormant: true, waking: false };
      }
      case "exited": {
        const word = L("已结束", "Exited");
        return { mood: "idle", say: word, word, away: null, dormant: false, waking: false };
      }
      default: {
        const word = L("空闲", "Idle");
        return { mood: "idle", say: word, word, away: null, dormant: false, waking: false };
      }
    }
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

  function openDesk(d, openerEl) {
    const group = typeof state !== "undefined" ? state.chat?.group || state.loaded : null;
    if (!group) return;
    const x = terminalFor(d, group);
    const ds = x && ChatTerminal?.derivedStateOf ? ChatTerminal.derivedStateOf(x) : "dormant";
    if (x && ds === "dormant" && x.resumable) {
      if (typeof openScope === "function") openScope({ kind: "desk", key: d.key }, openerEl);
      void ChatTerminal?.wake?.(x.id, { group, deskKey: d.key });
      return;
    }
    if (!x || ds === "dormant" || ds === "exited") {
      if (typeof openScope === "function") openScope({ kind: "desk", key: d.key }, openerEl);
      return;
    }
    if (typeof openScope === "function") openScope({ kind: "desk", key: d.key }, openerEl, { terminal: true });
    ChatTerminal?.focusFrame?.(x.id);
  }

  function deskMenu(d, openerEl, e) {
    if (typeof Actions === "undefined" || !Actions?.openMenu) return;
    const group = typeof state !== "undefined" ? state.chat?.group || state.loaded : null;
    const x = terminalFor(d, group);
    if (!x) {
      Actions.openMenu(e, [
        { label: L("查看消息", "View messages"), run: () => typeof openScope === "function" && openScope({ kind: "desk", key: d.key }, openerEl) },
      ]);
      return;
    }
    const ds = ChatTerminal?.derivedStateOf ? ChatTerminal.derivedStateOf(x) : "idle";
    Actions.openMenu(e, [
      { label: L("打开", "Open"), run: () => openDesk(d, openerEl) },
      ds === "dormant"
        ? (x.resumable ? { label: L("恢复", "Resume"), run: () => ChatTerminal?.wake?.(x.id, { group, deskKey: d.key }) } : null)
        : (ds !== "exited" ? { label: L("收起", "Suspend"), run: () => ChatTerminal?.stop?.(x.id) } : null),
      { label: L("关闭终端", "Close terminal"), danger: true, run: () => ChatTerminal?.close?.(x.id) },
    ].filter(Boolean));
  }

  return { mood, renderZone, openDesk, deskMenu };
})();
