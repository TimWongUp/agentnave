"use strict";
// Terminals in the office (ADR-0103), laid out after herdr: a project's "+" starts a new agent, an
// installed CLI in the project's folder. Its CLI is a boss; once it dispatches, its workspace is
// listed as usual and the conversation column can show the terminal instead of the messages.
// Until then the agent is listed on its own, its terminal in that column.
const ChatTerminal = (() => {
  const { h, L } = AN;
  const PREFIX = "term-";
  const DERIVED_PREFIX = "drv-";
  const encodeToken = (s) =>
    encodeURIComponent(String(s)).replace(/[_!~*'().-]|%([0-9A-F]{2})/gi, (m, hex) =>
      `_${hex || m.charCodeAt(0).toString(16).padStart(2, "0")}`,
    );
  const derivedRowId = (parentGroup, deskKey) => `${DERIVED_PREFIX}${encodeToken(parentGroup)}-${encodeToken(deskKey)}`;
  // Where a derived row leads: its desk in the parent's office. A row without either keeps term-N.
  const rowPath = (g) =>
    g.derived === true && g.desk_key && g.parent_workspace_id
      ? `/chat/${encodeURIComponent(g.parent_workspace_id)}#desk:${encodeURIComponent(g.desk_key)}`
      : `/chat/${g.group}`;
  const isDerivedRowId = (id) => typeof id === "string" && id.startsWith(DERIVED_PREFIX);
  // CLIs whose ended terminal can continue its chat (the App's cli_args).
  const RESUMES = new Set(["claude", "codex", "grok", "antigravity"]);
  const t = {
    list: [],
    derivedTitles: {},
    listGen: 0,
    clis: null,
    cwd: null,
    since: new Map(), // terminal id -> when the office first saw it, to order the list
    terminal: new Map(), // workspace -> whether its column shows the terminal
    frames: new Map(), // terminal id -> its frame, kept so the screen survives switching
    waking: new Map(), // derived sessions being brought back: terminal id (or desk) -> { open: show the terminal when done }
    shown: null,
    openOnce: null,
    wantDeskTerminal: null,
    derivedRevive: null,
    away: false, // the window was hidden since the list was last read
  };
  AN.onVisibility?.(() => {
    if (AN.isHidden()) t.away = true;
  });
  const TERM_FONT = '13px "SF Mono", Menlo, Monaco, Consolas, "Noto Sans Mono CJK SC", monospace';
  const CELL_HEIGHT = Math.ceil(13 * 1.3);
  const PAD_X = 24;
  const PAD_Y = 24;
  const RULER_W = 14;
  let cachedCellWidth = 0;
  function cellWidth() {
    if (cachedCellWidth > 0) return cachedCellWidth;
    try {
      const ctx = typeof document !== "undefined" ? document.createElement?.("canvas")?.getContext?.("2d") : null;
      if (ctx) {
        ctx.font = TERM_FONT;
        const w = ctx.measureText("W").width;
        if (w > 0) return (cachedCellWidth = w);
      }
    } catch { /* canvas unavailable in non-DOM tests */ }
    return 7.8;
  }
  function storedNumber(key) {
    try { return Number(localStorage.getItem(key)) || 0; } catch { return 0; }
  }
  function measureSize() {
    if (typeof window === "undefined" || typeof document === "undefined") return { cols: 100, rows: 30 };
    const vw = window.innerWidth || document.documentElement?.clientWidth || 0;
    const vh = window.innerHeight || document.documentElement?.clientHeight || 0;
    let width = box?.clientWidth > 0 && el.drawer?.classList?.contains?.("has-term") ? box.clientWidth : 0;
    if (!width && vw > 0) {
      if (vw <= 760) width = vw;
      else if (vw <= 920) width = Math.min(760, vw - 64);
      else if (vw <= 1100) width = Math.min(660, vw - 64 - 360);
      else {
        const appW = el.app?.getBoundingClientRect?.().width || vw;
        const sideW = state.sideCollapsed ? 0 : (el.app?.querySelector?.(".side")?.getBoundingClientRect?.().width || 248);
        const share = storedNumber("agentnave.office.conv.term");
        const desired = share > 0 ? Math.max(300, share * appW) : 684;
        width = Math.min(desired, appW - sideW - 350);
      }
    }
    let height = box?.clientHeight > 0 ? box.clientHeight : 0;
    if (!height) {
      const appH = el.app?.clientHeight || (vh > 48 ? vh - 48 : 0);
      const headH = el.drawer?.querySelector?.(".conv-head")?.offsetHeight || 57;
      if (appH > headH) height = appH - headH;
    }
    if (width <= PAD_X + RULER_W || height <= PAD_Y) return { cols: 100, rows: 30 };
    const cols = Math.max(20, Math.min(500, Math.floor((width - PAD_X - RULER_W) / cellWidth())));
    const rows = Math.max(5, Math.min(200, Math.floor((height - PAD_Y) / CELL_HEIGHT)));
    return { cols, rows };
  }
  const act = (path, body = "{}") =>
    fetch(path, { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body });
  // Grok paints its own background, so it starts in the theme the App shows.
  const light = () => document.documentElement.dataset.theme === "light";
  const folder = (p) => (p || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p || "";
  const byId = (id) => t.list.find((x) => x.id === id);
  const pendingId = (group) => {
    if (typeof group !== "string" || !group.startsWith(PREFIX)) return null;
    const raw = group.slice(PREFIX.length);
    return /^\d+$/.test(raw) ? Number(raw) : null;
  };
  // The terminal whose CLI is currently in this workspace (the latest confirmed one).
  const ofWorkspace = (group) => t.list.find((x) => !x.derived && x.workspaces.at(-1) === group);
  // A derived terminal's desk in its parent's office is the thread of its native session.
  const ofDesk = (group, key) =>
    key?.startsWith("thread:")
      ? t.list.find(
          (x) =>
            x.derived &&
            x.parent_workspace_id === group &&
            ((x.desk_key && x.desk_key === key) || (x.native && `thread:${x.native}` === key)),
        )
      : undefined;
  const isDerivedDeskOpen = (group) =>
    typeof state !== "undefined" &&
    state?.drawer?.kind === "desk" &&
    (state?.chat?.group === group || state?.loaded === group) &&
    Boolean(state?.ctx?.desks?.get?.(state.drawer.key)?.derived || ofDesk(group, state.drawer.key));
  // The terminal this workspace's column shows: the open desk's own derived terminal, else the boss's.
  const terminalOf = (group) => {
    const pending = byId(pendingId(group));
    if (pending) return pending;
    if (typeof state !== "undefined" && state?.drawer?.kind === "desk") {
      const derived = ofDesk(group, state.drawer.key);
      if (derived) return derived;
      if (isDerivedDeskOpen(group)) return null;
    }
    return ofWorkspace(group);
  };

  function activeShownId() {
    const hidden = typeof AN.isHidden === "function" ? AN.isHidden() : Boolean(typeof document !== "undefined" && document.hidden);
    if (hidden) return null;
    if (t.shown == null) return null;
    if (!el.drawer?.classList?.contains?.("term-mode")) return null;
    if (el.drawer?.hidden) return null;
    if (el.drawer?.classList?.contains?.("folded") && typeof state !== "undefined" && !state?.convPeek) return null;
    const f = t.frames.get(t.shown);
    if (!f || f.hidden) return null;
    return t.shown;
  }

  // A derived terminal left on screen while the window was hidden continues by itself if its
  // process ended meanwhile; one the reader ended while looking stays ended.
  function continueShown(before) {
    const away = t.away;
    t.away = false;
    const x = away && before?.derived && derivedStateOf(before) !== "dormant" ? byId(before.id) : null;
    if (x && derivedStateOf(x) === "dormant") void revive({ id: x.id, group: state.loaded, deskKey: openDeskKey() });
  }
  async function refresh() {
    const gen = ++t.listGen;
    let current;
    const shownId = activeShownId();
    const url = shownId != null ? `/app/terminal/state?shown=${encodeURIComponent(shownId)}` : "/app/terminal/state";
    try {
      current = await AN.load(url);
    } catch {
      return; // keep what is shown
    }
    if (gen !== t.listGen) return;
    const before = shownId != null && byId(shownId);
    const processes = new Map(t.list.map((x) => [x.id, x.started]));
    t.list = current.sessions;
    t.derivedTitles = current.derived_titles || {};
    t.cwd = current.cwd;
    continueShown(before);
    const now = new Date().toISOString();
    for (const x of t.list) if (!t.since.has(x.id)) t.since.set(x.id, now);
    // a frame stops reading once its process has ended, so one started in the same row needs its own
    for (const id of t.frames.keys()) {
      const x = byId(id);
      if (!x || (processes.has(id) && processes.get(id) !== x.started)) dropFrame(id);
    }
  }

  // The CLI's name, with the named account it runs as after it.
  const cliLabel = (x) => AN.cliName(x.provider) + (x.account_label ? L(`（${x.account_label}）`, ` (${x.account_label})`) : "");

  const DERIVED_STATES = new Set(["starting", "running", "waiting", "uncollected", "idle", "dormant", "exited"]);
  function derivedStateOf(x) {
    if (!x) return "idle";
    if (x.exit != null) {
      if (x.derived_state === "dormant" || x.derived_state === "exited") return x.derived_state;
      return x.resumable ? "dormant" : "exited";
    }
    if (x.derived_state && DERIVED_STATES.has(x.derived_state)) return x.derived_state;
    if (x.blocked_kind) return "waiting";
    if (x.derived_busy) return "running";
    return "idle";
  }
  function formatElapsed(sinceSec) {
    if (typeof sinceSec !== "number" || !Number.isFinite(sinceSec) || sinceSec <= 0) return "";
    const sec = Math.max(0, Math.floor(Date.now() / 1000 - sinceSec));
    if (sec < 60) return `${sec}s`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m`;
    const hr = Math.floor(min / 60);
    return `${hr}h${String(min % 60).padStart(2, "0")}m`;
  }
  // "dormant" (its process ended after idling) is not a state to the reader: it reads as idle.
  function derivedMeta(g, { withElapsed = true } = {}) {
    if (g.waking || (g.terminal != null && t.waking.has(g.terminal))) {
      return { text: L("正在恢复", "Resuming"), tone: "run" };
    }
    const ds = g.derived_state || derivedStateOf(g);
    switch (ds) {
      case "starting":
        return { text: L("启动中", "Starting"), tone: "run" };
      case "running": {
        const elapsed = withElapsed ? formatElapsed(g.derived_since) : "";
        const base = L("运行中", "Running");
        return { text: elapsed ? `${base} · ${elapsed}` : base, tone: "run" };
      }
      case "waiting":
        return { text: L("等待确认", "Needs attention"), tone: "review" };
      case "uncollected":
        return { text: L("待 boss 收取", "Awaiting boss"), tone: "review" };
      case "exited":
        return { text: L("已结束", "Exited"), tone: "idle" };
      default:
        return { text: L("空闲", "Idle"), tone: "idle" };
    }
  }

  const deskDisplayName = (d) => (d ? (AN.deskName?.(d) || AN.handleOf?.(d) || d.role || d.model || "") : "");
  function derivedTitleFor(deskKey, cliTitle, desk) {
    return cliTitle || (deskKey && t.derivedTitles?.[deskKey]) || deskDisplayName(desk) || L("派生终端", "Derived terminal");
  }

  function toItemRow(x, opts = {}) {
    const derived = Boolean(x.derived);
    const ds = derived ? derivedStateOf(x) : null;
    const waking = derived && t.waking.has(x.id);
    const deskKey = x.desk_key ?? opts.desk?.key ?? null;
    const parentGroup = x.parent_workspace_id ?? null;
    const rowId = opts.rowId || (derived && deskKey && parentGroup ? derivedRowId(parentGroup, deskKey) : PREFIX + x.id);
    const status = derived
      ? (waking || ds === "starting" || ds === "running")
        ? "running"
        : (ds === "waiting" || ds === "uncollected")
          ? "waiting"
          : "idle"
      : x.exit == null
        ? "running"
        : "idle";
    const live = derived
      ? (waking || ds === "starting" || ds === "running" || ds === "waiting")
      : x.exit == null;
    const preview = derived
      ? derivedMeta({ ...x, derived_state: ds, waking }, { withElapsed: false }).text
      : x.exit == null
        ? folder(x.cwd)
        : L("已结束", "Exited");
    const displayTitle = x.display_title ?? (derived && deskKey ? (t.derivedTitles?.[deskKey] ?? null) : null);
    const title = derived
      ? derivedTitleFor(deskKey, x.display_title, opts.desk)
      : (x.display_title || L("新 agent", "New agent"));
    return {
      group: rowId,
      terminal: x.id,
      has_terminal: true,
      host: x.provider,
      app: x.provider,
      boss_art_model: x.art_model || "",
      label: cliLabel(x),
      display_title: displayTitle,
      title,
      status,
      live,
      created: t.since.get(x.id),
      updated: t.since.get(x.id),
      project_key: x.project_key,
      project_label: x.project_label || folder(x.cwd),
      project_path: x.project_path || x.cwd,
      preview,
      color: 0,
      desks_count: 0,
      rooms_count: 0,
      isolated_count: 0,
      derived,
      derived_state: ds,
      derived_since: x.derived_since ?? null,
      has_draft: Boolean(x.has_draft),
      resumable: Boolean(x.resumable),
      parent_terminal_id: x.parent_terminal_id ?? null,
      parent_workspace_id: parentGroup,
      desk_key: deskKey,
      native: x.native ?? null,
      exit: x.exit ?? null,
      pending_results: x.pending_results ?? 0,
      waking,
      isolation_id: x.isolation_id ?? null,
      cwd: x.cwd || "",
      blocked_kind: x.blocked_kind ?? null,
    };
  }

  function historyItemRow(parent, d) {
    const parentGroup = parent.group;
    const deskKey = d.key;
    const waking = t.waking.has(`${parentGroup}|${deskKey}`);
    const ds = "idle";
    const displayTitle = t.derivedTitles?.[deskKey] ?? null;
    return {
      group: derivedRowId(parentGroup, deskKey),
      terminal: null,
      has_terminal: false,
      host: d.provider,
      app: d.provider,
      boss_art_model: d.art_model || "",
      label: AN.cliName(d.provider),
      display_title: displayTitle,
      title: derivedTitleFor(deskKey, null, d),
      status: waking ? "running" : "idle",
      live: waking,
      created: d.first_seen || parent.created || parent.updated,
      updated: d.last_active || d.first_seen || parent.updated,
      project_key: parent.project_key,
      project_label: parent.project_label || "",
      project_path: parent.project_path || "",
      preview: derivedMeta({ derived_state: ds, waking }, { withElapsed: false }).text,
      color: 0,
      desks_count: 0,
      rooms_count: 0,
      isolated_count: 0,
      derived: true,
      derived_state: ds,
      derived_since: null,
      has_draft: false,
      resumable: false,
      parent_terminal_id: null,
      parent_workspace_id: parentGroup,
      desk_key: deskKey,
      native: null,
      exit: null,
      pending_results: 0,
      waking,
      isolation_id: null,
      cwd: "",
      blocked_kind: null,
    };
  }

  // Agents that have never confirmed a workspace, shaped like workspaces so the list places them.
  function pending() {
    return t.list
      .filter((x) => !x.derived && !x.workspaces?.length)
      .map((x) => toItemRow(x));
  }

  function matchesParent(x, parentGroup, tid) {
    return (
      x.derived &&
      (x.parent_workspace_id === parentGroup || (!x.parent_workspace_id && tid != null && x.parent_terminal_id === tid))
    );
  }

  function derivedOf(parentGroup) {
    if (!parentGroup) return [];
    const tid = pendingId(parentGroup);
    const parent = typeof state !== "undefined" && Array.isArray(state?.groupsData)
      ? state.groupsData.find((g) => g.group === parentGroup)
      : null;
    const history = Array.isArray(parent?.derived) ? parent.derived : [];
    const terms = t.list.filter((x) => matchesParent(x, parentGroup, tid));
    const used = new Set();
    const rows = [];
    const matchFor = (key) => {
      const idx = terms.findIndex(
        (x, i) => !used.has(i) && ((x.desk_key && x.desk_key === key) || (x.native && `thread:${x.native}` === key)),
      );
      if (idx < 0) return null;
      used.add(idx);
      return terms[idx];
    };
    for (const d of history) {
      const x = matchFor(d.key);
      rows.push(x ? toItemRow(x, { rowId: derivedRowId(parentGroup, d.key), desk: d }) : historyItemRow(parent, d));
    }
    terms.forEach((x, i) => {
      if (used.has(i)) return;
      const rowId = x.desk_key && x.parent_workspace_id ? derivedRowId(x.parent_workspace_id, x.desk_key) : PREFIX + x.id;
      rows.push(toItemRow(x, { rowId }));
    });
    return rows;
  }

  function derivedSummary(parentGroup) {
    const list = derivedOf(parentGroup);
    if (!list.length) return null;
    let running = 0;
    let waiting = 0;
    for (const item of list) {
      if (item.derived_state === "running" || item.derived_state === "starting") running += 1;
      else if (item.derived_state === "waiting") waiting += 1;
    }
    if (!running && !waiting) return null;
    const parts = [];
    if (running) parts.push(L(`${running} 运行`, `${running} running`));
    if (waiting) parts.push(L(`${waiting} 待确认`, `${waiting} waiting`));
    return { running, waiting, text: parts.join(" · ") };
  }

  function derivedAll() {
    const parents = new Set(
      typeof state !== "undefined" && Array.isArray(state?.groupsData)
        ? state.groupsData.map((g) => g.group)
        : [],
    );
    const orphans = [];
    for (const x of t.list) {
      if (!x.derived) continue;
      if (x.parent_workspace_id) parents.add(x.parent_workspace_id);
      else if (x.parent_terminal_id != null) parents.add(`${PREFIX}${x.parent_terminal_id}`);
      else orphans.push(toItemRow(x));
    }
    const all = [];
    for (const p of parents) all.push(...derivedOf(p));
    all.push(...orphans);
    return all;
  }

  function isDerivedLive(x) {
    if (!x?.derived) return false;
    const ds = derivedStateOf(x);
    return ds !== "dormant" && ds !== "exited";
  }

  function liveDerivedCount(workspace) {
    if (!workspace) return 0;
    const tid = pendingId(workspace);
    const resolved = tid != null ? (byId(tid)?.workspaces?.at(-1) || workspace) : workspace;
    const resolvedTid = pendingId(resolved);
    return t.list.filter((x) => matchesParent(x, resolved, resolvedTid) && isDerivedLive(x)).length;
  }

  // The office of an agent that has not dispatched (or a derived terminal): its boss and nothing else yet.
  function office(group) {
    const x = byId(pendingId(group));
    const row = pending().find((g) => g.group === group)
      || derivedAll().find((g) => g.group === group)
      || (x?.derived ? toItemRow(x, { rowId: group }) : null);
    if (!row) throw Object.assign(new Error("404"), { status: 404 });
    return {
      ...row,
      title: row.title,
      title_id: null,
      project: row.project_label,
      handover: null,
      conversations: [],
      members: [],
      desks: [],
      optimized: { count: 0 },
      rooms: [],
      messages: [],
      isolated: [],
    };
  }

  // An agent that dispatched opens as its workspace, showing its terminal as it did; a derived
  // terminal opened before its session had an id moves to its desk once the history has it.
  function follow(selected) {
    const id = pendingId(selected);
    const x = id != null ? byId(id) : null;
    if (x?.derived) {
      const parent = x.desk_key && state.groupsData?.find((g) => g.group === x.parent_workspace_id);
      if (!parent?.derived?.some((d) => d.key === x.desk_key)) return selected;
      if (t.terminal.get(selected) ?? true) t.wantDeskTerminal = { group: parent.group, deskKey: x.desk_key };
      state.hashKey = `desk:${x.desk_key}`;
      state.focused = false;
      return parent.group;
    }
    if (!x || !x.workspaces.length) return selected;
    const group = x.workspaces.at(-1);
    t.terminal.set(group, true);
    return group;
  }

  // ---------------------------------------------------------------- new agents
  async function clis() {
    if (!t.clis) t.clis = (await AN.load("/app/terminal/clis")).clis;
    return t.clis;
  }
  // Pick a CLI to start in `cwd`, or in a folder picked first when there is none.
  async function newAgent(e, cwd, workspace = null) {
    const position = { clientX: e.clientX, clientY: e.clientY, preventDefault() {}, stopPropagation() {} };
    e.preventDefault();
    e.stopPropagation();
    const found = (await clis().catch(() => [])).filter(({ provider }) => !workspace || ["claude", "codex"].includes(provider));
    if (!found.length) {
      Actions.openMenu(position, [{ label: L("没有检测到已安装的 CLI", "No installed CLI found"), run() {} }]);
      return;
    }
    // A CLI with named accounts offers each as its own entry after the default one.
    Actions.openMenu(
      position,
      found.flatMap(({ provider, accounts }) => [
        { label: h("span.ctx-cli", {}, AN.hostTile(provider), AN.cliName(provider)), run: () => workspace ? handover(workspace, provider) : start(provider, cwd) },
        ...(accounts || []).map((account) => ({
          label: h("span.ctx-cli", {}, AN.hostTile(provider), L(`${AN.cliName(provider)}（${account.label}）`, `${AN.cliName(provider)} (${account.label})`)),
          run: () => workspace ? handover(workspace, provider, account.id) : start(provider, cwd, account.id),
        })),
      ]),
    );
  }
  // A failed request says why, as the office's other actions do.
  async function reply(response) {
    const result = await response.json().catch(() => ({}));
    if (!response.ok) Actions.say(result.error || L("操作失败", "Failed"), true);
    return response.ok ? result : null;
  }
  async function start(provider, cwd, account) {
    try {
      if (!cwd) {
        const picked = await reply(await act("/app/terminal/pick"));
        cwd = picked?.cwd;
        if (!cwd) return;
      }
      const { cols, rows } = measureSize();
      const result = await reply(await act("/app/terminal/start", JSON.stringify({ provider, account, cwd, cols, rows, light: light() })));
      if (!result) return;
      t.openOnce = PREFIX + result.session.id;
      await refresh();
      go(`/chat/${PREFIX}${result.session.id}`);
    } catch (error) {
      Actions.say(String(error?.message || error), true);
    }
  }
  async function handover(workspace, provider, account) {
    try {
      const { cols, rows } = measureSize();
      const result = await reply(await act("/app/terminal/handover", JSON.stringify({ workspace, provider, account, cols, rows, light: light() })));
      if (!result) return;
      t.openOnce = PREFIX + result.session.id;
      await refresh();
      go(`/chat/${PREFIX}${result.session.id}`);
    } catch (error) {
      Actions.say(String(error?.message || error), true);
    }
  }

  // The "+" on a project's header in the list; "other" has no folder, so one is picked. A
  // project's key is its whole path; the path it shows may be shortened.
  function headAction(rec) {
    if (!rec.add) {
      rec.add = h("span.gl-grp-add", { role: "button", tabindex: "0" }, "+");
      const run = (e) => newAgent(e, rec.section?.other ? null : rec.section?.key);
      rec.add.addEventListener("click", run);
      rec.add.addEventListener("keydown", (e) => {
        const tag = String(e.target?.tagName || "").toUpperCase();
        if (e.target?.isContentEditable || tag === "TEXTAREA" || tag === "INPUT" || tag === "SELECT") return;
        if (e.key === "Enter" || e.key === " ") run(e);
      });
      rec.el.append(rec.add);
    }
    const label = L("在这个项目里新开 agent", "New agent in this project");
    rec.add.hidden = !!rec.section?.pinned;
    rec.add.title = label;
    rec.add.setAttribute("aria-label", label);
  }

  // ---------------------------------------------------------------- the list
  // A workspace whose boss runs in a terminal here says so; an agent's row has its own menu.
  function decorate(rec, g) {
    if (!rec.term) {
      rec.term = h("span.gi-term", { "aria-hidden": "true" }, ">_");
      rec.label.after(rec.term);
    }
    const x = g.terminal != null ? byId(g.terminal) : ofWorkspace(g.group);
    rec.term.hidden = !x || x.exit != null || g.derived === true;
    rec.term.title = L("boss 在终端里运行", "boss runs in a terminal");
  }
  function menu(id, e) {
    const derivedRow = isDerivedRowId(id)
      ? derivedAll().find((g) => g.group === id)
      : (() => {
          const x = byId(pendingId(id));
          return x?.derived ? toItemRow(x) : null;
        })();
    if (derivedRow) {
      const x = derivedRow.terminal != null ? byId(derivedRow.terminal) : null;
      const live = isDerivedLive(x);
      Actions.openMenu(e, [
        { label: L("打开", "Open"), run: () => openDerived(id) },
        !live && { label: L("续接", "Resume"), run: () => resumeDerivedRow(derivedRow, x) },
        x && { label: L("关闭终端", "Close terminal"), danger: true, run: () => close(x.id) },
      ].filter(Boolean));
      return true;
    }
    const x = byId(pendingId(id));
    if (!x) return false;
    const derivedCount = liveDerivedCount(id);
    Actions.openMenu(e, [
      { label: L("打开", "Open"), run: () => go(`/chat/${id}`) },
      x.exit == null && { label: L("结束 CLI", "Stop CLI"), run: () => stop(x.id) },
      derivedCount > 0 && {
        label: L(`关闭全部派生终端（${derivedCount}）`, `Close all derived terminals (${derivedCount})`),
        danger: true,
        run: () => closeDerived(id),
      },
      { label: L("关闭终端", "Close terminal"), danger: true, run: () => close(x.id) },
    ].filter(Boolean));
    return true;
  }
  const canResume = (x) => Boolean(x?.workspaces?.length && RESUMES.has(x.provider));
  function hasRunningMembers(x) {
    const ws = x?.workspaces?.at(-1);
    if (!ws) return false;
    if (state.chat?.group === ws && Array.isArray(state.chat.desks)) {
      if (state.chat.desks.some((d) => AN.activities?.(d).some((a) => a.status === "running"))) return true;
    }
    return state.groupsData?.find((g) => g.group === ws)?.status === "running";
  }
  function affectedDerivedOf(target) {
    if (Array.isArray(target?.derivedSessions)) return target.derivedSessions;
    if (target?.derived) return [target];
    if (!target) return [];
    const wsSet = new Set(target.workspaces || []);
    return t.list.filter((s) => {
      if (!s.derived) return false;
      if (s.parent_terminal_id != null) return s.parent_terminal_id === target.id;
      return Boolean(s.parent_workspace_id && wsSet.has(s.parent_workspace_id));
    });
  }
  function isDerivedActive(s) {
    const ds = derivedStateOf(s);
    return ds === "running" || ds === "starting" || ds === "waiting";
  }
  async function confirmStop(target, { mode = "stop" } = {}) {
    const isBatch = Array.isArray(target?.derivedSessions);
    const x = isBatch ? null : target;
    const derivedList = affectedDerivedOf(target);
    const activeDerived = derivedList.filter(isDerivedActive).length;
    const draftDerived = derivedList.filter((s) => Boolean(s.has_draft)).length;

    // Closing skips the question only for a terminal whose CLI has already exited; a live new agent
    // that has dispatched nothing cannot be reconnected afterwards.
    if (mode === "close" && x && !x.derived && x.exit != null && !activeDerived && !draftDerived) return true;

    const running = x ? hasRunningMembers(x) : false;
    const resumable = x && !x.derived && canResume(x);

    const reasons = [];
    if (x?.derived) {
      if (activeDerived) {
        reasons.push(h("p.act-note", {}, L(
          "这一轮还没做完，结束后会向 boss 回报未完成。",
          "This turn has not finished yet; stopping will report it to the boss as unfinished.",
        )));
      }
      if (draftDerived) {
        reasons.push(h("p.act-note.warn", {}, L(
          "1 个有未发出的草稿。",
          "1 derived session has an unsent draft.",
        )));
      }
      if (running) {
        reasons.push(h("p.act-note.warn", {}, L(
          "这个 agent 目前仍有组员在运行任务。",
          "This agent still has members running tasks.",
        )));
      }
    } else {
      if ((mode === "stop" || mode === "close") && x) {
        if (!x.workspaces?.length) {
          reasons.push(h("p.act-note", {}, L(
            "这个新 agent 还没有派过活，结束终端后不能在 App 里续接这段对话。",
            "This new agent has not dispatched any work yet; stopping the terminal means it cannot be resumed from the App.",
          )));
        } else if (!resumable) {
          reasons.push(h("p.act-note", {}, L(
            `App 暂不支持在终端里续接 ${AN.cliName(x.provider)} 的对话，结束后无法从这里接回。`,
            `The App cannot resume ${AN.cliName(x.provider)} conversations yet; stopping ends this terminal session.`,
          )));
        }
        if (running) {
          reasons.push(h("p.act-note.warn", {}, L(
            "这个 agent 目前仍有组员在运行任务。",
            "This agent still has members running tasks.",
          )));
        }
      }
      if (activeDerived > 0) {
        reasons.push(h("p.act-note.warn", {}, L(
          `有 ${activeDerived} 个派生会话还没做完，结束后会向 boss 回报未完成。`,
          `${activeDerived} derived session(s) have not finished yet; stopping will report them to the boss as unfinished.`,
        )));
      }
      if (draftDerived > 0) {
        reasons.push(h("p.act-note.warn", {}, L(
          `${draftDerived} 个有未发出的草稿。`,
          `${draftDerived} derived session(s) have unsent drafts.`,
        )));
      }
    }

    if (!reasons.length) return true;

    const title = isBatch
      ? L("关闭全部派生终端？", "Close all derived terminals?")
      : x?.derived
        ? L("关闭派生终端？", "Close derived terminal?")
        : (mode === "close" ? L("关闭终端？", "Close terminal?") : L("结束终端？", "Stop terminal?"));
    const confirmLabel = isBatch
      ? L("关闭全部", "Close all")
      : x?.derived
        ? L("关闭终端", "Close terminal")
        : (mode === "close" ? L("关闭终端", "Close terminal") : L("结束终端", "Stop terminal"));

    const answer = await Actions.ask({
      title,
      body: reasons,
      buttons: [
        { label: L("取消", "Cancel"), focus: true },
        { label: confirmLabel, danger: true, value: "stop" },
      ],
    });
    return answer?.value === "stop";
  }
  async function confirmElsewhere(elsewhere) {
    const appName = elsewhere.app || L("另一个程序", "another app");
    const statusWord = elsewhere.paused ? L("已暂停", "paused") : L("运行中", "running");
    const answer = await Actions.ask({
      title: L("这段聊天正在别处打开", "This chat is open elsewhere"),
      body: [
        h("p.act-note", {}, L(`这段聊天当前正在 ${appName} 中打开（${statusWord}）。`, `This chat is currently open in ${appName} (${statusWord}).`)),
        h("p.act-note.warn", {}, L("如果原处恢复或继续运行，两边同时操作会发生冲突。", "If the original session resumes or continues, both sides will conflict.")),
      ],
      buttons: [
        { label: L("取消", "Cancel") },
        { label: L("仍然续接", "Continue anyway"), primary: true, value: "confirm" },
      ],
    });
    return answer?.value === "confirm";
  }
  async function postAttach(basePath) {
    const { cols, rows } = measureSize();
    const url = `${basePath}&light=${Number(light())}&cols=${cols}&rows=${rows}`;
    let response = await act(url, "");
    if (response.status === 409) {
      const data = await response.json().catch(() => ({}));
      if (data.elsewhere) {
        if (!(await confirmElsewhere(data.elsewhere))) return null;
        response = await act(`${url}&confirm=1`, "");
      } else {
        Actions.say(data.error || L("操作失败", "Failed"), true);
        return null;
      }
    }
    return await reply(response);
  }
  async function stop(id) {
    const x = byId(id);
    if (x && !(await confirmStop(x, { mode: "stop" }))) return;
    await reply(await act(`/app/terminal/stop?session=${id}`, ""));
    await refresh();
    refreshOffice();
  }
  async function close(id) {
    const closing = byId(id);
    if (closing && !(await confirmStop(closing, { mode: "close" }))) return;
    if (!(await reply(await act(`/app/terminal/close?session=${id}`, "")))) return;
    const shown = pendingId(state.selected) === id;
    await refresh();
    // Derived terminals return to their parent agent when closed; standalone agents return to /chat/.
    if (shown) {
      const back = closing?.derived && closing.parent_workspace_id ? closing.parent_workspace_id : "";
      return go(back ? `/chat/${back}` : "/chat/", true);
    }
    refreshOffice();
  }
  async function closeDerived(workspace) {
    const tid = pendingId(workspace);
    const affected = t.list.filter(
      (s) =>
        s.derived &&
        (s.parent_workspace_id === workspace || (!s.parent_workspace_id && tid != null && s.parent_terminal_id === tid)),
    );
    if (!affected.length) return null;
    if (!(await confirmStop({ workspace, derivedSessions: affected }, { mode: "close-derived" }))) return null;
    const apiWs = tid != null ? (byId(tid)?.workspaces?.at(-1) || workspace) : workspace;
    const result = await reply(await act(`/app/terminal/close-derived?workspace=${encodeURIComponent(apiWs)}`, ""));
    if (!result) return null;
    const affectedIds = new Set(affected.map((s) => s.id));
    const selectedId = pendingId(state.selected);
    await refresh();
    if (selectedId != null && affectedIds.has(selectedId)) {
      return go(workspace.startsWith(PREFIX) ? "/chat/" : `/chat/${workspace}`, true);
    }
    refreshOffice();
    return result;
  }
  function redrawWaking() {
    if (typeof state !== "undefined") {
      state.listKey = null;
      state.chatKey = null;
    }
    if (typeof renderList === "function") renderList();
    if (typeof state !== "undefined" && state?.chat && typeof renderAll === "function") renderAll(state.chat);
  }
  const openDeskKey = () => (state?.drawer?.kind === "desk" ? state.drawer.key : null);
  // Whether a derived session is being brought back (ADR-0124): by terminal row, or by desk when it has none.
  const reviving = (group, deskKey = null) => {
    const x = deskKey ? ofDesk(group, deskKey) : byId(pendingId(group));
    return t.waking.has(x ? x.id : `${group}|${deskKey}`);
  };
  // Starts a derived session's process again from its native session: `wake` for a terminal row,
  // `open-derived` for a desk that has none. The messages stay up meanwhile; the terminal replaces
  // them only if the reader is still on this session and has not asked for the messages since.
  async function revive({ id, group, deskKey = null }) {
    const key = id ?? `${group}|${deskKey}`;
    if (t.waking.has(key)) {
      t.waking.get(key).open = true;
      return false;
    }
    const job = { open: true };
    t.waking.set(key, job);
    redrawWaking();
    const base = id != null
      ? `/app/terminal/wake?session=${id}`
      : `/app/terminal/open-derived?workspace=${encodeURIComponent(group)}&desk=${encodeURIComponent(deskKey)}`;
    let session = null;
    try {
      session = (await postAttach(base))?.session ?? null;
    } catch (error) {
      Actions.say(String(error?.message || error), true);
    }
    t.waking.delete(key);
    if (session) {
      const at = t.list.findIndex((s) => s.id === session.id);
      if (at >= 0) t.list[at] = session;
      else t.list.push(session);
      dropFrame(session.id);
      if (job.open && state.selected === group) {
        if (deskKey && state.loaded !== group && state.hashKey === `desk:${deskKey}`) {
          t.wantDeskTerminal = { group, deskKey };
        } else if (openDeskKey() === deskKey) {
          t.terminal.set(group, true);
          if (state.loaded !== group) t.openOnce = group;
        }
      }
    }
    refreshOffice();
    return Boolean(session);
  }
  // The reader asked for this desk's messages while it is being brought back: stay on them.
  function stayOnMessages(group, deskKey) {
    const job = t.waking.get(ofDesk(group, deskKey)?.id) || t.waking.get(`${group}|${deskKey}`);
    if (job) job.open = false;
    t.wantDeskTerminal = null;
  }
  // Clicking a derived row in the sidebar opens its desk's messages in the parent workspace
  // without starting a process; rows without a desk key or known parent fall back to term-N.
  function openDerived(id) {
    const row = derivedAll().find((g) => g.group === id)
      || (() => {
        const x = byId(pendingId(id));
        return x?.derived ? toItemRow(x) : null;
      })();
    if (!row?.derived) return false;
    if (row.desk_key && row.parent_workspace_id) {
      const parent = row.parent_workspace_id;
      stayOnMessages(parent, row.desk_key);
      if (
        typeof state !== "undefined" &&
        state?.loaded === parent &&
        state.ctx?.desks?.has?.(row.desk_key) &&
        typeof openScope === "function"
      ) {
        openScope({ kind: "desk", key: row.desk_key }, state.items?.get?.(id)?.el);
      } else {
        go(rowPath(row));
      }
      return true;
    }
    const termId = row.terminal ?? pendingId(id);
    const x = termId != null ? byId(termId) : null;
    if (!x?.derived) return false;
    const live = isDerivedLive(x);
    t.terminal.set(id, live);
    // The row's own link may have navigated already, so what counts is what is loaded.
    if (state?.loaded !== id) {
      t.openOnce = id;
      if (state?.selected !== id) go(`/chat/${id}`);
    } else {
      state.convOpen = true;
      if (!state.convPeek && typeof setCollapsed === "function") setCollapsed(false);
      refreshOffice();
      if (live) focusFrame(x.id);
    }
    return true;
  }
  function resumeDerivedRow(row, x) {
    if (row?.desk_key && row?.parent_workspace_id) {
      const parent = row.parent_workspace_id;
      const deskKey = row.desk_key;
      if (
        typeof state !== "undefined" &&
        state?.loaded === parent &&
        state.ctx?.desks?.has?.(deskKey) &&
        typeof openScope === "function"
      ) {
        openScope({ kind: "desk", key: deskKey }, state.items?.get?.(row.group)?.el);
      } else {
        t.wantDeskTerminal = null;
        go(rowPath(row));
      }
      void revive({ id: x?.id, group: parent, deskKey });
      return;
    }
    if (!x) return;
    const group = row?.group || `${PREFIX}${x.id}`;
    if (state?.loaded !== group) {
      t.openOnce = group;
      if (state?.selected !== group) go(`/chat/${group}`);
    } else {
      state.convOpen = true;
      if (!state.convPeek && typeof setCollapsed === "function") setCollapsed(false);
    }
    void revive({ id: x.id, group });
  }
  // Continues the chat an ended terminal was the boss of, or starts its CLI afresh, in its place.
  async function restart(id) {
    const result = await postAttach(`/app/terminal/restart?session=${id}`);
    if (!result) return;
    await refresh();
    if (state.loaded) t.terminal.set(state.loaded, true);
    if (pendingId(state.selected) === id) {
      t.openOnce = PREFIX + result.session.id;
      return go(`/chat/${PREFIX}${result.session.id}`);
    }
    refreshOffice();
  }
  // Continues a workspace's chat in a new terminal, in the folder it ran in.
  async function resume(group) {
    const result = await postAttach(`/app/terminal/resume?workspace=${encodeURIComponent(group)}`);
    if (!result) return;
    t.terminal.set(group, true);
    await refresh();
    refreshOffice();
  }
  function refreshOffice() {
    state.listKey = null;
    state.chatKey = null;
    window.refresh();
  }

  // ---------------------------------------------------------------- the column
  const modes = h("div.dr-mode", { role: "group" });
  const toMessages = h("button", { type: "button" }, L("消息", "Messages"));
  const toTerminal = h("button", { type: "button" }, L("终端", "Terminal"));
  const stopButton = h("button.dr-stop", { type: "button" }, L("结束终端", "Stop terminal"));
  // Beside a desk, a room or the isolated directories the controls still belong to the boss's terminal;
  // its mark sits before them so that is never in doubt.
  const bossTag = h("span.dr-boss", { role: "img", hidden: true });
  const box = h("div.dr-term");
  modes.append(toMessages, toTerminal);
  // Right of 「‹ 全部」, which comes and goes: the header is right-aligned, so these stay put.
  el.drAll.after(modes, stopButton);
  modes.before(bossTag);
  el.tl.after(box);
  const switchTo = (on) => () => {
    if (!state.loaded) return;
    // A derived session whose process is gone (or coming back) shows the terminal once it is up.
    const x = byId(t.owned);
    const job = x?.derived ? t.waking.get(x.id) : null;
    if (job) job.open = on;
    if (on && x?.derived && (job || derivedStateOf(x) === "dormant")) {
      if (!job) void revive({ id: x.id, group: state.loaded, deskKey: openDeskKey() });
      return;
    }
    t.terminal.set(state.loaded, on);
    if (state.chat && typeof renderAll === "function") {
      renderAll(state.chat);
      if (on && t.shown != null) focusFrame(t.shown);
    } else {
      column(state.loaded);
    }
  };
  toMessages.addEventListener("click", switchTo(false));
  toTerminal.addEventListener("click", switchTo(true));
  stopButton.addEventListener("click", (event) => {
    const x = byId(t.owned);
    if (t.derivedRevive) void revive(t.derivedRevive);
    else if (t.handover) void newAgent(event, null, t.handover);
    else if (x) void (x.exit == null ? stop(x.id) : restart(x.id));
    else if (t.resumable) void resume(t.resumable);
  });

  function frame(id) {
    let f = t.frames.get(id);
    if (!f) {
      f = h("iframe.dr-term-frame", { src: AN.href(`/terminal/?session=${id}`), title: L("终端", "Terminal") });
      t.frames.set(id, f);
      box.append(f);
    }
    return f;
  }
  function dropFrame(id) {
    t.frames.get(id)?.remove();
    t.frames.delete(id);
    if (t.shown === id) t.shown = null;
  }
  function focusFrame(id) {
    const f = t.frames.get(id);
    if (!f) return;
    const origin = location.origin === "null" ? "*" : location.origin;
    const notify = () => {
      f.focus?.({ preventScroll: true });
      f.contentWindow?.postMessage({ type: "agentnave:terminal", action: "shown" }, origin);
    };
    if (f.dataset.loaded) requestAnimationFrame(notify);
    else f.addEventListener("load", () => { f.dataset.loaded = "1"; notify(); }, { once: true });
  }
  // Terminal width stays fixed whenever the workspace has any terminal row (boss or derived, live
  // or exited) or any historical derived desk, so clicking desks or resuming never resizes the floor.
  function usesTermWidth(group) {
    if (!group) return false;
    if (byId(pendingId(group)) || ofWorkspace(group)) return true;
    if (derivedOf(group).length > 0) return true;
    if (typeof state !== "undefined" && state?.chat?.group === group && Array.isArray(state?.chat?.desks)) {
      return state.chat.desks.some((d) => d.derived);
    }
    return false;
  }
  function stateFor(group) {
    const x = terminalOf(group);
    const derivedDesk = isDerivedDeskOpen(group);
    const isDerived = Boolean(derivedDesk || x?.derived);
    const ds = x ? derivedStateOf(x) : "dormant";
    const live = Boolean(x && ds !== "dormant" && ds !== "exited");
    const on = t.terminal.get(group) ?? !derivedDesk;
    // Keep the screen and mode switch while an active viewer watches the terminal exit on its own.
    const watchingEnded = Boolean(isDerived && !live && x && t.shown === x.id && on);
    const term = x && on && (!isDerived || live || watchingEnded) ? x : null;
    return { x, derivedDesk, isDerived, ds, live, on, watchingEnded, term, hasWidth: usesTermWidth(group) };
  }
  // The stop button names whose terminal it acts on, and the boss's mark stands before the controls
  // whenever the drawer is about something else than the boss.
  function describeControls(x, isDerived, action) {
    const name = isDerived ? (x?.display_title || L("派生终端", "derived terminal")) : "boss";
    const who = x ? L(`${name}（${cliLabel(x)}）`, `${name} (${cliLabel(x)})`) : name;
    const label = action ? L(`${action[0]} ${who}的终端`, `${action[1]} ${who} terminal`) : stopButton.textContent;
    setAttr(stopButton, "title", label);
    setAttr(stopButton, "aria-label", label);
    const kind = typeof state !== "undefined" ? state?.drawer?.kind : null;
    const aside = !isDerived && !!x && ["desk", "room", "isolated"].includes(kind) && !(el.drawer?.classList?.contains?.("term-mode"));
    bossTag.hidden = !aside || (modes.hidden && stopButton.hidden);
    if (bossTag.hidden) return;
    const tip = L(`右侧的终端操作属于 ${who}`, `The terminal controls at right belong to ${who}`);
    setAttr(bossTag, "title", tip);
    setAttr(bossTag, "aria-label", tip);
    if (bossTag.dataset.key !== x.provider) {
      bossTag.dataset.key = x.provider;
      bossTag.replaceChildren(AN.hostTile(x.provider));
    }
  }
  // The office's terminal `x`, shown in place of the messages when `on`. The switch and the stop
  // button depend only on the terminal, never on which side is showing, so neither moves.
  function show(x, on, group = state?.loaded, view = null) {
    const wasTerm = Boolean(el.drawer?.classList?.contains?.("term-mode"));
    const isDerived = view ? view.isDerived : Boolean((group && isDerivedDeskOpen(group)) || x?.derived);
    const live = view ? view.live : Boolean(x && (!isDerived || isDerivedLive(x)));
    const watchingEnded = view ? view.watchingEnded : Boolean(isDerived && !live && x && t.shown === x.id && on);
    const term = view ? view.term : (x && on && (!isDerived || live || watchingEnded) ? x : null);
    const hasWidth = view ? view.hasWidth : (group ? usesTermWidth(group) : Boolean(x));
    el.drawer.classList.toggle("has-term", hasWidth);
    if (el.app?.dataset) el.app.dataset.hasTerm = hasWidth ? "1" : "0";
    el.drawer.classList.toggle("term-mode", !!term);
    modes.hidden = isDerived ? !(live || watchingEnded) : !x;
    // One place for the terminal's action: stop it while it runs, start it again once it ended.
    // Without a terminal here, a chat its CLI can continue offers to, in the same place.
    const ended = x?.exit != null;
    const deskKey = openDeskKey();
    const isReviving = Boolean(isDerived && group && reviving(group, deskKey));
    let action = null; // [中文, English] of what the button does to the terminal
    if (isDerived) {
      const canRevive = !live && Boolean(x || deskKey?.startsWith("thread:"));
      t.derivedRevive = canRevive ? { id: x?.id, group, deskKey } : null;
      stopButton.hidden = !canRevive;
      stopButton.disabled = !live && isReviving;
      stopButton.classList.toggle("start", !live);
      setText(stopButton, L("续接", "Resume"));
      action = ["续接", "Resume"];
    } else {
      t.derivedRevive = null;
      stopButton.hidden = Boolean(!x && !t.resumable && !t.handover);
      stopButton.disabled = false;
      stopButton.classList.toggle("start", !x || ended);
      const resuming = !x || (ended && x.workspaces.length && RESUMES.has(x.provider));
      const btnLabel = t.handover ? L("转交到终端", "Hand over to terminal") : resuming ? L("续接", "Resume") : ended ? L("启动", "Start") : L("结束终端", "Stop terminal");
      setText(stopButton, btnLabel);
      action = t.handover ? null : resuming ? ["续接", "Resume"] : ended ? ["启动", "Start"] : ["结束", "Stop"];
    }
    describeControls(x, isDerived, action);
    t.owned = x?.id ?? null;
    setAttr(toMessages, "aria-pressed", term ? "false" : "true");
    setAttr(toTerminal, "aria-pressed", term ? "true" : "false");
    for (const [id, f] of t.frames) f.hidden = !term || id !== term.id;
    if (!term) {
      t.shown = null;
      if (wasTerm) {
        if (typeof measureAll === "function") measureAll();
        else if (typeof applyFolds === "function" && typeof state !== "undefined" && state?.nodes)
          applyFolds([...state.nodes.values()].filter((rec) => rec.dirty));
      }
      return;
    }
    frame(term.id);
    el.drIco.dataset.key = `term:${term.provider}`;
    el.drIco.replaceChildren(AN.hostTile(term.provider));
    setText(el.drTitle, term.derived ? (term.display_title || L("派生终端", "Derived terminal")) : "boss");
    const cli = cliLabel(term);
    let subText;
    if (term.derived) {
      const termDs = derivedStateOf(term);
      if (isReviving || t.waking.has(term.id)) subText = L("正在恢复", "Resuming");
      else if (termDs === "waiting") subText = L(`${cli} · 等待确认`, `${cli} · Needs attention`);
      else if (termDs === "uncollected") subText = term.pending_results ? L(`${term.pending_results} 项结果待 boss 收取`, `${term.pending_results} results waiting for boss`) : L(`${cli} · 待 boss 收取`, `${cli} · Awaiting boss`);
      else if (termDs === "starting") subText = L(`${cli} · 启动中`, `${cli} · Starting`);
      else if (termDs === "exited" || termDs === "dormant") subText = L(`${cli} · 已结束`, `${cli} · Exited`);
      else subText = cli;
    } else {
      subText = term.blocked_kind
        ? L(`${cli} · 等待确认`, `${cli} · Needs attention`)
        : term.pending_results
          ? L(`${term.pending_results} 项结果待 boss 收取`, `${term.pending_results} results waiting for boss`)
          : term.exit == null
            ? cli
            : L(`${cli} · 已结束`, `${cli} · Exited`);
    }
    setText(el.drSub, subText);
    setAttr(el.drSub, "title", term.cwd || null);
    el.drAll.hidden = true;
    setAttr(el.drawer, "aria-label", term.derived ? L("派生终端", "derived terminal") : L("boss 终端", "boss terminal"));
    if (t.shown !== term.id) {
      t.shown = term.id;
      focusFrame(term.id);
    }
  }
  function prepare(group) {
    const view = stateFor(group);
    el.drawer?.classList?.toggle?.("has-term", view.hasWidth);
    if (el.app?.dataset) el.app.dataset.hasTerm = view.hasWidth ? "1" : "0";
    el.drawer?.classList?.toggle?.("term-mode", !!view.term);
  }
  // After the conversation column is drawn for `group`: its terminal, shown unless the messages
  // were asked for.
  function column(group, host, app) {
    const view = stateFor(group);
    t.handover = !view.isDerived && (!view.x || view.x.exit != null) && ["claude-desktop", "antigravity-desktop"].includes(app) ? group : null;
    t.resumable = !view.isDerived && !view.x && !t.handover && RESUMES.has(host) ? group : null;
    show(view.x, view.on, group, view);
    if (t.shown == null && reviving(group, openDeskKey())) setText(el.drSub, L("正在恢复", "Resuming"));
  }
  function consumeOpen(group) {
    if (t.openOnce !== group) return false;
    t.openOnce = null;
    return true;
  }
  function consumeOpenDesk(group, deskKey) {
    if (!t.wantDeskTerminal || t.wantDeskTerminal.group !== group || t.wantDeskTerminal.deskKey !== deskKey) return false;
    t.wantDeskTerminal = null;
    return true;
  }
  const hasTerminal = (group) => Boolean(byId(pendingId(group)) || ofWorkspace(group));
  const bossActivity = (group) => {
    const x = byId(pendingId(group)) || ofWorkspace(group);
    if (!x || x.exit != null || x.derived) return null;
    const a = x.activity;
    return a === "blocked" || a === "working" || a === "idle" ? a : "unknown";
  };
  const displayState = (group) => {
    const x = terminalOf(group);
    const derived = derivedOf(group).map((d) => [d.terminal, byId(d.terminal)?.started, d.derived_state, d.waking, d.resumable, d.pending_results, d.display_title]);
    return [x ? [x.id, x.started, x.exit != null, x.derived_state, x.activity, t.waking.has(x.id)] : null, bossActivity(group), derived, reviving(group, openDeskKey())];
  };
  const bossOpensTerminal = (group) => {
    const agent = byId(pendingId(group));
    const workspace = ofWorkspace(group);
    return Boolean(agent || (workspace && workspace.exit == null));
  };
  // The CLI in this workspace's terminal is still running.
  const running = (group) => {
    const x = byId(pendingId(group)) || ofWorkspace(group);
    return Boolean(x && x.exit == null);
  };
  const showing = () => Boolean(t.shown != null && el.drawer?.classList?.contains?.("term-mode"));
  const showingDerived = () => showing() && Boolean(byId(t.shown)?.derived);
  const exitedOf = (group) => {
    const x = ofWorkspace(group);
    return x && x.exit != null ? x : null;
  };
  function openBoss() {
    if (!state.loaded) return false;
    const x = byId(pendingId(state.loaded)) || ofWorkspace(state.loaded);
    if (!x || !bossOpensTerminal(state.loaded)) return false;
    const alreadyShown = t.shown === x.id;
    t.terminal.set(state.loaded, true);
    state.drawer = { kind: "all" };
    state.opener = el.boss;
    state.convOpen = true;
    if (!state.convPeek && typeof setCollapsed === "function") setCollapsed(false);
    if (state.chat && typeof renderAll === "function") renderAll(state.chat);
    else show(x, true);
    if (alreadyShown) focusFrame(x.id);
    return true;
  }
  // Opening a desk, a room or everything is asking to read: the column turns to the messages.
  function readMessages(on = false) {
    if (state.loaded) t.terminal.set(state.loaded, Boolean(on));
  }
  // Theme and language changes reach the frames as they reach settings.
  addEventListener("agentnave:preferences", (event) => {
    for (const f of t.frames.values())
      f.contentWindow?.postMessage({ type: "agentnave:terminal", action: "preferences", general: event.data || event.detail }, location.origin === "null" ? "*" : location.origin);
  });
  addEventListener("message", (event) => {
    const msg = event.data;
    if (!msg || msg.type !== "agentnave:terminal" || msg.action !== "exited") return;
    const x = byId(msg.session);
    if (x) {
      if (x.exit == null) x.exit = msg.code ?? 0;
      if (x.derived && x.derived_state !== "dormant" && x.derived_state !== "exited") {
        x.derived_state = x.resumable ? "dormant" : "exited";
      }
      const current = state.loaded && terminalOf(state.loaded)?.id === x.id;
      if (current) column(state.loaded, state.chat?.host, state.chat?.app);
    }
    void refresh().then(() => refreshOffice());
  });

  return { refresh, all: () => t.list, pending, derivedOf, derivedAll, liveDerivedCount, derivedStateOf, derivedMeta, derivedSummary, rowPath, isDerivedRowId, isWaking: (id) => t.waking.has(id), reviving, ofDesk, follow, pendingId, headAction, decorate, menu, prepare, column, office, readMessages, newAgent, clis, start, consumeOpen, consumeOpenDesk, hasTerminal, displayState, bossOpensTerminal, bossActivity, running, showing, showingDerived, exitedOf, openBoss, openDerived, stayOnMessages, revive, closeDerived, focusFrame, close, stop };
})();
