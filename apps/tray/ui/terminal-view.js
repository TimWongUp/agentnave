"use strict";
// One CLI on the terminal page (ADR-0103): an xterm.js view that follows the App's output by long
// polling /app/terminal/output and sends keys, binary mouse reports and size changes back.
window.TerminalView = (() => {
  const FONT = '"SF Mono", Menlo, Monaco, Consolas, "Noto Sans Mono CJK SC", monospace';
  // The skin is the --term-* tokens in theme.css (text on a few steps of grey like Grok's, hues
  // from the office's status colours), read as resolved so it follows the App's theme with the
  // frame around it.
  const SKIN = {
    background: "bg", foreground: "fg", cursor: "cursor", cursorAccent: "cursor-accent",
    selectionBackground: "selection", scrollbarSliderBackground: "scroll",
    scrollbarSliderHoverBackground: "scroll-hover", scrollbarSliderActiveBackground: "scroll-active",
  };
  for (const name of ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]) {
    SKIN[name] = name;
    SKIN[`bright${name[0].toUpperCase()}${name.slice(1)}`] = `bright-${name}`;
  }
  const root = document.documentElement;
  const light = () => root.dataset.theme === "light";
  const theme = () => {
    const style = getComputedStyle(root);
    return Object.fromEntries(Object.entries(SKIN).map(([name, token]) => [name, style.getPropertyValue(`--term-${token}`).trim()]));
  };
  // The cursor stops blinking with "reduce motion", as the App's own animations do.
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
  const still = () => root.dataset.motion === "reduce" || (root.dataset.motion !== "full" && reducedMotion.matches);
  // A CLI picks its colours when it starts, so after the App changes appearance, or for CLIs that
  // ignore the background, text can sit too close to it. As in VS Code's terminal, such text is
  // shifted until it reads at 4.5:1.
  const CONTRAST = 4.5;
  // Like herdr and Ghostty, the terminal answers the light/dark query (CSI ? 996 n) and tells CLIs
  // that subscribed with mode 2031 (Claude Code) when the App changes appearance, so they redraw
  // in the matching theme.
  const scheme = () => `\x1b[?997;${light() ? 2 : 1}n`;
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const act = (path, body, type = "application/json") => fetch(path, {
    method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": type }, body,
  });

  class TerminalView {
    constructor(id, onExit) {
      this.id = id;
      this.onExit = onExit;
      this.view = typeof crypto !== "undefined" && crypto.randomUUID
        ? crypto.randomUUID()
        : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
      this.offset = 0;
      this.live = true;
      this.running = true;
      this.replaying = false;
      this.pendingFocus = false;
      this.notify = false;
      this.isLight = light();
      this.sending = Promise.resolve();
      this.writeDone = null;
      this.element = document.createElement("div");
      this.element.className = "tm-term";
      this.term = new Terminal({
        fontFamily: FONT, fontSize: 13, lineHeight: 1.3, fontWeightBold: 600, cursorBlink: !still(),
        cursorStyle: "bar", cursorWidth: 2, scrollback: 5000, macOptionClickForcesSelection: true,
        drawBoldTextInBrightColors: false, theme: theme(), minimumContrastRatio: CONTRAST,
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
      this.term.parser.registerCsiHandler({ prefix: "?", final: "h" }, params => {
        if (params.includes(2031)) this.notify = true;
        return false;
      });
      this.term.parser.registerCsiHandler({ prefix: "?", final: "l" }, params => {
        if (params.includes(2031)) this.notify = false;
        return false;
      });
      this.term.parser.registerCsiHandler({ prefix: "?", final: "n" }, params => {
        if (params[0] !== 996) return false;
        this.send(scheme());
        return true;
      });
      this.term.onData(data => this.send(data));
      this.term.onBinary(data => this.send(data, true));
      this.term.onResize(({ cols, rows }) => {
        if (this.running) act(`/app/terminal/resize?session=${this.id}`, JSON.stringify({ cols, rows })).catch(() => {});
      });
      this.observer = new ResizeObserver(() => this.layout());
      this.observer.observe(this.element);
      this.motion = new MutationObserver(() => this.blink());
      this.motion.observe(root, { attributes: true, attributeFilter: ["data-motion"] });
      this.blinking = () => this.blink();
      reducedMotion.addEventListener("change", this.blinking);
      this.pump();
    }

    layout() {
      if (this.element.offsetWidth && this.element.offsetHeight) this.fit.fit();
    }

    focus() {
      if (this.replaying) {
        this.pendingFocus = true;
        return;
      }
      this.term.focus();
    }

    blink() { this.term.options.cursorBlink = !still(); }

    restyle() {
      const next = light();
      if (next === this.isLight) return;
      this.isLight = next;
      this.term.options.theme = theme();
      if (this.notify) this.send(scheme());
    }

    hasSelection() { return this.term.hasSelection(); }

    // Keys go out in order; binary input (some mouse reports) keeps its bytes.
    send(data, binary = false) {
      if (!this.running || this.replaying) return;
      const body = binary ? Uint8Array.from(data, c => c.charCodeAt(0) & 255) : data;
      this.sending = this.sending
        .then(() => {
          if (this.running) return act(`/app/terminal/input?session=${this.id}`, body, "application/octet-stream");
        })
        .catch(() => {});
    }

    // Replays what the App kept, then streams until the CLI exits or the view goes.
    async pump() {
      while (this.live) {
        let response;
        try {
          response = await fetch(`/app/terminal/output?session=${this.id}&view=${encodeURIComponent(this.view)}&from=${this.offset}`, { cache: "no-store" });
        } catch { await sleep(1000); continue; }
        if (!this.live) return;
        if (response.status === 404) { this.ended(null); return; }
        if (!response.ok) { await sleep(1000); continue; }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (!this.live) return;
        const replay = response.headers.get("X-Terminal-Replay") === "1";
        const appearance = response.headers.get("X-Terminal-Appearance");
        if (appearance != null) this.notify = appearance === "1";
        if (bytes.length) {
          if (replay) this.replaying = true;
          try {
            await new Promise(resolve => {
              this.writeDone = resolve;
              this.term.write(bytes, resolve);
            });
          } finally {
            this.writeDone = null;
            if (replay) this.replaying = false;
          }
        }
        if (!this.live) return;
        if (appearance != null) this.notify = appearance === "1";
        if (this.pendingFocus) {
          this.pendingFocus = false;
          this.term.focus();
        }
        this.offset = Number(response.headers.get("X-Terminal-End"));
        const exit = response.headers.get("X-Terminal-Exit");
        if (exit) {
          const code = Number(exit);
          this.ended(Number.isFinite(code) ? code : exit);
          return;
        }
        if (replay && this.notify) this.send(scheme());
      }
    }

    ended(code) {
      if (!this.running) return;
      this.running = false;
      const exitCode = code == null || code === "" ? null : Number.isFinite(Number(code)) ? Number(code) : code;
      if (exitCode != null && exitCode !== 0) {
        const note = AN.L(`已结束（退出码 ${exitCode}）`, `Exited (code ${exitCode})`);
        this.term.write(`\r\n\x1b[2m${note}\x1b[0m\r\n`);
      }
      this.onExit?.(this.id, exitCode);
    }

    dispose() {
      this.live = false;
      this.replaying = false;
      this.pendingFocus = false;
      this.writeDone?.();
      this.writeDone = null;
      this.observer?.disconnect();
      this.motion?.disconnect();
      reducedMotion.removeEventListener("change", this.blinking);
      this.term.dispose();
    }
  }
  return TerminalView;
})();
