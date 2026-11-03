"use strict";
(function () {
  const { h, $, L, load, post } = AN;
  const root = $("general-language");
  let current = document.documentElement.dataset.lang || "system";
  let busy = false;

  const CHOICES = () => [
    ["system", L("跟随系统", "System")],
    ["zh", "中文"],
    ["en", "English"],
  ];

  function say(text, bad = false) {
    const notice = $("notice");
    if (!notice) return;
    notice.textContent = text;
    notice.classList.toggle("error", bad);
  }

  async function save(language) {
    if (busy) return;
    busy = true;
    render();
    try {
      await post("/app/preferences", { general: { language } });
      current = language;
      location.reload();
    } catch (err) {
      say(err.message || L("保存失败", "Failed to save"), true);
      busy = false;
      render();
    }
  }

  function render() {
    if (!root) return;
    const label = L("界面语言", "Interface language");
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [choice, text] of CHOICES()) {
      const active = current === choice;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled: busy,
      }, text);
      btn.addEventListener("click", () => {
        if (!active) void save(choice);
      });
      control.append(btn);
    }
    root.replaceChildren(h("div.setting-row", {}, h("h2", {}, label), control));
  }

  async function refresh() {
    if (!root || busy) return;
    render();
    try {
      const data = await load("/app/preferences");
      current = data?.effective?.general?.language || data?.preferences?.general?.language || "system";
      render();
    } catch {
      // Keep the language from <html data-lang>; general-settings.js reports load errors.
    }
  }

  const visible = () => !$("pane-general")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveLanguageSettings = { load: refresh };
  render();
  if (!$("pane-general")?.hidden) void refresh();
})();
