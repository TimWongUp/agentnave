"use strict";
{
  // Settings -> 评审设置 and 编排: Skills, orchestration and review seats. Viewing and picking
  // only change the drafts here; nothing is written until 保存设置. Sections save independently, so
  // one that fails stays unsaved for another try while the others land.

  const S = AN.settings ||= {};
  const {
    $, PROVIDERS, PROVIDER_NAMES, SCENE_IDS, apiPostMembers, apiPostReview, cliEffort, configurationErrors,
    confirmScenes, effortLevels, el, getAllModelIds, isRestricted, known, listedPool, nativeDefault,
    outOfPool, sameJson, savedEffort, sceneEffort, settingTip, showNotice, slot, unlisted
  } = S;
  // Orchestration lives in orchestration.json: the rules text each boss gets, by mode (`apps`, with
  // `preset` for rows without one, and bosses connected later), the user's own text, which catalog
  // roles are on, and the role and image generation seats boss reads from list_members. The App writes
  // the modes into the connected boss apps' global rules. The rules files are listed one row per file
  // with every app that reads it.
  const ORCHESTRATION_EFFECTS = {
    off: AN.L("不写入全局规则，boss 按自己的习惯派发。", "Does not write global rules; the boss dispatches in its own way."),
    meetings: AN.L("写入所选 boss 的全局规则：boss 只用 AgentNave 会议室，不私聊派活；是否主动开会由下方开关决定。", "Writes to selected bosses' global rules: the boss only uses AgentNave meeting rooms and does not delegate privately; the switch below controls whether it may call meetings on its own."),
    split: AN.L("写入所选 boss 的全局规则：boss 未被点名也会把活派给已开启的角色，会用到它们的额度；并行写入与是否主动开会由下方开关决定。", "Writes to selected bosses' global rules: even when not named, the boss delegates to the roles turned on, using their quota; the switches below control parallel writes and whether it may call meetings on its own."),
    decision: AN.L("写入所选 boss 的全局规则：选工位交给决策模型，从你自建的工位里挑；boss 未被点名也会把活派出去，会用到这些工位的额度。", "Writes to selected bosses' global rules: choosing a seat is left to the decision model, which picks from the seats you built; even when not named, the boss delegates, using those seats' quota."),
    custom: AN.L("写入你自己的规则，起始内容是选自定义前的档位。", "Writes your own rules, starting from the preset selected before Custom."),
  };
  // Medium is optional: a group without it leaves the row unset and its rules line without it.
  const DIFFICULTIES = [["easy", AN.L("简单", "Easy")], ["medium", AN.L("中等", "Medium")], ["hard", AN.L("困难", "Hard")]];
  const ROLE_ACCESS = { read: AN.L("只读", "Read-only"), write: AN.L("可写", "Writable") };
  const SEAT_UNSET = AN.L("未配置", "Not configured");
  // One seat with no difficulty and no switch, its model the one the CLI talks to, which calls the image service.
  const IMAGE_SEAT = { role: "image_generation", difficulty: null, name: AN.L("生图（可选）", "Image generation (optional)") };
  const orchestrationSeat = (doc, role, difficulty) => difficulty ? doc[role]?.[difficulty] : doc[role];
  const ORCHESTRATION_NAMES = { off: AN.L("关闭", "Off"), meetings: AN.L("只开会", "Meetings only"), split: AN.L("分工协作", "Split"), decision: AN.L("决策模型", "Decision model"), custom: AN.L("自定义", "Custom") };
  let orchestration = null; // { document, error, presets, roles, files, bosses } from GET /app/orchestration: what is saved.
  let orchestrationDraft = null; // The document as picked, without the custom text, which the editor holds.
  let orchestrationView = "off"; // The mode being looked at; looking changes nothing.
  let ruleOpenError = null; // { path, message }: why a rules file did not open, shown on its row.
  let customTouched = false; // Typed into the editor, so its text is saved even with no boss on it yet.
  let splitOpened = new Set(); // Specialists the user split by difficulty while both seats are still the same.
  const orchestrationShown = () => orchestrationDraft || orchestration?.document;
  // Whether a mode sends work to the orchestration seats; off and meetings-only never do.
  const usesSeats = (mode) => Boolean(mode) && mode !== "off" && mode !== "meetings";
  // The default meeting line from `meeting: { count, seats? }`, built from the same parts the rules file
  // uses; a seat left empty is one the boss picks. A seat is its CLI alone: the model and effort come
  // from 员工 when the meeting runs, so an older seat's saved ones are ignored.
  const MEETING_COUNTS = [2, 3, 4, 5, 6];
  const MEETING_MODES = ["meetings", "split", "decision"];
  const meetingDefaultCount = () => orchestration?.presets?.meeting_parts?.default_count ?? 3;
  const trimSeats = (seats) => {
    const list = [...seats];
    while (list.length && !list.at(-1)) list.pop();
    return list;
  };
  const meetingOf = (doc) => ({
    count: doc?.meeting?.count ?? meetingDefaultCount(),
    seats: trimSeats(doc?.meeting?.seats || []).map((seat) => (seat?.provider ? { provider: seat.provider } : null)),
  });
  const meetingLive = (doc) => Boolean(doc) && [doc.preset, ...Object.values(doc.apps || {})].some((mode) => MEETING_MODES.includes(mode));
  function meetingLine(doc) {
    const parts = orchestration?.presets?.meeting_parts;
    if (!parts || !doc?.meeting) return "";
    const { count, seats } = meetingOf(doc);
    const named = seats.slice(0, count).filter((seat) => seat && PROVIDER_NAMES[seat.provider])
      .map((seat) => parts.seat_provider.replace("{name}", PROVIDER_NAMES[seat.provider]).replace("{provider}", seat.provider));
    const template = !named.length ? parts.none : named.length === count ? parts.all : parts.some;
    return template.replace("{count}", String(count)).replace("{seats}", named.join(parts.separator));
  }
  const usesCustom = (doc) => doc?.preset === "custom" || Object.values(doc?.apps || {}).includes("custom");
  // The fixed catalog, or nothing from a page that was not given one: its roles are then left as saved.
  const roleCatalog = () => orchestration?.roles || [];
  const roleName = (role) => AN.L(role.name_zh, role.name_en);
  // General roles are what the specialists fall back to; they always show every difficulty.
  const isGeneral = (role) => !role.fallback_group;
  // A specialist is split by difficulty when its easy and hard seats differ or it has a medium one;
  // unset and identical ones share one row.
  const seatsSplit = (doc, group) => splitOpened.has(group) || Boolean(doc[group]?.medium)
    || !sameJson(doc[group]?.easy ?? null, doc[group]?.hard ?? null);
  // A role's rules line, which names its medium seat once one is picked.
  const roleSection = (presets, doc, group) =>
    (doc?.[group]?.medium ? presets.role_sections_medium?.[group] : null) ?? presets.role_sections?.[group] ?? "";
  // A document without the list keeps the groups from before the switches.
  const enabledGroups = (doc) => doc?.enabled_roles ?? roleCatalog().filter((role) => role.legacy_enabled).map((role) => role.group);
  // Every seat a document offers: each difficulty of each role turned on, and the image seat.
  function liveSeats(doc) {
    const enabled = enabledGroups(doc);
    return [...roleCatalog().filter((role) => enabled.includes(role.group)).flatMap((role) => DIFFICULTIES
      .map(([difficulty, level]) => ({ role: role.group, difficulty, name: `${roleName(role)} · ${level}` }))), IMAGE_SEAT];
  }
  function orchestrationWanted() {
    const wanted = orchestrationShown();
    if (usesCustom(wanted) || customTouched) return { ...wanted, custom: $("custom-text").value };
    return typeof wanted.custom === "string" ? { ...wanted, custom: customRoleText(wanted.custom, wanted) } : wanted;
  }
  function customRoleText(text, doc) {
    const presets = orchestration?.presets || {};
    const sections = [...Object.values(presets.role_sections || {}), ...Object.values(presets.role_sections_medium || {})];
    const generated = new Set([...sections, presets.image_generation || ""].flatMap((section) => section.split("\n").filter(Boolean)));
    const base = text.split("\n").filter((line) => !generated.has(line.replace(/\r$/, ""))).join("\n").trimEnd();
    const enabled = enabledGroups(doc);
    return [base, ...roleCatalog().filter((role) => enabled.includes(role.group)).map((role) => roleSection(presets, doc, role.group)), doc.image_generation ? presets.image_generation || "" : ""].join("");
  }
  const orchestrationChanged = () => {
    if (!orchestration || orchestration.error) return false;
    const saved = orchestration.document;
    const baseline = typeof saved.custom === "string" ? { ...saved, custom: customRoleText(saved.custom, saved) } : saved;
    return !sameJson(orchestrationWanted(), baseline);
  };
  // Bosses sharing a rules file have one mode: the first set among them, else the default.
  const bossMode = (doc, boss) => boss.keys.map((key) => doc.apps?.[key]).find(Boolean) ?? doc.preset;
  // The mode the page opens on: one the saved document uses, else the default.
  function savedView() {
    const modes = orchestration.bosses.map((boss) => bossMode(orchestration.document, boss));
    return modes.find((mode) => mode !== "off") || orchestration.document.preset;
  }

  async function loadOrchestration() {
    try {
      const response = await fetch("/app/orchestration", { cache: "no-store" });
      if (!response.ok) throw new Error(AN.L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
      const loaded = await response.json();
      // Asked once the answer is in: an edit made while it was on its way stays.
      const keep = orchestrationChanged();
      // A file error while an unsaved draft stands keeps the last good baseline, so the draft survives the next good load.
      if (!(loaded.error && keep)) orchestration = loaded;
      if (loaded.error) showNotice(AN.L(`${loaded.error}；请先修复或删除这个文件`, `${loaded.error}; fix or delete this file first`), true);
      if (!keep) {
        orchestrationDraft = null;
        customTouched = false;
        splitOpened = new Set();
        $("custom-text").value = typeof orchestration.document.custom === "string"
          ? customRoleText(orchestration.document.custom, orchestration.document) : "";
        orchestrationView = savedView();
      }
    } catch (error) {
      // Only a page that never loaded has no baseline; otherwise the last good one keeps tracking the draft.
      showNotice(AN.L(`无法读取编排设置：${error.message}`, `Failed to load orchestration settings: ${error.message}`), true);
    }
    renderOrchestration();
    const decisionLoaded = window.AgentNaveDecisionSeats?.load();
    window.AgentNaveT3Seats?.load(decisionLoaded);
    // Review seats depend on whether any boss orchestrates.
    renderReviewSeats();
    renderAdvancedBar();
    // Cards name the seats their pool left out.
    S.renderCards();
  }

  async function saveOrchestration() {
    const wanted = orchestrationWanted();
    const saved = orchestration.document;
    const wasLive = liveSeats(saved).map(({ role, difficulty }) => `${role}/${difficulty}`);
    // Only seats in use reach the member file; one kept behind a switch waits until it is turned on.
    for (const { role, difficulty } of liveSeats(wanted)) {
      const seat = orchestrationSeat(wanted, role, difficulty);
      if (seat && (!wasLive.includes(`${role}/${difficulty}`) || !sameJson(seat, orchestrationSeat(saved, role, difficulty)))) await listSeatModel(seat);
    }
    const response = await fetch("/app/orchestration", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(wanted),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok || data.error) throw new Error(data.error || AN.L(`保存失败（${response.status}）`, `Save failed (${response.status})`));
    orchestration.document = wanted;
    orchestration.files = data.files;
    orchestration.bosses = data.bosses;
    orchestrationDraft = null;
    customTouched = false;
    splitOpened = new Set();
    // The choice is saved even when a rules file could not take it; the list below shows which one.
    const failed = data.failures || [];
    if (failed.length) throw new Error(AN.L(`已保存，但 ${failed.map((f) => `${f.path}（${f.error}）`).join("、")} 未能写入`, `Saved, but failed to write ${failed.map((f) => `${f.path} (${f.error})`).join(", ")}`));
  }

  // The rules as the draft would write them: the mode's text, then the lines of the roles turned on
  // in catalog order, then the image line, with T3 sectioning when T3 orchestration is on.
  function orchestrationPresetText(mode, includeT3 = true) {
    const presets = orchestration?.presets;
    const t3On = Boolean(includeT3 && window.AgentNaveT3Seats?.enabled?.() && presets?.t3?.[mode]);
    const t3Suffix = t3On ? presets.t3[mode] : "";
    const nonT3Heading = t3On ? (presets.non_t3_heading || "") : "";
    if (mode === "custom") return t3Suffix ? t3Suffix.trim() : undefined;
    const text = presets?.[mode];
    if (!text) return text;
    const shown = orchestrationShown();
    if (mode === "meetings") {
      const { head, tail, meetings_asked = "", meetings_own = "" } = presets.meetings_parts || {};
      return head == null ? text : head + (shown?.meetings_options?.meetings ? meetings_own : meetings_asked) + meetingLine(shown) + tail;
    }
    const parts = mode === "decision" ? presets.decision_parts : mode === "split" ? presets.split_parts : null;
    let base;
    if (parts) {
      const options = shown?.[`${mode}_options`];
      base = parts.base
        + (options?.parallel_writes ? parts.parallel_writes_own : parts.parallel_writes_asked)
        + (parts.checks || "")
        + (options?.meetings ? parts.meetings_own : parts.meetings_asked)
        + meetingLine(shown)
        + nonT3Heading
        + (parts.tail || "");
      if (shown?.lead_only && presets.lead) base = base.replace(presets.lead.asked, presets.lead.always);
    } else {
      base = presets.role_base?.[mode] ?? text.replace(presets.image_generation || "", "");
    }
    if (mode === "decision") return base + t3Suffix;
    const enabled = enabledGroups(shown);
    const sections = roleCatalog().filter((role) => enabled.includes(role.group))
      .map((role) => roleSection(presets, shown, role.group));
    const image = presets.image_generation || "";
    return [base, ...sections, shown?.image_generation ? image : ""].join("") + t3Suffix;
  }

  function renderOrchestrationText() {
    const text = orchestrationPresetText(orchestrationView);
    const textFold = $("orchestration-text-fold");
    if (textFold) textFold.hidden = !text;
    $("orchestration-text").hidden = !text;
    $("orchestration-text").textContent = text || "";
  }

  function viewMode(mode) {
    const editor = $("custom-text");
    // The editor starts from the mode the user leaves; never copy the appended T3 suffix into it.
    if (mode === "custom" && !editor.value.trim()) {
      editor.value = orchestrationPresetText(orchestrationView === "off" ? "split" : orchestrationView, false) || "";
    }
    orchestrationView = mode;
    renderOrchestration();
    renderAdvancedBar();
  }

  // A row's mode lives in `apps` only where it differs from `preset`.
  function withModes(doc, preset, modes) {
    const apps = { ...doc.apps, ...modes };
    for (const key of Object.keys(apps)) if (apps[key] === preset) delete apps[key];
    const next = { ...doc, preset, apps };
    if (!Object.keys(apps).length) delete next.apps;
    return next;
  }

  function setBossMode(boss, mode) {
    const shown = orchestrationShown();
    orchestrationDraft = withModes(shown, shown.preset, Object.fromEntries(boss.keys.map((key) => [key, mode])));
    if (mode === "decision") window.AgentNaveDecisionSeats?.entered();
    renderOrchestration();
    advancedEdited();
  }

  // Rows keep the mode they had when the default for later bosses moves.
  function setDefaultMode(mode) {
    const shown = orchestrationShown();
    const modes = Object.fromEntries(orchestration.bosses.flatMap((boss) => boss.keys.map((key) => [key, bossMode(shown, boss)])));
    orchestrationDraft = withModes(shown, mode, modes);
    if (mode === "decision") window.AgentNaveDecisionSeats?.entered();
    renderOrchestration();
    advancedEdited();
  }

  // In the mode being looked at, a boss on it can be taken off (to 关闭), any other brought onto it.
  function modeChip(key, label, tile, mode, savedMode, onSet) {
    const on = mode === orchestrationView;
    const b = el("button", {
      type: "button", className: tile ? "skill-chip" : "skill-chip no-tile", dataset: { focusKey: key, pending: String(mode !== savedMode) },
      "aria-pressed": String(on), disabled: Boolean(orchestration.error),
    }, tile, label,
      mode !== savedMode ? el("small", {}, AN.L("待保存", "Unsaved")) : !on && mode !== "off" ? el("small", {}, AN.L(`现用${ORCHESTRATION_NAMES[mode]}`, `Using ${ORCHESTRATION_NAMES[mode]}`)) : null);
    b.addEventListener("click", () => {
      if (!on) onSet(orchestrationView);
      else if (mode !== "off") onSet("off");
    });
    return b;
  }

  function renderBossChips() {
    const box = $("orchestration-bosses");
    if (!orchestration) return box.replaceChildren();
    const focusKey = document.activeElement?.dataset?.focusKey;
    const shown = orchestrationShown();
    const saved = orchestration.document;
    box.replaceChildren(...orchestration.bosses.map((boss) => modeChip(`boss/${boss.keys[0]}`, boss.labels.join(AN.L("、", ", ")),
      AN.hostTile ? AN.hostTile(boss.mark, boss.labels[0]) : null,
      bossMode(shown, boss), bossMode(saved, boss), (mode) => setBossMode(boss, mode))),
    modeChip("boss/later", AN.L("之后新接入的 boss", "Future connected bosses"), null, shown.preset, saved.preset, setDefaultMode));
    if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  // The boss page saves a boss's choice for its own kind of CLI by itself; both the saved document
  // and a pending draft take it, so saving this page later keeps it.
  function adoptHostCli(hostCli) {
    const apply = (doc) => {
      if (!doc) return doc;
      const { host_cli: _, ...rest } = doc;
      return Object.keys(hostCli).length ? { ...rest, host_cli: hostCli } : rest;
    };
    if (orchestration?.document) orchestration.document = apply(orchestration.document);
    orchestrationDraft = apply(orchestrationDraft);
  }
  window.addEventListener("agentnave:host-cli", (event) => adoptHostCli(event.detail));

  function setOrchestrationSeat(role, difficulty, seat) {
    const shown = orchestrationShown();
    let value = seat;
    if (difficulty) {
      value = { ...(shown[role] || {}) };
      if (seat) value[difficulty] = seat;
      else delete value[difficulty];
    }
    orchestrationDraft = { ...shown, [role]: value };
    if (!Object.keys(value || {}).length) delete orchestrationDraft[role];
    renderOrchestration();
    advancedEdited();
  }

  // The list keeps its saved form when the switches come back to what is saved, so nothing is unsaved.
  function setRoleEnabled(group, on) {
    const shown = orchestrationShown();
    const saved = orchestration.document;
    const enabled = new Set(enabledGroups(shown));
    if (on) enabled.add(group);
    else enabled.delete(group);
    const next = { ...shown, enabled_roles: roleCatalog().map((role) => role.group).filter((key) => enabled.has(key)) };
    const same = enabled.size === enabledGroups(saved).length && enabledGroups(saved).every((key) => enabled.has(key));
    if (same && Object.hasOwn(saved, "enabled_roles")) next.enabled_roles = saved.enabled_roles;
    else if (same) delete next.enabled_roles;
    orchestrationDraft = next;
    renderOrchestration();
    advancedEdited();
  }

  // Bosses on a saved mode dispatch through the seats, each boss by its own row.
  // The employees' scene hint names them, so the page needs no paragraph of its own.
  function taskScopeNote() {
    const on = orchestration && !orchestration.error
      ? orchestration.bosses.filter((boss) => usesSeats(bossMode(orchestration.document, boss))) : [];
    const names = on.flatMap((boss) => boss.labels).join(AN.L("、", ", "));
    return on.length ? AN.L(`${names} 已启用编排，用“编排”里的工位派活；单独干活只提供给未启用编排的 boss。`, `${names} use orchestration and dispatch through the seats in Orchestration; solo tasks are offered only to bosses without orchestration.`) : "";
  }

  // One seat for easy and hard of a specialist that is not split; it has no medium seat.
  function setSharedSeat(group, seat) {
    const shown = orchestrationShown();
    orchestrationDraft = { ...shown, [group]: { easy: seat, hard: seat } };
    if (!seat) delete orchestrationDraft[group];
    renderOrchestration();
    advancedEdited();
  }

  // Joining the difficulties again keeps the hard seat, or the easy one when hard is unset, and drops medium.
  function setRoleSplit(group, on) {
    if (on) {
      splitOpened.add(group);
      renderOrchestration();
      return;
    }
    splitOpened.delete(group);
    const seats = orchestrationShown()[group];
    const seat = seats?.hard ?? seats?.easy ?? null;
    if (!seats?.medium && sameJson(seats?.easy ?? null, seats?.hard ?? null)) renderOrchestration();
    else setSharedSeat(group, seat);
  }

  function seatRow(group, label, name, seat, onSet, unset = SEAT_UNSET, scene = "task", sub = false) {
    return seatBlock(el("div", { className: `s-row${sub ? " is-sub" : ""}` },
      el("span", { className: "s-label" }, label),
      el("div", { className: "s-seat" }, ...seatSelects(group, name, seat, onSet, unset, undefined, scene))), group, name, seat, onSet, scene);
  }

  // A switch put back to what is saved restores the saved form of its key, so an explicit default is no edit.
  function setOption(key, matchesSaved, plain) {
    const saved = orchestration.document;
    const { [key]: _, ...rest } = orchestrationShown();
    const next = matchesSaved && Object.hasOwn(saved, key) ? saved[key] : plain;
    orchestrationDraft = next === undefined ? rest : { ...rest, [key]: next };
    renderOrchestration();
    advancedEdited();
  }

  // Whether the boss may call meetings without being asked in meetings-only mode (off by default).
  const meetingsOnlyOn = (doc) => doc.meetings_options?.meetings === true;
  const setMeetingsOnly = (on) => setOption("meetings_options", on === meetingsOnlyOn(orchestration.document), on ? { meetings: true } : undefined);

  // The split and decision modes' switch: the boss hands out all the work, small tasks too, and keeps only planning and checking.
  const setLeadOnly = (on) => setOption("lead_only", on === (orchestration.document.lead_only === true), on ? true : undefined);

  // The split and decision modes' own two switches, both off by default, saved as the ones that are on.
  const dispatchOptionOn = (doc, preset, field) => doc[`${preset}_options`]?.[field] === true;
  function setDispatchOption(preset, field, on) {
    const key = `${preset}_options`;
    const onlyOn = (options) => Object.fromEntries(Object.entries(options || {}).filter(([, value]) => value === true));
    const next = onlyOn({ ...orchestrationShown()[key], [field]: on });
    setOption(key, sameJson(next, onlyOn(orchestration.document[key])), Object.keys(next).length ? next : undefined);
  }

  // A switch lives in the control column; its words and hint are the row's label.
  function switchRow(focusKey, checked, onChange, label, tip, sub = false) {
    const id = `switch-${focusKey.replace(/[^\w-]/g, "-")}`;
    return el("div", { className: `s-row${sub ? " is-sub" : ""}` },
      el("label", { className: "s-label", htmlFor: id }, label, tip ? settingTip(tip) : null),
      el("input", { type: "checkbox", id, className: "switch-input", dataset: { focusKey }, checked, onChange: (e) => onChange(e.target.checked) }));
  }

  function renderSplitOptions(preset, editable) {
    const box = $("orchestration-split");
    box.hidden = !["meetings", "split", "decision"].includes(preset) || !editable;
    if (box.hidden) return box.replaceChildren();
    const shown = orchestrationShown();
    const meetingsTip = AN.L("打开后遇到有实质取舍或分歧的问题，boss 会自己召集几个员工开会讨论再定，会用到他们的额度；关闭时只在你要求时开会。", "On: when a choice has real trade-offs or disagreement, the boss calls a few employees into a meeting before deciding, using their quota. Off: meetings happen only when you ask.");
    if (preset === "meetings") {
      box.replaceChildren(switchRow("meetings/meetings", meetingsOnlyOn(shown), setMeetingsOnly, AN.L("boss 可主动开会", "Boss may call meetings"), meetingsTip));
      return;
    }
    box.replaceChildren(
      switchRow(`${preset}/parallel-writes`, dispatchOptionOn(shown, preset, "parallel_writes"), (on) => setDispatchOption(preset, "parallel_writes", on),
        AN.L("并行写入由 boss 判断", "Boss decides on parallel writes"),
        AN.L("打开后，计划确认后互不依赖的写入步骤由 boss 并行派给多个可写工位；关闭时并行写入须你对当前任务指明。", "On: once the plan is confirmed, the boss sends independent write steps to several writable seats at once. Off: parallel writes need you to say so for the task.")),
      switchRow(`${preset}/meetings`, dispatchOptionOn(shown, preset, "meetings"), (on) => setDispatchOption(preset, "meetings", on),
        AN.L("有分歧时主动开会", "Hold meetings on trade-offs"), meetingsTip),
      switchRow("lead-only", shown.lead_only === true, setLeadOnly, AN.L("boss 只负责编排（两个派活档共用）", "Boss only orchestrates (shared by both dispatch presets)"),
        AN.L("打开后小事也交给员工，boss 只拆分、写简报和验收；更慢，也更费员工额度。关闭时 boss 自己做小事，你也可以对某个任务说「这次只负责编排」。", "On: even small tasks go to employees, and the boss only splits work, writes briefs and checks results; slower and uses more employee quota. Off: the boss does small tasks itself, and you can still say \"only orchestrate this time\" for one task.")));
  }

  // Count and seats are saved together; no seat picked at the default count leaves the key out, and
  // a pick that comes back to what is saved restores the saved form. A smaller count drops the seats past it.
  function setMeeting({ count = meetingOf(orchestrationShown()).count, seats = meetingOf(orchestrationShown()).seats }) {
    const saved = orchestration.document;
    const { meeting: _, ...rest } = orchestrationShown();
    const kept = trimSeats(seats.slice(0, count));
    const next = kept.length ? { count, seats: kept } : { count };
    const empty = !kept.length && count === meetingDefaultCount();
    const same = Object.hasOwn(saved, "meeting") && sameJson(meetingOf(saved), { count, seats: kept });
    orchestrationDraft = same ? { ...rest, meeting: saved.meeting } : empty ? rest : { ...rest, meeting: next };
    renderOrchestration();
    advancedEdited();
  }

  function setMeetingSeat(index, seat) {
    const seats = Array.from({ length: Math.max(index + 1, meetingOf(orchestrationShown()).seats.length) },
      (_, i) => meetingOf(orchestrationShown()).seats[i] ?? null);
    seats[index] = seat;
    setMeeting({ seats });
  }

  function renderMeetingSettings(preset, editable) {
    const box = $("orchestration-meeting");
    box.hidden = !MEETING_MODES.includes(preset) || !editable || !orchestration.presets?.meeting_parts;
    if (box.hidden) return box.replaceChildren();
    const focusKey = document.activeElement?.dataset?.focusKey;
    const { count, seats } = meetingOf(orchestrationShown());
    const sizeLabel = AN.L("开会人数", "Meeting size");
    const sizes = el("div", { className: "segment-control", role: "radiogroup", "aria-label": sizeLabel },
      ...MEETING_COUNTS.map((n) => el("button", {
        type: "button", className: `segment-btn${n === count ? " active" : ""}`, role: "radio", dataset: { focusKey: `meeting/count/${n}` },
        "aria-checked": String(n === count), onClick: () => setMeeting({ count: n }),
      }, String(n))));
    box.replaceChildren(el("div", { className: "s-row" }, el("span", { className: "s-label" }, sizeLabel), sizes),
      ...Array.from({ length: count }, (_, i) => {
        const label = AN.L(`席位 ${i + 1}`, `Seat ${i + 1}`);
        return seatRow(`meeting/${i}`, label, label, seats[i] ?? null, (seat) => setMeetingSeat(i, seat),
          AN.L("由 boss 选", "Chosen by boss"), "discussion");
      }));
    if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  // A role's switch, and its seats while on; turned off, they stay saved out of sight. A specialist
  // shows one seat for both difficulties until it is split.
  function roleRows(entry, doc) {
    const { group, access } = entry;
    const name = roleName(entry);
    const on = enabledGroups(doc).includes(group);
    const general = isGeneral(entry);
    const split = general || seatsSplit(doc, group);
    const seats = !on ? [] : split
      ? DIFFICULTIES.map(([difficulty, level]) => seatRow(`${group}/${difficulty}`, level, `${name} · ${level}`,
        orchestrationSeat(doc, group, difficulty), (next) => setOrchestrationSeat(group, difficulty, next), SEAT_UNSET, "task", true))
      : [seatRow(`${group}/both`, AN.L("简单 · 困难", "Easy · Hard"), name,
        doc[group]?.hard ?? doc[group]?.easy, (next) => setSharedSeat(group, next), SEAT_UNSET, "task", true)];
    return [
      switchRow(`${group}/enabled`, on, (checked) => setRoleEnabled(group, checked), [name, el("span", { className: "badge badge-muted" }, ROLE_ACCESS[access])]),
      ...(on && !general ? [switchRow(`${group}/split`, split, (checked) => setRoleSplit(group, checked), AN.L("区分难度", "By difficulty"),
        AN.L("关闭时这个专项各档共用一个模型；打开后简单、困难各选一个，中等可选。", "Off: every level of this specialist shares one model. On: pick one each for easy and hard; medium is optional."), true)] : []),
      ...seats];
  }

  // General roles, then the specialists, each under its own heading.
  function roleSections(doc) {
    const section = (title, entries) => el("div", { className: "s-group" },
      el("h2", { className: "s-title" }, title), el("div", { className: "s-card" }, ...entries.map((entry) => el("div", { className: "s-wrap" }, ...roleRows(entry, doc)))));
    return [
      section(AN.L("通用", "General"), roleCatalog().filter(isGeneral)),
      section(AN.L("专项", "Specialist"), roleCatalog().filter((entry) => !isGeneral(entry))),
    ];
  }

  function renderOrchestration() {
    const editor = $("custom-text");
    if (orchestration && !orchestration.error && editor.value?.trim()) {
      editor.value = customRoleText(editor.value, orchestrationShown());
    }
    const shown = orchestrationShown();
    const preset = orchestrationView;
    for (const button of $("orchestration-presets").querySelectorAll("button")) {
      const active = button.dataset.preset === preset;
      button.classList.toggle("active", active);
      button.setAttribute("aria-checked", String(active));
      button.disabled = !orchestration || Boolean(orchestration.error);
    }
    $("orchestration-effect").setAttribute("aria-label", orchestration ? ORCHESTRATION_EFFECTS[preset] : "");
    renderBossChips();
    renderOrchestrationText();
    // An unread or broken file cannot take edits: nothing here could be saved or discarded.
    const editable = orchestration && !orchestration.error;
    renderSplitOptions(preset, editable);
    renderMeetingSettings(preset, editable);
    $("orchestration-custom").hidden = preset !== "custom" || !editable;
    const roles = $("orchestration-roles");
    roles.hidden = !editable;
    if (!roles.hidden) {
      const focusKey = document.activeElement?.dataset?.focusKey;
      const { role, difficulty, name } = IMAGE_SEAT;
      const setImage = (next) => setOrchestrationSeat(role, difficulty, next);
      // In the decision mode the user's own seat catalog stands in for the fixed seats.
      const catalog = preset === "decision" ? window.AgentNaveDecisionSeats?.panel() : null;
      const fixed = !usesSeats(preset) || preset === "decision" || !roleCatalog().length ? [] : roleSections(shown);
      roles.replaceChildren(...(catalog ? [catalog] : fixed),
        // The decision mode's image seats are cards of its catalog, not the fixed one.
        ...(preset === "meetings" || preset === "decision" ? [] : [el("div", { className: "s-group" }, el("div", { className: "s-card" }, seatBlock(el("div", { className: "s-row" },
          el("span", { className: "s-label" }, name,
            settingTip(AN.L("这里选的是和 CLI 对话的模型；图片由那个 CLI 调用它自己能用的生图服务来出。", "This picks the model you talk to in the CLI; the image itself comes from whatever image service that CLI can call."))),
          el("div", { className: "s-seat" }, ...seatSelects(`${role}/${difficulty}`, name, shown[role], setImage, SEAT_UNSET, AN.L("生图的对话模型", "Chat model for image generation")))),
        `${role}/${difficulty}`, name, shown[role], setImage)))]));
      if (focusKey) roles.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
    }
    renderRuleFiles();
    window.AgentNaveT3Seats?.render();
    S.syncScopeViews?.();
  }

  function renderRuleFiles() {
    const list = $("rule-files");
    if (!orchestration) return list.replaceChildren();
    list.replaceChildren(...orchestration.files.map((file) => {
      const state = !file.target ? null
        : file.current ? ["written", AN.L("已写入", "Written")]
          : file.blocked ? ["failed", AN.L("经过符号链接，无法写入", "Goes through a symlink; cannot write")] : ["failed", AN.L("未写入", "Not written")];
      return el("div", { className: "s-row" },
        el("span", { className: "s-label" }, file.readers.join(AN.L("、", ", "))),
        el("div", { className: "s-control" },
          el("code", { className: "s-value is-mono" }, file.path),
          state && el("span", { className: `rule-state ${state[0]}` }, state[1]),
          ruleOpenError?.path === file.path && el("span", { className: "rule-state failed" }, ruleOpenError.message),
          file.exists
            ? el("button", { type: "button", onClick: () => openRuleFile(file) }, AN.L("打开", "Open"))
            : el("span", { className: "s-muted" }, AN.L("未找到", "Not found"))));
    }));
  }

  async function openRuleFile(file) {
    try {
      const response = await fetch("/app/rules/open", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
        body: JSON.stringify({ path: file.path }),
      });
      const data = await response.json();
      if (!response.ok || data.error) throw new Error(data.error || AN.L(`打开失败（${response.status}）`, `Open failed (${response.status})`));
      ruleOpenError = null;
    } catch (error) {
      ruleOpenError = { path: file.path, message: error.message };
    }
    renderRuleFiles();
  }

  // agentnave-review roles, in the order the Skill grades them; saved in review.json.
  const REVIEW_ROLES = [
    ["correctness-reviewer", AN.L("正确性", "Correctness"), AN.L("每次评审必派：行为、回归、边界", "Always dispatched: behavior, regressions, edge cases")],
    ["correctness-reviewer-easy", AN.L("正确性（简单）", "Correctness (easy)"), AN.L("R1 低风险改动用", "For low-risk R1 changes"), AN.L("同正确性", "Same as Correctness")],
    ["architecture-reviewer", AN.L("架构", "Architecture"), AN.L("职责、耦合、公开接口", "Responsibilities, coupling, public interfaces")],
    ["security-reviewer", AN.L("安全", "Security"), AN.L("信任边界、授权、数据流", "Trust boundaries, authorization, data flow")],
    ["performance-reviewer", AN.L("性能", "Performance"), AN.L("热路径、规模、资源", "Hot paths, scale, resources")],
    ["test-reliability-reviewer", AN.L("测试可靠性", "Test reliability"), AN.L("测试能否护住行为", "Whether tests protect behavior")],
    ["simplicity-reviewer", AN.L("简化", "Simplicity"), AN.L("多余的复杂度", "Unnecessary complexity")],
    ["specialist-reviewer", AN.L("专项", "Specialist"), AN.L("其他明确的专业领域", "Other specific domain expertise")],
    ["adversarial-verifier", AN.L("反证", "Adversarial"), AN.L("高风险改动修完后尝试推翻结论", "Attempts to disprove conclusions after high-risk fixes")],
  ];
  let seatsDraft = null; // Review seats as picked; `reviewSeats` holds what is saved.
  // The one review model picked for every role left unset, and the roles it filled in this draft,
  // which follow it until picked one by one. The easy correctness role stays as it is: unset, it
  // already runs the correctness seat.
  let uniformReview = null;
  let uniformFilled = new Set();
  const seatsShown = () => seatsDraft || S.reviewSeats;
  const seatsChanged = () => !sameJson(seatsShown(), S.reviewSeats);

  // Every seat in use with a model its CLI's pool left out, which blocks saving its section, or one
  // the CLI no longer lists, which is only shown. Orchestration seats wait unused while no boss runs
  // orchestration.
  function seatConflicts() {
    const review = REVIEW_ROLES.map(([role, name]) => ({ section: "review", name: AN.L(`${name}评审`, `${name} review`), seat: seatsShown()[role] }));
    const doc = orchestrationShown();
    const live = Boolean(doc) && [doc.preset, ...Object.values(doc.apps || {})].some(usesSeats);
    const roles = !live ? [] : liveSeats(doc)
      .map(({ role, difficulty, name }) => ({ section: "orchestration", name, seat: orchestrationSeat(doc, role, difficulty) }));
    return [...review, ...roles]
      .flatMap((one) => [one, ...(one.seat?.fallbacks || []).map((seat, i) => ({ ...one, name: AN.L(`${one.name} · 备选 ${i + 1}`, `${one.name} · fallback ${i + 1}`), seat }))])
      .filter(({ seat }) => seat && (outOfPool(seat.provider, seat.model) || unlisted(seat.provider, seat.model)))
      .map((conflict) => ({ ...conflict, blocking: outOfPool(conflict.seat.provider, conflict.seat.model) }));
  }
  const seatConflictText = ({ name, seat }) => AN.L(`${name} 用的 ${seat.model} 不在 ${PROVIDER_NAMES[seat.provider]} 的可用模型里`, `${name} uses ${seat.model}, which is not in the available models of ${PROVIDER_NAMES[seat.provider]}`);
  const memberSaveErrors = () => configurationErrors();

  // A picked model missing from that CLI's configured models is added to the last saved
  // members.json, where the server looks for it; unsaved 员工 edits stay unsaved. A CLI with a pool
  // already configures all it may run, so a model it left out is refused instead.
  async function listSeatModel(seat) {
    for (const pick of [seat, ...(seat.fallbacks || [])]) await listPickModel(pick);
  }
  async function listPickModel(seat) {
    const add = (doc) => {
      doc.clis ||= {};
      doc.clis[seat.provider] ||= { models: {} };
      doc.clis[seat.provider].models ||= {};
      doc.clis[seat.provider].models[seat.model] ||= {};
      return doc;
    };
    // Until 员工 is read there is nothing to add to: an empty document would replace the file.
    if (!S.savedSnapshot && !S.documentError) throw new Error(AN.L("员工配置还在读取，请稍后再保存", "Employee settings are still loading; please save in a moment"));
    const saved = JSON.parse(S.savedSnapshot || JSON.stringify({ version: 1, clis: {}, modes: {} }));
    const pool = saved.clis?.[seat.provider]?.allowed_models;
    if (pool && !pool.includes(seat.model)) throw new Error(AN.L(`${seat.model} 不在 ${PROVIDER_NAMES[seat.provider]} 的可用模型里`, `${seat.model} is not in the available models of ${PROVIDER_NAMES[seat.provider]}`));
    if (Object.hasOwn(saved.clis?.[seat.provider]?.models || {}, seat.model)) return;
    if (S.documentError) throw new Error(AN.L("配置文件有错误，请先在“员工”里修复", "Configuration file has errors; fix them in “Employees” first"));
    await apiPostMembers(add(saved));
    S.savedSnapshot = JSON.stringify(saved);
    add(S.currentDoc);
    confirmScenes();
    S.refreshDirty();
  }

  async function saveSeats() {
    const seats = seatsShown();
    for (const [role] of REVIEW_ROLES) {
      if (seats[role] && !sameJson(seats[role], S.reviewSeats[role])) await listSeatModel(seats[role]);
    }
    await apiPostReview({ version: 1, seats });
    S.reviewSeats = seats;
    seatsDraft = null;
    uniformReview = null;
    uniformFilled = new Set();
    S.reviewSaves += 1;
  }

  function setReviewSeat(role, seat) {
    if (S.reviewError) {
      showNotice(AN.L(`${S.reviewError}；请先修复或删除这个文件`, `${S.reviewError}; fix or delete this file first`), true);
      return renderReviewSeats();
    }
    const seats = { ...seatsShown() };
    if (seat) seats[role] = seat;
    else delete seats[role];
    uniformFilled.delete(role);
    seatsDraft = seats;
    renderReviewSeats();
    advancedEdited();
  }

  function setUniformReview(seat) {
    if (S.reviewError) {
      showNotice(AN.L(`${S.reviewError}；请先修复或删除这个文件`, `${S.reviewError}; fix or delete this file first`), true);
      return renderReviewSeats();
    }
    const seats = { ...seatsShown() };
    for (const [role] of REVIEW_ROLES) {
      if (role === "correctness-reviewer-easy" || (seats[role] && !uniformFilled.has(role))) continue;
      if (seat) {
        seats[role] = seat;
        uniformFilled.add(role);
      } else {
        delete seats[role];
        uniformFilled.delete(role);
      }
    }
    uniformReview = seat;
    seatsDraft = seats;
    renderReviewSeats();
    advancedEdited();
  }

  function reviewSelect(key, label, value, options, onPick, disabled = false) {
    return el("select", {
      dataset: { focusKey: key }, "aria-label": label, disabled,
      onChange: (e) => onPick(e.target.value),
    }, options.map(([v, text, off]) => el("option", { value: v, selected: v === value, disabled: Boolean(off) }, text)));
  }

  const seatModels = (provider) => listedPool(provider)
    ? listedPool(provider).filter((id) => getAllModelIds(provider).includes(id)) : getAllModelIds(provider);

  // The CLIs a seat may pick: only CLIs Employees has enabled; a meeting seat follows the 开会 scene.
  // A boss's own CLI gets no exception: seats are shared by every boss (ADR-0053, 2026-11-17).
  function cliUsable(scene) {
    // Orchestrated bosses use confirmed CLI configurations, not the hidden solo-task switches.
    const doc = orchestrationShown();
    const orchestrated = doc && [doc.preset, ...Object.values(doc.apps || {})].some(usesSeats);
    const inScene = (p, scene) => Object.hasOwn(S.currentDoc.modes?.[scene] || {}, p);
    const confirmed = (p) => {
      if (SCENE_IDS.every(isRestricted)) return SCENE_IDS.some((scene) => inScene(p, scene));
      if (isRestricted("task")) return inScene(p, "task");
      return Object.hasOwn(S.currentDoc.clis || {}, p) || SCENE_IDS.some((scene) => inScene(p, scene));
    };
    const usable = (p) => scene !== "task" ? known(p) && slot(scene, p).enabled
      : known(p) && (orchestrated ? confirmed(p) : slot("task", p).enabled);
    return usable;
  }

  // A seat saves the level it runs at, as a member does; a named account is saved by its id, and
  // the default account by nothing.
  function seatPick(next, model, scene, account) {
    const effort = savedEffort(next, model, sceneEffort(scene, next, model));
    return { provider: next, ...(account ? { account } : {}), model, ...(effort ? { effort } : {}) };
  }
  const withFallbacks = (pick, list) => (list?.length ? { ...pick, fallbacks: list } : pick);
  // What tells two picks apart: effort does not, an account does.
  const pairKey = (provider, model, account) => `${provider}\u0000${account || ""}\u0000${model}`;
  const pickKey = (pick) => pairKey(pick.provider, pick.model, pick.account);
  // The accounts a CLI can run as, the default one (null) first; only Claude has named ones.
  const accountsOf = (provider) => [null, ...(provider === "claude" ? S.accounts.map((account) => account.id) : [])];
  const accountLabel = (id) => S.accounts.find((account) => account.id === id)?.label;
  // A CLI and account as one menu value; the default account is the CLI alone.
  const cliValue = (provider, account) => (account ? `${provider}@${account}` : provider);
  const parseCli = (value) => {
    const [provider, account = null] = value.split("@");
    return { provider, account };
  };
  const MAX_FALLBACKS = 3;

  // The first CLI, account and model no pick of the seat has taken, a CLI other than the seat's own
  // first.
  function freshPick(scene, taken, own) {
    const usable = cliUsable(scene);
    for (const provider of PROVIDERS.filter(usable).sort((a, b) => (a === own) - (b === own))) {
      for (const account of accountsOf(provider)) {
        const free = seatModels(provider).filter((model) => !taken.has(pairKey(provider, model, account)));
        if (!free.length) continue;
        const preferred = slot(scene, provider).model || nativeDefault(provider).model;
        return seatPick(provider, free.includes(preferred) ? preferred : free[0], scene, account);
      }
    }
    return null;
  }

  // A task seat's row gets a button in its last column that adds a fallback while there are fewer than
  // three; with fallbacks, the row comes with one line under it for each, in order: the same three
  // selects as the seat and a button to remove it.
  function seatBlock(row, key, name, seat, onSet, scene = "task") {
    if (!seat?.provider || !seat?.model || scene !== "task") return row;
    const list = seat.fallbacks || [];
    const store = (next) => {
      const { fallbacks, ...rest } = seat;
      onSet(next.length ? { ...rest, fallbacks: next } : rest);
    };
    const takenBy = (skip) => new Set([seat, ...list.filter((_, i) => i !== skip)].map(pickKey));
    if (list.length < MAX_FALLBACKS) {
      const fresh = freshPick(scene, takenBy(-1), seat.provider);
      (row.querySelector?.(".s-seat") || row).appendChild(el("button", {
        type: "button", disabled: !fresh, dataset: { focusKey: `${key}/fb-add` }, onClick: () => store([...list, fresh]),
      }, AN.L("添加备选", "Add fallback")));
    }
    if (!list.length) return row;
    return el("div", { className: "s-wrap" }, row, ...list.map((pick, index) => {
      const label = AN.L(`备选 ${index + 1}`, `Fallback ${index + 1}`);
      const set = (next) => store(list.map((one, i) => (i === index ? next : one)));
      return el("div", { className: "s-row is-sub", dataset: { fallback: "true" } },
        el("span", { className: "s-label" }, label),
        el("div", { className: "s-seat" },
          ...seatSelects(`${key}/fb${index}`, `${name} · ${label}`, pick, set, null, undefined, scene, takenBy(index)),
          el("button", {
            type: "button", dataset: { focusKey: `${key}/fb${index}/remove` }, "aria-label": AN.L(`移除${name}的${label}`, `Remove ${label} of ${name}`),
            onClick: () => store(list.filter((_, i) => i !== index)),
          }, AN.L("移除", "Remove"))));
    }));
  }

  // A seat's CLI, then one of that CLI's models, then an effort or the member's default.
  function seatSelects(key, name, seat, onSet, unset = AN.L("自动", "Auto"), modelLabel = AN.L(`${name}的模型`, `Model for ${name}`), scene = "task", taken = new Set((seat?.fallbacks || []).map(pickKey))) {
    const provider = seat?.provider || "";
    const usable = cliUsable(scene);
    const note = (p) => !usable(p) ? AN.L("（员工里已停用）", " (disabled in Employees)")
      : seatModels(p).length ? "" : AN.L("（没有可选模型）", " (no models available)");
    const free = (p, account) => seatModels(p).filter((m) => !taken.has(pairKey(p, m, account)));
    // A meeting seat names only its CLI; a task seat also names the account of a CLI that has some.
    const named = scene === "task";
    const clis = PROVIDERS.filter((p) => usable(p) || p === provider).flatMap((p) => {
      const accounts = named ? accountsOf(p) : [null];
      return accounts.map((account) => {
        const current = p === provider && (account || null) === (seat?.account || null);
        const name = PROVIDER_NAMES[p] + (account ? AN.L(`（${accountLabel(account)}）`, ` (${accountLabel(account)})`) : "");
        return [cliValue(p, account), name + note(p), (Boolean(note(p)) || !free(p, account).length) && !current];
      });
    });
    // A seat that names an account since removed keeps showing it, so the file is never changed unseen.
    if (named && seat?.account && !accountLabel(seat.account)) {
      clis.push([cliValue(provider, seat.account), AN.L(`${PROVIDER_NAMES[provider]}（已移除的账号）`, `${PROVIDER_NAMES[provider]} (removed account)`), true]);
    }
    const shownCli = seat ? cliValue(provider, seat.account) : "";
    // A meeting seat stores only its CLI and shows what Employees sets for it in that scene.
    if (scene === "discussion") {
      // As the server runs it: nothing while 开会 has the CLI off, and a model list needs a default.
      const current = provider ? slot(scene, provider) : { enabled: false };
      const model = current.enabled ? current.model || (current.allowed ? "" : nativeDefault(provider).model) || "" : "";
      const effort = model ? current.effort || cliEffort(provider, model) || "" : "";
      const shown = (value) => [[value, value || "—"]];
      return [
        reviewSelect(`${key}/cli`, AN.L(`${name}的 CLI`, `CLI for ${name}`), provider, [["", unset], ...clis], (next) => onSet(next ? { provider: next } : null)),
        reviewSelect(`${key}/model`, modelLabel, model, shown(model), () => {}, true),
        reviewSelect(`${key}/effort`, AN.L(`${name}的思考档位`, `Effort for ${name}`), effort, shown(effort), () => {}, true),
      ];
    }
    const models = provider ? seatModels(provider) : [];
    const excluded = seat?.model && (outOfPool(provider, seat.model) || unlisted(provider, seat.model));
    if (seat?.model && !excluded && !models.includes(seat.model)) models.unshift(seat.model);
    const levels = seat?.model ? effortLevels(provider, seat.model) : [];
    if (seat?.effort && !levels.includes(seat.effort)) levels.push(seat.effort);
    const seatModel = (next, model, account) => withFallbacks(seatPick(next, model, scene, account), seat?.fallbacks);
    const pickCli = (value) => {
      if (!value) return onSet(null);
      const { provider: next, account } = parseCli(value);
      const ids = free(next, account);
      const preferred = slot(scene, next).model || nativeDefault(next).model;
      onSet(seatModel(next, ids.includes(preferred) ? preferred : ids[0], account));
    };
    const pickModel = (model) => onSet(seatModel(provider, model, seat?.account));
    const pickEffort = (effort) => onSet({ ...seat, effort });
    const efforts = !seat || !levels.length ? [["", "—"]]
      : [...(seat.effort ? [] : [["", AN.L("选一个档位", "Choose effort"), true]]), ...levels.map((l) => [l, l])];
    return [
      reviewSelect(`${key}/cli`, AN.L(`${name}的 CLI`, `CLI for ${name}`), shownCli, [...(unset === null ? [] : [["", unset]]), ...clis], pickCli),
      reviewSelect(`${key}/model`, modelLabel, seat?.model || "",
        seat ? [...(excluded ? [[seat.model, AN.L(`${seat.model}（不在可用模型里）`, `${seat.model} (not available)`), true]] : []), ...models.map((m) => [m, m, taken.has(pairKey(provider, m, seat.account)) && m !== seat.model])] : [["", "—"]], pickModel, !seat),
      reviewSelect(`${key}/effort`, AN.L(`${name}的思考档位`, `Effort for ${name}`), seat?.effort || "", efforts, pickEffort, !seat || !levels.length),
    ];
  }

  // One row per role.
  function renderReviewSeats() {
    const box = $("review-seats");
    if (!box) return;
    const focusKey = document.activeElement?.dataset?.focusKey;
    const shown = seatsShown();
    const uniformName = AN.L("统一评审模型", "One review model");
    const uniform = el("div", { className: "s-row" },
      el("span", { className: "s-label" }, uniformName,
        settingTip(AN.L("给还没选模型的评审角色一次填上这个模型；已经单独选过的角色和「正确性（简单）」不变。", "Fills this model into every review role that has none yet; roles you already picked and Correctness (easy) stay as they are."))),
      el("div", { className: "s-seat" }, ...seatSelects("review/uniform", uniformName, uniformReview, setUniformReview, AN.L("未选择", "Not chosen"))));
    const rows = REVIEW_ROLES.map(([role, name, hint, unset]) => {
      const seat = shown[role];
      const label = AN.L(`${name}评审`, `${name} review`);
      const set = (next) => setReviewSeat(role, next);
      return seatBlock(el("div", { className: "s-row" },
        el("span", { className: "s-label" }, name, settingTip(hint)),
        el("div", { className: "s-seat" }, ...seatSelects(role, label, seat, set, unset))), role, label, seat, set);
    });
    box.replaceChildren(uniform, ...rows);
    window.AgentNaveT3Seats?.renderReview();
    S.syncScopeViews?.();
    if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  // What 保存设置 would write, by section.
  function advancedChanges() {
    return [
      window.AgentNaveSkills?.changed() && AN.L("评审 Skill 安装", "Review Skill"),
      seatsChanged() && AN.L("评审工位", "Review seats"),
      orchestrationChanged() && AN.L("编排", "Orchestration"),
      window.AgentNaveDecisionSeats?.changed() && AN.L("决策模型推荐工位", "Decision-model seats"),
      window.AgentNaveT3Seats?.seatsChanged() && "T3 Code",
      window.AgentNaveT3Seats?.reviewChanged() && AN.L("T3 评审工位", "T3 review seats"),
    ].filter(Boolean);
  }

  const customEmpty = () => usesCustom(orchestrationShown()) && !$("custom-text").value.trim();
  // A changed section cannot save while one of its seats is outside its CLI's pool.
  const blockedSeat = () => seatConflicts().find(({ section, blocking }) => blocking && (section === "review" ? seatsChanged() : orchestrationChanged()));

  function renderAdvancedBar() {
    const changes = advancedChanges();
    const blocked = blockedSeat();
    const decisionProblem = window.AgentNaveDecisionSeats?.changed() ? window.AgentNaveDecisionSeats.problem() : null;
    const t3Problem = window.AgentNaveT3Seats?.changed() ? window.AgentNaveT3Seats.problem() : null;
    const invalid = (orchestrationChanged() && customEmpty()) || Boolean(blocked) || Boolean(decisionProblem) || Boolean(t3Problem);
    $("advanced-dirty").textContent = changes.length ? AN.L(`未保存：${changes.join("、")}`, `Unsaved: ${changes.join(", ")}`) : "";
    $("advanced-badge").hidden = !(orchestrationChanged() || window.AgentNaveDecisionSeats?.changed() || window.AgentNaveT3Seats?.seatsChanged());
    $("skills-badge").hidden = !(window.AgentNaveSkills?.changed() || seatsChanged() || window.AgentNaveT3Seats?.reviewChanged());
    $("advanced-save").disabled = S.busy || !changes.length || invalid;
    $("advanced-discard").disabled = S.busy || !changes.length;
    const feedback = $("advanced-feedback");
    if (invalid) {
      feedback.textContent = blocked ? seatConflictText(blocked)
        : (orchestrationChanged() && customEmpty()) ? AN.L("自定义规则不能为空；不想写入时选「关闭」", "Custom rules cannot be empty; choose “Off” if you don’t want to write rules")
        : decisionProblem || t3Problem;
      feedback.className = "save-feedback error";
      feedback.dataset.validation = "true";
    } else if (feedback.dataset.validation === "true") {
      delete feedback.dataset.validation;
      feedback.textContent = "";
      feedback.className = "save-feedback";
    }
    S.syncScopeViews?.();
  }

  // An edit makes the last save's result stale.
  function advancedEdited() {
    const feedback = $("advanced-feedback");
    delete feedback.dataset.validation;
    feedback.textContent = "";
    feedback.className = "save-feedback";
    S.updateControls();
  }

  function discardAdvanced() {
    seatsDraft = null;
    uniformReview = null;
    uniformFilled = new Set();
    orchestrationDraft = null;
    customTouched = false;
    splitOpened = new Set();
    $("custom-text").value = orchestration?.document.custom ?? "";
    if (orchestration) orchestrationView = savedView();
    window.AgentNaveSkills?.discard();
    window.AgentNaveDecisionSeats?.discard();
    window.AgentNaveT3Seats?.discard();
    renderOrchestration();
    renderReviewSeats();
    advancedEdited();
  }

  async function saveAdvanced() {
    if (S.busy || !advancedChanges().length || (orchestrationChanged() && customEmpty()) || blockedSeat()) return;
    if (window.AgentNaveDecisionSeats?.changed() && window.AgentNaveDecisionSeats.problem()) return;
    if (window.AgentNaveT3Seats?.changed() && window.AgentNaveT3Seats.problem()) return;
    S.busy = true;
    $("pane-advanced").inert = $("pane-orchestration").inert = true;
    S.updateControls();
    const feedback = $("advanced-feedback");
    feedback.textContent = AN.L("正在保存…", "Saving…");
    feedback.className = "save-feedback";
    const failures = [];
    // True when there was nothing to save or it was saved.
    const attempt = async (changed, name, save) => {
      if (!changed()) return true;
      try {
        await save();
        return true;
      } catch (error) {
        failures.push(AN.L(`${name}：${error.message}`, `${name}: ${error.message}`));
        return false;
      }
    };
    await attempt(seatsChanged, AN.L("评审工位", "Review seats"), saveSeats);
    // The seat catalog first: a boss in the decision mode must not find it empty, so the modes are
    // saved only once the catalog is, or when it had no change to save.
    const catalogSaved = await attempt(() => window.AgentNaveDecisionSeats?.changed(), AN.L("决策模型推荐工位", "Decision-model seats"), window.AgentNaveDecisionSeats?.save);
    if (catalogSaved) await attempt(orchestrationChanged, AN.L("编排", "Orchestration"), saveOrchestration);
    else if (orchestrationChanged()) failures.push(AN.L("编排：档位未保存，因为决策模型推荐工位没有保存成功", "Orchestration: modes not saved, because the decision-model seats were not saved"));
    await attempt(() => window.AgentNaveT3Seats?.changed(), AN.L("T3 Code", "T3 Code"), window.AgentNaveT3Seats?.save);
    failures.push(...await window.AgentNaveSkills.save());
    S.busy = false;
    $("pane-advanced").inert = $("pane-orchestration").inert = false;
    feedback.textContent = failures.length ? failures.join(AN.L("；", "; ")) : AN.L("设置已保存", "Settings saved");
    feedback.className = `save-feedback ${failures.length ? "error" : "success"}`;
    renderOrchestration();
    renderReviewSeats();
    S.updateControls();
  }

  function bindAdvanced() {
    for (const button of $("orchestration-presets").querySelectorAll("button")) {
      button.addEventListener("click", () => {
        if (button.dataset.preset !== orchestrationView) viewMode(button.dataset.preset);
      });
    }
    $("custom-text").addEventListener("input", () => {
      customTouched = true;
      advancedEdited();
    });
    $("advanced-save").addEventListener("click", saveAdvanced);
    $("advanced-discard").addEventListener("click", discardAdvanced);
  }

  Object.assign(S, {
    orchestrationWanted, orchestrationChanged, loadOrchestration, saveOrchestration, orchestrationPresetText,
    renderOrchestrationText, orchestrationSupportsT3: () => usesSeats(orchestrationView),
    viewMode, setOrchestrationSeat, taskScopeNote, renderOrchestration, renderRuleFiles, seatsShown, seatConflicts,
    seatConflictText, memberSaveErrors, listSeatModel, setReviewSeat, seatSelects, seatBlock, renderReviewSeats,
    advancedChanges, renderAdvancedBar, advancedEdited, discardAdvanced, bindAdvanced, REVIEW_ROLES,
    orchestrationShown, bossMode, setBossMode, liveSeats, roleCatalog, roleName, orchestrationSeat, seatModels,
    reviewSeatsChanged: seatsChanged,
  });
  S.share({
    orchestration: [() => orchestration, (v) => { orchestration = v; }],
  });
}
