"use strict";
{
  const S = AN.settings ||= {};
  const {
    $, L, OFF, PROVIDERS, PROVIDER_NAMES, SCENES, SCENE_IDS, apiGetMembers,
    apiPostMembers, changeSlot, cliBusy, cliEffort, confirmScenes, effortLevels, el, emptyScene,
    getAllModelIds, hasPool, isSplit, known, limitChanged, loadCliInstallCommands, loadCliUpdates, markSaved,
    nativeDefault, outOfPool, renderCliInstallations, renderCliUpdate, renderCliUpdateError,
    renderCliUpdateSummary, renderCliVersion, renderCodexSource, renderMissingClis, renderPermission,
    sameJson, saveLimit, savedEffort, sceneEffort, setInteractive, setProviderEnabled, setSplit, settingTip,
    showNotice, shownPool, slot, takeReview, textNode, unlisted, writePool
  } = S;
  function slotRows(provider, scene, split) {
    const current = slot(scene || "task", provider);
    const key = `${provider}/${scene || "both"}`;
    const native = nativeDefault(provider);
    const all = getAllModelIds(provider);
    const offered = current.allowed ? all.filter((m) => current.allowed.includes(m)) : all;
    const tip = scene === "discussion" ? null : settingTip(L("单独干活只用于没开编排的 boss，开会始终按这里设置。分开设置可以给两者选不同的默认模型和档位，或只停用其中一个。", "Solo tasks only serve bosses without orchestration; meetings always use these settings. Separate lets the two use different default models and efforts, or turns off just one."));
    // The last scene keeps the tip beside it when the narrow column wraps the label.
    const label = el("span", { className: "member-scene" }, split ? [SCENES[scene], tip]
      : [L(`${SCENES.task}、`, `${SCENES.task}, `), el("span", { className: "nowrap" }, SCENES.discussion, tip)]);
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
        ...(native.model || current.allowed ? [] : [el("option", { value: "" }, L("CLI 自己决定", "Follow CLI"))]),
        ...offered.map((m) => el("option", { value: m }, m === native.model ? L(`${m}（CLI 当前设置）`, `${m} (CLI current)`) : m))), el("span", { className: "member-reset-slot" })))];
    }

    const model = current.model || (current.allowed ? "" : native.model) || "";
    const options = [
      el("option", { value: OFF }, L("不用", "Off")),
      ...(current.allowed && !model ? [el("option", { value: "", selected: true, disabled: true }, L("选一个默认模型", "Choose default model"))]
        : native.model ? [] : [el("option", { value: "", selected: !model }, L("CLI 自己决定", "Follow CLI"))]),
      ...(model && !offered.includes(model) ? [el("option", { value: model, selected: true, disabled: true }, L(`${model}（不在可用模型里）`, `${model} (not available)`))] : []),
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
      current.model && offered.includes(native.model) && current.model !== native.model
        ? el("button", {
          type: "button",
          className: "member-reset",
          dataset: { focusKey: `${key}/model-reset` },
          title: L(`改回 CLI 当前设置（${native.model}）`, `Reset to CLI current (${native.model})`),
          "aria-label": L("改回 CLI 当前设置", "Reset to CLI current"),
          onClick: () => pick({ model: native.model, effort: savedEffort(provider, native.model, sceneEffort(scene || "task", provider, native.model)) }),
        }, "↺")
        : el("span", { className: "member-reset-slot" }));

    return [el("div", { className: "member-row" }, label, modelLine, slotEffortLine({ provider, scene, current, model, key, pick }))];
  }

  // What the pool row reads: scenes of an older file can still hold lists of their own.
  function poolSummary(provider, all, pool) {
    const visible = (list) => list?.filter((id) => all.includes(id));
    const count = (list) => (list ? L(`${list.length} 个`, `${list.length}`) : L(`全部 ${all.length} 个`, `All ${all.length}`));
    const scenes = SCENE_IDS.filter((s) => slot(s, provider).enabled);
    const lists = scenes.map((s) => visible(slot(s, provider).allowed));
    if (!hasPool(provider) && lists.length > 1 && !sameJson(lists[0], lists[1])) {
      return scenes.map((s, i) => `${SCENES[s]} ${count(lists[i])}`).join(L("、", ", "));
    }
    if (!pool) return L("全部（含以后新出的）", "All, including future ones");
    const shown = visible(pool);
    if (!shown.length) return L("未选择", "None selected");
    const names = shown.slice(0, 3).join(L("、", ", "));
    return L(`${shown.length} 个：${names}${shown.length > 3 ? " 等" : ""}`, `${shown.length}: ${names}${shown.length > 3 ? ", etc." : ""}`);
  }

  // One list per CLI; every scene and seat of the CLI picks its model from it.
  function poolRows(provider) {
    const key = `${provider}/pool`;
    const all = getAllModelIds(provider);
    const pool = shownPool(provider);
    // Hidden and no longer listed models neither count nor survive an edit of the list.
    const chosen = pool?.filter((id) => all.includes(id));
    const open = S.pickerOpen === key;
    const rows = [el("div", { className: "member-row" },
      el("span", { className: "member-scene" }, L("可用模型", "Available models")),
      el("button", {
        type: "button",
        className: "member-list",
        dataset: { focusKey: `${key}/list` },
        title: L("单独干活、开会和工位都从这里选", "Solo tasks, meetings and seats pick from these"),
        "aria-expanded": String(open),
        onClick: () => { S.pickerOpen = open ? null : key; S.renderCards(); },
      }, el("span", {}, poolSummary(provider, all, pool)), el("span", { className: "member-muted" }, open ? "▴" : "▾")))];
    if (open) {
      // With every model on, unticking one leaves the rest ticked, so the saved list names them.
      const toggle = (m, on) => {
        if (!chosen && on) return;
        const list = chosen || all;
        writePool(provider, on ? [...list, m] : list.filter((x) => x !== m));
      };
      // An older file may still limit one scene by its own list; every model lifts that too.
      const sceneLimited = !hasPool(provider) && SCENE_IDS.some((s) => slot(s, provider).allowed);
      const useAll = () => {
        if (!chosen && !sceneLimited) return;
        writePool(provider, null);
      };
      const useTicked = () => {
        if (!chosen) writePool(provider, all);
      };
      const tier = (active, text, onClick, focus, disabled) => el("button", {
        type: "button", className: `segment-btn${active ? " active" : ""}`, role: "radio",
        dataset: { focusKey: `${key}/${focus}` }, "aria-checked": String(active), disabled, onClick,
      }, text);
      const defaults = SCENE_IDS.map((s) => slot(s, provider).model);
      rows.push(el("div", { className: "member-picker" },
        el("div", { className: "member-picker-actions" },
          el("div", { className: "segment-control", role: "radiogroup", "aria-label": L("可用模型", "Available models") },
            tier(!chosen && !sceneLimited, L("全部模型（含以后新出的）", "All models, including future ones"), useAll, "all"),
            tier(Boolean(chosen), L("只用勾选的", "Only the ticked"), useTicked, "ticked", !all.length)),
          chosen ? el("button", { type: "button", dataset: { focusKey: `${key}/none` },
            onClick: () => writePool(provider, []),
          }, L("全不选", "Clear all")) : null,
          el("span", {}, L(`已选 ${chosen ? chosen.length : all.length}`, `Selected ${chosen ? chosen.length : all.length}`)),
          chosen ? el("span", { className: "member-muted" }, L("建议每个 CLI 不超过 10 个模型", "Up to 10 models per CLI recommended")) : null),
        ...all.map((m) => el("label", { className: "member-option" },
          el("input", {
            type: "checkbox",
            dataset: { focusKey: `${key}/allow/${m}` },
            checked: !pool || pool.includes(m),
            onChange: (e) => toggle(m, e.target.checked),
          }),
          el("span", { className: "member-option-id" }, m),
          defaults.includes(m) ? el("span", { className: "badge badge-muted" }, L("默认", "Default")) : null))));
    }
    return rows;
  }

  // Bindings the pool no longer has stay as saved, each named on its CLI's card until changed.
  function poolConflicts(provider) {
    const scenes = SCENE_IDS.map((s) => [SCENES[s], slot(s, provider)])
      .filter(([, current]) => current.enabled && current.model && (outOfPool(provider, current.model) || unlisted(provider, current.model)))
      .map(([name, current]) => L(`${name}的默认模型 ${current.model} 不在可用模型里`, `${name} default model ${current.model} is not in the available models`));
    const seats = S.seatConflicts().filter(({ seat }) => seat.provider === provider).map(S.seatConflictText);
    return [...scenes, ...seats].map((text) => el("p", { className: "member-error" }, text));
  }

  function renderMembership(provider, enabled) {
    return el("button", {
      type: "button",
      className: "member-toggle",
      dataset: { focusKey: `${provider}/membership` },
      onClick: () => setProviderEnabled(provider, !enabled),
    }, enabled ? L("移除", "Remove") : L("启用", "Enable"));
  }

  // CLIs with an interactive runtime: the settings.json key and what turning it on means.
  const INTERACTIVE = {
    claude: {
      key: "claude_interactive",
      tip: () => L("用终端界面驱动 Claude Code，代替 -p。子任务会出现在 Claude 的 /resume 列表和历史记录里，不想留这些痕迹就关掉。", "Drives Claude Code through its terminal UI instead of -p. Subtasks then appear in Claude’s /resume list and history; turn this off if you don’t want those traces."),
    },
    antigravity: {
      key: "antigravity_interactive",
      tip: () => L("用终端界面驱动 Antigravity CLI，代替 -p 以省额度。会在 Antigravity 的全局 hooks.json 加一条只在 AgentNave 派发时起作用的 hook。", "Drives Antigravity CLI through its terminal UI instead of -p to save quota. Adds a hook to Antigravity’s global hooks.json that acts only in runs AgentNave starts."),
    },
  };

  function renderInteractive(provider) {
    const runtime = INTERACTIVE[provider];
    if (!runtime || (provider === "antigravity" && AN.shellOS?.() === "windows")) return null;
    return el("div", { className: "member-row member-runtime" },
      el("label", { className: "member-interactive" },
        el("input", {
          type: "checkbox",
          className: "switch-input",
          dataset: { focusKey: `${provider}/interactive` },
          checked: S.limitDoc?.[runtime.key] === true,
          disabled: S.busy || !S.limitDoc || Boolean(S.limitError),
          onChange: (e) => setInteractive(runtime.key, e.target.checked),
        }), textNode(L("交互模式运行", "Run interactively")),
        settingTip(runtime.tip())));
  }

  // One card per CLI: one row for both scenes, or one row each once split.
  function renderCards() {
    const container = $("cli-cards");
    // Build off-screen: emptying the live list first clamps the document scroll position.
    // Restore the operated control after replacement without native focus auto-scrolling.
    // WebKit does not focus every checkbox/button on a pointer click.
    const active = S.operatedControl?.isConnected ? S.operatedControl : document.activeElement;
    const focusKey = active?.dataset.focusKey;
    const anchorTop = focusKey ? active.getBoundingClientRect().top : null;
    const cards = document.createDocumentFragment();
    // Not installed and named by no scene or default: only in the folded list at the end.
    const missing = [];
    cards.appendChild(renderCliUpdateSummary());
    // Employees as last saved come first; a card stays put while its switch is being changed.
    const saved = confirmScenes(JSON.parse(S.savedSnapshot || JSON.stringify(S.currentDoc)));
    const hired = (provider) => SCENE_IDS.some((s) => saved.modes?.[s]?.[provider]);

    for (const provider of [...PROVIDERS].sort((a, b) => hired(b) - hired(a))) {
      const info = S.detected[provider] || { installed: false, source: "none", path: null, error: null, models: [], efforts: [] };
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
          renderCliVersion(provider, info),
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
      const card = el("div", { className: `cli-card member-card${enabled ? "" : " off"}`, dataset: { provider } }, head, renderCliInstallations(provider));
      if (S.pendingDisable === provider) {
        card.appendChild(el("div", { className: "disable-confirm", role: "group", "aria-label": L(`确认移除 ${name}`, `Confirm removing ${name}`) },
          el("p", {}, L(`移除会清除 ${name} 的默认模型。重新启用不会恢复；点击“保存设置”后才会生效。`, `Removing clears default models for ${name}. Re-enabling will not restore them; takes effect after clicking “Save settings”.`)),
          el("div", { className: "btn-group" },
            el("button", { type: "button", dataset: { focusKey: `${provider}/cancel-disable` }, onClick: () => { S.pendingDisable = null; S.renderCards(); } }, L("保留", "Keep")),
            el("button", { type: "button", className: "danger", dataset: { focusKey: `${provider}/confirm-disable` }, onClick: () => setProviderEnabled(provider, false, true) }, L("确认移除", "Remove")))));
      }
      const rows = split ? SCENE_IDS.flatMap((s) => slotRows(provider, s, true)) : slotRows(provider, null, false);
      if (provider === "codex") card.appendChild(renderCodexSource());
      for (const row of [...poolRows(provider), ...rows, ...poolConflicts(provider)]) card.appendChild(row);
      const runtime = renderInteractive(provider);
      if (runtime) card.appendChild(runtime);
      if (info.error) card.appendChild(el("p", { className: "member-error" }, L(`读取模型列表失败：${info.error}`, `Failed to load model list: ${info.error}`)));
      card.appendChild(renderCliUpdateError(provider));
      cards.appendChild(card);
    }

    if (missing.length) cards.appendChild(renderMissingClis(missing));

    container.replaceChildren(cards);
    S.updateControls();
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
    S.renderReviewSeats();
    if (S.orchestration) S.renderOrchestration();
  }

  // Save members.json with race version checking
  async function saveMembers() {
    if (S.busy || !S.dirty || S.documentError || emptyScene()) return;
    const errors = S.memberSaveErrors();
    if (errors.length) {
      showNotice(errors[0].message, true);
      S.updateControls();
      return;
    }
    S.busy = true;
    S.updateControls();
    const feedback = $("save-feedback");
    feedback.textContent = L("正在保存…", "Saving…");
    feedback.className = "save-feedback";

    const saving = JSON.stringify(confirmScenes());
    const sourceChanged = S.currentDoc.clis?.codex?.executable_source !== JSON.parse(S.savedSnapshot || "{}").clis?.codex?.executable_source;
    let saved = false;

    try {
      if (saving !== S.savedSnapshot) {
        await apiPostMembers(JSON.parse(saving));
        S.savedSnapshot = saving;
      }
      if (limitChanged()) await saveLimit();
      S.refreshDirty();
      // Saved membership decides each card's place and the folded uninstalled list.
      S.renderCards();
      feedback.textContent = L("设置已保存 · 参数是否有效以 boss 读取时的校验为准", "Settings saved · Option validity is verified when read by the boss");
      feedback.className = "save-feedback success";
      showNotice(L("设置已保存", "Settings saved"));
      saved = true;
    } catch (err) {
      feedback.textContent = L(`保存失败: ${err.message}`, `Save failed: ${err.message}`);
      feedback.className = "save-feedback error";
      showNotice(err.message, true);
      S.refreshDirty();
    } finally {
      S.busy = false;
      S.updateControls();
      if (saved && sourceChanged) void refreshDetection(true);
    }
  }

  function renderErrorBanner() {
    const errBanner = $("error-banner");
    const errDesc = $("error-desc");
    if (!errBanner) return;
    if (S.documentError) {
      errBanner.hidden = false;
      if (errDesc) errDesc.textContent = S.documentError;
    } else {
      errBanner.hidden = true;
    }
  }

  // Re-probe the CLIs. A quiet probe runs when Settings opens or regains focus and only speaks
  // up when the lists changed.
  async function refreshDetection(quiet = false) {
    if (S.busy || S.probing || cliBusy()) return;
    S.probing = true;
    S.probedAt = Date.now();
    const before = S.savedSnapshot;
    const seatsBefore = S.reviewSaves;
    S.updateControls();
    if (!quiet) showNotice(L("正在重新检测 CLI 与模型…", "Re-detecting CLIs and models…"));
    try {
      const data = await apiGetMembers(true);
      const changed = JSON.stringify(data.detected || {}) !== JSON.stringify(S.detected);
      S.detected = data.detected || {};
      const tiersBefore = JSON.stringify([S.permissionTiers, S.permissionsError]);
      if (!S.busy && S.reviewSaves === seatsBefore) takeReview(data);
      const tiersChanged = JSON.stringify([S.permissionTiers, S.permissionsError]) !== tiersBefore;
      // Without unsaved edits, or a save while probing, take the file as read too.
      // An unchanged file keeps the current objects, which the rendered cards still edit.
      const fileDoc = data.document || { version: 1, clis: {}, modes: {} };
      const fresh = !S.dirty && !S.busy && S.savedSnapshot === before;
      const reloaded = fresh && JSON.stringify(fileDoc) !== S.savedSnapshot;
      if (reloaded) {
        S.currentDoc = fileDoc;
        markSaved();
      }
      if (fresh) {
        S.documentError = data.error || null;
        S.renderErrorBanner();
      }
      if (!quiet) showNotice(L("检测结果已刷新", "Detection refreshed"));
      else if (changed && S.currentTab === "members") showNotice(L("检测到工具或模型列表有变化，已自动刷新", "Detected changes in tools or models; refreshed automatically"));
      // A quiet probe that found nothing new leaves the cards, and any input in them, alone.
      if (!quiet || changed || reloaded || tiersChanged) S.renderCards();
      void loadCliUpdates();
    } catch (err) {
      if (!quiet) showNotice(L(`刷新检测失败: ${err.message}`, `Refresh failed: ${err.message}`), true);
    } finally {
      S.probing = false;
      S.updateControls();
    }
  }

  function probeWhenStale() {
    if (!cliBusy() && document.visibilityState === "visible" && Date.now() - S.probedAt > 30_000) refreshDetection(true);
  }

  // Load initial members data
  async function loadMembers() {
    if (S.currentTab === "members") showNotice(L("正在读取成员与模型配置…", "Loading employee and model settings…"));
    try {
      await loadCliInstallCommands();
      const data = await apiGetMembers(false);
      S.detected = data.detected || {};
      S.currentDoc = data.document || { version: 1, clis: {}, modes: {} };
      markSaved();
      S.documentError = data.error || null;
      takeReview(data);
      S.renderErrorBanner();

      if (S.documentError) {
        showNotice(L(`配置文件损坏: ${S.documentError}。当前只读，禁止保存。`, `Configuration file corrupted: ${S.documentError}. Read-only; saving is disabled.`), true);
      } else if (S.currentTab === "members") {
        showNotice("");
      }

      S.renderCards();
      void loadCliUpdates();
    } catch (err) {
      S.documentError = err.message;
      S.renderErrorBanner();
      showNotice(L(`读取失败: ${err.message}`, `Load failed: ${err.message}`), true);
      S.updateControls();
    }
  }

  // Helpers for version check

  function slotEffortLine({ provider, scene, current, model, key, pick }) {
    const conf = S.currentDoc.modes?.[scene || "task"]?.[provider]?.overrides?.[model];
    // A scene's own range, even an empty one, replaces the shared one.
    const range = (conf && Object.hasOwn(conf, "efforts") ? conf.efforts : S.currentDoc.clis?.[provider]?.models?.[model]?.efforts) || [];
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

  Object.assign(S, {
    slotRows, poolSummary, poolRows, poolConflicts, renderInteractive, renderCards, saveMembers,
    renderErrorBanner, refreshDetection, probeWhenStale, loadMembers
  });
}
