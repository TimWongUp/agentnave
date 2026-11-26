"use strict";
// Custom time range popover for the Usage page (dates + whole hours + quick presets).
// All DOM nodes are built via AN.h with zero inline style attributes.
window.UsagePicker = (() => {
  const { h, L } = AN;
  const HOUR_MS = 3600000;
  const DAY_MS = 86400000;
  const pad = n => String(n).padStart(2, "0");

  const toDayKey = ms => {
    const dt = new Date(ms);
    return Date.UTC(dt.getFullYear(), dt.getMonth(), dt.getDate()) / DAY_MS;
  };
  const dayToYMD = key => {
    const dt = new Date(key * DAY_MS);
    return [dt.getUTCFullYear(), dt.getUTCMonth(), dt.getUTCDate()];
  };
  const localDayMs = (key, hour) => {
    const [y, m, d] = dayToYMD(key);
    return new Date(y, m, d, hour, 0, 0, 0).getTime();
  };
  const formatMD = key => {
    const [, m, d] = dayToYMD(key);
    return `${pad(m + 1)}/${pad(d)}`;
  };
  const monthKey = (y, m) => y * 12 + m;
  const fromMonthKey = mk => [Math.floor(mk / 12), ((mk % 12) + 12) % 12];

  function formatCustomLabel(custom) {
    if (!custom) return L("自定义", "Custom");
    if (custom.kind === "preset90d") return L("近 90 天", "90 days");
    if (custom.kind === "hours") return L(`近 ${custom.n} 小时`, `Last ${custom.n}h`);
    if (custom.labelZh && custom.labelEn) return L(custom.labelZh, custom.labelEn);
    const timed = custom.startHour !== 0 || custom.endHour !== 24;
    const fromTxt = formatMD(custom.fromDay);
    const toTxt = formatMD(custom.toDay);
    const t0 = `${pad(custom.startHour)}:00`, t1 = `${pad(custom.endHour)}:00`;
    if (custom.fromDay === custom.toDay) return timed ? `${fromTxt} ${t0}–${t1}` : fromTxt;
    return timed ? `${fromTxt} ${t0}–${toTxt} ${t1}` : `${fromTxt}–${toTxt}`;
  }

  function buildHourSelect(id, fromH, toH, selected, labelText) {
    const sel = h("select.u-hour-sel", { id, "aria-label": labelText });
    for (let hr = fromH; hr <= toH; hr++) {
      const opt = h("option", { value: String(hr) }, `${pad(hr)}:00`);
      if (hr === selected) opt.selected = true;
      sel.append(opt);
    }
    sel.value = String(selected);
    return sel;
  }

  function quickSpecs(todayKey) {
    const [ty, tm] = dayToYMD(todayKey);
    const yest = todayKey - 1;
    const monthStart = Date.UTC(ty, tm, 1) / DAY_MS;
    const prevStart = Date.UTC(ty, tm - 1, 1) / DAY_MS;
    const prevEnd = Date.UTC(ty, tm, 0) / DAY_MS;
    return [
      {
        id: "yesterday", zh: "昨天", en: "Yesterday",
        spec: () => ({
          kind: "days", presetId: "yesterday", labelZh: "昨天", labelEn: "Yesterday",
          fromDay: yest, toDay: yest, startHour: 0, endHour: 24,
          fromMs: localDayMs(yest, 0), toMs: localDayMs(yest, 24),
        }),
      },
      {
        id: "this-month", zh: "本月", en: "This month",
        spec: () => ({
          kind: "days", presetId: "this-month", labelZh: "本月", labelEn: "This month",
          fromDay: monthStart, toDay: todayKey, startHour: 0, endHour: 24,
          fromMs: localDayMs(monthStart, 0), toMs: localDayMs(todayKey, 24),
        }),
      },
      {
        id: "last-month", zh: "上月", en: "Last month",
        spec: () => ({
          kind: "days", presetId: "last-month", labelZh: "上月", labelEn: "Last month",
          fromDay: prevStart, toDay: prevEnd, startHour: 0, endHour: 24,
          fromMs: localDayMs(prevStart, 0), toMs: localDayMs(prevEnd, 24),
        }),
      },
      {
        id: "90d", zh: "近 90 天", en: "90 days",
        spec: () => ({ kind: "preset90d", presetId: "90d", labelZh: "近 90 天", labelEn: "90 days" }),
      },
    ];
  }

  function renderCalendarGrid(calBox, state, bounds) {
    const { todayKey, minKey } = bounds;
    const [y, m] = fromMonthKey(state.mk);
    const [minY, minM] = dayToYMD(minKey);
    const [maxY, maxM] = dayToYMD(todayKey);
    const minMk = monthKey(minY, minM), maxMk = monthKey(maxY, maxM);
    const firstDow = (new Date(Date.UTC(y, m, 1)).getUTCDay() + 6) % 7;
    const daysInMonth = new Date(Date.UTC(y, m + 1, 0)).getUTCDate();
    const weekHeaders = L("一,二,三,四,五,六,日", "Mo,Tu,We,Th,Fr,Sa,Su").split(",").map(d => h("span", {}, d));
    const blanks = Array.from({ length: firstDow }, () => h("i"));
    const dayButtons = Array.from({ length: daysInMonth }, (_, idx) => {
      const d = idx + 1, key = Date.UTC(y, m, d) / DAY_MS;
      const btn = h("button.u-day", {
        type: "button", "data-day": String(key),
        "aria-label": L(`${y} 年 ${m + 1} 月 ${d} 日`, `${y}-${pad(m + 1)}-${pad(d)}`),
        disabled: key < minKey || key > todayKey,
      }, String(d));
      if (key === todayKey) btn.classList.add("today");
      return btn;
    });
    // Pad to 42 slots (6 full weeks) so month transitions never jump popover height.
    const trailing = Array.from({ length: Math.max(0, 42 - firstDow - daysInMonth) }, () => h("i"));
    const prevBtn = h("button", { type: "button", "data-month": "-1", "aria-label": L("上个月", "Previous month"), disabled: state.mk <= minMk }, "‹");
    const nextBtn = h("button", { type: "button", "data-month": "1", "aria-label": L("下个月", "Next month"), disabled: state.mk >= maxMk }, "›");
    const title = L(`${y} 年 ${m + 1} 月`, new Date(Date.UTC(y, m, 1)).toLocaleDateString("en-US", { month: "short", year: "numeric", timeZone: "UTC" }));
    const grid = h("div.u-cal-grid", {}, ...weekHeaders, ...blanks, ...dayButtons, ...trailing);
    calBox.replaceChildren(h("div.u-cal-head", {}, prevBtn, h("b", {}, title), nextBtn), grid);
    return { dayButtons, grid };
  }

  function paintRange(dayButtons, state) {
    const end = state.to !== null ? state.to : state.preview !== null ? state.preview : state.from;
    const lo = Math.min(state.from, end), hi = Math.max(state.from, end);
    for (const btn of dayButtons) {
      const d = Number(btn.getAttribute?.("data-day") ?? btn.dataset?.day);
      const on = state.from !== null && d >= lo && d <= hi;
      btn.classList.toggle("from", on && d === lo);
      btn.classList.toggle("to", on && d === hi);
      btn.classList.toggle("edge", on && (d === lo || d === hi));
      btn.classList.toggle("in", on && d !== lo && d !== hi);
      btn.setAttribute("aria-pressed", String(on));
    }
  }

  function create({ pop, app, anchor, customBtn, getNowMs, getMinTsMs, getCurrent, onApply }) {
    const picker = { mk: 0, from: 0, to: 0, preview: null, startHour: 0, endHour: 24 };
    let calBox = null, nInput = null, t0Sel = null, t1Sel = null, dayButtons = [];

    const bounds = () => {
      const nowMs = Number(getNowMs()) || Date.now();
      const todayKey = toDayKey(nowMs), winMin = todayKey - 89, rawMin = getMinTsMs();
      const minKey = rawMin != null && Number.isFinite(Number(rawMin))
        ? Math.max(winMin, Math.min(todayKey, toDayKey(Number(rawMin))))
        : winMin;
      return { nowMs, todayKey, minKey };
    };
    const redrawCal = (dir = 0) => {
      const res = renderCalendarGrid(calBox, picker, bounds());
      dayButtons = res.dayButtons;
      paintRange(dayButtons, picker);
      if (dir) globalThis.ANMotion?.reveal?.(res.grid, { dx: dir * 10, dy: 0, duration: 150 });
    };
    const place = () => {
      if (pop.hidden) return;
      const a = app?.getBoundingClientRect?.(), s = anchor?.getBoundingClientRect?.();
      if (!a || !s || !a.width) return;
      const width = pop.offsetWidth || 404;
      pop.style.top = `${Math.max(0, s.bottom - a.top + 6)}px`;
      pop.style.left = `${Math.max(0, Math.min(a.width - width, s.right - a.left - width))}px`;
    };
    const close = (refocus = false) => {
      if (pop.hidden) return;
      pop.hidden = true;
      customBtn?.setAttribute("aria-expanded", "false");
      if (refocus) customBtn?.focus?.();
    };
    const applyHours = () => {
      const raw = String(nInput?.value || "").trim() || nInput?.getAttribute("placeholder") || "48";
      const n = Number(raw), ok = /^\d+$/.test(raw) && n >= 1 && n <= 720;
      nInput?.classList.toggle("bad", !ok);
      if (!ok) return;
      const dt = new Date(Number(getNowMs()) || Date.now());
      dt.setMinutes(0, 0, 0);
      const end = dt.getTime() + HOUR_MS;
      close(false);
      onApply({ kind: "hours", n, fromMs: end - n * HOUR_MS, toMs: end });
    };
    const applyCustom = () => {
      const { nowMs } = bounds(), fromDay = picker.from, toDay = picker.to === null ? picker.from : picker.to;
      const startHour = Number(t0Sel?.value ?? picker.startHour), endHour = Number(t1Sel?.value ?? picker.endHour);
      const badSame = fromDay === toDay && endHour <= startHour;
      const fromMs = localDayMs(fromDay, startHour), toMs = localDayMs(toDay, endHour);
      const badFuture = fromMs >= nowMs;
      t0Sel?.classList.toggle("bad", badFuture);
      t1Sel?.classList.toggle("bad", badSame);
      if (badSame || badFuture) return;
      close(false);
      onApply({ kind: "days", fromDay, toDay, startHour, endHour, fromMs, toMs });
    };
    const open = () => {
      const { todayKey, minKey } = bounds(), cur = getCurrent() || {}, c = cur.range === "custom" ? cur.custom : null;
      const curFrom = cur.fromMs != null ? toDayKey(cur.fromMs) : todayKey - 6;
      const curTo = cur.toMs != null ? toDayKey(Math.max(cur.fromMs || 0, cur.toMs - 1)) : todayKey;
      picker.from = Math.max(minKey, Math.min(todayKey, c?.fromDay ?? curFrom));
      picker.to = Math.max(picker.from, Math.min(todayKey, c?.toDay ?? curTo));
      picker.preview = null;
      picker.startHour = c?.kind === "days" ? c.startHour : 0;
      picker.endHour = c?.kind === "days" ? c.endHour : 24;
      const [ty, tm] = dayToYMD(picker.to);
      picker.mk = monthKey(ty, tm);

      const quicks = quickSpecs(todayKey);
      const quickCol = h("div.u-quick", {}, ...quicks.map((item, idx) =>
        h("button", { type: "button", "data-quick": String(idx), "aria-pressed": String(c?.presetId === item.id) }, L(item.zh, item.en))));
      nInput = h("input.u-field", {
        id: "u-nhours", inputmode: "numeric", maxlength: "3", placeholder: "48",
        "aria-label": L("小时数，1 到 720", "Hours, 1 to 720"), value: c?.kind === "hours" ? String(c.n) : "",
      });
      quickCol.append(h("div.u-nhours", {}, L("近 ", "Last "), nInput, L(" 小时", " h"), h("button.u-mini", { type: "button", id: "u-napply" }, L("应用", "Apply"))));
      calBox = h("div.u-cal");
      t0Sel = buildHourSelect("u-t0", 0, 23, picker.startHour, L("开始整点", "Start hour"));
      t1Sel = buildHourSelect("u-t1", 1, 24, picker.endHour, L("结束整点", "End hour"));
      pop.replaceChildren(quickCol, calBox, h("div.u-pop-foot", {},
        h("label", {}, L("起", "From"), t0Sel), h("label", {}, L("止", "To"), t1Sel),
        h("button.u-apply", { type: "button", id: "u-apply" }, L("应用", "Apply"))));
      redrawCal();
      pop.hidden = false;
      customBtn?.setAttribute("aria-expanded", "true");
      place();
    };

    pop.addEventListener("click", e => {
      e.stopPropagation?.();
      const btn = e.target?.closest?.("button");
      if (!btn || btn.disabled) return;
      const quickIdx = btn.getAttribute?.("data-quick") ?? btn.dataset?.quick;
      if (quickIdx != null) {
        const spec = quickSpecs(bounds().todayKey)[Number(quickIdx)]?.spec();
        if (spec) { close(false); onApply(spec); }
        return;
      }
      const day = btn.getAttribute?.("data-day") ?? btn.dataset?.day;
      if (day != null) {
        const k = Number(day);
        if (picker.to !== null || picker.from === null) { picker.from = k; picker.to = null; }
        else { picker.to = Math.max(picker.from, k); picker.from = Math.min(picker.from, k); }
        picker.preview = null;
        t0Sel?.classList.toggle("bad", false);
        t1Sel?.classList.toggle("bad", false);
        paintRange(dayButtons, picker);
        return;
      }
      const monthDelta = btn.getAttribute?.("data-month") ?? btn.dataset?.month;
      if (monthDelta != null) { const d = Number(monthDelta); picker.mk += d; redrawCal(d); return; }
      if (btn.id === "u-apply" || btn.attributes?.id === "u-apply") applyCustom();
      else if (btn.id === "u-napply" || btn.attributes?.id === "u-napply") applyHours();
    });
    pop.addEventListener("pointerover", e => {
      const day = e.target?.closest?.(".u-day");
      if (!day || day.disabled || picker.to !== null) return;
      picker.preview = Number(day.getAttribute?.("data-day") ?? day.dataset?.day);
      paintRange(dayButtons, picker);
    });
    pop.addEventListener("keydown", e => {
      if (e.key !== "Enter") return;
      if (e.target === nInput || e.target?.id === "u-nhours") { e.preventDefault(); applyHours(); }
      else if (e.target === t0Sel || e.target === t1Sel) { e.preventDefault(); applyCustom(); }
    });
    pop.addEventListener("input", e => e.target?.classList?.toggle?.("bad", false));
    pop.addEventListener("change", () => { t0Sel?.classList?.toggle?.("bad", false); t1Sel?.classList?.toggle?.("bad", false); });

    return { open, close, place, isOpen: () => !pop.hidden };
  }

  return { create, formatCustomLabel, toDayKey, localDayMs };
})();
