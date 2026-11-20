"use strict";
// Terminals in the office (ADR-0103), laid out after herdr: a project's "+" starts a new agent, an
// installed CLI in the project's folder. Its CLI is a boss; once it dispatches, its workspace is
// listed as usual and the conversation column can show the terminal instead of the messages.
// Until then the agent is listed on its own, its terminal in that column.
const ChatTerminal = (() => {
  const { h, L } = AN;
  const PREFIX = "term-";
  // CLIs whose ended terminal can continue its chat (the App's cli_args).
  const RESUMES = new Set(["claude", "codex", "grok", "antigravity"]);
  const t = {
    list: [],
    clis: null,
    cwd: null,
    since: new Map(), // terminal id -> when the office first saw it, to order the list
    terminal: new Map(), // workspace -> whether its column shows the terminal
    frames: new Map(), // terminal id -> its frame, kept so the screen survives switching
    shown: null,
    openOnce: null,
  };
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
  const pendingId = (group) => (group?.startsWith(PREFIX) ? Number(group.slice(PREFIX.length)) : null);
  // The terminal whose CLI is currently in this workspace (the latest confirmed one).
  const ofWorkspace = (group) => t.list.find((x) => x.workspaces.at(-1) === group);

  async function refresh() {
    try {
      const current = await AN.load("/app/terminal/state");
      t.list = current.sessions;
      t.cwd = current.cwd;
    } catch {
      return; // keep what is shown
    }
    const now = new Date().toISOString();
    for (const x of t.list) if (!t.since.has(x.id)) t.since.set(x.id, now);
    for (const id of t.frames.keys()) if (!byId(id)) dropFrame(id);
  }

  // Agents that have never confirmed a workspace, shaped like workspaces so the list places them.
  function pending() {
    return t.list
      .filter((x) => !x.workspaces?.length)
      .map((x) => ({
        group: PREFIX + x.id,
        terminal: x.id,
        host: x.provider,
        app: x.provider,
        label: AN.cliName(x.provider),
        display_title: x.display_title ?? null,
        title: x.display_title || L("新 agent", "New agent"),
        status: x.exit == null ? "running" : "idle",
        live: x.exit == null,
        updated: t.since.get(x.id),
        project_key: x.project_key,
        project_label: x.project_label || folder(x.cwd),
        project_path: x.project_path || x.cwd,
        preview: x.exit == null ? folder(x.cwd) : L("已结束", "Exited"),
        color: 0,
        desks_count: 0,
        rooms_count: 0,
        isolated_count: 0,
      }));
  }

  // The office of an agent that has not dispatched: its boss and nothing else yet.
  function office(group) {
    const row = pending().find((g) => g.group === group);
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

  // An agent that dispatched opens as its workspace, showing its terminal as it did.
  function follow(selected) {
    const id = pendingId(selected);
    const x = id != null ? byId(id) : null;
    if (!x?.workspaces.length) return selected;
    const group = x.workspaces.at(-1);
    t.terminal.set(group, true);
    return group;
  }

  // ---------------------------------------------------------------- new agents
  async function clis() {
    if (!t.clis) t.clis = (await AN.load("/app/terminal/clis")).clis.map((c) => c.provider);
    return t.clis;
  }
  // Pick a CLI to start in `cwd`, or in a folder picked first when there is none.
  async function newAgent(e, cwd) {
    const position = { clientX: e.clientX, clientY: e.clientY, preventDefault() {}, stopPropagation() {} };
    e.preventDefault();
    e.stopPropagation();
    const found = await clis().catch(() => []);
    if (!found.length) {
      Actions.openMenu(position, [{ label: L("没有检测到已安装的 CLI", "No installed CLI found"), run() {} }]);
      return;
    }
    Actions.openMenu(
      position,
      found.map((provider) => ({
        label: h("span.ctx-cli", {}, AN.hostTile(provider), AN.cliName(provider)),
        run: () => start(provider, cwd),
      })),
    );
  }
  // A failed request says why, as the office's other actions do.
  async function reply(response) {
    const result = await response.json().catch(() => ({}));
    if (!response.ok) Actions.say(result.error || L("操作失败", "Failed"), true);
    return response.ok ? result : null;
  }
  async function start(provider, cwd) {
    try {
      if (!cwd) {
        const picked = await reply(await act("/app/terminal/pick"));
        cwd = picked?.cwd;
        if (!cwd) return;
      }
      const { cols, rows } = measureSize();
      const result = await reply(await act("/app/terminal/start", JSON.stringify({ provider, cwd, cols, rows, light: light() })));
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
      rec.add.addEventListener("keydown", (e) => (e.key === "Enter" || e.key === " ") && run(e));
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
    rec.term.hidden = !x || x.exit != null;
    rec.term.title = L("boss 在终端里运行", "boss runs in a terminal");
  }
  function menu(id, e) {
    const x = byId(pendingId(id));
    if (!x) return false;
    Actions.openMenu(e, [
      { label: L("打开", "Open"), run: () => go(`/chat/${id}`) },
      x.exit == null && { label: L("结束 CLI", "Stop CLI"), run: () => stop(x.id) },
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
  async function confirmStop(x) {
    const resumable = canResume(x);
    const running = hasRunningMembers(x);
    if (resumable && !running) return true;
    const reasons = [];
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
    const answer = await Actions.ask({
      title: L("结束终端？", "Stop terminal?"),
      body: reasons,
      buttons: [
        { label: L("取消", "Cancel"), focus: true },
        { label: L("结束终端", "Stop terminal"), danger: true, value: "stop" },
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
    if (x && !(await confirmStop(x))) return;
    await reply(await act(`/app/terminal/stop?session=${id}`, ""));
    await refresh();
    refreshOffice();
  }
  async function close(id) {
    if (!(await reply(await act(`/app/terminal/close?session=${id}`, "")))) return;
    const shown = pendingId(state.selected) === id;
    await refresh();
    // The agent closed here was only its terminal, so nothing is left to show.
    if (shown) return go("/chat/", true);
    refreshOffice();
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
  const box = h("div.dr-term");
  modes.append(toMessages, toTerminal);
  // Right of 「‹ 全部」, which comes and goes: the header is right-aligned, so these stay put.
  el.drAll.after(modes, stopButton);
  el.tl.after(box);
  const switchTo = (on) => () => {
    if (!state.loaded) return;
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
  stopButton.addEventListener("click", () => {
    const x = byId(t.owned);
    if (x) void (x.exit == null ? stop(x.id) : restart(x.id));
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
  // The office's terminal `x`, shown in place of the messages when `on`. The switch and the stop
  // button depend only on the terminal, never on which side is showing, so neither moves.
  function show(x, on) {
    const term = x && on ? x : null;
    el.drawer.classList.toggle("has-term", !!x);
    if (el.app?.dataset) el.app.dataset.hasTerm = x ? "1" : "0";
    el.drawer.classList.toggle("term-mode", !!term);
    modes.hidden = !x;
    // One place for the terminal's action: stop it while it runs, start it again once it ended.
    // Without a terminal here, a chat its CLI can continue offers to, in the same place.
    const ended = x?.exit != null;
    stopButton.hidden = !x && !t.resumable;
    stopButton.classList.toggle("start", !x || ended);
    setText(stopButton, !x || (ended && x.workspaces.length && RESUMES.has(x.provider)) ? L("续接", "Resume") : ended ? L("启动", "Start") : L("结束终端", "Stop terminal"));
    t.owned = x?.id ?? null;
    setAttr(toMessages, "aria-pressed", term ? "false" : "true");
    setAttr(toTerminal, "aria-pressed", term ? "true" : "false");
    for (const [id, f] of t.frames) f.hidden = !term || id !== term.id;
    if (!term) {
      t.shown = null;
      return;
    }
    frame(term.id);
    el.drIco.dataset.key = `term:${term.provider}`;
    el.drIco.replaceChildren(AN.hostTile(term.provider));
    setText(el.drTitle, "boss");
    const cli = AN.cliName(term.provider);
    setText(el.drSub, term.exit == null ? cli : L(`${cli} · 已结束`, `${cli} · Exited`));
    setAttr(el.drSub, "title", term.cwd || null);
    el.drAll.hidden = true;
    setAttr(el.drawer, "aria-label", L("boss 终端", "boss terminal"));
    if (t.shown !== term.id) {
      t.shown = term.id;
      focusFrame(term.id);
    }
  }
  function prepare(group) {
    const x = byId(pendingId(group)) || ofWorkspace(group);
    el.drawer?.classList?.toggle?.("has-term", !!x);
    if (el.app?.dataset) el.app.dataset.hasTerm = x ? "1" : "0";
  }
  // After the conversation column is drawn for `group`: its terminal, shown unless the messages
  // were asked for.
  function column(group, host) {
    const agent = byId(pendingId(group));
    const x = agent || ofWorkspace(group);
    t.resumable = !x && RESUMES.has(host) ? group : null;
    show(x, t.terminal.get(group) ?? true);
  }
  function consumeOpen(group) {
    if (t.openOnce !== group) return false;
    t.openOnce = null;
    return true;
  }
  const hasTerminal = (group) => Boolean(byId(pendingId(group)) || ofWorkspace(group));
  const displayState = (group) => {
    const x = byId(pendingId(group)) || ofWorkspace(group);
    return x ? [x.id, x.exit != null] : null;
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
  function readMessages() {
    if (state.loaded) t.terminal.set(state.loaded, false);
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
      const current = state.loaded && (pendingId(state.loaded) === x.id || ofWorkspace(state.loaded)?.id === x.id);
      if (current) show(x, t.terminal.get(state.loaded) ?? true);
    }
    void refresh().then(() => refreshOffice());
  });

  return { refresh, pending, follow, pendingId, headAction, decorate, menu, prepare, column, office, readMessages, newAgent, consumeOpen, hasTerminal, displayState, bossOpensTerminal, running, showing, exitedOf, openBoss, close, stop };
})();
