"use strict";
{
  const S = AN.settings ||= {};
  const {
    $, L, discardAdvanced, el, loadMembers, loadStatus, probeWhenStale, refreshDetection, saveMembers,
    setTab, triggerCheckUpdate, unsaved
  } = S;
  Object.assign(S, { renderLeaveConfirm, confirmLeave });
  const OLD_TABS = { "#voice-settings": "voice", "#updates": "about", "#skills": "advanced", "#orchestration": "advanced" }; // Preserve old links.
  const tabFromHash = () => OLD_TABS[location.hash] || (["general", "members", "avatars", "voice", "advanced", "storage", "about"].includes(location.hash.slice(1)) ? location.hash.slice(1) : "hosts");
  window.addEventListener("hashchange", () => setTab(tabFromHash()));
  setTab(tabFromHash());

  $("refresh-detection").addEventListener("click", () => refreshDetection());
  window.addEventListener("focus", probeWhenStale);
  document.addEventListener("visibilitychange", probeWhenStale);
  $("save-btn").addEventListener("click", saveMembers);
  $("max-running").addEventListener("change", () => {
    const feedback = $("save-feedback");
    feedback.textContent = "";
    feedback.className = "save-feedback";
    S.refreshDirty();
    S.updateControls();
  });

  $("btn-check-update").addEventListener("click", () => triggerCheckUpdate());
  // Updating goes through the shared dialog: it shows what changed first and then the progress.
  $("btn-apply-update").addEventListener("click", () => AN.updateFlow());
  $("btn-retry-update").addEventListener("click", () => AN.updateFlow());

  // The app's web view does not show beforeunload prompts, so hold back in-app navigation.
  function renderLeaveConfirm() {
    const box = $(S.currentTab === "advanced" ? "advanced-leave-confirm" : "leave-confirm");
    const other = $(S.currentTab === "advanced" ? "leave-confirm" : "advanced-leave-confirm");
    other.hidden = true;
    other.replaceChildren();
    box.hidden = !S.leaveTarget;
    if (!S.leaveTarget) return box.replaceChildren();
    if (box.childElementCount) return;
    const stay = el("button", { type: "button", onClick: () => { S.leaveTarget = null; renderLeaveConfirm(); } }, L("继续编辑", "Keep editing"));
    box.replaceChildren(
      el("p", {}, L("有未保存的修改，离开设置页会丢失这些修改。", "You have unsaved changes. Leaving Settings will discard them.")),
      el("div", { className: "btn-group" }, stay,
        el("button", { type: "button", className: "danger", onClick: () => {
          const target = S.leaveTarget;
          S.leaveTarget = null;
          S.dirty = false;
          discardAdvanced();
          if (typeof target === "function") target();
          else if (!window.AgentNaveSettingsFrame?.navigate(target)) location.href = target;
        } }, L("放弃修改并离开", "Discard and leave"))));
    stay.focus({ preventScroll: true });
  }
  function confirmLeave(target) {
    if (!(unsaved() || S.busy)) return false;
    S.leaveTarget = target;
    // The confirmation sits in the save bar of the page holding the edits.
    const tab = S.dirty || !S.advancedChanges().length ? "members" : "advanced";
    if (S.currentTab !== tab) {
      setTab(tab);
      history.replaceState(null, "", `#${tab}`);
    }
    renderLeaveConfirm();
    return true;
  }
  document.querySelector("header.bar").addEventListener("click", (e) => {
    const link = e.target.closest("a[href]");
    // The settings link points at this page; following it would reload and drop the edits.
    if (link?.getAttribute("href") === "/settings/") return e.preventDefault();
    if (link && confirmLeave(link.href)) e.preventDefault();
  }, true);

  window.addEventListener("beforeunload", (e) => {
    if (unsaved() || S.busy) {
      e.preventDefault();
      e.returnValue = "";
    }
  });

  // Initialize: show the cached detection at once, then re-probe for newly added models.
  loadMembers().then(() => refreshDetection(true));
  loadStatus();

  // Model names: short (without the routing prefix) or full; shared with every window.
  (function () {
    const buttons = { short: document.getElementById("model-names-short"), full: document.getElementById("model-names-full") };
    function show() {
      const full = AN.fullModelNames();
      for (const [key, button] of Object.entries(buttons)) {
        const on = (key === "full") === full;
        button.classList.toggle("active", on);
        button.setAttribute("aria-checked", on ? "true" : "false");
      }
      document.getElementById("model-names-example").textContent = full
        ? L("显示为 opencode-go/deepseek-v4.1-flash，带上前面的路由", "Shown as opencode-go/deepseek-v4.1-flash, with the routing prefix")
        : L("显示为 deepseek-v4.1-flash，省去前面的路由 opencode-go/", "Shown as deepseek-v4.1-flash, without the opencode-go/ prefix");
    }
    for (const [key, button] of Object.entries(buttons))
      button.addEventListener("click", () => { AN.setFullModelNames(key === "full"); show(); });
    show();
    // Another window (or the embedded frame's parent) may change it; the cards in view are drawn again.
    window.addEventListener("agentnave:model-names", () => {
      show();
      if (!$("pane-members").hidden) S.renderCards();
      if (!$("pane-advanced").hidden) { S.renderReviewSeats(); S.renderOrchestration(); }
    });
  })();
}
