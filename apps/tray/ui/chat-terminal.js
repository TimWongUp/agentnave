"use strict";
// Terminals in the office (ADR-0103), laid out after herdr: a project's "+" starts a new agent, an
// installed CLI in the project's folder. Its CLI is a boss; once it dispatches, its workspace is
// listed as usual and the conversation column can show the terminal instead of the messages.
// Until then the agent is listed on its own, its terminal in that column.
const ChatTerminal = (() => {
  const { h, L } = AN;
  const PREFIX = "term-";
  const t = {
    list: [],
    clis: null,
    cwd: null,
    since: new Map(), // terminal id -> when the office first saw it, to order the list
    terminal: new Map(), // workspace -> whether its column shows the terminal
    frames: new Map(), // terminal id -> its frame, kept so the screen survives switching
    shown: null,
  };
  const act = (path, body = "{}") =>
    fetch(path, { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body });
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
        title: L("新 agent", "New agent"),
        status: x.exit == null && x.active ? "running" : "idle",
        live: x.exit == null && x.active,
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
      const result = await reply(await act("/app/terminal/start", JSON.stringify({ provider, cwd, cols: 100, rows: 30 })));
      if (!result) return;
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
    rec.add.hidden = state.groupBy !== "project" || !!rec.section?.pinned;
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
    if (g.terminal != null) setText(rec.meta, g.label);
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
  async function stop(id) {
    await reply(await act(`/app/terminal/stop?session=${id}`, ""));
    await refresh();
    refreshOffice();
  }
  async function close(id) {
    await reply(await act(`/app/terminal/close?session=${id}`, ""));
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
  const stopButton = h("button.dr-stop", { type: "button" }, L("结束", "Stop"));
  const box = h("div.dr-term");
  modes.append(toMessages, toTerminal);
  el.drAll.before(modes, stopButton);
  el.tl.after(box);
  const switchTo = (on) => () => {
    if (!state.loaded) return;
    t.terminal.set(state.loaded, on);
    column(state.loaded);
  };
  toMessages.addEventListener("click", switchTo(false));
  toTerminal.addEventListener("click", switchTo(true));
  stopButton.addEventListener("click", () => t.shown != null && stop(t.shown));

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
  // Shows terminal `x` in the column, or the messages when there is none.
  function show(x, { pendingOnly = false } = {}) {
    el.drawer.classList.toggle("term-mode", !!x);
    modes.hidden = pendingOnly || !x;
    stopButton.hidden = !x || x.exit != null;
    setAttr(toMessages, "aria-pressed", x ? "false" : "true");
    setAttr(toTerminal, "aria-pressed", x ? "true" : "false");
    for (const [id, f] of t.frames) f.hidden = !x || id !== x.id;
    if (!x) {
      t.shown = null;
      return;
    }
    const f = frame(x.id);
    el.drIco.dataset.key = `term:${x.provider}`;
    el.drIco.replaceChildren(AN.hostTile(x.provider));
    setText(el.drTitle, AN.cliName(x.provider));
    setText(el.drSub, x.exit == null ? folder(x.cwd) : L("已结束", "Exited"));
    el.drAll.hidden = true;
    if (t.shown !== x.id) {
      t.shown = x.id;
      const shown = () => f.contentWindow?.postMessage({ type: "agentnave:terminal", action: "shown" }, location.origin === "null" ? "*" : location.origin);
      if (f.dataset.loaded) requestAnimationFrame(shown);
      else f.addEventListener("load", () => { f.dataset.loaded = "1"; shown(); }, { once: true });
    }
  }
  // After the conversation column is drawn for `group`: the switch, and the terminal if chosen.
  function column(group) {
    const x = ofWorkspace(group);
    if (!x) {
      modes.hidden = true;
      show(null);
      modes.hidden = true;
      return;
    }
    show(t.terminal.get(group) ? x : null);
    modes.hidden = false;
  }
  // An agent that has not dispatched: the column holds its terminal, the floor waits for work.
  function showPending(group) {
    const x = byId(pendingId(group));
    if (!x) return false;
    el.drawer.hidden = false;
    el.drawer.classList.remove("folded");
    state.convCollapsed = false;
    el.convToggle.hidden = true;
    show(x, { pendingOnly: true });
    return true;
  }
  // Theme and language changes reach the frames as they reach settings.
  addEventListener("agentnave:preferences", (event) => {
    for (const f of t.frames.values())
      f.contentWindow?.postMessage({ type: "agentnave:terminal", action: "preferences", general: event.detail }, location.origin === "null" ? "*" : location.origin);
  });

  return { refresh, pending, follow, pendingId, headAction, decorate, menu, column, showPending, newAgent };
})();
