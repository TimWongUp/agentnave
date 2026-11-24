"use strict";
{
  // Shared Settings pieces for Orchestration and Review Settings:
  // 1) In-memory "General / T3 Code" scope switch shared by both pages.
  // 2) Single-card expandable seat rows shared by Decision-model seats and T3 Code seats.

  const S = AN.settings ||= {};
  const { $, el } = S;
  const L = AN.L || ((zh) => zh);

  let scope = "general"; // "general" | "t3", kept in memory only.
  const t3Visible = () => Boolean(window.AgentNaveT3Seats?.visible?.());
  const orchestrationScope = () => (t3Visible() ? scope : "general");

  function scopeSwitch(page, generalDirty, t3Dirty) {
    const active = orchestrationScope();
    const unsavedTitle = L("有未保存的修改", "Unsaved changes");
    const choices = [
      ["general", L("通用", "General"), Boolean(generalDirty)],
      ["t3", "T3 Code", Boolean(t3Dirty)],
    ];
    return el("div", {
      className: "segment-control",
      role: "radiogroup",
      "aria-label": L("配置范围", "Configuration scope"),
    }, ...choices.map(([id, text, dirty]) => {
      const selected = active === id;
      return el("button", {
        type: "button",
        className: `segment-btn${selected ? " active" : ""}`,
        role: "radio",
        "aria-checked": String(selected),
        dataset: { scope: id, focusKey: `scope/${page}/${id}` },
        onClick: () => setOrchestrationScope(id),
      }, text, el("span", { className: "badge-dot", title: unsavedTitle, hidden: selected || !dirty }));
    }));
  }

  function renderScopeBox(box, page, generalDirty, t3Dirty, show) {
    if (!box) return;
    const focusKey = document.activeElement?.dataset?.focusKey;
    const keepFocus = typeof focusKey === "string" && focusKey.startsWith(`scope/${page}/`);
    box.hidden = !show;
    if (!show) return box.replaceChildren();
    box.replaceChildren(scopeSwitch(page, generalDirty, t3Dirty));
    if (keepFocus) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  function syncScopeViews() {
    const show = t3Visible();
    const isT3 = show && orchestrationScope() === "t3";

    const orchGeneralDirty = Boolean(S.orchestrationChanged?.() || window.AgentNaveDecisionSeats?.changed?.());
    const orchT3Dirty = Boolean(window.AgentNaveT3Seats?.seatsChanged?.());
    renderScopeBox($("orchestration-scope"), "orchestration", orchGeneralDirty, orchT3Dirty, show);
    const orchGeneral = $("orchestration-general");
    if (orchGeneral) orchGeneral.hidden = isT3;
    const t3Section = $("t3-seats-section");
    if (t3Section) t3Section.hidden = !isT3;

    const revGeneralDirty = Boolean(S.reviewSeatsChanged?.());
    const revT3Dirty = Boolean(window.AgentNaveT3Seats?.reviewChanged?.());
    renderScopeBox($("review-scope"), "review", revGeneralDirty, revT3Dirty, show);
    const revGeneral = $("review-seats-group");
    if (revGeneral) revGeneral.hidden = isT3;
    const t3Review = $("t3-review-seats");
    if (t3Review) t3Review.hidden = !isT3;
  }

  function setOrchestrationScope(next) {
    const wanted = next === "t3" ? "t3" : "general";
    if (wanted === scope) return;
    scope = wanted;
    syncScopeViews();
  }

  // ---- Summary helpers ----

  function summarizeTiers(entries) {
    const parts = entries.filter(([, model]) => Boolean(model)).map(([label, model]) => `${label} ${model}`);
    return parts.length
      ? { text: parts.join(" · "), unset: false }
      : { text: L("未设置", "Not set"), unset: true };
  }

  function summarizeNames(names) {
    const clean = names.map((one) => (one || "").trim()).filter(Boolean);
    return clean.length
      ? { text: clean.join(" · "), unset: false }
      : { text: L("未设置", "Not set"), unset: true };
  }

  // ---- Expandable single-card seat rows ----

  function seatRowItem(item, expanded, onToggle) {
    const isOpen = Boolean(item.invalid || expanded.has(item.cid));
    const toggle = () => {
      if (item.isInvalid ? item.isInvalid() : item.invalid) return;
      if (expanded.has(item.cid)) expanded.delete(item.cid);
      else expanded.add(item.cid);
      onToggle(item.cid, expanded.has(item.cid));
    };

    const nameEl = el("span", { className: `seat-row-name${item.nameUnset ? " is-unset" : ""}` }, item.name);
    const toggleBtn = el("button", {
      type: "button",
      className: "seat-row-toggle",
      "aria-expanded": String(isOpen),
      dataset: { focusKey: `${item.focusPrefix}/toggle` },
      onClick: toggle,
    }, el("span", { className: "seat-row-chevron", "aria-hidden": "true" }), nameEl);

    const summaryEl = el("span", {
      className: `seat-row-summary${item.summaryUnset ? " is-unset" : ""}`,
      title: item.summary,
    }, item.summary);

    const removeBtn = el("button", {
      type: "button",
      className: "ds-remove seat-row-remove",
      "aria-label": L(`删除${item.ariaTitle}`, `Delete ${item.ariaTitle}`),
      dataset: { focusKey: `${item.focusPrefix}/remove` },
      onClick: item.onRemove,
    }, L("删除", "Delete"));

    const head = el("div", {
      className: "seat-row-head",
      onClick: (e) => {
        if (e?.target?.closest?.("button, .setting-tip, input, select, a")) return;
        toggle();
      },
    },
    el("div", { className: "seat-row-lead" }, toggleBtn, item.tip ? S.settingTip(item.tip) : null),
    el("div", { className: "seat-row-meta" },
      el("span", { className: "seat-row-access" }, item.access),
      summaryEl),
    removeBtn);

    const helpers = {
      setName(text, unset = false) {
        nameEl.textContent = text;
        nameEl.className = `seat-row-name${unset ? " is-unset" : ""}`;
      },
      setSummary(text, unset = false) {
        summaryEl.textContent = text;
        summaryEl.className = `seat-row-summary${unset ? " is-unset" : ""}`;
        summaryEl.setAttribute("title", text);
      },
    };

    const body = isOpen ? el("div", { className: "seat-row-body" }, ...item.renderBody(helpers)) : null;
    return el("div", {
      className: `seat-row${isOpen ? " is-expanded" : ""}`,
      dataset: { cid: item.cid, expanded: String(isOpen), invalid: String(Boolean(item.invalid)) },
    }, head, body);
  }

  function renderSeatRows(items, { expanded, onToggle, groupClass = "" }) {
    if (!items.length) return null;
    const cls = `s-group seat-rows-group${groupClass ? ` ${groupClass}` : ""}`;
    return el("div", { className: cls },
      el("div", { className: "s-card seat-rows-card" },
        ...items.map((item) => seatRowItem(item, expanded, onToggle))));
  }

  function customSeatHeadRow({ key, role, access, title, accessOptions, roleMax, onRoleInput, onAccessChange }) {
    const roleInput = el("input", {
      type: "text", className: "ds-role", maxlength: String(roleMax), spellcheck: "false",
      autocorrect: "off", autocapitalize: "off", autocomplete: "off", value: role,
      "aria-label": L("专业", "Specialty"), placeholder: L("专业", "Specialty"),
      dataset: { focusKey: `${key}/role` },
      onInput: (e) => onRoleInput(e.target.value),
    });
    const accessSelect = el("select", {
      dataset: { focusKey: `${key}/access` },
      "aria-label": L(`${title}的读写类型`, `Access of ${title}`),
      onChange: (e) => onAccessChange(e.target.value),
    }, accessOptions.map(([v, text]) => el("option", { value: v, selected: v === access }, text)));
    return el("div", { className: "s-row is-sub seat-row-custom" },
      el("span", { className: "s-label" }, L("专业与读写", "Specialty & access")),
      el("div", { className: "s-control" }, roleInput, accessSelect));
  }

  function seatAddMenu({ id, focusKey, disabled, taken, groups, includeImage = false, onAdd }) {
    const option = (value, label) => el("option", { value, disabled: taken.has(value) && value !== "custom" }, label);
    const groupList = (title, entries) => entries.length
      ? el("optgroup", { label: title }, ...entries.map(([value, label]) => option(value, label))) : null;
    const named = (general) => groups
      .filter((entry) => !entry.fallback_group === general)
      .map((entry) => [entry.group, S.roleName(entry)]);
    return el("select", {
      className: "decision-add", id, "aria-label": L("添加工位", "Add seat"),
      dataset: { focusKey }, disabled,
      onChange: (e) => e.target.value && onAdd(e.target.value),
    },
    el("option", { value: "", selected: true }, L("添加工位", "Add seat")),
    groupList(L("通用工位", "General seats"), named(true)),
    groupList(L("专业工位", "Specialist seats"), named(false)),
    groupList(L("自定义工位", "Custom seats"), [["custom", L("自定义工位", "Custom seat")]]),
    includeImage ? groupList(L("生图工位", "Image seats"), [["image", L("生图工位", "Image seats")]]) : null);
  }

  Object.assign(S, {
    orchestrationScope,
    setOrchestrationScope,
    syncScopeViews,
    summarizeTiers,
    summarizeNames,
    renderSeatRows,
    customSeatHeadRow,
    seatAddMenu,
  });
}
