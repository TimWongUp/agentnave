"use strict";
// The embedded terminal (ADR-0101): an installed CLI the user starts in a folder they picked,
// shown with xterm.js. Output comes by long polling /app/terminal/output; keys, size changes and
// stop are action POSTs. The CLI is the user's own process and keeps running when the page goes.
{
  const root = document.documentElement;
  const params = new URLSearchParams(location.search);
  const embedded = window.parent !== window && params.get("embedded") === "1";
  // A pane shows one terminal and nothing else; the office around it draws the rest.
  const pane = window.parent !== window ? Number(params.get("pane")) || null : null;
  // Before common.js runs, which leaves the bar to the office when the page is embedded.
  if (embedded || pane) root.dataset.embedded = "true";
  if (pane) root.dataset.pane = "true";

  const FONT = '"SF Mono", Menlo, Monaco, Consolas, "Noto Sans Mono CJK SC", monospace';
  const THEMES = {
    dark: {
      background: "#141517", foreground: "#e6e6e8", cursor: "#f5f5f7", cursorAccent: "#141517",
      selectionBackground: "#ffffff38", scrollbarSliderBackground: "#ffffff26",
      scrollbarSliderHoverBackground: "#ffffff40", scrollbarSliderActiveBackground: "#ffffff55",
      black: "#2a2c30", red: "#ff7d6e", green: "#7fca90", yellow: "#f0b45a", blue: "#6ea8ff",
      magenta: "#b9a4f5", cyan: "#4fd6c9", white: "#d1d1d6",
      brightBlack: "#6b7079", brightRed: "#ff9d91", brightGreen: "#9be0aa", brightYellow: "#f7c97f",
      brightBlue: "#93bfff", brightMagenta: "#cfbfff", brightCyan: "#7fe6dc", brightWhite: "#f5f5f7",
    },
    light: {
      background: "#fbfbfd", foreground: "#1d1d1f", cursor: "#1d1d1f", cursorAccent: "#fbfbfd",
      selectionBackground: "#0a64e033", scrollbarSliderBackground: "#00000022",
      scrollbarSliderHoverBackground: "#00000033", scrollbarSliderActiveBackground: "#00000044",
      black: "#1d1d1f", red: "#b83d31", green: "#2f8a49", yellow: "#a06512", blue: "#2f6fe0",
      magenta: "#7a54e0", cyan: "#087c73", white: "#6e6e73",
      brightBlack: "#8a8f98", brightRed: "#d6584a", brightGreen: "#3fa35b", brightYellow: "#c4862a",
      brightBlue: "#4f8cf0", brightMagenta: "#9673ee", brightCyan: "#1d9a90", brightWhite: "#3a3a3c",
    },
  };

  const state = { clis: [], provider: null, cwd: null, session: null, ended: null, running: false, offset: 0, pumping: false };
  let term, fit, sending = Promise.resolve();

  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const act = (path, body, type = "application/json") => fetch(path, {
    method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": type }, body,
  });

  function themed() {
    const theme = THEMES[root.dataset.theme === "light" ? "light" : "dark"];
    root.style.setProperty("--term-bg", theme.background);
    if (term) term.options.theme = theme;
  }

  function folderLabel(path) {
    if (!path) return AN.L("选择文件夹…", "Choose Folder…");
    return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
  }

  function render() {
    if (pane) return;
    const { $, L } = AN;
    for (const chip of $("term-clis").children) {
      const chosen = chip.dataset.provider === state.provider;
      chip.setAttribute("aria-checked", String(chosen));
      chip.disabled = state.running;
    }
    $("term-folder-name").textContent = folderLabel(state.cwd);
    $("term-folder").title = state.cwd || "";
    $("term-folder").disabled = state.running;
    const run = $("term-run");
    run.dataset.state = state.running ? "running" : "idle";
    run.textContent = state.running ? L("结束", "Stop") : L("启动", "Start");
    run.disabled = !state.running && !(state.provider && state.cwd);
  }

  function buildClis() {
    const box = AN.$("term-clis");
    box.replaceChildren(...state.clis.map(provider => {
      const chip = AN.h("button.term-cli", { type: "button", role: "radio", "data-provider": provider },
        AN.hostTile(provider), AN.cliName(provider));
      chip.addEventListener("click", () => { state.provider = provider; render(); term?.focus(); });
      return chip;
    }));
  }

  async function refresh() {
    const current = await AN.load("/app/terminal/state");
    const session = pane ? current.sessions.find(s => s.id === pane) : current.sessions.at(-1);
    if (pane) { attach(session); return; }
    const clis = await AN.load("/app/terminal/clis");
    state.clis = clis.clis.map(cli => cli.provider);
    state.cwd = session?.cwd || current.cwd || null;
    if (session) state.provider = session.provider;
    else if (!state.clis.includes(state.provider)) state.provider = state.clis[0] || null;
    buildClis();
    attach(session);
  }

  // Follows the session the App holds: replays its kept output, then streams until it exits.
  function attach(session) {
    state.running = Boolean(session && session.exit == null);
    if (session && session.id !== state.session) {
      state.session = session.id;
      state.offset = 0;
      term.reset();
    }
    render();
    if (session && !state.pumping && state.ended !== session.id) pump();
  }

  async function pump() {
    state.pumping = true;
    try {
      while (state.session != null) {
        let response;
        try {
          response = await fetch(`/app/terminal/output?session=${state.session}&from=${state.offset}`, { cache: "no-store" });
        } catch { await sleep(1000); continue; }
        if (!response.ok) { state.running = false; break; }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (bytes.length) term.write(bytes);
        state.offset = Number(response.headers.get("X-Terminal-End"));
        const exit = response.headers.get("X-Terminal-Exit");
        if (exit) { ended(exit); break; }
        state.running = true;
        render();
      }
    } finally { state.pumping = false; }
  }

  function ended(code) {
    const note = AN.L(`已结束（退出码 ${code}）`, `Exited (code ${code})`);
    term.write(`\r\n\x1b[2m${note}\x1b[0m\r\n`);
    state.ended = state.session;
    state.running = false;
    render();
  }

  async function start() {
    fit.fit();
    const body = JSON.stringify({ provider: state.provider, cwd: state.cwd, cols: term.cols, rows: term.rows });
    const response = await act("/app/terminal/start", body);
    const result = await response.json().catch(() => ({}));
    if (!response.ok) { term.write(`\r\n\x1b[31m${result.error || response.status}\x1b[0m\r\n`); return; }
    attach(result.session);
    term.focus();
  }

  async function pickFolder() {
    const response = await act("/app/terminal/pick", "{}");
    if (!response.ok) return;
    const result = await response.json();
    if (result.cwd) state.cwd = result.cwd;
    render();
  }

  // Keys go out in order; binary input (some mouse reports) keeps its bytes.
  function send(data, binary = false) {
    if (!state.running) return;
    const body = binary ? Uint8Array.from(data, c => c.charCodeAt(0) & 255) : data;
    sending = sending.then(() => act(`/app/terminal/input?session=${state.session}`, body, "application/octet-stream")).catch(() => {});
  }

  function resize({ cols, rows }) {
    if (state.running) act(`/app/terminal/resize?session=${state.session}`, JSON.stringify({ cols, rows })).catch(() => {});
  }

  function mount() {
    term = new Terminal({
      fontFamily: FONT, fontSize: 13, lineHeight: 1.18, letterSpacing: 0,
      cursorBlink: true, cursorStyle: "bar", cursorWidth: 2, scrollback: 5000,
      allowTransparency: false, macOptionClickForcesSelection: true, drawBoldTextInBrightColors: false,
      theme: THEMES.dark,
    });
    fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(AN.$("term"));
    try {
      const webgl = new WebglAddon.WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch { /* The DOM renderer draws the same cells. */ }
    themed();
    term.onData(data => send(data));
    term.onBinary(data => send(data, true));
    term.onResize(resize);
    new ResizeObserver(() => fit.fit()).observe(AN.$("term"));
    new MutationObserver(themed).observe(root, { attributes: true, attributeFilter: ["data-theme"] });
  }

  // A pane tells the office when it is clicked or right-clicked, so the office can mark it and open
  // its own menu over the whole window.
  function reportToOffice() {
    const origin = location.origin === "null" ? "*" : location.origin;
    const tell = data => window.parent.postMessage({ type: "agentnave:pane", pane, ...data }, origin);
    document.addEventListener("pointerdown", () => tell({ action: "focus" }), true);
    document.addEventListener("contextmenu", event => {
      if (term.hasSelection()) return;
      event.preventDefault();
      tell({ action: "menu", x: event.clientX, y: event.clientY });
    }, true);
    window.addEventListener("message", event => {
      if (event.source !== window.parent || event.data?.type !== "agentnave:pane") return;
      if (event.data.action === "focus") { fit.fit(); term.focus(); }
      else if (event.data.action === "fit") fit.fit();
      else if (event.data.action === "preferences") window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: event.data.general }));
    });
  }

  function listenToOffice() {
    if (pane) return reportToOffice();
    if (!embedded) return;
    const origin = location.origin === "null" ? "*" : location.origin;
    window.addEventListener("message", event => {
      if (event.source !== window.parent || event.data?.type !== "agentnave:terminal") return;
      if (event.data.action === "preferences") {
        window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: event.data.general }));
      } else if (event.data.action === "shown") {
        fit.fit();
        term.focus();
        refresh().catch(() => {});
      }
    });
    window.parent.postMessage({ type: "agentnave:terminal", action: "ready" }, origin);
  }

  document.addEventListener("DOMContentLoaded", () => {
    mount();
    if (pane) { listenToOffice(); refresh().catch(() => {}); return; }
    AN.$("term-folder").addEventListener("click", () => pickFolder().catch(() => {}));
    AN.$("term-run").addEventListener("click", () => {
      if (state.running) act(`/app/terminal/stop?session=${state.session}`, "").catch(() => {});
      else start().catch(() => {});
    });
    listenToOffice();
    refresh().catch(() => render());
  });
}
