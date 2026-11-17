"use strict";
// The terminal page (ADR-0101), laid out like herdr: the terminals listed by folder on the left, split
// panes on the right (dockview). It works with the mouse first: click to focus, drag a tab to split
// or move it, drag a border to resize, right-click for actions. A pane closed from the layout keeps
// its CLI running; it stays in the list until it is closed there.
{
  const root = document.documentElement;
  // Before common.js runs, which leaves the bar to the office when the page is embedded.
  const embedded = window.parent !== window && new URLSearchParams(location.search).get("embedded") === "1";
  if (embedded) root.dataset.embedded = "true";

  const LAYOUT_KEY = "agentnave.terminal.layout";
  const state = { sessions: [], clis: [], cwd: null, loaded: false };
  const views = new Map(); // session id -> its TerminalView, while a pane shows it
  const tabs = new Map(); // session id -> its tab, to show the status
  let api, menu;

  const act = (path, body = "{}") => fetch(path, {
    method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body,
  });
  const panelId = id => `t${id}`;
  const byId = id => state.sessions.find(s => s.id === id);
  const folder = p => (p || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p || "";
  const status = s => !s ? "gone" : s.exit != null ? "exited" : s.active ? "working" : "idle";
  const label = s => ({ working: AN.L("工作中", "Working"), idle: AN.L("等你输入", "Your turn"), exited: AN.L("已结束", "Exited"), gone: "" })[status(s)];
  const tone = s => ({ working: "tone-run", idle: "tone-review" })[status(s)] || "tone-idle";

  // ---- the App's terminals ------------------------------------------------------------------
  async function poll() {
    const current = await AN.load("/app/terminal/state");
    state.sessions = current.sessions;
    state.cwd = current.cwd;
    if (!state.loaded) { state.loaded = true; restore(); }
    for (const panel of [...api.panels]) if (!byId(panel.params?.session)) panel.api.close();
    renderList();
    for (const [id, tab] of tabs) tab.show(byId(id));
    renderEmpty();
  }

  async function start(provider, position) {
    const response = await act("/app/terminal/start", JSON.stringify({ provider, cwd: state.cwd, cols: 100, rows: 30 }));
    const result = await response.json().catch(() => ({}));
    if (!response.ok) { console.warn(result.error); return; }
    await poll();
    show(result.session.id, position);
  }

  async function pickFolder() {
    const response = await act("/app/terminal/pick");
    if (response.ok) state.cwd = (await response.json()).cwd || state.cwd;
  }

  const stop = id => act(`/app/terminal/stop?session=${id}`, "").then(poll);
  const close = id => act(`/app/terminal/close?session=${id}`, "").then(poll);

  // Shows a terminal: brings its pane forward, or opens one where `position` says (a tab in the
  // active group by default).
  function show(id, position) {
    const s = byId(id);
    if (!s) return;
    const existing = api.getPanel(panelId(id));
    if (existing) { existing.api.setActive(); return; }
    api.addPanel({
      id: panelId(id), component: "terminal", tabComponent: "terminal",
      title: AN.cliName(s.provider), params: { session: id }, position,
    });
  }

  // ---- layout -------------------------------------------------------------------------------
  // The layout a viewer left is kept in this browser only; terminals that ended meanwhile drop out.
  function restore() {
    let saved = null;
    try { saved = JSON.parse(localStorage.getItem(LAYOUT_KEY) || "null"); } catch { /* none kept */ }
    if (saved) {
      try { api.fromJSON(saved); } catch { api.clear(); }
    }
    if (!api.panels.length) tile(state.sessions.filter(s => s.exit == null));
  }
  // Without a kept layout the running terminals are tiled: the first two side by side, the rest
  // stacked under the last one shown.
  function tile(sessions) {
    sessions.forEach((s, i) => {
      const last = api.panels.at(-1);
      show(s.id, last && { referencePanel: last.id, direction: i === 1 ? "right" : "below" });
    });
  }
  function save() {
    try { localStorage.setItem(LAYOUT_KEY, JSON.stringify(api.toJSON())); } catch { /* storage may be unavailable */ }
  }

  // A pane's content: the terminal itself, which answers a right-click with the pane's menu unless
  // text is selected (copy stays the browser's).
  function paneComponent() {
    const element = AN.h("div.tm-pane");
    let id = null;
    return {
      element,
      init(params) {
        id = params.params.session;
        const view = new TerminalView(id, () => poll().catch(() => {}));
        views.set(id, view);
        element.append(view.element);
        view.open();
        params.api.onDidActiveChange(event => { if (event.isActive) view.focus(); });
        params.api.onDidDimensionsChange(() => view.layout());
        element.addEventListener("contextmenu", event => {
          if (view.hasSelection()) return;
          event.preventDefault();
          openMenu(event.clientX, event.clientY, paneItems(id, event.clientX, event.clientY));
        });
      },
      dispose() { views.get(id)?.dispose(); views.delete(id); },
    };
  }

  // A tab: the CLI, its folder and a status dot; × takes the pane away and leaves the CLI running.
  function tabComponent() {
    const dot = AN.h("span.tm-dot");
    const tile = AN.h("span.tm-tile");
    const name = AN.h("span.tm-tab-name");
    const where = AN.h("span.tm-tab-where");
    const hide = AN.L("收起窗格（CLI 继续运行）", "Hide pane (the CLI keeps running)");
    const shut = AN.h("button.tm-tab-x", { type: "button", "aria-label": hide, title: hide }, "×");
    const element = AN.h("div.tm-tab", {}, dot, tile, name, where, shut);
    let id = null;
    const show = s => {
      dot.className = `tm-dot ${tone(s)}`;
      dot.title = label(s);
      if (!s) return;
      if (!tile.firstChild) tile.append(AN.hostTile(s.provider));
      name.textContent = AN.cliName(s.provider);
      where.textContent = folder(s.cwd);
      element.title = s.cwd;
    };
    return {
      element,
      init(params) {
        id = params.params.session;
        tabs.set(id, { show });
        show(byId(id));
        shut.addEventListener("pointerdown", event => event.stopPropagation());
        shut.addEventListener("click", event => { event.stopPropagation(); params.api.close(); });
        element.addEventListener("contextmenu", event => {
          event.preventDefault();
          openMenu(event.clientX, event.clientY, paneItems(id, event.clientX, event.clientY));
        });
      },
      dispose() { tabs.delete(id); },
    };
  }

  // The buttons at the right end of each group's tab strip: a new terminal here, or beside it.
  function groupActions(group) {
    const button = (glyph, title, run) => {
      const b = AN.h("button.tm-act", { type: "button", title, "aria-label": title }, glyph);
      b.addEventListener("click", run);
      return b;
    };
    const beside = direction => e => {
      const reference = group.activePanel ? { referencePanel: group.activePanel.id } : { referenceGroup: group };
      newMenu(e.clientX, e.clientY, { ...reference, direction });
    };
    const element = AN.h("div.tm-acts", {},
      button("+", AN.L("在这里新开终端", "New terminal here"), e => newMenu(e.clientX, e.clientY, { referenceGroup: group })),
      button("◫", AN.L("向右拆分", "Split right"), beside("right")),
      button("⊟", AN.L("向下拆分", "Split down"), beside("below")));
    return { element, init() {}, dispose() {} };
  }

  function mountDock() {
    const dockview = window["dockview-core"];
    api = dockview.createDockview(AN.$("tm-dock"), {
      theme: { ...dockview.themeAbyssSpaced, name: "agentnave", gap: 8 },
      disableFloatingGroups: true,
      createComponent: () => paneComponent(),
      createTabComponent: () => tabComponent(),
      createRightHeaderActionComponent: group => groupActions(group),
    });
    api.onDidLayoutChange(() => { save(); renderEmpty(); renderList(); });
    api.onDidActivePanelChange(() => renderList());
  }

  // ---- the list on the left -----------------------------------------------------------------
  function renderList() {
    const working = state.sessions.filter(s => status(s) === "working").length;
    const waiting = state.sessions.filter(s => status(s) === "idle").length;
    const parts = [working && AN.L(`${working} 个在工作`, `${working} working`), waiting && AN.L(`${waiting} 个等你`, `${waiting} waiting`)];
    AN.$("tm-count").textContent = parts.filter(Boolean).join(" · ");
    const groups = new Map();
    for (const s of state.sessions) {
      if (!groups.has(s.cwd)) groups.set(s.cwd, []);
      groups.get(s.cwd).push(s);
    }
    const active = api.activePanel?.params?.session;
    AN.$("tm-list").replaceChildren(...[...groups].map(([cwd, sessions]) => AN.h("section.tm-group", {},
      AN.h("h2.tm-group-h", { title: cwd }, folderIcon(), AN.h("span", {}, folder(cwd))),
      ...sessions.map(s => row(s, s.id === active)))));
  }

  function row(s, active) {
    const shown = Boolean(api.getPanel(panelId(s.id)));
    const element = AN.h(`button.tm-row${active ? ".sel" : ""}${shown ? "" : ".off"}`, { type: "button", title: s.cwd },
      AN.h("span.tm-tile", {}, AN.hostTile(s.provider)),
      AN.h("span.tm-row-name", {}, AN.cliName(s.provider)),
      AN.h(`span.pill.${tone(s)}`, {}, label(s)));
    element.addEventListener("click", () => show(s.id));
    element.addEventListener("contextmenu", event => {
      event.preventDefault();
      openMenu(event.clientX, event.clientY, [
        { label: AN.L("显示", "Show"), run: () => show(s.id) },
        ...paneItems(s.id, event.clientX, event.clientY),
      ]);
    });
    return element;
  }

  // A launcher where the panes would be, when none is open.
  function renderEmpty() {
    const empty = AN.$("tm-empty");
    empty.hidden = api.panels.length > 0;
    if (empty.hidden) return;
    const launch = state.clis.map(p => {
      const b = AN.h("button.tm-launch", { type: "button" }, AN.hostTile(p), AN.h("span", {}, AN.cliName(p)));
      b.addEventListener("click", () => start(p));
      return b;
    });
    const where = AN.h("button.tm-where", { type: "button", title: state.cwd || "" }, folderIcon(), folder(state.cwd));
    where.addEventListener("click", () => pickFolder().then(renderEmpty));
    empty.replaceChildren(AN.h("div.tm-launches", {}, ...launch), where);
  }

  // ---- menus --------------------------------------------------------------------------------
  function closeMenu() { menu.hidden = true; menu.replaceChildren(); }
  function openMenu(x, y, items) {
    menu.replaceChildren(...items.map(item => {
      if (item.sep) return AN.h("div.tm-sep");
      const b = AN.h(`button.tm-item${item.danger ? ".danger" : ""}`, { type: "button", role: "menuitem" },
        item.icon || null, AN.h("span", {}, item.label), item.note ? AN.h("small", {}, item.note) : null);
      b.addEventListener("click", () => { closeMenu(); item.run(); });
      return b;
    }));
    menu.hidden = false;
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(6, Math.min(x, innerWidth - width - 6))}px`;
    menu.style.top = `${Math.max(6, Math.min(y, innerHeight - height - 6))}px`;
  }

  // Pick a CLI to start in the folder shown at the bottom; that row changes the folder first.
  function newMenu(x, y, position) {
    const clis = state.clis.map(p => ({ icon: AN.hostTile(p), label: AN.cliName(p), run: () => start(p, position) }));
    openMenu(x, y, [
      ...(clis.length ? clis : [{ label: AN.L("没有检测到已安装的 CLI", "No installed CLI found"), run: () => {} }]),
      { sep: true },
      { icon: folderIcon(), label: folder(state.cwd), note: AN.L("更换…", "Change…"), run: () => pickFolder().then(() => newMenu(x, y, position)) },
    ]);
  }

  function paneItems(id, x, y) {
    const s = byId(id);
    const beside = direction => () => newMenu(x, y, api.getPanel(panelId(id)) ? { referencePanel: panelId(id), direction } : undefined);
    return [
      { label: AN.L("向右拆分新终端…", "New terminal on the right…"), run: beside("right") },
      { label: AN.L("向下拆分新终端…", "New terminal below…"), run: beside("below") },
      { sep: true },
      ...(s && s.exit == null ? [{ label: AN.L("结束 CLI", "Stop CLI"), run: () => stop(id) }] : []),
      { label: AN.L("关闭终端", "Close terminal"), danger: true, run: () => close(id) },
    ];
  }

  function folderIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 20 20");
    svg.setAttribute("class", "tm-ico");
    const path = document.createElementNS(svg.namespaceURI, "path");
    path.setAttribute("d", "M2.5 6.5a2 2 0 0 1 2-2h3.2l1.8 2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-11a2 2 0 0 1-2-2z");
    svg.append(path);
    return svg;
  }

  // ---- page ---------------------------------------------------------------------------------
  function themed() {
    root.style.setProperty("--term-bg", TerminalView.background());
    for (const view of views.values()) view.restyle();
  }

  function listenToOffice() {
    if (!embedded) return;
    const origin = location.origin === "null" ? "*" : location.origin;
    window.addEventListener("message", event => {
      if (event.source !== window.parent || event.data?.type !== "agentnave:terminal") return;
      if (event.data.action === "preferences") {
        window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: event.data.general }));
      } else if (event.data.action === "shown") {
        for (const view of views.values()) view.layout();
        const active = api.activePanel?.params?.session;
        if (active != null) views.get(active)?.focus();
        poll().catch(() => {});
      }
    });
    window.parent.postMessage({ type: "agentnave:terminal", action: "ready" }, origin);
  }

  document.addEventListener("DOMContentLoaded", () => {
    menu = AN.h("div.tm-menu", { role: "menu", hidden: true });
    document.body.append(menu);
    document.addEventListener("pointerdown", e => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); }, true);
    window.addEventListener("blur", closeMenu);
    themed();
    new MutationObserver(themed).observe(root, { attributes: true, attributeFilter: ["data-theme"] });
    mountDock();
    AN.$("tm-new").addEventListener("click", e => {
      const r = e.currentTarget.getBoundingClientRect();
      newMenu(r.left, r.bottom + 4);
    });
    listenToOffice();
    AN.load("/app/terminal/clis").then(r => { state.clis = r.clis.map(c => c.provider); renderEmpty(); }).catch(() => {});
    poll().catch(() => renderEmpty());
    setInterval(() => { if (!document.hidden) poll().catch(() => {}); }, 1500);
  });
}
