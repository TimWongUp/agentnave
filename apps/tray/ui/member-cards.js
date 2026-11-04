"use strict";
function slotRows(provider, scene, split) {
  const current = slot(scene || "task", provider);
  const key = `${provider}/${scene || "both"}`;
  const native = nativeDefault(provider);
  const all = getAllModelIds(provider);
  if (native.model && !all.includes(native.model)) all.unshift(native.model);
  const label = el("span", { className: "member-scene" }, split ? SCENES[scene] : L(`${SCENES.task}、${SCENES.discussion}`, `${SCENES.task}, ${SCENES.discussion}`));
  const pick = (change) => changeSlot(provider, scene, change);
  // Turning a CLI off for both scenes asks first when it drops settings.
  const off = () => (scene ? pick({ enabled: false }) : setProviderEnabled(provider, false));

  if (!current.enabled) {
    return [el("div", { className: "member-row" }, label, el("div", { className: "member-line" },
      el("select", {
        className: "member-model",
        dataset: { focusKey: `${key}/model` },
        "aria-label": L("默认模型", "Default model"),
        onChange: (e) => pick({ enabled: true, model: e.target.value, effort: savedEffort(provider, e.target.value || native.model, "") }),
      }, el("option", { value: OFF, selected: true }, L("不用", "Off")),
      ...(native.model ? [] : [el("option", { value: "" }, L("CLI 自己决定", "Follow CLI"))]),
      ...all.map((m) => el("option", { value: m }, m === native.model ? L(`${m}（CLI 当前设置）`, `${m} (CLI current)`) : m))), el("span", { className: "member-reset-slot" })))];
  }

  const offered = current.allowed ? all.filter((m) => current.allowed.includes(m)) : all;
  const model = current.model || (current.allowed ? "" : native.model) || "";
  const options = [
    el("option", { value: OFF }, L("不用", "Off")),
    ...(current.allowed && !model ? [el("option", { value: "", selected: true, disabled: true }, L("选一个默认模型", "Choose default model"))]
      : native.model ? [] : [el("option", { value: "", selected: !model }, L("CLI 自己决定", "Follow CLI"))]),
    ...offered.map((m) => el("option", { value: m, selected: m === model }, m === native.model ? L(`${m}（CLI 当前设置）`, `${m} (CLI current)`) : m)),
  ];
  const modelLine = el("div", { className: "member-line" },
    el("select", {
      className: "member-model",
      dataset: { focusKey: `${key}/model` },
      "aria-label": L("默认模型", "Default model"),
      onChange: (e) => {
        const value = e.target.value;
        if (value === OFF) return off();
        const id = value || native.model;
        pick({ model: value, effort: savedEffort(provider, id, sceneEffort(scene || "task", provider, id)) });
      },
    }, ...options),
    current.model && native.model && current.model !== native.model
      ? el("button", {
        type: "button",
        className: "member-reset",
        dataset: { focusKey: `${key}/model-reset` },
        title: L(`改回 CLI 当前设置（${native.model}）`, `Reset to CLI current (${native.model})`),
        "aria-label": L("改回 CLI 当前设置", "Reset to CLI current"),
        onClick: () => pick({ model: native.model, effort: savedEffort(provider, native.model, sceneEffort(scene || "task", provider, native.model)) }),
      }, "↺")
      : el("span", { className: "member-reset-slot" }));

  const effortLine = slotEffortLine({ provider, scene, current, model, key, pick });

  return slotAllowedRows({ provider, current, native, all, offered, model, key, pick, label, modelLine, effortLine });
}

function slotAllowedRows({ provider, current, native, all, offered, model, key, pick, label, modelLine, effortLine }) {
  const open = pickerOpen === key;
  const summary = current.allowed
    ? L(`${offered.length} 个：${offered.slice(0, 3).join("、")}${offered.length > 3 ? " 等" : ""}`, `${offered.length}: ${offered.slice(0, 3).join(", ")}${offered.length > 3 ? ", etc." : ""}`)
    : L(`全部 ${all.length} 个`, `All ${all.length}`);
  const listButton = el("button", {
    type: "button",
    className: "member-list",
    dataset: { focusKey: `${key}/list` },
    title: L("boss 可以选用的模型", "Models the boss may choose from"),
    "aria-expanded": String(open),
    onClick: () => { pickerOpen = open ? null : key; renderCards(); },
  }, el("span", { className: "member-muted" }, L("可选 ", "Allowed: ")), textNode(summary), el("span", { className: "member-muted" }, open ? " ▴" : " ▾"));

  const rows = [el("div", { className: "member-row" }, label, modelLine, listButton, effortLine)];
  if (open) {
    const toggle = (m, on) => {
      const list = current.allowed || [];
      const allowed = on ? [...list, m] : list.filter((x) => x !== m);
      if (allowed.length) pick({ allowed });
      else renderCards();
    };
    rows.push(el("div", { className: "member-picker" },
      el("label", { className: "member-option member-option-all" },
        el("input", {
          type: "checkbox",
          dataset: { focusKey: `${key}/all` },
          checked: !current.allowed,
          disabled: !all.length,
          onChange: (e) => pick(e.target.checked
            ? { allowed: null, model: current.model === native.model ? "" : current.model }
            : { allowed: [model || all[0]] }),
        }), textNode(L("全部模型（以后新出的也算）", "All models (including future releases)"))),
      ...all.map((m) => el("label", { className: "member-option" },
        el("input", {
          type: "checkbox",
          dataset: { focusKey: `${key}/allow/${m}` },
          checked: !current.allowed || current.allowed.includes(m),
          disabled: !current.allowed || m === model,
          onChange: (e) => toggle(m, e.target.checked),
        }),
        el("span", { className: "member-option-id" }, m),
        m === model ? el("span", { className: "badge badge-muted" }, L("默认", "Default")) : null))));
  }
  return rows;
}

function renderMembership(provider, enabled) {
  return el("button", {
    type: "button",
    className: "member-toggle",
    dataset: { focusKey: `${provider}/membership` },
    onClick: () => setProviderEnabled(provider, !enabled),
  }, enabled ? L("移除", "Remove") : L("启用", "Enable"));
}

// One card per CLI: one row for both scenes, or one row each once split.
function renderCards() {
  const container = $("cli-cards");
  // Build off-screen: emptying the live list first clamps the document scroll position.
  // Restore the operated control after replacement without native focus auto-scrolling.
  // WebKit does not focus every checkbox/button on a pointer click.
  const active = operatedControl?.isConnected ? operatedControl : document.activeElement;
  const focusKey = active?.dataset.focusKey;
  const anchorTop = focusKey ? active.getBoundingClientRect().top : null;
  const cards = document.createDocumentFragment();
  // Not installed and named by no scene or default: only in the folded list at the end.
  const missing = [];
  cards.appendChild(renderCliUpdateSummary());
  // Employees as last saved come first; a card stays put while its switch is being changed.
  const saved = confirmScenes(JSON.parse(savedSnapshot || JSON.stringify(currentDoc)));
  const hired = (provider) => SCENE_IDS.some((s) => saved.modes?.[s]?.[provider]);

  for (const provider of [...PROVIDERS].sort((a, b) => hired(b) - hired(a))) {
    const info = detected[provider] || { installed: false, source: "none", path: null, error: null, models: [], efforts: [] };
    const enabled = SCENE_IDS.some((s) => slot(s, provider).enabled);
    const listed = known(provider, saved);
    if (!info.installed && !listed) {
      missing.push(provider);
      continue;
    }
    const name = PROVIDER_NAMES[provider] || provider;
    const split = enabled && isSplit(provider);
    const head = el("div", { className: "member-head" },
      el("div", { className: "member-title" },
        el("span", { className: "cli-icon", "aria-hidden": "true" }, AN.hostTile(provider, name)),
        el("span", { className: "cli-name", title: name }, name),
        info.installed && info.version ? el("span", { className: "cli-version" }, info.version) : null,
        renderCliUpdate(provider),
        info.installed ? null : el("span", { className: "badge badge-bad" }, L("未安装", "Not installed"))),
      el("div", { className: "member-end" },
        el("div", { className: `segment-control${enabled ? "" : " member-hidden"}`, role: "radiogroup", "aria-label": L(`${name} 的两个场景`, `Two scenes for ${name}`), "aria-hidden": enabled ? null : "true" },
          [[false, L("一起设置", "Together")], [true, L("分开设置", "Separate")]].map(([value, text]) => el("button", {
            type: "button",
            role: "radio",
            className: `segment-btn${value === split ? " active" : ""}`,
            "aria-checked": String(value === split),
            dataset: { focusKey: `${provider}/split/${value}` },
            onClick: () => value !== split && setSplit(provider, value),
          }, text))),
        el("div", { className: "member-tail" }, renderPermission(provider) || el("span"), renderMembership(provider, enabled))));
    const card = el("div", { className: `cli-card member-card${enabled ? "" : " off"}`, dataset: { provider } }, head);
    if (pendingDisable === provider) {
      card.appendChild(el("div", { className: "disable-confirm", role: "group", "aria-label": L(`确认移除 ${name}`, `Confirm removing ${name}`) },
        el("p", {}, L(`移除会清除 ${name} 的默认模型和可选模型。重新启用不会恢复；点击“保存设置”后才会生效。`, `Removing clears default and allowed models for ${name}. Re-enabling will not restore them; takes effect after clicking “Save settings”.`)),
        el("div", { className: "btn-group" },
          el("button", { type: "button", dataset: { focusKey: `${provider}/cancel-disable` }, onClick: () => { pendingDisable = null; renderCards(); } }, L("保留", "Keep")),
          el("button", { type: "button", className: "danger", dataset: { focusKey: `${provider}/confirm-disable` }, onClick: () => setProviderEnabled(provider, false, true) }, L("确认移除", "Remove")))));
    }
    const rows = split ? SCENE_IDS.flatMap((s) => slotRows(provider, s, true)) : slotRows(provider, null, false);
    for (const row of rows) card.appendChild(row);
    if (info.error) card.appendChild(el("p", { className: "member-error" }, L(`读取模型列表失败：${info.error}`, `Failed to load model list: ${info.error}`)));
    card.appendChild(renderCliUpdateError(provider));
    cards.appendChild(card);
  }

  if (missing.length) cards.appendChild(renderMissingClis(missing));

  container.replaceChildren(cards);
  updateControls();
  if (focusKey) {
    const controls = [...container.querySelectorAll("[data-focus-key]")];
    const replacement = controls.find((control) => control.dataset.focusKey === focusKey)
      || (focusKey.endsWith("-disable") ? controls.find((control) => control.dataset.focusKey === `${focusKey.split("/")[0]}/both/model`) : null);
    if (replacement && !replacement.disabled) {
      replacement.focus({ preventScroll: true });
      // A confirmation button returns focus to the switch, not its screen position.
      if (replacement.dataset.focusKey === focusKey) {
        window.scrollBy(0, replacement.getBoundingClientRect().top - anchorTop);
      }
    }
  }
  renderReviewSeats();
  if (orchestration) renderOrchestration();
}

// Save members.json with race version checking
async function saveMembers() {
  if (busy || !dirty || documentError || emptyScene()) return;
  const errors = configurationErrors();
  if (errors.length) {
    showNotice(errors[0].message, true);
    updateControls();
    return;
  }
  busy = true;
  updateControls();
  const feedback = $("save-feedback");
  feedback.textContent = L("正在保存…", "Saving…");
  feedback.className = "save-feedback";

  const saving = JSON.stringify(confirmScenes());

  try {
    if (saving !== savedSnapshot) {
      await apiPostMembers(JSON.parse(saving));
      savedSnapshot = saving;
    }
    if (limitChanged()) await saveLimit();
    refreshDirty();
    // Saved membership decides each card's place and the folded uninstalled list.
    renderCards();
    feedback.textContent = L("设置已保存 · 参数是否有效以 boss 读取时的校验为准", "Settings saved · Option validity is verified when read by the boss");
    feedback.className = "save-feedback success";
    showNotice(L("设置已保存", "Settings saved"));
  } catch (err) {
    feedback.textContent = L(`保存失败: ${err.message}`, `Save failed: ${err.message}`);
    feedback.className = "save-feedback error";
    showNotice(err.message, true);
    refreshDirty();
  } finally {
    busy = false;
    updateControls();
  }
}

function renderErrorBanner() {
  const errBanner = $("error-banner");
  const errDesc = $("error-desc");
  if (!errBanner) return;
  if (documentError) {
    errBanner.hidden = false;
    if (errDesc) errDesc.textContent = documentError;
  } else {
    errBanner.hidden = true;
  }
}

// Re-probe the CLIs. A quiet probe runs when Settings opens or regains focus and only speaks
// up when the lists changed.
async function refreshDetection(quiet = false) {
  if (busy || probing || cliUpdating || cliBatch) return;
  probing = true;
  probedAt = Date.now();
  const before = savedSnapshot;
  const seatsBefore = reviewSaves;
  updateControls();
  if (!quiet) showNotice(L("正在重新检测 CLI 与模型…", "Re-detecting CLIs and models…"));
  try {
    const data = await apiGetMembers(true);
    const changed = JSON.stringify(data.detected || {}) !== JSON.stringify(detected);
    detected = data.detected || {};
    const tiersBefore = JSON.stringify([permissionTiers, permissionsError]);
    if (!busy && reviewSaves === seatsBefore) takeReview(data);
    const tiersChanged = JSON.stringify([permissionTiers, permissionsError]) !== tiersBefore;
    // Without unsaved edits, or a save while probing, take the file as read too.
    // An unchanged file keeps the current objects, which the rendered cards still edit.
    const fileDoc = data.document || { version: 1, clis: {}, modes: {} };
    const fresh = !dirty && !busy && savedSnapshot === before;
    const reloaded = fresh && JSON.stringify(fileDoc) !== savedSnapshot;
    if (reloaded) {
      currentDoc = fileDoc;
      markSaved();
    }
    if (fresh) {
      documentError = data.error || null;
      renderErrorBanner();
    }
    if (!quiet) showNotice(L("检测结果已刷新", "Detection refreshed"));
    else if (changed && currentTab === "members") showNotice(L("检测到工具或模型列表有变化，已自动刷新", "Detected changes in tools or models; refreshed automatically"));
    // A quiet probe that found nothing new leaves the cards, and any input in them, alone.
    if (!quiet || changed || reloaded || tiersChanged) renderCards();
    void loadCliUpdates();
  } catch (err) {
    if (!quiet) showNotice(L(`刷新检测失败: ${err.message}`, `Refresh failed: ${err.message}`), true);
  } finally {
    probing = false;
    updateControls();
  }
}

function probeWhenStale() {
  if (!cliUpdating && !cliBatch && document.visibilityState === "visible" && Date.now() - probedAt > 30_000) refreshDetection(true);
}

// Load initial members data
async function loadMembers() {
  if (currentTab === "members") showNotice(L("正在读取成员与模型配置…", "Loading employee and model settings…"));
  try {
    await loadCliInstallCommands();
    const data = await apiGetMembers(false);
    detected = data.detected || {};
    currentDoc = data.document || { version: 1, clis: {}, modes: {} };
    markSaved();
    documentError = data.error || null;
    takeReview(data);
    renderErrorBanner();

    if (documentError) {
      showNotice(L(`配置文件损坏: ${documentError}。当前只读，禁止保存。`, `Configuration file corrupted: ${documentError}. Read-only; saving is disabled.`), true);
    } else if (currentTab === "members") {
      showNotice("");
    }

    renderCards();
    void loadCliUpdates();
  } catch (err) {
    documentError = err.message;
    renderErrorBanner();
    showNotice(L(`读取失败: ${err.message}`, `Load failed: ${err.message}`), true);
    updateControls();
  }
}

// Helpers for version check

function slotEffortLine({ provider, scene, current, model, key, pick }) {
  const conf = currentDoc.modes?.[scene || "task"]?.[provider]?.overrides?.[model];
  // A scene's own range, even an empty one, replaces the shared one.
  const range = (conf && Object.hasOwn(conf, "efforts") ? conf.efforts : currentDoc.clis?.[provider]?.models?.[model]?.efforts) || [];
  const levels = model ? effortLevels(provider, model).filter((l) => !range.length || range.includes(l)) : [];
  const cli = model ? cliEffort(provider, model) : null;
  let effortLine = el("span");
  if (!model) effortLine = el("span", { className: "member-muted" }, L("CLI 自己决定", "Follow CLI"));
  else if (levels.length === 1) effortLine = el("span", { className: "member-muted" }, L(`只有 ${levels[0]} 一档`, `Only ${levels[0]}`));
  else if (levels.length > 1) {
    const shown = current.effort || (levels.includes(cli) ? cli : null);
    effortLine = el("div", { className: "member-line" },
      el("div", { className: "segment-control member-efforts", role: "radiogroup", "aria-label": L("思考档位", "Effort") },
        levels.map((level) => el("button", {
          type: "button",
          role: "radio",
          className: `segment-btn${level === shown ? " active" : ""}`,
          "aria-checked": String(level === shown),
          title: level === cli ? L("CLI 当前设置", "CLI current") : null,
          dataset: { focusKey: `${key}/effort/${level}` },
          onClick: () => pick({ effort: level }),
        }, level))),
      !shown ? el("span", { className: "member-muted" }, L("选一个档位", "Choose effort"))
        : current.effort && cli && levels.includes(cli) && current.effort !== cli
          ? el("button", {
            type: "button",
            className: "member-reset",
            dataset: { focusKey: `${key}/effort-reset` },
            title: L(`改回 CLI 当前设置（${cli}）`, `Reset to CLI current (${cli})`),
            "aria-label": L("改回 CLI 当前设置", "Reset to CLI current"),
            onClick: () => pick({ effort: cli }),
          }, "↺")
          : el("span", { className: "member-reset-slot" }));
  }

  return effortLine;
}
