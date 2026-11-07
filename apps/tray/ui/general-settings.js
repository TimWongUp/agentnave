"use strict";
{
  const { h, $, L, load, post, shellOS } = AN;
  const root = $("general-prefs");
  let data = null;
  let busy = false;
  let generation = 0;

  const DEFAULTS = {
    theme: "system",
    motion: "system",
    scale: 100,
    autostart: false,
    open_on_start: true,
    tray_click: "panel",
    tray_count: true,
    tray_font: "baskerville",
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
      macOnly: true,
      label: () => L("菜单栏显示工作人数", "Show active count in menu bar"),
      choices: () => [
        [true, L("显示", "Show")],
        [false, L("隐藏", "Hide")],
      ],
    },
    {
      key: "tray_font",
      macOnly: true,
      label: () => L("菜单栏数字字体", "Menu bar count font"),
      choices: () => [
        ["futura", "Futura", "font-family: Futura; font-weight: 500"],
        ["baskerville", "Baskerville", "font-family: Baskerville; font-style: italic"],
        ["typewriter", "Typewriter", "font-family: 'American Typewriter'"],
        ["marker", "Marker", "font-family: 'Marker Felt'"],
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

  // appearance.js applies theme and motion on every page; the App zooms the window itself.
  function applyAppearance() {
    const detail = { theme: effectiveValue("theme"), motion: effectiveValue("motion") };
    window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail }));
  }

  async function save(key, value) {
    if (busy) return;
    busy = true;
    render();
    const expected = generation;
    try {
      const res = await post("/app/preferences", { general: { [key]: value } });
      if (generation === expected) {
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
        applyAppearance();
      }
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
    const unavailable = item.key === "autostart" && data?.autostart_available !== true;
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
    // A third entry styles that choice, e.g. in its own font.
    for (const [choice, text, style] of item.choices()) {
      const active = val === choice;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled,
        ...(style && { style }),
      }, text);
      btn.addEventListener("click", () => {
        if (effectiveValue(item.key) !== choice) void save(item.key, choice);
      });
      control.append(btn);
    }
    return control;
  }

  function render() {
    if (!root || !data) return;
    root.replaceChildren(
      // Only the macOS menu bar shows the count beside the icon.
      ...ITEMS.filter((item) => !item.macOnly || shellOS() === "macos").map((item) => h("div.setting-row", {}, h("h2", {}, item.label()), renderControl(item))),
    );
  }

  async function refresh() {
    if (!root || busy) return;
    const expected = generation;
    try {
      const next = await load("/app/preferences");
      // A shortcut/native event received during this GET is newer than its snapshot.
      if (generation !== expected) {
        next.preferences = { ...next.preferences, general: { ...next.preferences?.general, ...data?.preferences?.general } };
        next.effective = { ...next.effective, general: { ...next.effective?.general, ...data?.effective?.general } };
      }
      data = next;
      if (data.error) say(data.error, true);
      render();
    } catch (err) {
      say(err.message || L("无法读取常规设置", "Failed to load general settings"), true);
    }
  }

  window.addEventListener("agentnave:preferences", ({ detail }) => {
    if (!detail) return;
    generation++;
    data = {
      ...data,
      preferences: { ...data?.preferences, general: { ...data?.preferences?.general, ...detail } },
      effective: { ...data?.effective, general: { ...data?.effective?.general, ...detail } },
    };
    render();
  });

  const visible = () => !$("pane-general")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveGeneralSettings = { load: refresh };
  if (!$("pane-general")?.hidden) void refresh();
}
