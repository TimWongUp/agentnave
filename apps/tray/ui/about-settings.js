"use strict";
// The About pane's automatic update checks. The choice is stored with the App's preferences,
// while "立即检查" always works. The App reads the same field before its own daily check.
(function () {
  const { h, L, load, post } = AN;
  const root = document.getElementById("update-auto-check");
  if (!root) return;
  const notice = document.getElementById("notice");
  const label = L("自动检查更新", "Check for updates automatically");
  let saved = true; // No setting yet means on, like the App before this choice existed.
  let busy = true;
  const input = h("input.switch-input", {
    type: "checkbox",
    id: "updates-auto-check",
    "aria-label": label,
    disabled: true,
  });
  root.replaceChildren(h("label", { for: "updates-auto-check" }, label), input);

  function render() {
    input.checked = saved;
    input.disabled = busy;
  }

  // The saved value arrives in a moment; show what the App does until then.
  render();

  function say(message, error) {
    if (!notice) return;
    notice.textContent = message;
    notice.classList.toggle("error", Boolean(error));
  }

  // Through AN, so the browser preview's mock answers too.
  const preferences = payload => payload === undefined ? load("/app/preferences") : post("/app/preferences", payload);

  function chosen(data) {
    return data.preferences?.updates?.auto_check !== false;
  }

  input.addEventListener("change", async () => {
    if (busy) return;
    const wanted = input.checked;
    busy = true;
    input.disabled = true;
    try {
      saved = chosen(await preferences({ updates: { auto_check: wanted } }));
      say(saved
        ? L("已开启自动检查更新", "Automatic update checks are on")
        : L("已关闭自动检查更新；仍可手动检查", "Automatic update checks are off; you can still check manually"));
    } catch (failure) {
      // Show the saved choice again, and say why the click did not hold.
      say(failure.message, true);
    } finally {
      busy = false;
      render();
    }
  });

  const ready = (async () => {
    try {
      saved = chosen(await preferences());
    } catch (failure) {
      say(L("无法读取设置：", "Could not read the setting: ") + failure.message, true);
    } finally {
      busy = false;
      render();
    }
  })();
  // The pane's own check on opening obeys the same switch as the App's daily one.
  window.AgentNaveAboutSettings = { automatic: async () => { await ready; return saved; } };
})();
