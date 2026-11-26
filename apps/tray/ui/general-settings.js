"use strict";
{
  const { h, $, L, load, post, shellOS } = AN;
  // Each group of rows fills the card the page gives it.
  const roots = { main: $("general-main"), motion: $("general-motion"), launch: $("general-launch") };
  const root = roots.main;
  let data = null;
  let busy = false;
  let generation = 0;
  let pending = null; // { key, value } shown immediately while a save is in flight
  const rows = new Map(); // item.key -> { row, labelEl, control, input, buttons, thumb, choicesKey }

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
    battery_still: false,
    pause_unfocused: true,
  };

  const ITEMS = [
    {
      key: "theme",
      group: "main",
      label: () => L("主题", "Theme"),
      choices: () => [
        ["system", L("跟随系统", "System")],
        ["light", L("浅色", "Light")],
        ["dark", L("深色", "Dark")],
      ],
    },
    {
      key: "motion",
      group: "main",
      label: () => L("减少动效", "Reduce motion"),
      choices: () => [
        ["system", L("跟随系统", "System")],
        ["reduce", L("开", "On")],
        ["full", L("关", "Off")],
      ],
    },
    {
      key: "battery_still",
      group: "motion",
      label: () => L("用电池时暂停动画", "Pause animation on battery"),
      kind: "switch",
    },
    {
      key: "pause_unfocused",
      group: "motion",
      label: () => L("窗口失焦时暂停动画", "Pause animation when unfocused"),
      kind: "switch",
    },
    {
      key: "scale",
      group: "main",
      label: () => L("界面大小", "Interface scale"),
      choices: () => [
        [90, "90%"],
        [100, "100%"],
        [115, "115%"],
      ],
    },
    {
      key: "autostart",
      group: "launch",
      label: () => L("开机启动", "Launch at login"),
      kind: "switch",
    },
    {
      key: "open_on_start",
      group: "launch",
      label: () => L("启动时打开主窗口", "Open main window on launch"),
      kind: "switch",
    },
    {
      key: "tray_click",
      group: "launch",
      label: () => L("点托盘图标打开", "Clicking tray icon opens"),
      choices: () => [
        ["panel", L("托盘面板", "Tray panel")],
        ["office", L("办公室窗口", "Office window")],
      ],
    },
    {
      key: "tray_count",
      group: "launch",
      macOnly: true,
      label: () => L("菜单栏显示工作人数", "Show active count in menu bar"),
      kind: "switch",
    },
    {
      key: "tray_font",
      group: "launch",
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
      group: "launch",
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
    if (pending && pending.key === key) return pending.value;
    const eff = data?.effective?.general;
    if (eff && Object.hasOwn(eff, key)) return eff[key];
    const pref = data?.preferences?.general;
    if (pref && Object.hasOwn(pref, key)) return pref[key];
    return DEFAULTS[key];
  }

  // appearance.js applies theme and motion on every page; the App zooms the window itself.
  function applyAppearance() {
    const detail = { theme: effectiveValue("theme"), motion: effectiveValue("motion"), battery_still: effectiveValue("battery_still"), pause_unfocused: effectiveValue("pause_unfocused") };
    window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail }));
  }

  async function save(key, value) {
    if (busy) return;
    busy = true;
    pending = { key, value };
    render();
    const expected = generation;
    try {
      const res = await post("/app/preferences", { general: { [key]: value } });
      if (generation === expected) {
        pending = null;
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
      pending = null;
      busy = false;
      render();
    }
  }

  function updateRow(item) {
    const val = effectiveValue(item.key);
    const label = item.label();
    const unavailable = item.key === "autostart" && data?.autostart_available !== true;
    const disabled = busy || unavailable;
    let rec = rows.get(item.key);
    if (!rec) {
      const labelEl = h("span.s-label", {}, label);
      rec = { row: h("div.s-row", {}, labelEl), labelEl, label, control: null, input: null, buttons: [], thumb: null, choicesKey: "" };
      rows.set(item.key, rec);
    } else if (rec.label !== label) {
      rec.label = label;
      rec.labelEl.textContent = label;
    }
    if (item.kind === "switch") {
      if (!rec.input) {
        const input = h("input.switch-input", { type: "checkbox", checked: Boolean(val), disabled, "aria-label": label });
        input.addEventListener("change", () => save(item.key, input.checked));
        rec.input = input;
        rec.control = input;
        rec.row.append(input);
      } else {
        rec.input.checked = Boolean(val);
        rec.input.disabled = disabled;
        rec.input.setAttribute("aria-label", label);
      }
      return rec.row;
    }
    const choices = item.choices();
    const choicesKey = choices.map(([c, t]) => `${c}:${t}`).join("|");
    if (!rec.control || rec.choicesKey !== choicesKey) {
      const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
      const buttons = [];
      for (const [choice, text, style] of choices) {
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
        buttons.push({ choice, btn });
      }
      if (rec.control) rec.control.replaceWith(control);
      else rec.row.append(control);
      rec.control = control;
      rec.buttons = buttons;
      rec.choicesKey = choicesKey;
      rec.thumb = globalThis.ANMotion?.attachThumb?.(control, ".segment-btn.active") || null;
      rec.thumb?.sync(true);
    } else {
      rec.control.setAttribute("aria-label", label);
      for (const { choice, btn } of rec.buttons) {
        const active = val === choice;
        btn.classList.toggle("active", active);
        btn.setAttribute("aria-checked", String(active));
        btn.disabled = disabled;
      }
      rec.thumb?.sync(false);
    }
    return rec.row;
  }

  function render() {
    if (!root || !data) return;
    // Only the macOS menu bar shows the count beside the icon.
    const showCount = Boolean(effectiveValue("tray_count"));
    const shown = ITEMS.filter((item) => (!item.macOnly || shellOS() === "macos") && (item.key !== "tray_font" || showCount));
    for (const [name, box] of Object.entries(roots)) {
      if (!box) continue;
      const nodes = shown.filter((item) => item.group === name).map(updateRow);
      if (typeof AN.reconcile === "function" && typeof box.insertBefore === "function") AN.reconcile(box, nodes);
      else box.replaceChildren(...nodes);
    }
    for (const item of shown) rows.get(item.key)?.thumb?.sync(false);
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

  const visible = () => (!$("pane-general")?.hidden || !$("pane-avatars")?.hidden) && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveGeneralSettings = { load: refresh };
  if (!$("pane-general")?.hidden || !$("pane-avatars")?.hidden) void refresh();
}
