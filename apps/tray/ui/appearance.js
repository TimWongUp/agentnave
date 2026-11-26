"use strict";
// Loaded before common.js on every page, so all windows use the same preferences.
window.ANAppearance = (() => {
  const root = document.documentElement;
  const light = matchMedia("(prefers-color-scheme: light)");
  const motion = matchMedia("(prefers-reduced-motion: reduce)");
  const inApp = () => location.protocol === "agentnave:" || location.hostname === "agentnave.localhost";
  let theme = root.dataset.theme || "system";
  let characterStyle = root.dataset.characterStyle || "static";
  let generation = 0;
  const uiReducedMotion = { get matches() {
    return root.dataset.motion === "reduce" || (root.dataset.motion !== "full" && motion.matches);
  } };
  const reducedMotion = { get matches() {
    if (root.dataset.onBattery === "true" && root.dataset.batteryStill === "true") return true;
    return uiReducedMotion.matches;
  } };

  function applyTheme() {
    root.dataset.theme = theme === "system" ? (light.matches ? "light" : "dark") : theme;
    document.querySelectorAll("[data-theme-choice]").forEach(button => {
      button.setAttribute("aria-pressed", String(button.dataset.themeChoice === theme));
    });
  }

  function applyPreferences(general) {
    if (!general) return;
    generation++;
    if (["system", "light", "dark"].includes(general.theme)) theme = general.theme;
    if (["system", "reduce", "full"].includes(general.motion)) root.dataset.motion = general.motion;
    if (typeof general.battery_still === "boolean") root.dataset.batteryStill = String(general.battery_still);
    if (typeof general.pause_unfocused === "boolean") root.dataset.pauseUnfocused = String(general.pause_unfocused);
    if (["static", "video-272", "video-544"].includes(general.character_style)) {
      characterStyle = general.character_style;
      root.dataset.characterStyle = characterStyle;
    }
    applyTheme();
  }

  function publish(general, expected) {
    if (generation === expected && general) {
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: general }));
    }
  }

  async function saveTheme(choice) {
    const expected = generation;
    const headers = { "X-AgentNave-Action": "1", "Content-Type": "application/json" };
    const response = await fetch("/app/preferences", {
      method: "POST", headers,
      body: JSON.stringify({ general: { theme: choice } }),
    });
    if (!response.ok) throw new Error(`Preferences: ${response.status}`);
    const result = await response.json();
    publish(result.effective?.general || result.preferences?.general, expected);
  }

  async function refresh() {
    if (!inApp()) return;
    const expected = generation;
    try {
      const response = await fetch("/app/preferences", { cache: "no-store" });
      if (!response.ok) return;
      const state = await response.json();
      if (!state.preferences || state.error) return;
      publish(state.effective?.general || state.preferences.general, expected);
    } catch { /* Keep the attributes the App supplied when preferences cannot be read. */ }
  }

  function themeSwitch(h, L) {
    const labels = { system: ["◐", L("跟随系统", "System")], light: ["☀", L("浅色", "Light")], dark: ["☾", L("深色", "Dark")] };
    const group = h("div.theme", { role: "group", "aria-label": L("外观", "Appearance") },
      ...Object.entries(labels).map(([choice, [icon, title]]) => {
        const button = h("button", { type: "button", title, "aria-label": title, "data-theme-choice": choice }, icon);
        button.addEventListener("click", async () => {
          button.disabled = true;
          try {
            if (inApp()) await saveTheme(choice);
            else { theme = choice; applyTheme(); }
          } catch { /* A failed write leaves the saved choice selected. */ }
          finally { button.disabled = false; }
        });
        return button;
      }));
    queueMicrotask(applyTheme);
    return group;
  }

  light.addEventListener("change", applyTheme);
  addEventListener("agentnave:preferences", event => applyPreferences(event.detail));
  addEventListener("focus", refresh);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
  addEventListener("DOMContentLoaded", refresh);
  applyTheme();
  return { applyTheme, applyPreferences, themeSwitch, reducedMotion, uiReducedMotion,
    preferences: () => ({ theme, motion: root.dataset.motion, character_style: characterStyle,
      battery_still: root.dataset.batteryStill === "true", pause_unfocused: root.dataset.pauseUnfocused !== "false" }) };
})();

// Shared lightweight motion primitives for every tray page (zero dependencies, VM-test safe).
globalThis.ANMotion = (() => {
  const EASE_OUT = "cubic-bezier(0.2, 0, 0, 1)";
  const EASE_IN = "cubic-bezier(0.4, 0, 1, 1)";
  const root = document.documentElement;
  const reduced = () => Boolean(globalThis.ANAppearance?.uiReducedMotion?.matches);
  const visibleAndFocused = () => typeof document !== "undefined" && !document.hidden
    && !root.classList?.contains?.("paused") && !root.classList?.contains?.("blurred")
    && !root.hasAttribute?.("data-window-hidden");

  function ms(name, fallback) {
    if (typeof getComputedStyle !== "function") return fallback;
    const raw = getComputedStyle(root).getPropertyValue(name)?.trim();
    if (!raw) return fallback;
    const n = parseFloat(raw);
    if (!Number.isFinite(n)) return fallback;
    return raw.endsWith("ms") ? n : (raw.endsWith("s") ? n * 1000 : n);
  }

  function scrollBehavior() {
    return reduced() ? "auto" : "smooth";
  }

  // Single-layout vertical FLIP using offsetTop so scroll anchoring never skews deltas.
  function flipY(nodes, mutate, opts = {}) {
    const list = [...(nodes || [])].filter(n => n && typeof n.offsetTop === "number" && typeof n.animate === "function");
    if (!list.length || reduced() || !visibleAndFocused()) { mutate(); return; }
    const before = new Map();
    for (const n of list) {
      if (n.offsetParent !== null) before.set(n, n.offsetTop);
    }
    mutate();
    const duration = opts.duration ?? ms("--dur-view", 260);
    const easing = opts.easing || EASE_OUT;
    for (const [n, oldTop] of before) {
      if (!n.isConnected || n.offsetParent === null) continue;
      const dy = oldTop - n.offsetTop;
      if (Math.abs(dy) < 1) continue;
      n.animate(
        [{ transform: `translate3d(0, ${dy}px, 0)` }, { transform: "translate3d(0, 0, 0)" }],
        { duration, easing },
      );
    }
  }

  // Modal <dialog> entrance via WAAPI so blurred/unfocused launches never freeze at opacity 0.
  function dialogEnter(dialog) {
    if (!dialog || typeof dialog.animate !== "function") return;
    if (!visibleAndFocused()) return;
    const isRed = reduced();
    const duration = isRed ? ms("--dur-pop", 100) : ms("--dur-pop", 210);
    const frames = isRed
      ? [{ opacity: 0 }, { opacity: 1 }]
      : [
          { opacity: 0, transform: "translate3d(0, 6px, 0) scale(0.97)" },
          { opacity: 1, transform: "translate3d(0, 0, 0) scale(1)" },
        ];
    dialog.animate(frames, { duration, easing: isRed ? "linear" : EASE_OUT });
  }

  // Subtle GPU-only reveal for newly shown content blocks (never loops, skips when reduced/blurred).
  function reveal(node, opts = {}) {
    if (!node || typeof node.animate !== "function" || !visibleAndFocused()) return null;
    const isRed = reduced();
    const duration = opts.duration ?? (isRed ? ms("--dur-pop", 100) : ms("--dur-in", 180));
    const dx = isRed ? 0 : (opts.dx || 0);
    const dy = isRed ? 0 : (opts.dy ?? 4);
    const frames = (dx || dy)
      ? [{ opacity: 0, transform: `translate3d(${dx}px, ${dy}px, 0)` }, { opacity: 1, transform: "translate3d(0, 0, 0)" }]
      : [{ opacity: 0 }, { opacity: 1 }];
    return node.animate(frames, { duration, easing: isRed ? "linear" : EASE_OUT, delay: opts.delay || 0, fill: opts.delay ? "backwards" : "none" });
  }

  // Sliding thumb controller for persistent segmented controls via ::before (zero extra child DOM).
  function attachThumb(container, activeSelector = '[aria-checked="true"], .active') {
    if (!container || !container.style) return { sync() {} };
    let ready = false;
    let raf = 0;

    function sync(instant = false) {
      const btn = container.querySelector?.(activeSelector);
      const btns = container.children ? [...container.children].filter(b => b.offsetWidth > 0 && b.offsetHeight > 0) : [];
      const wrapped = btns.length > 1 && btns.some(b => Math.abs(b.offsetTop - btns[0].offsetTop) > 2);
      if (!btn || !container.offsetWidth || !btn.offsetWidth || wrapped) {
        ready = false;
        container.removeAttribute?.("data-thumb-ready");
        return;
      }
      if (instant || !ready || reduced()) {
        ready = false;
        container.removeAttribute?.("data-thumb-ready");
      }
      container.style.setProperty("--thumb-x", `${btn.offsetLeft}px`);
      container.style.setProperty("--thumb-w", `${btn.offsetWidth}px`);
      if (!ready && !reduced() && typeof requestAnimationFrame === "function") {
        cancelAnimationFrame(raf);
        raf = requestAnimationFrame(() => {
          if (container.offsetWidth && !reduced()) {
            ready = true;
            container.setAttribute?.("data-thumb-ready", "1");
          }
        });
      }
    }

    if (typeof ResizeObserver === "function") {
      new ResizeObserver(() => sync(true)).observe(container);
    }
    if (typeof document !== "undefined" && document.fonts?.ready) {
      document.fonts.ready.then(() => sync(true), () => {});
    }
    return { sync };
  }

  return { EASE_OUT, EASE_IN, reduced, visibleAndFocused, ms, scrollBehavior, flipY, dialogEnter, reveal, attachThumb };
})();
