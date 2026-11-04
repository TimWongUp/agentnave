"use strict";

// The window header's update button: always there as an icon, so nothing beside it moves. It
// checks for updates, marks one that is ready, spins while one runs and ticks after one finished;
// a click runs the update flow. The menu bar panel is too narrow for its dialog and opens the main
// window's instead.
(function () {
  const { h, L, load } = AN;
  const end = document.querySelector("header.bar .bar-end, header.top .top-end");
  if (!end) return;
  // Glyphs, like the theme switch beside it.
  const GLYPHS = { check: "↻", busy: "↻", available: "↑", updated: "✓" };
  const glyph = h("span", { "aria-hidden": "true" });
  const button = h("button.bar-update", { type: "button" }, glyph);
  end.prepend(button);
  const short = end.matches(".top-end");
  let status = null;
  const draw = kind => { if (glyph.textContent !== GLYPHS[kind]) glyph.textContent = GLYPHS[kind]; };
  function percent(update) {
    return update?.step === "download" && update.total > 0 ? ` ${Math.min(100, Math.floor(update.downloaded / update.total * 100))}%` : "";
  }
  function render() {
    const update = status?.update?.state;
    const service = status?.service?.state;
    const busy = update === "installing" || ["upgrading", "installing"].includes(service);
    const found = update === "available" || service === "outdated";
    const updated = !busy && !found && status?.updated;
    const kind = busy ? "busy" : found ? "available" : updated ? "updated" : "check";
    draw(kind);
    button.dataset.state = kind;
    button.title = busy ? L(`更新中${percent(status.update) || "…"}`, `Updating${percent(status.update) || "…"}`)
      : update === "available" ? L(`可更新到 v${status.update.version}`, `Update to v${status.update.version}`)
      : found ? L("可升级本机服务", "Service upgrade available")
      : updated ? L(`已更新到 v${status.updated}`, `Updated to v${status.updated}`)
      : L("检查更新", "Check for updates");
    button.setAttribute("aria-label", button.title);
  }
  render();
  async function refresh() {
    if (document.hidden) return;
    try { status = await load("/app/status"); } catch { return; }
    render();
  }
  // Fast only while an update runs, which another window may have started.
  const busyNow = () => status?.update?.state === "installing" || ["upgrading", "installing"].includes(status?.service?.state);
  async function tick() {
    await refresh();
    setTimeout(tick, busyNow() ? 1000 : 60000);
  }
  button.addEventListener("click", () => {
    if (short) location.href = "/app/update";
    else AN.updateFlow();
  });
  // The settings page and the update flow read the status themselves; reuse what they got.
  addEventListener("agentnave:status", ({ detail }) => { status = detail; render(); });
  document.addEventListener("visibilitychange", refresh);
  // After every deferred script has initialized.
  addEventListener("DOMContentLoaded", tick);
})();

// The brand's N lets its dot drip when the pointer comes over it, as the menu bar icon does on a
// click; once started it plays to the end.
for (const brand of document.querySelectorAll(".brand")) {
  brand.addEventListener("mouseenter", () => {
    const mark = brand.querySelector(".mark");
    if (!mark || mark.classList.contains("drip") || window.ANAppearance.reducedMotion.matches) return;
    mark.classList.add("drip");
    mark.querySelector(".drop").addEventListener("animationend", () => mark.classList.remove("drip"), { once: true });
  });
}
