"use strict";
{
  const { h, $, L, load, post } = AN;
  const root = $("isolated-cleanup-settings");
  let data = null;
  let busy = false;

  const IDLE_CHOICES = [
    [null, () => L("关闭", "Off")],
    [7, () => L("7 天", "7 days")],
    [14, () => L("14 天", "14 days")],
    [30, () => L("30 天", "30 days")],
    [90, () => L("90 天", "90 days")],
  ];

  function say(text, bad = false) {
    const notice = $("notice");
    if (!notice) return;
    notice.textContent = text;
    notice.classList.toggle("error", bad);
  }

  const value = (key) => data?.preferences?.isolated_cleanup?.[key] ?? (key === "idle_days" ? null : false);

  async function save(key, next) {
    if (busy) return;
    busy = true;
    render();
    try {
      const res = await post("/app/preferences", { isolated_cleanup: { [key]: next } });
      data = { ...data, ...res };
      say(L("已保存", "Saved"));
    } catch (err) {
      say(err.message || L("保存失败", "Failed to save"), true);
    } finally {
      busy = false;
      render();
    }
  }

  function switchRow(key, label) {
    const toggle = h("input.switch-input", {
      type: "checkbox",
      checked: Boolean(value(key)),
      disabled: busy,
      "aria-label": label,
    });
    toggle.addEventListener("change", () => void save(key, toggle.checked));
    return h("div.s-row", {}, h("span.s-label", {}, label), toggle);
  }

  function idleRow() {
    const label = L("闲置", "Idle");
    const current = value("idle_days");
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [days, text] of IDLE_CHOICES) {
      const active = current === days;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled: busy,
      }, text());
      btn.addEventListener("click", () => {
        if (!active) void save("idle_days", days);
      });
      control.append(btn);
    }
    return h("div.s-row", {}, h("span.s-label", {}, label), control);
  }

  function render() {
    if (!root || !data) return;
    root.replaceChildren(
      switchRow("merged", L("PR 已合并", "PR merged")),
      switchRow("unchanged", L("无新提交", "No new commits")),
      idleRow(),
      switchRow("workspace_deleted", L("工作区已删除", "Workspace deleted")),
    );
  }

  async function refresh() {
    if (!root || busy) return;
    try {
      data = await load("/app/preferences");
      render();
    } catch (err) {
      say(err.message || L("无法读取隔离目录清理设置", "Failed to load isolated directory cleanup settings"), true);
    }
  }

  const visible = () => !$("pane-storage")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveIsolatedCleanupSettings = { load: refresh };
  if (!$("pane-storage")?.hidden) void refresh();
}
