"use strict";
{
  const S = AN.settings ||= {};
  const {
    $, L, discardAdvanced, el, loadMembers, loadStatus, probeWhenStale, refreshDetection,
    setTab, triggerCheckUpdate, unsaved
  } = S;
  Object.assign(S, { renderLeaveConfirm, confirmLeave });
  const OLD_TABS = { "#voice-settings": "voice", "#updates": "about", "#skills": "advanced" }; // Preserve old links.
  const tabFromHash = () => OLD_TABS[location.hash] || (["general", "members", "orchestration", "avatars", "voice", "storage", "advanced", "about"].includes(location.hash.slice(1)) ? location.hash.slice(1) : "hosts");
  window.addEventListener("hashchange", () => setTab(tabFromHash()));
  setTab(tabFromHash());

  $("refresh-detection").addEventListener("click", () => refreshDetection());
  window.addEventListener("focus", probeWhenStale);
  document.addEventListener("visibilitychange", probeWhenStale);
  $("max-running").addEventListener("change", () => { void S.saveLimitImmediate?.(); });

  $("btn-check-update").addEventListener("click", () => triggerCheckUpdate());
  // Updating goes through the shared dialog: it shows what changed first and then the progress.
  $("btn-apply-update").addEventListener("click", () => AN.updateFlow());
  $("btn-retry-update").addEventListener("click", () => AN.updateFlow());

  let leaveDialog = null;
  function closeLeaveDialog() {
    if (!leaveDialog) return;
    if (leaveDialog.open) leaveDialog.close?.();
    leaveDialog.open = false;
    leaveDialog.remove?.();
    leaveDialog = null;
  }

  function discardAllLeaveChanges() {
    S.dirty = false;
    S.discardMembersDraft?.();
    window.AgentNaveVoiceSettings?.discardDrafts?.();
    window.AgentNaveDecisionSeats?.discardKeyDraft?.();
    discardAdvanced();
  }

  function proceedLeaveTarget() {
    const target = S.leaveTarget;
    S.leaveTarget = null;
    discardAllLeaveChanges();
    closeLeaveDialog();
    const box = $("advanced-leave-confirm");
    if (box) {
      box.hidden = true;
      box.replaceChildren();
    }
    if (typeof target === "function") target();
    else if (!window.AgentNaveSettingsFrame?.navigate(target)) location.href = target;
  }

  // The app's web view does not show beforeunload prompts, so hold back in-app navigation.
  function renderLeaveConfirm() {
    const box = $("advanced-leave-confirm");
    const showAdvanced = Boolean(S.leaveTarget) && S.advancedChanges().length > 0 && ["orchestration", "advanced"].includes(S.currentTab);
    if (box) {
      box.hidden = !showAdvanced;
      if (!showAdvanced) box.replaceChildren();
      else if (!box.childElementCount) {
        const stay = el("button", { type: "button", onClick: () => { S.leaveTarget = null; renderLeaveConfirm(); } }, L("继续编辑", "Keep editing"));
        box.replaceChildren(
          el("p", {}, L("有未保存的修改，离开设置页会丢失这些修改。", "You have unsaved changes. Leaving Settings will discard them.")),
          el("div", { className: "btn-group" }, stay,
            el("button", { type: "button", className: "danger", onClick: proceedLeaveTarget }, L("放弃修改并离开", "Discard and leave"))));
        stay.focus({ preventScroll: true });
      }
    }
    const showDialog = Boolean(S.leaveTarget) && !showAdvanced;
    if (!showDialog) {
      closeLeaveDialog();
      return;
    }
    if (leaveDialog) return;
    const stay = el("button", { type: "button", onClick: () => { S.leaveTarget = null; renderLeaveConfirm(); } }, L("继续编辑", "Keep editing"));
    const leave = el("button", { type: "button", className: "danger", onClick: proceedLeaveTarget }, L("放弃修改并离开", "Discard and leave"));
    const body = S.busy
      ? L("正在保存设置，现在离开可能会丢失未完成的修改。", "Settings are still saving. Leaving now may lose unfinished changes.")
      : L("有未保存的修改或草稿，离开设置页会丢失这些内容。", "You have unsaved changes or drafts. Leaving Settings will discard them.");
    leaveDialog = el("dialog", { className: "upd leave-dialog", "aria-label": L("离开设置页", "Leave settings") },
      el("h2", { className: "upd-title" }, L("离开设置页", "Leave settings")),
      el("p", { className: "upd-p" }, body),
      el("div", { className: "upd-row" }, stay, leave));
    leaveDialog.addEventListener("cancel", (e) => {
      e.preventDefault();
      S.leaveTarget = null;
      renderLeaveConfirm();
    });
    document.body?.appendChild?.(leaveDialog);
    if (typeof leaveDialog.showModal === "function") leaveDialog.showModal();
    else leaveDialog.open = true;
    stay.focus?.({ preventScroll: true });
  }
  function confirmLeave(target) {
    if (!(unsaved() || S.busy)) return false;
    S.leaveTarget = target;
    // The confirmation sits in the save bar of the page holding the edits when advanced changes exist.
    if (S.advancedChanges().length > 0) {
      const tab = ["orchestration", "advanced"].includes(S.currentTab) ? S.currentTab
        : (S.orchestrationChanged() || window.AgentNaveDecisionSeats?.changed() || window.AgentNaveT3Seats?.seatsChanged()) ? "orchestration" : "advanced";
      if (S.currentTab !== tab) {
        setTab(tab);
        history.replaceState(null, "", `#${tab}`);
      }
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
      document.getElementById("model-names-example").setAttribute("aria-label", full
        ? L("显示为 opencode-go/deepseek-v4.1-flash，带上前面的路由", "Shown as opencode-go/deepseek-v4.1-flash, with the routing prefix")
        : L("显示为 deepseek-v4.1-flash，省去前面的路由 opencode-go/", "Shown as deepseek-v4.1-flash, without the opencode-go/ prefix"));
    }
    for (const [key, button] of Object.entries(buttons))
      button.addEventListener("click", () => { AN.setFullModelNames(key === "full"); show(); });
    show();
    // Another window (or the embedded frame's parent) may change it; the cards in view are drawn again.
    window.addEventListener("agentnave:model-names", () => {
      show();
      if (!$("pane-members").hidden) S.renderCards();
      if (!$("pane-advanced").hidden || !$("pane-orchestration").hidden) { S.renderReviewSeats(); S.renderOrchestration(); }
    });
  })();
}
