"use strict";
// Derived session presentation in parent boss offices (.boss-side list & menu) and derived offices
// (origin metadata & boss status) per ADR-0124, ADR-0128, and ADR-0130.
const ChatDerived = (() => {
  const { L } = AN;
  const relTimeOf = (v) => (AN.relTime ? AN.relTime(v) : typeof relTime === "function" ? relTime(v) : "");
  const deskNameOf = (d) => (AN.deskName ? AN.deskName(d) : typeof deskName === "function" ? deskName(d) : d?.model || "");
  const cliOfDesk = (d) => (AN.cliOf ? AN.cliOf(d) : typeof cliOf === "function" ? cliOf(d) : d?.provider || "");
  const cliNameOf = (p) => (AN.cliName ? AN.cliName(p) : p || "");
  const shownNameOf = (g) => (AN.shownName ? AN.shownName(g) : typeof shownName === "function" ? shownName(g) : g?.display_title || g?.title || g?.group || "");

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

  function isOffice(c) {
    return Boolean(c && (c.derived === true || c.derived_origin || (c.parent_workspace_id && c.parent_desk_key)));
  }

  function hasDownstream(c) {
    return Boolean((c?.rooms && c.rooms.length) || (c?.messages && c.messages.length) || c?.handover === "pending");
  }

  function originOf(c) {
    if (!isOffice(c)) return null;
    const raw = c.derived_origin || null;
    const parentGroup = raw?.parent_workspace_id || c.parent_workspace_id || "";
    const deskKey = raw?.parent_desk_key || c.parent_desk_key || "";
    if (!parentGroup || !deskKey) return null;
    const desk = raw?.desk || {
      key: deskKey,
      provider: c.host || "",
      model: c.boss_art_model || "",
      art_model: c.boss_art_model || "",
      derived: true,
    };
    const parentRow = typeof state !== "undefined" && Array.isArray(state?.groupsData)
      ? state.groupsData.find((g) => g.group === parentGroup)
      : null;
    const parentTitle = raw?.parent_title || (parentRow ? shownNameOf(parentRow) : "") || raw?.parent_label || parentGroup;
    const parentLabel = raw?.parent_label || parentRow?.label || "";
    const messages = Array.isArray(raw?.messages) ? raw.messages : [];
    return { parentGroup, deskKey, parentTitle, parentLabel, desk, messages };
  }

  function terminalFor(d, group = groupOf()) {
    if (!group || !d?.key || typeof ChatTerminal === "undefined" || !ChatTerminal?.ofDesk) return null;
    return ChatTerminal.ofDesk(group, d.key) || null;
  }

  // A terminal is matched by its native session, which a call just sent does not have yet (Codex
  // asks about trust before it starts one): the workspace's one such terminal of that CLI is it.
  function unnamedTerminal(d, group) {
    if (typeof ChatTerminal === "undefined" || !d) return null;
    const found = (ChatTerminal?.all?.() || []).filter(
      (x) => x.derived && x.parent_workspace_id === group && !x.native && x.provider === d.provider && x.exit == null,
    );
    return found.length === 1 ? found[0] : null;
  }

  function mood(d, _ctx, group = groupOf()) {
    const starting = d.status === "running" && d.key?.startsWith("task:");
    const x = terminalFor(d, group) || (starting ? unnamedTerminal(d, group) : null);
    const waking = x ? ChatTerminal.isWaking(x.id) : ChatTerminal?.reviving?.(group, d.key);
    const ds = waking ? "waking" : x ? ChatTerminal.derivedStateOf(x) : starting ? "starting" : "idle";
    const [kind, zh, en] = STATES[ds] || STATES.idle;
    const word = L(zh, en);
    return { mood: kind, say: kind === "working" ? [word] : word, word, away: null, ds, x };
  }

  function rowTone(ds, childStatus) {
    if (ds === "waiting" || ds === "uncollected" || childStatus === "review") return "review";
    if (ds === "running" || ds === "starting" || ds === "waking" || childStatus === "running") return "run";
    return "";
  }

  function isActiveRank(ds, childStatus) {
    return (
      ds === "waiting" ||
      ds === "running" ||
      ds === "starting" ||
      ds === "waking" ||
      ds === "uncollected" ||
      childStatus === "running" ||
      childStatus === "review"
    );
  }

  function sideItems(c) {
    if (isOffice(c)) return [];
    const parentGroup = c.group;
    const derivedDesks = (c.desks || []).filter((d) => d.derived);
    const liveRows = typeof ChatTerminal !== "undefined" ? (ChatTerminal.derivedOf?.(parentGroup) || []) : [];
    const byDesk = new Map();
    for (const row of liveRows) {
      if (row.desk_key) byDesk.set(row.desk_key, row);
    }
    const usedRowGroups = new Set();
    const entries = [];
    for (const d of derivedDesks) {
      const row = byDesk.get(d.key) || null;
      if (row) usedRowGroups.add(row.group);
      const m = mood(d, null, parentGroup);
      const childStatus = row?.child_status || "";
      const active = isActiveRank(m.ds, childStatus);
      const tone = rowTone(m.ds, childStatus);
      const statusWord = !active && childStatus === "review"
        ? L("待审核", "Review")
        : !active && childStatus === "running"
          ? L("运行中", "Running")
          : m.word;
      const name = row?.display_title || row?.title || deskNameOf(d) || L("派生终端", "Derived terminal");
      const sub = [statusWord, cliOfDesk(d), d.role].filter(Boolean).join(" · ");
      const at = Math.max(AN.at(d.last_active) || 0, AN.at(row?.updated) || 0);
      entries.push({
        active,
        at,
        id: d.key,
        desk: d,
        row,
        parentGroup,
        scope: { kind: "desk", key: d.key },
        heads: [d],
        name,
        sub,
        tone,
        time: d.last_active ? relTimeOf(d.last_active) : row?.updated ? relTimeOf(row.updated) : "",
      });
    }
    for (const row of liveRows) {
      if (usedRowGroups.has(row.group)) continue;
      const ds = row.waking ? "waking" : row.derived_state || "idle";
      const childStatus = row.child_status || "";
      const active = isActiveRank(ds, childStatus);
      const tone = rowTone(ds, childStatus);
      const meta = typeof ChatTerminal !== "undefined" && ChatTerminal.derivedMeta
        ? ChatTerminal.derivedMeta(row, { withElapsed: false }).text
        : L("空闲", "Idle");
      const headDesk = { provider: row.host, model: row.boss_art_model || "", art_model: row.boss_art_model || "" };
      entries.push({
        active,
        at: AN.at(row.updated) || 0,
        id: row.group,
        desk: null,
        row,
        parentGroup,
        scope: null,
        heads: [headDesk],
        name: row.display_title || row.title || L("派生终端", "Derived terminal"),
        sub: [meta, row.label || cliNameOf(row.host)].filter(Boolean).join(" · "),
        tone,
        time: row.updated ? relTimeOf(row.updated) : "",
      });
    }
    entries.sort((a, b) => Number(b.active) - Number(a.active) || b.at - a.at || String(a.id).localeCompare(String(b.id)));
    return entries;
  }

  function openSideItem(item, openerEl) {
    if (item.desk) {
      if (typeof openScope === "function") openScope({ kind: "desk", key: item.desk.key }, openerEl);
      return;
    }
    if (item.row && typeof ChatTerminal !== "undefined") {
      ChatTerminal.openDerived?.(item.row.group);
    }
  }

  function sideMenu(item, openerEl, e) {
    if (typeof Actions === "undefined" || !Actions?.openMenu) return;
    const group = item.parentGroup || groupOf();
    const d = item.desk;
    const row = item.row;
    const x = (d ? terminalFor(d, group) : null)
      || (row?.terminal != null && typeof ChatTerminal !== "undefined"
        ? (ChatTerminal.all?.() || []).find((s) => s.id === row.terminal) || null
        : null);
    const ds = x && typeof ChatTerminal !== "undefined" ? ChatTerminal.derivedStateOf(x) : "dormant";
    const live = Boolean(x && ds !== "dormant" && ds !== "exited");
    const canResume = !live && Boolean(x || d?.key?.startsWith("thread:") || row?.desk_key?.startsWith("thread:"));
    Actions.openMenu(e, [
      d && { label: L("查看消息", "View messages"), run: () => openSideItem(item, openerEl) },
      {
        label: L("进入派生办公室", "Open derived office"),
        run: () => ChatTerminal?.openDerivedOffice?.(group, d?.key || row?.desk_key, row?.group),
      },
      canResume && {
        label: L("续接", "Resume"),
        run: () => ChatTerminal?.resumeDerived?.({ group, deskKey: d?.key || row?.desk_key, row, terminal: x }),
      },
      live && x && { label: L("关闭终端", "Close terminal"), danger: true, run: () => ChatTerminal?.close?.(x.id) },
    ].filter(Boolean));
  }

  // Boss figure state inside a derived office: combines derived terminal activity and downstream waits.
  function bossMood(c, n, recentlyActive) {
    const x = typeof ChatTerminal !== "undefined" ? ChatTerminal.terminalOf?.(c.group) : null;
    const origin = originOf(c);
    const waking = Boolean(
      (x && ChatTerminal?.isWaking?.(x.id)) ||
      (origin && ChatTerminal?.reviving?.(origin.parentGroup, origin.deskKey)),
    );
    const ds = x && typeof ChatTerminal !== "undefined" ? ChatTerminal.derivedStateOf(x) : "idle";
    if (ds === "waiting") return { mood: "review", text: L("等待确认", "Needs confirmation") };
    if (n.running) return { mood: "working", text: L(`等 ${n.running} 个回复`, `Waiting on ${n.running}`) };
    if (n.review) return { mood: "working", text: L("会议进行中", "In meeting") };
    if (waking) return { mood: "working", text: L("正在恢复", "Resuming") };
    if (ds === "starting") return { mood: "working", text: L("启动中", "Starting") };
    if (ds === "running") return { mood: "working", text: L("工作中", "Working") };
    if (ds === "uncollected") return { mood: "review", text: L("待 boss 收取", "Awaiting boss") };
    return { mood: recentlyActive ? "idle" : "sleep", text: L("空闲", "Idle") };
  }

  return { isOffice, hasDownstream, originOf, mood, sideItems, openSideItem, sideMenu, bossMood };
})();
