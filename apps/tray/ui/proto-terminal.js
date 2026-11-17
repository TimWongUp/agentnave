"use strict";
// PROTOTYPE — throwaway, lives on the prototype branch only. Three ways to talk to CLIs from the
// office, switched with /chat/?variant=A|B|C or the bar at the bottom (← → also switch):
//   A  desks in the office, the terminal docked in the right column
//   B  a tiled terminal wall in place of the floor (herdr style splits)
//   C  floating terminal windows over the floor, launched and parked in a dock
// Everything is mouse first: click to focus, drag to move or resize, right-click for actions.
{
  const { h, L } = AN;
  const VARIANTS = {
    A: L("工位 + 右侧栏", "Desks + right column"),
    B: L("终端墙（分屏）", "Terminal wall (splits)"),
    C: L("浮动窗口 + 程序坞", "Floating windows + dock"),
  };
  const KEYS = Object.keys(VARIANTS);
  const params = new URLSearchParams(location.search);
  const variant = KEYS.includes(params.get("variant")) ? params.get("variant") : "A";
  const origin = location.origin === "null" ? "*" : location.origin;
  document.documentElement.dataset.tpVariant = variant;

  // ---- terminals ------------------------------------------------------------------------
  const T = { sessions: [], clis: [], cwd: null, listeners: new Set() };
  const act = (path, body = "{}") => fetch(path, {
    method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body,
  });
  const changed = () => T.listeners.forEach(f => f());
  // The static preview (preview.html?mock) has no App behind it: show three made-up terminals.
  const inApp = location.protocol === "agentnave:" || location.hostname === "agentnave.localhost";
  async function poll() {
    if (!inApp) {
      T.clis = ["claude", "codex", "grok", "antigravity", "deepseek"];
      T.cwd = "/Users/me/Code/agentnave";
      T.sessions = [
        { id: 1, provider: "claude", cwd: "/Users/me/Code/agentnave", exit: null, active: true },
        { id: 2, provider: "codex", cwd: "/Users/me/Code/magpie", exit: null, active: false },
        { id: 3, provider: "grok", cwd: "/Users/me/Code/agentnave", exit: 0, active: false },
      ];
      return changed();
    }
    try {
      const state = await AN.load("/app/terminal/state");
      T.sessions = state.sessions;
      T.cwd = state.cwd;
      changed();
    } catch { /* the App may not have a terminal yet */ }
  }
  setInterval(() => { if (!document.hidden) poll(); }, 1500);
  if (inApp) AN.load("/app/terminal/clis").then(r => { T.clis = r.clis.map(c => c.provider); changed(); }).catch(() => {});
  const byId = id => T.sessions.find(s => s.id === id);
  const folder = p => (p || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p || "";
  const status = s => !s ? "gone" : s.exit != null ? "exited" : s.active ? "working" : "idle";
  const STATUS = { working: L("工作中", "Working"), idle: L("等你输入", "Your turn"), exited: L("已结束", "Exited"), gone: "" };
  const TONE = { working: "tone-run", idle: "tone-review", exited: "tone-idle", gone: "tone-idle" };
  const MOOD = { working: "working", idle: "idle", exited: "sleep", gone: "sleep" };
  const pill = s => h(`span.pill.${TONE[status(s)]}`, {}, STATUS[status(s)]);

  async function start(provider) {
    const response = await act("/app/terminal/start", JSON.stringify({ provider, cwd: T.cwd, cols: 100, rows: 30 }));
    const result = await response.json().catch(() => ({}));
    await poll();
    return result.session?.id ?? null;
  }
  const pickFolder = () => act("/app/terminal/pick").then(r => r.ok && r.json()).then(r => { if (r?.cwd) T.cwd = r.cwd; changed(); });
  const stop = id => act(`/app/terminal/stop?session=${id}`, "").then(poll);
  const close = id => act(`/app/terminal/close?session=${id}`, "").then(poll);

  // ---- the office's own context menu look --------------------------------------------------
  const menu = h("div.ctx.pt-ctx", { role: "menu", hidden: true });
  document.body.append(menu);
  const closeMenu = () => { menu.hidden = true; menu.replaceChildren(); };
  function openMenu(x, y, items) {
    menu.replaceChildren(...items.map(item => {
      if (item.sep) return h("div.pt-sep");
      const b = h(`button.ctx-item.pt-item${item.danger ? ".danger" : ""}`, { type: "button", role: "menuitem" },
        item.icon || null, h("span", {}, item.label), item.note ? h("small", {}, item.note) : null);
      b.addEventListener("click", () => { closeMenu(); item.run(); });
      return b;
    }));
    menu.hidden = false;
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(6, Math.min(x, innerWidth - width - 6))}px`;
    menu.style.top = `${Math.max(6, Math.min(y, innerHeight - height - 6))}px`;
  }
  document.addEventListener("pointerdown", e => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); }, true);
  window.addEventListener("blur", closeMenu);
  // Pick a CLI to start in the current folder; the folder row changes it first.
  function newMenu(x, y, then) {
    const clis = T.clis.map(p => ({ icon: AN.hostTile(p), label: AN.cliName(p), run: async () => then(await start(p)) }));
    openMenu(x, y, [
      ...(clis.length ? clis : [{ label: L("没有检测到已安装的 CLI", "No installed CLI found"), run: () => {} }]),
      { sep: true },
      { icon: folderIcon(), label: folder(T.cwd), note: L("更换…", "Change…"), run: () => pickFolder().then(() => newMenu(x, y, then)) },
    ]);
  }
  function terminalMenu(x, y, id, extra = []) {
    const s = byId(id);
    openMenu(x, y, [
      ...extra,
      ...(s && s.exit == null ? [{ label: L("结束 CLI", "Stop CLI"), run: () => stop(id) }] : []),
      { label: L("关闭终端", "Close terminal"), danger: true, run: () => close(id).then(() => panes.closed?.(id)) },
    ]);
  }
  function folderIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 20 20");
    svg.classList.add("pt-ico");
    const path = document.createElementNS(svg.namespaceURI, "path");
    path.setAttribute("d", "M2.5 6.5a2 2 0 0 1 2-2h3.2l1.8 2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-11a2 2 0 0 1-2-2z");
    svg.append(path);
    return svg;
  }

  // ---- panes: one terminal page per iframe, reporting clicks and right-clicks ------------------
  const panes = { focus: null, menu: null, closed: null };
  const frame = id => h("iframe.pt-frame", { src: inApp ? `/terminal/?pane=${id}` : "about:blank", title: L("终端", "Terminal") });
  const tell = (iframe, data) => iframe?.contentWindow?.postMessage({ type: "agentnave:pane", ...data }, origin);
  window.addEventListener("message", e => {
    if (e.data?.type !== "agentnave:pane") return;
    const iframe = [...document.querySelectorAll("iframe.pt-frame")].find(f => f.contentWindow === e.source);
    if (!iframe) return;
    if (e.data.action === "focus") panes.focus?.(e.data.pane);
    if (e.data.action === "menu") {
      const r = iframe.getBoundingClientRect();
      panes.menu?.(e.data.pane, r.left + e.data.x, r.top + e.data.y);
    }
  });
  window.addEventListener("agentnave:preferences", e => {
    for (const f of document.querySelectorAll("iframe.pt-frame")) tell(f, { action: "preferences", general: e.detail });
  });
  // While a border or window is dragged the iframes must not swallow the pointer.
  const dragging = on => document.documentElement.classList.toggle("pt-dragging", on);
  function drag(e, move, end) {
    e.preventDefault();
    dragging(true);
    const up = () => { dragging(false); removeEventListener("pointermove", move); removeEventListener("pointerup", up); end?.(); };
    addEventListener("pointermove", move);
    addEventListener("pointerup", up);
  }

  // A desk like the office's, with the CLI's character at it.
  function deskScene(provider) {
    const deskImg = h("img.l-desk", { src: AN.art.url("office/desk.webp"), alt: "", draggable: "false" });
    const lid = h("span.lid");
    lid.replaceChildren(AN.art.lid(provider));
    const scene = h("div.scene", {},
      h("img.l-chair", { src: AN.art.url("office/chair.webp"), alt: "", draggable: "false" }),
      deskImg, window.Laptop ? Laptop.create(lid) : null);
    scene.insertBefore(AN.art.figure(AN.art.character(provider)), deskImg);
    return scene;
  }

  // ---- A: desks + right column -------------------------------------------------------------
  function mountA() {
    const zone = h("section.zone.wide.desks-zone.pt-zone", { "aria-label": L("我的终端", "My terminals") });
    const count = h("span.zone-count");
    const grid = h("div.desks.pt-desks");
    zone.append(h("h2.zone-h", {}, L("我的终端", "My terminals"), count), grid);
    const keep = () => {
      const floor = document.getElementById("floor");
      if (floor && zone.parentNode !== floor) floor.insertBefore(zone, document.getElementById("rooms"));
    };
    new MutationObserver(keep).observe(document.getElementById("floor"), { childList: true });
    keep();
    const recs = new Map();
    let open = null, wide = false;
    const add = h("button.desk.pt-add", { type: "button" }, h("span.pt-plus", {}, "+"), h("div.label", {}, h("div.nm", {}, L("新终端", "New terminal"))));
    add.addEventListener("click", e => newMenu(e.clientX, e.clientY, id => id && show(id)));
    // The panel covers the right column; its own border is the column's, so it drags the same way.
    const panel = h("div.pt-panel", { hidden: true });
    const head = h("header.conv-head.pt-panel-head");
    const body = h("div.pt-panel-body");
    panel.append(head, body);
    document.getElementById("drawer").append(panel);
    function show(id) {
      open = id;
      if (typeof setCollapsed === "function") setCollapsed(false);
      if (typeof state === "object") { state.convOpen = true; if (state.chat && typeof renderAll === "function") renderAll(state.chat); }
      body.replaceChildren(frame(id));
      panel.hidden = false;
      render();
    }
    function hide() { open = null; panel.hidden = true; body.replaceChildren(); widen(false); render(); }
    function widen(on) {
      wide = on;
      const app = document.getElementById("app");
      if (on) app.style.setProperty("--conv-col", "min(62vw, 1040px)");
      else app.style.removeProperty("--conv-col");
    }
    function renderHead() {
      const s = byId(open);
      if (!s) return;
      const button = (label, title, run, cls = "") => {
        const b = h(`button.pt-hbtn${cls}`, { type: "button", title, "aria-label": title }, label);
        b.addEventListener("click", run);
        return b;
      };
      head.replaceChildren(
        h("span.pt-tile", {}, AN.hostTile(s.provider)),
        h("div.dr-id", {}, h("b.dr-title", {}, AN.cliName(s.provider)), h("span.dr-sub", {}, folder(s.cwd))),
        pill(s),
        button(wide ? "⇥" : "⇤", wide ? L("还原宽度", "Restore width") : L("加宽", "Widen"), () => { widen(!wide); renderHead(); }),
        button("×", L("收起终端", "Hide terminal"), hide, ".pt-x"),
      );
      head.oncontextmenu = e => { e.preventDefault(); terminalMenu(e.clientX, e.clientY, open); };
    }
    function render() {
      count.textContent = T.sessions.length ? ` · ${T.sessions.length}` : "";
      for (const [id, rec] of recs) if (!byId(id)) { rec.el.remove(); recs.delete(id); }
      for (const s of T.sessions) {
        let rec = recs.get(s.id);
        if (!rec) {
          rec = { say: h("span.say"), tags: h("span.desk-tags"), nm: h("div.nm"), cli: h("div.cli") };
          rec.el = h("button.desk.pt-desk", { type: "button" }, rec.tags, rec.say, deskScene(s.provider), h("div.label", {}, rec.nm, rec.cli));
          rec.el.addEventListener("click", () => show(s.id));
          rec.el.addEventListener("contextmenu", e => { e.preventDefault(); terminalMenu(e.clientX, e.clientY, s.id, [{ label: L("打开", "Open"), run: () => show(s.id) }]); });
          recs.set(s.id, rec);
        }
        const st = status(s);
        rec.el.dataset.mood = MOOD[st];
        rec.el.classList.toggle("sel", open === s.id);
        rec.say.replaceChildren(st === "working" ? h("span.dots", {}, h("i"), h("i"), h("i")) : st === "idle" ? L("等你", "You") : "");
        rec.tags.replaceChildren(pill(s));
        rec.nm.textContent = AN.cliName(s.provider);
        rec.cli.textContent = folder(s.cwd);
        grid.append(rec.el);
      }
      grid.append(add);
      if (open && !byId(open)) hide();
      renderHead();
    }
    panes.focus = () => {};
    panes.menu = (id, x, y) => terminalMenu(x, y, id);
    panes.closed = id => { if (open === id) hide(); };
    T.listeners.add(render);
    render();
  }

  // ---- B: a tiled wall in place of the floor -------------------------------------------------
  function mountB() {
    const pane = document.getElementById("pane");
    const wall = h("div.pt-wall", { hidden: true });
    const tabs = h("div.pt-tabs");
    const tiles = h("div.pt-tiles");
    wall.append(tabs, tiles);
    pane.insertBefore(wall, document.getElementById("ticker"));
    let on = false, layout = null, focused = null, seeded = false;
    // The switch between floor and wall sits at the floor head's right end.
    const seg = h("div.pt-seg", { role: "group" });
    const floorBtn = h("button", { type: "button" }, L("楼层", "Floor"));
    const wallBtn = h("button", { type: "button" });
    seg.append(floorBtn, wallBtn);
    floorBtn.addEventListener("click", () => setOn(false));
    wallBtn.addEventListener("click", () => setOn(true));
    const head = document.getElementById("head");
    const keep = () => { if (seg.parentNode !== head) head.append(seg); };
    new MutationObserver(keep).observe(head, { childList: true });
    keep();
    function setOn(value) {
      on = value;
      pane.classList.toggle("pt-wall-on", on);
      wall.hidden = !on;
      renderAll();
    }
    // The layout is a tree: a pane, or a split of two subtrees with the first one's share.
    const ids = node => !node ? [] : node.id != null ? [node.id] : [...ids(node.a), ...ids(node.b)];
    function place(id, dir = "row", beside = focused) {
      if (ids(layout).includes(id)) return;
      if (!layout) layout = { id };
      else {
        const replace = node => {
          if (node.id != null) return node.id === beside ? { dir, ratio: 0.5, a: node, b: { id } } : node;
          return { ...node, a: replace(node.a), b: replace(node.b) };
        };
        const target = ids(layout).includes(beside) ? beside : ids(layout).at(-1);
        beside = target;
        layout = replace(layout);
      }
      focused = id;
    }
    function remove(id) {
      const drop = node => {
        if (!node || node.id === id) return null;
        if (node.id != null) return node;
        const a = drop(node.a), b = drop(node.b);
        return a && b ? { ...node, a, b } : a || b;
      };
      layout = drop(layout);
      if (focused === id) focused = ids(layout)[0] ?? null;
    }
    const frames = new Map();
    function tile(id) {
      const s = byId(id);
      const iframe = frames.get(id) || frame(id);
      frames.set(id, iframe);
      const button = (label, title, run) => {
        const b = h("button.pt-hbtn", { type: "button", title, "aria-label": title }, label);
        b.addEventListener("click", e => { e.stopPropagation(); run(e); });
        return b;
      };
      const split = dir => e => newMenu(e.clientX, e.clientY, nid => { if (nid) { place(nid, dir, id); renderAll(); } });
      const bar = h("div.pt-tile-h", {},
        h("span.pt-tile", {}, AN.hostTile(s?.provider)),
        h("b", {}, AN.cliName(s?.provider)), h("span.pt-dim", {}, folder(s?.cwd)), pill(s), h("span.pt-grow"),
        button("◫", L("向右拆分", "Split right"), split("row")),
        button("⊟", L("向下拆分", "Split down"), split("column")),
        button("×", L("关闭窗格", "Close pane"), () => { remove(id); renderAll(); }));
      const card = h("div.pt-card", { "data-id": id }, bar, h("div.pt-card-body", {}, iframe));
      card.classList.toggle("sel", focused === id);
      bar.addEventListener("pointerdown", () => focus(id));
      bar.addEventListener("contextmenu", e => { e.preventDefault(); paneMenu(id, e.clientX, e.clientY); });
      return card;
    }
    function build(node) {
      if (node.id != null) return tile(node.id);
      const box = h(`div.pt-split.pt-${node.dir}`);
      const a = build(node.a), b = build(node.b);
      a.style.flex = `${node.ratio} 1 0`;
      b.style.flex = `${1 - node.ratio} 1 0`;
      const gutter = h(`div.pt-gutter.pt-gutter-${node.dir}`);
      gutter.addEventListener("pointerdown", e => drag(e, m => {
        const r = box.getBoundingClientRect();
        const share = node.dir === "row" ? (m.clientX - r.left) / r.width : (m.clientY - r.top) / r.height;
        node.ratio = Math.min(0.85, Math.max(0.15, share));
        a.style.flex = `${node.ratio} 1 0`;
        b.style.flex = `${1 - node.ratio} 1 0`;
      }));
      box.append(a, gutter, b);
      return box;
    }
    function paneMenu(id, x, y) {
      const split = dir => () => newMenu(x, y, nid => { if (nid) { place(nid, dir, id); renderAll(); } });
      terminalMenu(x, y, id, [
        { label: L("向右拆分…", "Split right…"), run: split("row") },
        { label: L("向下拆分…", "Split down…"), run: split("column") },
        { label: L("移出终端墙", "Remove from wall"), run: () => { remove(id); renderAll(); } },
        { sep: true },
      ]);
    }
    function focus(id) {
      focused = id;
      for (const card of tiles.querySelectorAll(".pt-card")) card.classList.toggle("sel", Number(card.dataset.id) === id);
      for (const tab of tabs.children) tab.classList.toggle("sel", Number(tab.dataset.id) === id);
      tell(frames.get(id), { action: "focus" });
    }
    function empty() {
      const box = h("div.pt-empty");
      const row = h("div.pt-launch");
      for (const p of T.clis) {
        const b = h("button.pt-launch-btn", { type: "button" }, AN.hostTile(p), h("span", {}, AN.cliName(p)));
        b.addEventListener("click", async () => { const id = await start(p); if (id) { place(id); renderAll(); } });
        row.append(b);
      }
      const where = h("button.pt-where", { type: "button" }, folderIcon(), folder(T.cwd));
      where.addEventListener("click", () => pickFolder().then(renderAll));
      box.append(h("div.pt-empty-h", {}, L("开一个终端", "Open a terminal")), row, where);
      return box;
    }
    let shape = "";
    function renderTabs() {
      tabs.replaceChildren(...T.sessions.map(s => {
        const tab = h("button.pt-tab", { type: "button", "data-id": s.id }, h(`span.pt-dot.${TONE[status(s)]}`), AN.hostTile(s.provider),
          h("span", {}, `${AN.cliName(s.provider)} · ${folder(s.cwd)}`));
        tab.classList.toggle("sel", focused === s.id);
        tab.classList.toggle("off", !ids(layout).includes(s.id));
        tab.addEventListener("click", () => { if (!ids(layout).includes(s.id)) { place(s.id); renderAll(); } focus(s.id); });
        tab.addEventListener("contextmenu", e => { e.preventDefault(); paneMenu(s.id, e.clientX, e.clientY); });
        return tab;
      }));
      const plus = h("button.pt-tab.pt-tab-add", { type: "button", title: L("新终端", "New terminal") }, "+");
      plus.addEventListener("click", e => newMenu(e.clientX, e.clientY, id => { if (id) { place(id); renderAll(); } }));
      tabs.append(plus);
    }
    function renderAll() {
      wallBtn.textContent = T.sessions.length ? `${L("终端墙", "Wall")} · ${T.sessions.length}` : L("终端墙", "Wall");
      floorBtn.setAttribute("aria-pressed", String(!on));
      wallBtn.setAttribute("aria-pressed", String(on));
      for (const id of ids(layout)) if (!byId(id)) { remove(id); frames.delete(id); }
      // The wall starts with every open terminal on it.
      if (!seeded && T.sessions.length) { seeded = true; for (const s of T.sessions) place(s.id, ids(layout).length % 2 ? "row" : "column"); }
      renderTabs();
      if (!on) return;
      // Rebuild the tiles only when their arrangement changed; a rebuild reloads the panes.
      const next = JSON.stringify(layout, (k, v) => (k === "ratio" ? undefined : v)) + T.clis.join();
      if (next === shape && tiles.firstChild) {
        for (const card of tiles.querySelectorAll(".pt-card")) {
          const s = byId(Number(card.dataset.id));
          card.querySelector(".pill")?.replaceWith(pill(s));
        }
        return;
      }
      shape = next;
      tiles.replaceChildren(layout ? build(layout) : empty());
    }
    panes.focus = id => focus(id);
    panes.menu = (id, x, y) => paneMenu(id, x, y);
    panes.closed = id => { remove(id); renderAll(); };
    T.listeners.add(renderAll);
    renderAll();
    if (params.has("wall")) setOn(true);
  }

  // ---- C: floating windows over the floor, with a dock ---------------------------------------
  function mountC() {
    const pane = document.getElementById("pane");
    const desk = h("div.pt-desktop");
    const dock = h("div.pt-dock");
    pane.append(desk, dock);
    const wins = new Map();
    let top = 10;
    function raise(win) { win.el.style.zIndex = String(++top); for (const w of wins.values()) w.el.classList.toggle("sel", w === win); }
    function openWin(id) {
      let win = wins.get(id);
      if (win) { win.el.hidden = false; win.min = false; raise(win); tell(win.iframe, { action: "focus" }); renderDock(); return; }
      const n = wins.size, box = pane.getBoundingClientRect();
      const width = Math.min(640, box.width * 0.74), height = Math.min(420, box.height * 0.58);
      win = { id, iframe: frame(id), min: false, max: false };
      win.title = h("div.pt-win-title");
      const button = (label, title, cls, run) => {
        const b = h(`button.pt-light.${cls}`, { type: "button", title, "aria-label": title }, label);
        b.addEventListener("pointerdown", e => e.stopPropagation());
        b.addEventListener("click", run);
        return b;
      };
      const head = h("div.pt-win-h", {},
        h("span.pt-lights", {},
          button("×", L("关闭", "Close"), "x", () => { win.el.remove(); wins.delete(id); renderDock(); }),
          button("–", L("收进程序坞", "Minimize to dock"), "min", () => { win.min = true; win.el.hidden = true; renderDock(); }),
          button("+", L("铺满楼层", "Fill the floor"), "max", () => maximize(win))),
        win.title);
      const grip = h("div.pt-grip");
      win.el = h("div.pt-win", {}, head, h("div.pt-win-body", {}, win.iframe), grip);
      Object.assign(win.el.style, { left: `${30 + n * 28}px`, top: `${70 + n * 28}px`, width: `${width}px`, height: `${height}px` });
      head.addEventListener("pointerdown", e => {
        raise(win);
        if (win.max) return;
        const x0 = e.clientX - win.el.offsetLeft, y0 = e.clientY - win.el.offsetTop;
        drag(e, m => {
          win.el.style.left = `${Math.max(-200, m.clientX - x0)}px`;
          win.el.style.top = `${Math.max(0, m.clientY - y0)}px`;
        });
      });
      head.addEventListener("dblclick", () => maximize(win));
      head.addEventListener("contextmenu", e => { e.preventDefault(); winMenu(id, e.clientX, e.clientY); });
      grip.addEventListener("pointerdown", e => {
        raise(win);
        const w0 = win.el.offsetWidth, h0 = win.el.offsetHeight, x0 = e.clientX, y0 = e.clientY;
        drag(e, m => {
          win.el.style.width = `${Math.max(320, w0 + m.clientX - x0)}px`;
          win.el.style.height = `${Math.max(200, h0 + m.clientY - y0)}px`;
        });
      });
      wins.set(id, win);
      desk.append(win.el);
      raise(win);
      renderWins();
      renderDock();
    }
    function maximize(win) {
      win.max = !win.max;
      win.el.classList.toggle("max", win.max);
      raise(win);
    }
    function winMenu(id, x, y) {
      const win = wins.get(id);
      terminalMenu(x, y, id, [
        { label: win?.max ? L("还原大小", "Restore size") : L("铺满楼层", "Fill the floor"), run: () => win && maximize(win) },
        { label: L("收进程序坞", "Minimize to dock"), run: () => { if (win) { win.min = true; win.el.hidden = true; renderDock(); } } },
        { sep: true },
      ]);
    }
    function renderWins() {
      for (const [id, win] of wins) {
        const s = byId(id);
        if (!s) { win.el.remove(); wins.delete(id); continue; }
        win.title.replaceChildren(h("span.pt-tile", {}, AN.hostTile(s.provider)), h("b", {}, AN.cliName(s.provider)),
          h("span.pt-dim", {}, folder(s.cwd)), pill(s));
      }
    }
    function renderDock() {
      const launch = T.clis.map(p => {
        const b = h("button.pt-dock-app", { type: "button", title: `${L("新开", "New")} ${AN.cliName(p)}` }, AN.hostTile(p));
        b.addEventListener("click", async () => { const id = await start(p); if (id) openWin(id); });
        return b;
      });
      const running = T.sessions.map(s => {
        const win = wins.get(s.id);
        const b = h("button.pt-dock-run", { type: "button", title: `${AN.cliName(s.provider)} · ${folder(s.cwd)} · ${STATUS[status(s)]}` },
          AN.hostTile(s.provider), h(`span.pt-dot.${TONE[status(s)]}`), h("span.pt-dock-name", {}, folder(s.cwd)));
        b.classList.toggle("min", !win || win.min);
        b.addEventListener("click", () => openWin(s.id));
        b.addEventListener("contextmenu", e => { e.preventDefault(); winMenu(s.id, e.clientX, e.clientY); });
        return b;
      });
      const where = h("button.pt-dock-where", { type: "button", title: T.cwd || "" }, folderIcon(), h("span", {}, folder(T.cwd)));
      where.addEventListener("click", () => pickFolder());
      dock.replaceChildren(...launch, h("span.pt-dock-sep"), ...(running.length ? running : [h("span.pt-dim.pt-dock-none", {}, L("点左边的图标开一个终端", "Click an icon to open a terminal"))]), h("span.pt-dock-sep"), where);
    }
    panes.focus = id => { const win = wins.get(id); if (win) raise(win); };
    panes.menu = (id, x, y) => winMenu(id, x, y);
    panes.closed = id => { const win = wins.get(id); if (win) { win.el.remove(); wins.delete(id); } renderDock(); };
    let seeded = false;
    T.listeners.add(() => {
      // Terminals still running come back as windows when the office opens.
      if (!seeded && T.sessions.length) { seeded = true; for (const s of T.sessions) if (s.exit == null) openWin(s.id); }
      renderWins();
      renderDock();
    });
    renderDock();
  }

  // ---- the prototype's switcher --------------------------------------------------------------
  function switcher() {
    const go = step => {
      const next = KEYS[(KEYS.indexOf(variant) + step + KEYS.length) % KEYS.length];
      const url = new URL(location.href);
      url.searchParams.set("variant", next);
      location.replace(url.href);
    };
    const prev = h("button", { type: "button", "aria-label": "previous" }, "‹");
    const next = h("button", { type: "button", "aria-label": "next" }, "›");
    prev.addEventListener("click", () => go(-1));
    next.addEventListener("click", () => go(1));
    document.body.append(h("div.pt-switch", {}, prev, h("span", {}, h("b", {}, variant), ` — ${VARIANTS[variant]}`), next));
    document.addEventListener("keydown", e => {
      if (e.target.closest?.("input, textarea, [contenteditable]")) return;
      if (e.key === "ArrowLeft") go(-1);
      if (e.key === "ArrowRight") go(1);
    });
  }

  switcher();
  ({ A: mountA, B: mountB, C: mountC })[variant]();
  poll();
}
