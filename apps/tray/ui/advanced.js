"use strict";
// Settings -> 高级: Skills, orchestration and review seats. Viewing and picking
// only change the drafts here; nothing is written until 保存设置. Sections save independently, so
// one that fails stays unsaved for another try while the others land.

// Orchestration lives in orchestration.json: the rules text each boss gets, by mode (`apps`, with
// `preset` for rows without one, and bosses connected later), the user's own text, which catalog
// roles are on, and the role and image generation seats boss reads from list_members. The App writes
// the modes into the connected boss apps' global rules. The rules files are listed one row per file
// with every app that reads it.
const ORCHESTRATION_EFFECTS = {
  off: AN.L("不写入全局规则，boss 按自己的习惯派发。", "Does not write global rules; the boss dispatches in its own way."),
  split: AN.L("写入所选 boss 的全局规则：boss 未被点名也会把活派给已开启的角色，会用到它们的额度；同时改多处文件要你同意。", "Writes to selected bosses' global rules: even when not named, the boss delegates to the roles turned on, using their quota; changing several things at once needs your consent."),
  parallel: AN.L("在分工协作之外，boss 自己决定同时改多处文件，额度用得更多。", "Beyond split work, the boss decides on its own to change several things at once, using more quota."),
  custom: AN.L("写入你自己的规则，起始内容是选自定义前的档位。", "Writes your own rules, starting from the preset selected before Custom."),
};
const DIFFICULTIES = [["easy", AN.L("简单", "Easy")], ["hard", AN.L("困难", "Hard")]];
const ROLE_ACCESS = { read: AN.L("只读", "Read-only"), write: AN.L("可写", "Writable") };
const SEAT_UNSET = AN.L("未配置", "Not configured");
// One seat with no difficulty and no switch, its model the one the CLI talks to, which calls the image service.
const IMAGE_SEAT = { role: "image_generation", difficulty: null, name: AN.L("生图（可选）", "Image generation (optional)") };
const orchestrationSeat = (doc, role, difficulty) => difficulty ? doc[role]?.[difficulty] : doc[role];
const ORCHESTRATION_NAMES = { off: AN.L("关闭", "Off"), split: AN.L("分工协作", "Split"), parallel: AN.L("并行分工", "Parallel"), custom: AN.L("自定义", "Custom") };
let orchestration = null; // { document, error, presets, roles, files, bosses } from GET /app/orchestration: what is saved.
let orchestrationDraft = null; // The document as picked, without the custom text, which the editor holds.
let orchestrationView = "off"; // The mode being looked at; looking changes nothing.
let ruleOpenError = null; // { path, message }: why a rules file did not open, shown on its row.
let customTouched = false; // Typed into the editor, so its text is saved even with no boss on it yet.
let splitOpened = new Set(); // Specialists the user split by difficulty while both seats are still the same.
const orchestrationShown = () => orchestrationDraft || orchestration?.document;
const usesCustom = (doc) => doc?.preset === "custom" || Object.values(doc?.apps || {}).includes("custom");
// The fixed catalog, or nothing from a page that was not given one: its roles are then left as saved.
const roleCatalog = () => orchestration?.roles || [];
const roleName = (role) => AN.L(role.name_zh, role.name_en);
// General roles are what the specialists fall back to; they always show both difficulties.
const isGeneral = (role) => !role.fallback_group;
// A specialist is split by difficulty when its two saved seats differ; unset and identical ones share one row.
const seatsSplit = (doc, group) => splitOpened.has(group) || !sameJson(doc[group]?.easy ?? null, doc[group]?.hard ?? null);
// A document without the list keeps the groups from before the switches.
const enabledGroups = (doc) => doc?.enabled_roles ?? roleCatalog().filter((role) => role.legacy_enabled).map((role) => role.group);
// Every seat a document offers: both difficulties of each role turned on, and the image seat.
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
  const sections = presets.role_sections || {};
  const generated = new Set([...Object.values(sections), presets.image_generation || ""].flatMap((section) => section.split("\n").filter(Boolean)));
  const base = text.split("\n").filter((line) => !generated.has(line.replace(/\r$/, ""))).join("\n").trimEnd();
  const enabled = enabledGroups(doc);
  return [base, ...roleCatalog().filter((role) => enabled.includes(role.group)).map((role) => sections[role.group] || ""), doc.image_generation ? presets.image_generation || "" : ""].join("");
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
    orchestration = loaded;
    if (orchestration.error) showNotice(AN.L(`${orchestration.error}；请先修复或删除这个文件`, `${orchestration.error}; fix or delete this file first`), true);
    if (!keep) {
      orchestrationDraft = null;
      customTouched = false;
      splitOpened = new Set();
      $("custom-text").value = typeof orchestration.document.custom === "string"
        ? customRoleText(orchestration.document.custom, orchestration.document) : "";
      orchestrationView = savedView();
    }
  } catch (error) {
    orchestration = null;
    showNotice(AN.L(`无法读取编排设置：${error.message}`, `Failed to load orchestration settings: ${error.message}`), true);
  }
  renderOrchestration();
  // Review seats list the connected bosses' CLIs too.
  renderReviewSeats();
  renderAdvancedBar();
  // Cards name the seats their pool left out.
  renderCards();
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
// in catalog order, then the image line.
function orchestrationPresetText(mode) {
  const presets = orchestration?.presets;
  const text = presets?.[mode];
  if (!text) return text;
  const image = presets.image_generation || "";
  const shown = orchestrationShown();
  const enabled = enabledGroups(shown);
  const sections = roleCatalog().filter((role) => enabled.includes(role.group))
    .map((role) => presets.role_sections?.[role.group] || "");
  const parts = presets.split_parts;
  let base = mode === "split" && parts
    ? parts.base + (shown?.split_options?.meetings ? parts.meetings_own : parts.meetings_asked)
    : mode === "parallel" && parts && presets.parallel_base
      ? presets.parallel_base + (shown?.parallel_options?.meetings === false ? parts.meetings_asked : parts.meetings_own)
      : presets.role_base?.[mode] ?? text.replace(image, "");
  if (shown?.lead_only && presets.lead && (mode === "split" || mode === "parallel")) base = base.replace(presets.lead.asked, presets.lead.always);
  return [base, ...sections, shown?.image_generation ? image : ""].join("");
}

function viewMode(mode) {
  const editor = $("custom-text");
  // The editor starts from the mode the user leaves; what it shows is what gets written.
  if (mode === "custom" && !editor.value.trim()) {
    editor.value = orchestrationPresetText(orchestrationView === "off" ? "split" : orchestrationView);
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
  renderOrchestration();
  advancedEdited();
}

// Rows keep the mode they had when the default for later bosses moves.
function setDefaultMode(mode) {
  const shown = orchestrationShown();
  const modes = Object.fromEntries(orchestration.bosses.flatMap((boss) => boss.keys.map((key) => [key, bossMode(shown, boss)])));
  orchestrationDraft = withModes(shown, mode, modes);
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
function renderTaskScope() {
  const on = orchestration && !orchestration.error
    ? orchestration.bosses.filter((boss) => bossMode(orchestration.document, boss) !== "off") : [];
  const names = on.flatMap((boss) => boss.labels).join(AN.L("、", ", "));
  $("task-scope").hidden = !on.length;
  $("task-scope").textContent = on.length ? AN.L(`${names} 已启用编排，用“高级”里的工位派活；单独干活只提供给未启用编排的 boss。`, `${names} use orchestration and dispatch through the seats in Advanced; solo tasks are offered only to bosses without orchestration.`) : "";
}

// One seat for both difficulties of a specialist that is not split.
function setSharedSeat(group, seat) {
  const shown = orchestrationShown();
  orchestrationDraft = { ...shown, [group]: { easy: seat, hard: seat } };
  if (!seat) delete orchestrationDraft[group];
  renderOrchestration();
  advancedEdited();
}

// Joining the difficulties again keeps the hard seat, or the easy one when hard is unset.
function setRoleSplit(group, on) {
  if (on) {
    splitOpened.add(group);
    renderOrchestration();
    return;
  }
  splitOpened.delete(group);
  const seats = orchestrationShown()[group];
  const seat = seats?.hard ?? seats?.easy ?? null;
  if (sameJson(seats?.easy ?? null, seats?.hard ?? null)) renderOrchestration();
  else setSharedSeat(group, seat);
}

function seatRow(group, label, name, seat, onSet) {
  return el("div", { className: "review-row" },
    el("div", { className: "review-role" }, label),
    ...seatSelects(group, name, seat, onSet, SEAT_UNSET));
}

// Whether the boss may call meetings without being asked: off by default in split work, on in parallel.
const meetingsOn = (doc, preset) => preset === "split" ? doc.split_options?.meetings === true : doc.parallel_options?.meetings !== false;
function setMeetings(preset, on) {
  const key = `${preset}_options`;
  const { [key]: _, ...rest } = orchestrationShown();
  orchestrationDraft = on === (preset === "parallel") ? rest : { ...rest, [key]: { meetings: on } };
  renderOrchestration();
  advancedEdited();
}

// Both presets' switch: the boss hands out all the work, small tasks too, and keeps only planning and checking.
function setLeadOnly(on) {
  const { lead_only: _, ...rest } = orchestrationShown();
  orchestrationDraft = on ? { ...rest, lead_only: true } : rest;
  renderOrchestration();
  advancedEdited();
}

function renderSplitOptions(preset, editable) {
  const box = $("orchestration-split");
  box.hidden = (preset !== "split" && preset !== "parallel") || !editable;
  if (box.hidden) return box.replaceChildren();
  const meetings = el("label", { className: "orchestration-switch" },
    el("input", {
      type: "checkbox", className: "switch-input", dataset: { focusKey: `${preset}/meetings` },
      checked: meetingsOn(orchestrationShown(), preset),
      onChange: (e) => setMeetings(preset, e.target.checked),
    }),
    el("span", {}, AN.L("boss 可主动开会", "Boss may call meetings")),
    settingTip(AN.L("打开后遇到有实质取舍或分歧的问题，boss 会自己召集几个员工开会讨论再定，会用到他们的额度；关闭时只在你要求时开会。", "On: when a choice has real trade-offs or disagreement, the boss calls a few employees into a meeting before deciding, using their quota. Off: meetings happen only when you ask."), true));
  const lead = el("label", { className: "orchestration-switch" },
    el("input", {
      type: "checkbox", className: "switch-input", dataset: { focusKey: "lead-only" },
      checked: orchestrationShown().lead_only === true,
      onChange: (e) => setLeadOnly(e.target.checked),
    }),
    el("span", {}, AN.L("boss 只负责编排", "Boss only orchestrates")),
    settingTip(AN.L("打开后小事也交给员工，boss 只拆分、写简报和验收；更慢，也更费员工额度。关闭时 boss 自己做小事，你也可以对某个任务说「这次只负责编排」。", "On: even small tasks go to employees, and the boss only splits work, writes briefs and checks results; slower and uses more employee quota. Off: the boss does small tasks itself, and you can still say \"only orchestrate this time\" for one task."), true));
  box.replaceChildren(meetings, lead);
}

// A role's switch, and its seats while on; turned off, they stay saved out of sight. A specialist
// shows one seat for both difficulties until it is split.
function roleGroup(entry, doc) {
  const { group, access } = entry;
  const name = roleName(entry);
  const on = enabledGroups(doc).includes(group);
  const general = isGeneral(entry);
  const split = general || seatsSplit(doc, group);
  const seats = !on ? [] : split
    ? DIFFICULTIES.map(([difficulty, level]) => seatRow(`${group}/${difficulty}`, level, `${name} · ${level}`,
      orchestrationSeat(doc, group, difficulty), (next) => setOrchestrationSeat(group, difficulty, next)))
    : [seatRow(`${group}/both`, AN.L("简单 · 困难", "Easy · Hard"), name,
      doc[group]?.hard ?? doc[group]?.easy, (next) => setSharedSeat(group, next))];
  return el("div", { className: "orchestration-group" },
    el("div", { className: "orchestration-head" },
      el("label", { className: "orchestration-switch" },
        el("input", {
          type: "checkbox", className: "switch-input", dataset: { focusKey: `${group}/enabled` }, checked: on,
          onChange: (e) => setRoleEnabled(group, e.target.checked),
        }),
        el("strong", {}, name), el("small", {}, ROLE_ACCESS[access])),
      on && !general && el("label", { className: "orchestration-switch" },
        el("input", {
          type: "checkbox", className: "switch-input", dataset: { focusKey: `${group}/split` }, checked: split,
          onChange: (e) => setRoleSplit(group, e.target.checked),
        }),
        el("span", {}, AN.L("区分难度", "By difficulty")),
        settingTip(AN.L("关闭时这个专项的简单、困难两档共用一个模型；打开后两档各选一个。", "Off: easy and hard share one model for this specialist. On: pick a model for each."), true))),
    ...seats);
}

// General roles, then the specialists, each under its own heading.
function roleSections(doc) {
  const section = (title, entries) => el("div", { className: "orchestration-section" },
    el("h3", {}, title), ...entries.map((entry) => roleGroup(entry, doc)));
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
  renderTaskScope();
  const shown = orchestrationShown();
  const preset = orchestrationView;
  for (const button of $("orchestration-presets").querySelectorAll("button")) {
    const active = button.dataset.preset === preset;
    button.classList.toggle("active", active);
    button.setAttribute("aria-checked", String(active));
    button.disabled = !orchestration || Boolean(orchestration.error);
  }
  $("orchestration-effect").textContent = orchestration ? ORCHESTRATION_EFFECTS[preset] : "";
  renderBossChips();
  const text = orchestrationPresetText(preset);
  $("orchestration-text").hidden = !text;
  $("orchestration-text").textContent = text || "";
  // An unread or broken file cannot take edits: nothing here could be saved or discarded.
  const editable = orchestration && !orchestration.error;
  renderSplitOptions(preset, editable);
  $("orchestration-custom").hidden = preset !== "custom" || !editable;
  const roles = $("orchestration-roles");
  roles.hidden = !editable;
  if (!roles.hidden) {
    const focusKey = document.activeElement?.dataset?.focusKey;
    const { role, difficulty, name } = IMAGE_SEAT;
    roles.replaceChildren(...(preset === "off" || !roleCatalog().length ? [] : roleSections(shown)),
      el("div", { className: "review-row" },
        el("div", { className: "review-role" }, el("strong", {}, name,
          settingTip(AN.L("这里选的是和 CLI 对话的模型；图片由那个 CLI 调用它自己能用的生图服务来出。", "This picks the model you talk to in the CLI; the image itself comes from whatever image service that CLI can call.")))),
        ...seatSelects(`${role}/${difficulty}`, name, shown[role], (next) => setOrchestrationSeat(role, difficulty, next), SEAT_UNSET, AN.L("生图的对话模型", "Chat model for image generation"))));
    if (focusKey) roles.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }
  renderRuleFiles();
}

function renderRuleFiles() {
  const list = $("rule-files");
  if (!orchestration) return list.replaceChildren();
  list.replaceChildren(...orchestration.files.map((file) => {
    const state = !file.target ? null
      : file.current ? ["written", AN.L("已写入", "Written")]
        : file.blocked ? ["failed", AN.L("经过符号链接，无法写入", "Goes through a symlink; cannot write")] : ["failed", AN.L("未写入", "Not written")];
    return el("li", {},
      el("span", { className: "rule-label" }, file.readers.join(AN.L("、", ", "))),
      el("code", {}, file.path),
      state && el("span", { className: `rule-state ${state[0]}` }, state[1]),
      ruleOpenError?.path === file.path && el("span", { className: "rule-state failed" }, ruleOpenError.message),
      file.exists
        ? el("button", { type: "button", onClick: () => openRuleFile(file) }, AN.L("打开", "Open"))
        : el("span", { className: "rule-missing" }, AN.L("未找到", "Not found")));
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
const seatsShown = () => seatsDraft || reviewSeats;
const seatsChanged = () => !sameJson(seatsShown(), reviewSeats);

// Every seat in use with the model its CLI's pool left out. Orchestration seats wait unused
// while no boss runs orchestration.
function seatConflicts() {
  const review = REVIEW_ROLES.map(([role, name]) => ({ section: "review", name: AN.L(`${name}评审`, `${name} review`), seat: seatsShown()[role] }));
  const doc = orchestrationShown();
  const live = Boolean(doc) && (doc.preset !== "off" || Object.values(doc.apps || {}).some((mode) => mode !== "off"));
  const roles = !live ? [] : liveSeats(doc)
    .map(({ role, difficulty, name }) => ({ section: "orchestration", name, seat: orchestrationSeat(doc, role, difficulty) }));
  return [...review, ...roles].filter(({ seat }) => seat && outOfPool(seat.provider, seat.model));
}
const seatConflictText = ({ name, seat }) => AN.L(`${name} 用的 ${seat.model} 不在 ${PROVIDER_NAMES[seat.provider]} 的可用模型里`, `${name} uses ${seat.model}, which is not in the available models of ${PROVIDER_NAMES[seat.provider]}`);
const memberSaveErrors = () => configurationErrors();

// A picked model missing from that CLI's configured models is added to the last saved
// members.json, where the server looks for it; unsaved 员工 edits stay unsaved. A CLI with a pool
// already configures all it may run, so a model it left out is refused instead.
async function listSeatModel(seat) {
  const add = (doc) => {
    doc.clis ||= {};
    doc.clis[seat.provider] ||= { models: {} };
    doc.clis[seat.provider].models ||= {};
    doc.clis[seat.provider].models[seat.model] ||= {};
    return doc;
  };
  // Until 员工 is read there is nothing to add to: an empty document would replace the file.
  if (!savedSnapshot && !documentError) throw new Error(AN.L("员工配置还在读取，请稍后再保存", "Employee settings are still loading; please save in a moment"));
  const saved = JSON.parse(savedSnapshot || JSON.stringify({ version: 1, clis: {}, modes: {} }));
  const pool = saved.clis?.[seat.provider]?.allowed_models;
  if (pool && !pool.includes(seat.model)) throw new Error(AN.L(`${seat.model} 不在 ${PROVIDER_NAMES[seat.provider]} 的可用模型里`, `${seat.model} is not in the available models of ${PROVIDER_NAMES[seat.provider]}`));
  if (Object.hasOwn(saved.clis?.[seat.provider]?.models || {}, seat.model)) return;
  if (documentError) throw new Error(AN.L("配置文件有错误，请先在“员工”里修复", "Configuration file has errors; fix them in “Employees” first"));
  await apiPostMembers(add(saved));
  savedSnapshot = JSON.stringify(saved);
  add(currentDoc);
  confirmScenes();
  refreshDirty();
}

async function saveSeats() {
  const seats = seatsShown();
  for (const [role] of REVIEW_ROLES) {
    if (seats[role] && !sameJson(seats[role], reviewSeats[role])) await listSeatModel(seats[role]);
  }
  await apiPostReview({ version: 1, seats });
  reviewSeats = seats;
  seatsDraft = null;
  uniformReview = null;
  uniformFilled = new Set();
  reviewSaves += 1;
}

function setReviewSeat(role, seat) {
  if (reviewError) {
    showNotice(AN.L(`${reviewError}；请先修复或删除这个文件`, `${reviewError}; fix or delete this file first`), true);
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
  if (reviewError) {
    showNotice(AN.L(`${reviewError}；请先修复或删除这个文件`, `${reviewError}; fix or delete this file first`), true);
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
  ? listedPool(provider).filter((id) => !hiddenModel(provider, id)) : getAllModelIds(provider);

// A seat's CLI, then one of that CLI's models, then an effort or the member's default.
function seatSelects(key, name, seat, onSet, unset = AN.L("自动", "Auto"), modelLabel = AN.L(`${name}的模型`, `Model for ${name}`)) {
  const provider = seat?.provider || "";
  // Orchestrated bosses use confirmed CLI configurations, not the hidden solo-task switches.
  const bossClis = (orchestration?.bosses || []).flatMap((boss) => (boss.ids || []).map((id) => id === "workbuddy" ? "codebuddy" : id));
  const doc = orchestrationShown();
  const orchestrated = doc && [doc.preset, ...Object.values(doc.apps || {})].some((mode) => mode && mode !== "off");
  const inScene = (p, scene) => Object.hasOwn(currentDoc.modes?.[scene] || {}, p);
  const confirmed = (p) => {
    if (SCENE_IDS.every(isRestricted)) return SCENE_IDS.some((scene) => inScene(p, scene));
    if (isRestricted("task")) return inScene(p, "task");
    return Object.hasOwn(currentDoc.clis || {}, p) || SCENE_IDS.some((scene) => inScene(p, scene));
  };
  const usable = (p) => (known(p) && (orchestrated ? confirmed(p) : slot("task", p).enabled)) || bossClis.includes(p);
  const note = (p) => !usable(p) ? AN.L("（员工里已停用）", " (disabled in Employees)")
    : seatModels(p).length ? "" : AN.L("（没有可选模型）", " (no models available)");
  const clis = PROVIDERS.filter((p) => usable(p) || p === provider)
    .map((p) => [p, PROVIDER_NAMES[p] + note(p), Boolean(note(p)) && p !== provider]);
  const models = provider ? seatModels(provider) : [];
  const excluded = seat?.model && outOfPool(provider, seat.model);
  if (seat?.model && !excluded && !models.includes(seat.model)) models.unshift(seat.model);
  const levels = seat?.model ? effortLevels(provider, seat.model) : [];
  if (seat?.effort && !levels.includes(seat.effort)) levels.push(seat.effort);
  // A seat saves the level it runs at, as a member does.
  const seatModel = (next, model) => {
    const effort = savedEffort(next, model, sceneEffort("task", next, model));
    return effort ? { provider: next, model, effort } : { provider: next, model };
  };
  const pickCli = (next) => {
    if (!next) return onSet(null);
    const ids = seatModels(next);
    const preferred = slot("task", next).model || nativeDefault(next).model;
    onSet(seatModel(next, ids.includes(preferred) ? preferred : ids[0]));
  };
  const pickModel = (model) => onSet(seatModel(provider, model));
  const pickEffort = (effort) => onSet({ ...seat, effort });
  const efforts = !seat || !levels.length ? [["", "—"]]
    : [...(seat.effort ? [] : [["", AN.L("选一个档位", "Choose effort"), true]]), ...levels.map((l) => [l, l])];
  return [
    reviewSelect(`${key}/cli`, AN.L(`${name}的 CLI`, `CLI for ${name}`), provider, [["", unset], ...clis], pickCli),
    reviewSelect(`${key}/model`, modelLabel, seat?.model || "",
      seat ? [...(excluded ? [[seat.model, AN.L(`${seat.model}（不在可用模型里）`, `${seat.model} (not available)`), true]] : []), ...models.map((m) => [m, m])] : [["", "—"]], pickModel, !seat),
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
  const uniform = el("div", { className: "review-row review-uniform" },
    el("div", { className: "review-role" }, el("strong", {}, uniformName,
      settingTip(AN.L("给还没选模型的评审角色一次填上这个模型；已经单独选过的角色和「正确性（简单）」不变。", "Fills this model into every review role that has none yet; roles you already picked and Correctness (easy) stay as they are.")))),
    ...seatSelects("review/uniform", uniformName, uniformReview, setUniformReview, AN.L("未选择", "Not chosen")));
  const rows = REVIEW_ROLES.map(([role, name, hint, unset]) => {
    const seat = shown[role];
    return el("div", { className: "review-row" },
      el("div", { className: "review-role" }, el("strong", {}, name), el("small", {}, hint)),
      ...seatSelects(role, AN.L(`${name}评审`, `${name} review`), seat, (next) => setReviewSeat(role, next), unset));
  });
  box.replaceChildren(uniform, ...rows);
  if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
}

// What 保存设置 would write, by section.
function advancedChanges() {
  return [
    window.AgentNaveSkills?.changed() && AN.L("技能安装", "Skills"),
    seatsChanged() && AN.L("评审工位", "Review seats"),
    orchestrationChanged() && AN.L("编排", "Orchestration"),
  ].filter(Boolean);
}

const customEmpty = () => usesCustom(orchestrationShown()) && !$("custom-text").value.trim();
// A changed section cannot save while one of its seats is outside its CLI's pool.
const blockedSeat = () => seatConflicts().find(({ section }) => (section === "review" ? seatsChanged() : orchestrationChanged()));

function renderAdvancedBar() {
  const changes = advancedChanges();
  const blocked = blockedSeat();
  const invalid = (orchestrationChanged() && customEmpty()) || Boolean(blocked);
  $("advanced-dirty").textContent = changes.length ? AN.L(`未保存：${changes.join("、")}`, `Unsaved: ${changes.join(", ")}`) : "";
  $("advanced-badge").hidden = !changes.length;
  $("advanced-save").disabled = busy || !changes.length || invalid;
  $("advanced-discard").disabled = busy || !changes.length;
  const feedback = $("advanced-feedback");
  if (invalid) {
    feedback.textContent = blocked ? seatConflictText(blocked) : AN.L("自定义规则不能为空；不想写入时选「关闭」", "Custom rules cannot be empty; choose “Off” if you don’t want to write rules");
    feedback.className = "save-feedback error";
    feedback.dataset.validation = "true";
  } else if (feedback.dataset.validation === "true") {
    delete feedback.dataset.validation;
    feedback.textContent = "";
    feedback.className = "save-feedback";
  }
}

// An edit makes the last save's result stale.
function advancedEdited() {
  const feedback = $("advanced-feedback");
  delete feedback.dataset.validation;
  feedback.textContent = "";
  feedback.className = "save-feedback";
  updateControls();
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
  renderOrchestration();
  renderReviewSeats();
  advancedEdited();
}

async function saveAdvanced() {
  if (busy || !advancedChanges().length || (orchestrationChanged() && customEmpty()) || blockedSeat()) return;
  busy = true;
  $("pane-advanced").inert = true;
  updateControls();
  const feedback = $("advanced-feedback");
  feedback.textContent = AN.L("正在保存…", "Saving…");
  feedback.className = "save-feedback";
  const failures = [];
  const attempt = async (changed, name, save) => {
    if (!changed()) return;
    try {
      await save();
    } catch (error) {
      failures.push(AN.L(`${name}：${error.message}`, `${name}: ${error.message}`));
    }
  };
  await attempt(seatsChanged, AN.L("评审工位", "Review seats"), saveSeats);
  await attempt(orchestrationChanged, AN.L("编排", "Orchestration"), saveOrchestration);
  failures.push(...await window.AgentNaveSkills.save());
  busy = false;
  $("pane-advanced").inert = false;
  feedback.textContent = failures.length ? failures.join(AN.L("；", "; ")) : AN.L("设置已保存", "Settings saved");
  feedback.className = `save-feedback ${failures.length ? "error" : "success"}`;
  renderOrchestration();
  renderReviewSeats();
  updateControls();
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
