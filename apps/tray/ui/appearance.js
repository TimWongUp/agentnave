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
  const reducedMotion = { get matches() {
    if (root.dataset.onBattery === "true" && root.dataset.batteryStill === "true") return true;
    return root.dataset.motion === "reduce" || (root.dataset.motion !== "full" && motion.matches);
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
  return { applyTheme, applyPreferences, themeSwitch, reducedMotion,
    preferences: () => ({ theme, motion: root.dataset.motion, character_style: characterStyle,
      battery_still: root.dataset.batteryStill === "true", pause_unfocused: root.dataset.pauseUnfocused !== "false" }) };
})();
