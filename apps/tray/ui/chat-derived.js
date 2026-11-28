"use strict";
// Derived session presentation in parent boss offices (.boss-side list & menu) and derived offices
// (origin metadata & boss status) per ADR-0124, ADR-0128, and ADR-0130.
const ChatDerived = (() => {
  const { L } = AN;
  const DERIVED_PREFIX = "drv-";
  const relTimeOf = (v) => (AN.relTime ? AN.relTime(v) : typeof relTime === "function" ? relTime(v) : "");
  const deskNameOf = (d) => (AN.deskName ? AN.deskName(d) : typeof deskName === "function" ? deskName(d) : d?.model || "");
  const cliOfDesk = (d) => (AN.cliOf ? AN.cliOf(d) : typeof cliOf === "function" ? cliOf(d) : d?.provider || "");
  const cliNameOf = (p) => (AN.cliName ? AN.cliName(p) : p || "");
  const shownNameOf = (g) => (AN.shownName ? AN.shownName(g) : typeof shownName === "function" ? shownName(g) : g?.display_title || g?.title || g?.group || "");

  const encodeToken = (s) =>
    encodeURIComponent(String(s)).replace(/[_!~*'().-]|%([0-9A-F]{2})/gi, (m, hex) =>
      `_${hex || m.charCodeAt(0).toString(16).padStart(2, "0")}`,
    );
  const decodeToken = (s) => {
    try {
      return decodeURIComponent(String(s).replace(/_([0-9a-f]{2})/gi, "%$1"));
    } catch {
      return null;
    }
  };
  const derivedRowId = (parentGroup, deskKey) => `${DERIVED_PREFIX}${encodeToken(parentGroup)}-${encodeToken(deskKey)}`;
  const isDerivedRowId = (id) => typeof id === "string" && id.startsWith(DERIVED_PREFIX);
  function parseDerivedRowId(id) {
    if (!isDerivedRowId(id)) return null;
    const rest = id.slice(DERIVED_PREFIX.length);
    const sep = rest.indexOf("-");
    if (sep <= 0 || sep === rest.length - 1 || rest.indexOf("-", sep + 1) >= 0) return null;
    const parentGroup = decodeToken(rest.slice(0, sep));
    const deskKey = decodeToken(rest.slice(sep + 1));
    if (!parentGroup || !deskKey) return null;
    return { parentGroup, deskKey };
  }

  function derivedContextOf(group, { pendingId, byId, derivedAll, ofDesk }) {
    if (!group) return null;
    const pid = pendingId(group);
    if (pid != null) {
      const x = byId(pid);
      if (x?.derived) {
        return {
          group,
          parentGroup: x.parent_workspace_id || "",
          deskKey: x.desk_key || "",
          terminal: x,
          row: null,
        };
      }
      return null;
    }
    const row = derivedAll().find((g) => g.group === group) || null;
    if (row) {
      const term = row.terminal != null
        ? byId(row.terminal)
        : (row.parent_workspace_id && row.desk_key ? ofDesk(row.parent_workspace_id, row.desk_key) : null);
      return {
        group,
        parentGroup: row.parent_workspace_id || "",
        deskKey: row.desk_key || "",
        terminal: term || null,
        row,
      };
    }
    const parsed = parseDerivedRowId(group);
    if (parsed) {
      return {
        group,
        parentGroup: parsed.parentGroup,
        deskKey: parsed.deskKey,
        terminal: ofDesk(parsed.parentGroup, parsed.deskKey) || null,
        row: null,
      };
    }
    if (typeof state !== "undefined" && state?.chat?.group === group && isOffice(state.chat)) {
      const origin = originOf(state.chat);
      const parentGroup = origin?.parentGroup || state.chat.parent_workspace_id || "";
      const deskKey = origin?.deskKey || state.chat.parent_desk_key || "";
      const term = parentGroup && deskKey ? ofDesk(parentGroup, deskKey) : null;
      return { group, parentGroup, deskKey, terminal: term || null, row: null };
    }
    return null;
  }

  function mergeChildIntoDerivedRow(row, childGroup, { derivedTitles, derivedMeta }) {
    if (!childGroup) return { ...row, child_workspace_id: null, child_status: "" };
    const childStatus = childGroup.status || "idle";
    const childActive = childStatus === "running" || childStatus === "review";
    const displayTitle = childGroup.display_title
      || row.display_title
      || (row.desk_key && derivedTitles?.[row.desk_key])
      || null;
    const title = displayTitle || childGroup.title || row.title;
    const latestUpdated = (AN.at?.(childGroup.updated) || 0) > (AN.at?.(row.updated) || 0)
      ? childGroup.updated
      : row.updated;
    const merged = {
      ...row,
      group: childGroup.group,
      child_workspace_id: childGroup.group,
      child_status: childStatus,
      boss_key: row.boss_key || childGroup.owner || null,
      isolated_count: childGroup.isolated_count || 0,
      desks_count: childGroup.desks_count || 0,
      rooms_count: childGroup.rooms_count || 0,
      display_title: displayTitle,
      title,
      live: row.live || Boolean(childGroup.live) || childActive,
      status: row.status === "idle" && childActive ? childStatus : row.status,
      updated: latestUpdated,
    };
    merged.preview = derivedMeta(merged, { withElapsed: false }).text;
    return merged;
  }

  function derivedChildrenOf(parentGroup, deskKey, ofDesk) {
    if (!parentGroup || !deskKey || typeof state === "undefined" || !Array.isArray(state?.groupsData)) return [];
    const parent = state.groupsData.find((g) => g.group === parentGroup);
    if (parent?.parent_workspace_id) return []; // one-level nesting only
    const histEntry = Array.isArray(parent?.derived) ? parent.derived.find((d) => d.key === deskKey) : null;
    const liveTerm = ofDesk(parentGroup, deskKey);
    const bossKey = histEntry?.boss_key || liveTerm?.boss_key || null;
    const childWsId = histEntry?.child_workspace_id || null;
    return state.groupsData.filter(
      (g) =>
        g.group !== parentGroup &&
        g.handover !== "pending" &&
        (Boolean(childWsId && g.group === childWsId) ||
          (g.parent_workspace_id === parentGroup && g.parent_desk_key === deskKey) ||
          Boolean(bossKey && g.owner === bossKey)),
    );
  }

  function liveDerivedOwnerOf(group, { list, isDerivedLive }) {
    if (!group) return null;
    const g = typeof state !== "undefined" && Array.isArray(state?.groupsData)
      ? state.groupsData.find((item) => item.group === group)
      : null;
    const parentWs = g?.parent_workspace_id || "";
    const parentDesk = g?.parent_desk_key || "";
    const owner = g?.owner || group;
    const x = list.find(
      (item) =>
        item.derived &&
        isDerivedLive(item) &&
        ((parentWs && parentDesk && item.parent_workspace_id === parentWs && item.desk_key === parentDesk) ||
          Boolean(owner && item.boss_key === owner)),
    );
    if (!x) return null;
    return {
      id: x.id,
      parentGroup: x.parent_workspace_id || parentWs,
      deskKey: x.desk_key || parentDesk,
    };
  }

  function derivedSummary(parentGroup, derivedOf) {
    const list = derivedOf(parentGroup);
    if (!list.length) return null;
    let running = 0;
    let waiting = 0;
    for (const item of list) {
      if (item.derived_state === "running" || item.derived_state === "starting" || item.child_status === "running") running += 1;
      else if (item.derived_state === "waiting" || item.child_status === "review") waiting += 1;
    }
    if (!running && !waiting) return null;
    const parts = [];
    if (running) parts.push(L(`${running} 运行`, `${running} running`));
    if (waiting) parts.push(L(`${waiting} 待确认`, `${waiting} waiting`));
    return { running, waiting, text: parts.join(" · ") };
  }

  function decorateDerivedOffice(chat, { ofDesk, derivedTitles }) {
    if (!chat) return chat;
    const parentGroup = chat.derived_origin?.parent_workspace_id || chat.parent_workspace_id || "";
    const deskKey = chat.derived_origin?.parent_desk_key || chat.parent_desk_key || "";
    const x = parentGroup && deskKey ? ofDesk(parentGroup, deskKey) : null;
    const cliTitle = x?.display_title || (deskKey && derivedTitles?.[deskKey]) || null;
    if (cliTitle && !chat.display_title) return { ...chat, display_title: cliTitle };
    return chat;
  }

  function openDerivedOffice(parentGroup, deskKey, fallbackGroup, { derivedOf, openDerived, ofDesk, isDerivedLive, setTerminal, setOpenOnce }) {
    if (parentGroup && deskKey) {
      const row = derivedOf(parentGroup).find((d) => d.desk_key === deskKey);
      if (row) return openDerived(row.group);
      const target = fallbackGroup || derivedRowId(parentGroup, deskKey);
      const live = isDerivedLive(ofDesk(parentGroup, deskKey));
      setTerminal(target, live);
      setOpenOnce(target);
      go(`/chat/${target}`);
      return true;
    }
    if (fallbackGroup) return openDerived(fallbackGroup);
    return false;
  }

  function resumeDerivedRow(row, x, { setOpenOnce, revive, prefix }) {
    const parentGroup = row?.parent_workspace_id || x?.parent_workspace_id || null;
    const deskKey = row?.desk_key || x?.desk_key || null;
    const target = row?.group || (parentGroup && deskKey ? derivedRowId(parentGroup, deskKey) : (x ? `${prefix}${x.id}` : null));
    if (!target) return;
    if (typeof state !== "undefined" && state?.loaded !== target) {
      setOpenOnce(target);
      if (state?.selected !== target) go(`/chat/${target}`);
    } else if (typeof state !== "undefined") {
      state.convOpen = true;
      if (!state.convPeek && typeof setCollapsed === "function") setCollapsed(false);
    }
    void revive({ id: x?.id, group: target, parentGroup, deskKey });
  }

  function resumeDerived({ group, deskKey, row = null, terminal = null }, { derivedOf, ofDesk, setOpenOnce, revive, prefix }) {
    const resolvedRow = row || (group && deskKey ? derivedOf(group).find((d) => d.desk_key === deskKey) : null) || {
      group: group && deskKey ? derivedRowId(group, deskKey) : null,
      parent_workspace_id: group,
      desk_key: deskKey,
    };
    const x = terminal || (group && deskKey ? ofDesk(group, deskKey) : null);
    resumeDerivedRow(resolvedRow, x, { setOpenOnce, revive, prefix });
  }

  function derivedMenuItems(id, { findDerivedRow, byId, isDerivedLive, openDerived, resumeRow, close }) {
    const derivedRow = findDerivedRow(id);
    if (!derivedRow) return null;
    const x = derivedRow.terminal != null ? byId(derivedRow.terminal) : null;
    const live = isDerivedLive(x);
    const childWsId = derivedRow.child_workspace_id;
    const childGroup = childWsId && typeof state !== "undefined" && Array.isArray(state?.groupsData)
      ? state.groupsData.find((g) => g.group === childWsId)
      : null;
    const manageItems = childGroup && typeof Actions !== "undefined"
      ? (Actions.workspaceManageItems?.(childWsId, childGroup) || [])
      : [];
    return [
      { label: L("打开", "Open"), run: () => openDerived(id) },
      !live && { label: L("续接", "Resume"), run: () => resumeRow(derivedRow, x) },
      x && { label: L("结束派生终端", "End derived terminal"), danger: true, run: () => close(x.id) },
      ...manageItems,
    ].filter(Boolean);
  }

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
      const selfActive = isActiveRank(m.ds, "");
      const active = selfActive || childStatus === "running" || childStatus === "review";
      const tone = rowTone(m.ds, childStatus);
      const statusWord = !selfActive && childStatus === "review"
        ? L("待审核", "Review")
        : !selfActive && childStatus === "running"
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
      live && x && { label: L("结束派生终端", "End derived terminal"), danger: true, run: () => ChatTerminal?.close?.(x.id) },
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

  return {
    encodeToken, decodeToken, derivedRowId, isDerivedRowId, parseDerivedRowId,
    derivedContextOf, mergeChildIntoDerivedRow, derivedChildrenOf, liveDerivedOwnerOf,
    derivedSummary, decorateDerivedOffice, openDerivedOffice, resumeDerivedRow, resumeDerived,
    derivedMenuItems,
    isOffice, hasDownstream, originOf, mood, sideItems, openSideItem, sideMenu, bossMood,
  };
})();
