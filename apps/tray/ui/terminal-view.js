"use strict";
// One CLI on the terminal page (ADR-0101): an xterm.js view that follows the App's output by long
// polling /app/terminal/output and sends keys, binary mouse reports and size changes back.
window.TerminalView = (() => {
  const root = document.documentElement;
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
  const theme = () => THEMES[root.dataset.theme === "light" ? "light" : "dark"];
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const act = (path, body, type = "application/json") => fetch(path, {
    method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": type }, body,
  });

  class TerminalView {
    constructor(id, onExit) {
      this.id = id;
      this.onExit = onExit;
      this.offset = 0;
      this.live = true;
      this.running = true;
      this.sending = Promise.resolve();
      this.element = document.createElement("div");
      this.element.className = "tm-term";
      this.term = new Terminal({
        fontFamily: FONT, fontSize: 13, lineHeight: 1.18, cursorBlink: true, cursorStyle: "bar",
        cursorWidth: 2, scrollback: 5000, macOptionClickForcesSelection: true,
        drawBoldTextInBrightColors: false, theme: theme(),
      });
      this.fit = new FitAddon.FitAddon();
      this.term.loadAddon(this.fit);
    }

    open() {
      this.term.open(this.element);
      try {
        const webgl = new WebglAddon.WebglAddon();
        webgl.onContextLoss(() => webgl.dispose());
        this.term.loadAddon(webgl);
      } catch { /* The DOM renderer draws the same cells. */ }
      this.term.onData(data => this.send(data));
      this.term.onBinary(data => this.send(data, true));
      this.term.onResize(({ cols, rows }) => {
        if (this.running) act(`/app/terminal/resize?session=${this.id}`, JSON.stringify({ cols, rows })).catch(() => {});
      });
      this.observer = new ResizeObserver(() => this.layout());
      this.observer.observe(this.element);
      this.pump();
    }

    layout() {
      if (this.element.offsetWidth && this.element.offsetHeight) this.fit.fit();
    }

    focus() { this.term.focus(); }

    restyle() { this.term.options.theme = theme(); }

    hasSelection() { return this.term.hasSelection(); }

    // Keys go out in order; binary input (some mouse reports) keeps its bytes.
    send(data, binary = false) {
      if (!this.running) return;
      const body = binary ? Uint8Array.from(data, c => c.charCodeAt(0) & 255) : data;
      this.sending = this.sending
        .then(() => act(`/app/terminal/input?session=${this.id}`, body, "application/octet-stream"))
        .catch(() => {});
    }

    // Replays what the App kept, then streams until the CLI exits or the view goes.
    async pump() {
      while (this.live) {
        let response;
        try {
          response = await fetch(`/app/terminal/output?session=${this.id}&from=${this.offset}`, { cache: "no-store" });
        } catch { await sleep(1000); continue; }
        if (!this.live) return;
        if (!response.ok) { this.ended(null); return; }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (bytes.length) this.term.write(bytes);
        this.offset = Number(response.headers.get("X-Terminal-End"));
        const exit = response.headers.get("X-Terminal-Exit");
        if (exit) { this.ended(exit); return; }
      }
    }

    ended(code) {
      this.running = false;
      if (code != null) {
        const note = AN.L(`已结束（退出码 ${code}）`, `Exited (code ${code})`);
        this.term.write(`\r\n\x1b[2m${note}\x1b[0m\r\n`);
      }
      this.onExit?.(this.id);
    }

    dispose() {
      this.live = false;
      this.observer?.disconnect();
      this.term.dispose();
    }
  }
  TerminalView.background = () => theme().background;
  return TerminalView;
})();
