"use strict";
{
  const { h, L, load, post } = AN;
  const launch = document.getElementById("btn-uninstall");
  let dialog, toggle, status, cancel, confirm, failures;
  let busy = false, polling = null, generation = 0;

  async function dismiss() {
    if (busy) return;
    try {
      await post("/app/uninstall/cancel", {});
      generation += 1;
      clearTimeout(polling);
      dialog.close();
    } catch (error) { status.textContent = error.message; }
  }

  async function refresh() {
    if (!dialog?.open || busy) return;
    const current = generation;
    try {
      const state = await load("/app/uninstall");
      if (current !== generation || busy || !dialog.open) return;
      status.textContent = state.blocked || (!state.ready ? L("正在读取宿主接入…", "Reading host connections…") : "");
      toggle.disabled = confirm.disabled = Boolean(state.blocked || !state.ready);
    } catch (error) {
      if (current !== generation || busy || !dialog.open) return;
      status.textContent = error.message;
      toggle.disabled = confirm.disabled = true;
    }
    if (current === generation && dialog.open && !busy) polling = setTimeout(refresh, 1500);
  }

  async function remove() {
    if (busy || confirm.disabled) return;
    busy = true;
    generation += 1;
    clearTimeout(polling);
    toggle.disabled = confirm.disabled = cancel.disabled = true;
    status.textContent = L("正在卸载…", "Uninstalling…");
    failures.replaceChildren();
    try {
      const result = await post("/app/uninstall", { delete_history: toggle.checked });
      if (result.ok) return; // The App exits only after all synchronous cleanup has succeeded.
      for (const item of result.failures || []) {
        failures.append(h("li", {}, `${item.host}：${item.error}`));
      }
    } catch (error) {
      failures.append(h("li", {}, error.message));
    }
    busy = false;
    status.textContent = "";
    confirm.textContent = L("重试", "Retry");
    cancel.disabled = false;
    await refresh();
  }

  function open() {
    if (!dialog) {
      toggle = h("input.switch-input", { type: "checkbox", role: "switch" });
      status = h("p.uninstall-status", { role: "status", "aria-live": "polite" });
      failures = h("ul.uninstall-failures", { "aria-live": "polite" });
      cancel = h("button", { type: "button" }, L("取消", "Cancel"));
      confirm = h("button.danger", { type: "button" }, L("卸载", "Uninstall"));
      cancel.addEventListener("click", dismiss);
      confirm.addEventListener("click", remove);
      dialog = h("dialog.upd.uninstall-dialog", { "aria-labelledby": "uninstall-title" },
        h("h2.upd-title", { id: "uninstall-title" }, L("卸载 AgentNave？", "Uninstall AgentNave?")),
        h("div.uninstall-content", {},
          h("label.uninstall-option", {}, L("同时删除任务和会议记录", "Also delete task and meeting history"), toggle),
          status, failures),
        h("div.upd-row", {}, cancel, confirm));
      dialog.addEventListener("cancel", (event) => { event.preventDefault(); dismiss(); });
      document.body.append(dialog);
    }
    if (dialog.open) return;
    toggle.checked = false;
    toggle.disabled = confirm.disabled = true;
    cancel.disabled = false;
    busy = false;
    failures.replaceChildren();
    confirm.textContent = L("卸载", "Uninstall");
    status.textContent = "";
    generation += 1;
    dialog.showModal();
    cancel.focus();
    refresh();
  }
  launch?.addEventListener("click", open);
  globalThis.addEventListener?.("agentnave:uninstall", open);
  const query = new URLSearchParams(location.search);
  if (query.has("uninstall")) {
    query.delete("uninstall");
    const search = query.toString();
    if (typeof history !== "undefined" && history.replaceState) {
      history.replaceState(null, "", `${location.pathname}${search ? `?${search}` : ""}${location.hash}`);
    }
    open();
  }
}
