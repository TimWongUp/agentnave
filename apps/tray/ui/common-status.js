"use strict";

// The window header's update button: shown only when the App or the local service can be updated,
// or for a while after an update; a click runs the update flow. The menu bar panel is too narrow
// for its dialog and opens the main window's instead.
(function () {
  const { h, L, load } = AN;
  const end = document.querySelector("header.bar .bar-end, header.top .top-end");
  if (!end) return;
  const button = h("button.bar-update", { type: "button", hidden: true });
  end.prepend(button);
  const short = end.matches(".top-end");
  let status = null;
  function percent(update) {
    return update?.step === "download" && update.total > 0 ? ` ${Math.min(100, Math.floor(update.downloaded / update.total * 100))}%` : "";
  }
  function render() {
    const update = status?.update?.state;
    const service = status?.service?.state;
    const busy = update === "installing" || ["upgrading", "installing"].includes(service);
    const found = update === "available" || service === "outdated";
    const updated = !busy && !found && status?.updated;
    button.hidden = !busy && !found && !updated;
    button.classList.toggle("updated", Boolean(updated));
    button.textContent = busy ? L(`更新中${percent(status.update) || "…"}`, `Updating${percent(status.update) || "…"}`)
      : updated ? L(`已更新到 v${status.updated}`, `Updated to v${status.updated}`)
      : update === "available" ? `↑ ${short ? "" : L("更新到 ", "Update to ")}v${status.update.version}`
      : short ? L("↑ 升级服务", "↑ Upgrade") : L("↑ 升级本机服务", "↑ Upgrade service");
    button.title = busy ? L("查看更新进度", "View update progress") : updated ? L("点此查看是否还有更新", "Check for more updates") : L("先看看更新了什么，确认后再更新", "Review release notes before updating");
  }
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
