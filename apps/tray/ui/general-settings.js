"use strict";
(function () {
  const { h, $, L, load, post } = AN;
  const root = $("general-prefs");
  let data = null;
  let busy = false;

  const DEFAULTS = {
    theme: "system",
    motion: "system",
    scale: 100,
    autostart: false,
    open_on_start: true,
    tray_click: "panel",
    tray_count: true,
    keep_awake: false,
  };

  const ITEMS = [
    {
      key: "theme",
      label: () => L("主题", "Theme"),
      choices: () => [
        ["system", L("跟随系统", "System")],
        ["light", L("浅色", "Light")],
        ["dark", L("深色", "Dark")],
      ],
    },
    {
      key: "motion",
      label: () => L("减少动效", "Reduce motion"),
      choices: () => [
        ["system", L("跟随系统", "System")],
        ["reduce", L("开", "On")],
        ["full", L("关", "Off")],
      ],
    },
    {
      key: "scale",
      label: () => L("界面大小", "Interface scale"),
      choices: () => [
        [90, "90%"],
        [100, "100%"],
        [115, "115%"],
      ],
    },
    {
      key: "autostart",
      label: () => L("开机启动", "Launch at login"),
      kind: "switch",
    },
    {
      key: "open_on_start",
      label: () => L("启动时打开主窗口", "Open main window on launch"),
      kind: "switch",
    },
    {
      key: "tray_click",
      label: () => L("点托盘图标打开", "Clicking tray icon opens"),
      choices: () => [
        ["panel", L("托盘面板", "Tray panel")],
        ["office", L("办公室窗口", "Office window")],
      ],
    },
    {
      key: "tray_count",
      label: () => L("菜单栏显示工作人数", "Show active count in menu bar"),
      choices: () => [
        [true, L("显示", "Show")],
        [false, L("隐藏", "Hide")],
      ],
    },
    {
      key: "keep_awake",
      label: () => L("员工工作时不让电脑睡眠", "Prevent sleep while working"),
      kind: "switch",
    },
  ];

  function say(text, bad = false) {
    const notice = $("notice");
    if (!notice) return;
    notice.textContent = text;
    notice.classList.toggle("error", bad);
  }

  function effectiveValue(key) {
    const eff = data?.effective?.general;
    if (eff && Object.hasOwn(eff, key)) return eff[key];
    const pref = data?.preferences?.general;
    if (pref && Object.hasOwn(pref, key)) return pref[key];
    return DEFAULTS[key];
  }

  function applyAppearance(key, value) {
    if (key === "theme") {
      try { localStorage.setItem("agentnave.theme", value); } catch { /* ignore */ }
      AN.applyTheme?.();
    } else if (key === "motion") {
      if (value === "system") delete document.documentElement.dataset.motion;
      else document.documentElement.dataset.motion = value;
    } else if (key === "scale") {
      if (!value || value === 100) delete document.documentElement.dataset.scale;
      else document.documentElement.dataset.scale = String(value);
    }
  }

  async function save(key, value) {
    if (busy) return;
    busy = true;
    render();
    try {
      const res = await post("/app/preferences", { general: { [key]: value } });
      data = {
        ...data,
        ...res,
        preferences: res.preferences || {
          ...data?.preferences,
          general: { ...data?.preferences?.general, [key]: value },
        },
        effective: {
          ...data?.effective,
          ...res.effective,
          general: {
            ...data?.effective?.general,
            ...res.effective?.general,
            [key]: res.effective?.general?.[key] ?? value,
          },
        },
      };
      applyAppearance(key, effectiveValue(key));
      say(L("已保存", "Saved"));
    } catch (err) {
      say(err.message || L("保存失败", "Failed to save"), true);
    } finally {
      busy = false;
      render();
    }
  }

  function renderControl(item) {
    const val = effectiveValue(item.key);
    const label = item.label();
    const unavailable = item.key === "autostart" && data?.autostart_available === false;
    const disabled = busy || unavailable;
    if (item.kind === "switch") {
      const input = h("input.switch-input", {
        type: "checkbox",
        checked: Boolean(val),
        disabled,
        "aria-label": label,
      });
      input.addEventListener("change", () => save(item.key, input.checked));
      return input;
    }
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [choice, text] of item.choices()) {
      const active = val === choice;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled,
      }, text);
      btn.addEventListener("click", () => {
        if (!active) void save(item.key, choice);
      });
      control.append(btn);
    }
    return control;
  }

  function render() {
    if (!root || !data) return;
    root.replaceChildren(
      ...ITEMS.map((item) => h("div.setting-row", {}, h("h2", {}, item.label()), renderControl(item))),
    );
  }

  async function refresh() {
    if (!root || busy) return;
    try {
      data = await load("/app/preferences");
      if (data.error) say(data.error, true);
      applyAppearance("theme", effectiveValue("theme"));
      applyAppearance("motion", effectiveValue("motion"));
      applyAppearance("scale", effectiveValue("scale"));
      render();
    } catch (err) {
      say(err.message || L("无法读取常规设置", "Failed to load general settings"), true);
    }
  }

  const visible = () => !$("pane-general")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveGeneralSettings = { load: refresh };
  if (!$("pane-general")?.hidden) void refresh();
})();
