"use strict";
// The About pane's automatic update checks: off, or every 4, 12 or 24 hours. The choice is stored
// with the App's preferences, while "立即检查" always works. The App reads the same fields before
// its own checks.
(function () {
  const { h, L, load, post } = AN;
  const root = document.getElementById("update-auto-check");
  if (!root) return;
  const notice = document.getElementById("notice");
  const label = L("自动检查更新", "Check for updates automatically");
  const CHOICES = [[0, L("关闭", "Off")], ...[4, 12, 24].map((hours) => [hours, L(`${hours} 小时`, `${hours} hours`)])];
  let saved = 24; // Hours between checks, 0 for off. No setting yet means daily, like the App before this choice existed.
  let busy = true;
  const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
  const buttons = CHOICES.map(([hours, text]) => {
    const button = h("button.segment-btn", { type: "button", role: "radio" }, text);
    button.addEventListener("click", () => choose(hours));
    control.append(button);
    return [hours, button];
  });
  root.replaceChildren(h("span.update-auto-label", {}, label), control);

  function render() {
    for (const [hours, button] of buttons) {
      const active = hours === saved;
      button.classList.toggle("active", active);
      button.setAttribute("aria-checked", String(active));
      button.disabled = busy;
    }
  }

  // The saved value arrives in a moment; show what the App does until then.
  render();

  function say(message, error) {
    if (!notice) return;
    notice.textContent = message;
    notice.classList.toggle("error", Boolean(error));
  }

  // Share the same API path as the other settings pages.
  const preferences = payload => payload === undefined ? load("/app/preferences") : post("/app/preferences", payload);

  function chosen(data) {
    const updates = data.preferences?.updates;
    if (updates?.auto_check === false) return 0;
    return [4, 12, 24].includes(updates?.check_hours) ? updates.check_hours : 24;
  }

  async function choose(hours) {
    if (busy || hours === saved) return;
    busy = true;
    render();
    try {
      saved = chosen(await preferences({ updates: hours ? { auto_check: true, check_hours: hours } : { auto_check: false } }));
      say(saved
        ? L(`已设为每 ${saved} 小时自动检查更新`, `Updates are checked every ${saved} hours`)
        : L("已关闭自动检查更新；仍可手动检查", "Automatic update checks are off; you can still check manually"));
    } catch (failure) {
      // Show the saved choice again, and say why the click did not hold.
      say(failure.message, true);
    } finally {
      busy = false;
      render();
    }
  }

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
  // The pane's own check on opening obeys the same setting as the App's.
  window.AgentNaveAboutSettings = { automatic: async () => { await ready; return saved !== 0; } };
})();
