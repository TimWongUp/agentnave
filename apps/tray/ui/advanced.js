"use strict";
// Settings -> 高级: Skills, orchestration and review seats. Viewing and picking
// only change the drafts here; nothing is written until 保存设置. Sections save independently, so
// one that fails stays unsaved for another try while the others land.

// Orchestration lives in orchestration.json: the rules text each boss gets, by mode (`apps`, with
// `preset` for rows without one, and bosses connected later), the user's own text, and the Explorer,
// Worker and image generation seats boss reads from list_members. The App writes the modes into the
// connected boss apps' global rules. The rules files are listed one row per file with every app that
// reads it.
const ORCHESTRATION_EFFECTS = {
  off: AN.L("不写入全局规则，boss 按自己的习惯派发。", "Does not write global rules; the boss dispatches in its own way."),
  split: AN.L("写入所选 boss 的全局规则：boss 未被点名也会把探索、规划交给 Explorer，实现交给 Worker，会用到它们的额度。", "Writes to selected bosses' global rules: even when not named, the boss delegates exploration and planning to Explorer, and implementation to Worker, using their quota."),
  parallel: AN.L("在分工协作之外，互不依赖的步骤会同时派给多个 Worker，额度用得更多。", "In addition to split collaboration, independent steps are dispatched to multiple Workers in parallel, using more quota."),
  custom: AN.L("写入你自己的规则，起始内容是选自定义前的档位。", "Writes your own rules, starting from the preset selected before Custom."),
};
const ORCHESTRATION_ROLES = [
  ["explorer", "easy", AN.L("Explorer · 简单", "Explorer · Easy"), AN.L("只读：探索、规划、评审", "Read-only: explore, plan, review")],
  ["explorer", "hard", AN.L("Explorer · 困难", "Explorer · Hard"), AN.L("只读：探索、规划、评审", "Read-only: explore, plan, review")],
  ["worker", "easy", AN.L("Worker · 简单", "Worker · Easy"), AN.L("可写：界面以外的实现", "Writable: implementation other than interfaces")],
  ["worker", "hard", AN.L("Worker · 困难", "Worker · Hard"), AN.L("可写：界面以外的实现", "Writable: implementation other than interfaces")],
  ["worker_frontend", "easy", AN.L("Worker · 界面 · 简单", "Worker · Interface · Easy"), AN.L("可写：页面、样式、交互、看图", "Writable: pages, styles, interactions, images")],
  ["worker_frontend", "hard", AN.L("Worker · 界面 · 困难", "Worker · Interface · Hard"), AN.L("可写：页面、样式、交互、看图", "Writable: pages, styles, interactions, images")],
  // One seat with no difficulty, its model the one the CLI talks to, which calls the image service.
  ["image_generation", null, AN.L("生图（可选）", "Image generation (optional)"), null, AN.L("未配置", "Not configured"), AN.L("生图的对话模型", "Chat model for image generation")],
];
const orchestrationSeat = (doc, role, difficulty) => difficulty ? doc[role]?.[difficulty] : doc[role];
const ORCHESTRATION_NAMES = { off: AN.L("关闭", "Off"), split: AN.L("分工协作", "Split"), parallel: AN.L("并行分工", "Parallel"), custom: AN.L("自定义", "Custom") };
let orchestration = null; // { document, error, presets, files, bosses } from GET /app/orchestration: what is saved.
let orchestrationDraft = null; // The document as picked, without the custom text, which the editor holds.
let orchestrationView = "off"; // The mode being looked at; looking changes nothing.
let ruleOpenError = null; // { path, message }: why a rules file did not open, shown on its row.
let customTouched = false; // Typed into the editor, so its text is saved even with no boss on it yet.
const orchestrationShown = () => orchestrationDraft || orchestration?.document;
const usesCustom = (doc) => doc?.preset === "custom" || Object.values(doc?.apps || {}).includes("custom");
function orchestrationWanted() {
  const wanted = orchestrationShown();
  return usesCustom(wanted) || customTouched ? { ...wanted, custom: $("custom-text").value } : wanted;
}
const orchestrationChanged = () => Boolean(orchestration) && !orchestration.error && !sameJson(orchestrationWanted(), orchestration.document);
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
      $("custom-text").value = orchestration.document.custom ?? "";
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
  for (const [role, difficulty] of ORCHESTRATION_ROLES) {
    const seat = orchestrationSeat(wanted, role, difficulty);
    if (seat && !sameJson(seat, orchestrationSeat(orchestration.document, role, difficulty))) await listSeatModel(seat);
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
  // The choice is saved even when a rules file could not take it; the list below shows which one.
  const failed = data.failures || [];
  if (failed.length) throw new Error(AN.L(`已保存，但 ${failed.map((f) => `${f.path}（${f.error}）`).join("、")} 未能写入`, `Saved, but failed to write ${failed.map((f) => `${f.path} (${f.error})`).join(", ")}`));
}

function orchestrationPresetText(mode) {
  const text = orchestration?.presets[mode];
  const image = orchestration?.presets.image_generation;
  if (!text || !image) return text;
  const base = text.replace(image, "");
  return orchestrationShown()?.image_generation ? base + image : base;
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

// Bosses on a saved mode dispatch through the seats, each boss by its own row.
function renderTaskScope() {
  const on = orchestration && !orchestration.error
    ? orchestration.bosses.filter((boss) => bossMode(orchestration.document, boss) !== "off") : [];
  const names = on.flatMap((boss) => boss.labels).join(AN.L("、", ", "));
  $("task-scope").hidden = !on.length;
  $("task-scope").textContent = on.length ? AN.L(`${names} 已启用编排，用“高级”里的工位派活；单独干活只提供给未启用编排的 boss。`, `${names} use orchestration and dispatch through the seats in Advanced; solo tasks are offered only to bosses without orchestration.`) : "";
}

function renderOrchestration() {
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
  $("orchestration-custom").hidden = preset !== "custom" || !editable;
  const roles = $("orchestration-roles");
  roles.hidden = !editable;
  if (!roles.hidden) {
    const focusKey = document.activeElement?.dataset?.focusKey;
    const visibleRoles = preset === "off" ? ORCHESTRATION_ROLES.filter(([role]) => role === "image_generation") : ORCHESTRATION_ROLES;
    roles.replaceChildren(...visibleRoles.map(([role, difficulty, name, hint, unsetName, modelLabel]) => {
      const seat = orchestrationSeat(shown, role, difficulty);
      // An unset interface seat runs the Worker seat of its difficulty, as the server fills it.
      const general = role === "worker_frontend" && shown.worker?.[difficulty] && ORCHESTRATION_ROLES.find(([r, d]) => r === "worker" && d === difficulty);
      const unset = general ? AN.L(`沿用 ${general[2]}`, `Same as ${general[2]}`) : unsetName;
      return el("div", { className: "review-row" },
        el("div", { className: "review-role" }, el("strong", {}, name), hint && el("small", {}, hint)),
        ...seatSelects(`${role}/${difficulty}`, name, seat, (next) => setOrchestrationSeat(role, difficulty, next), unset, modelLabel));
    }));
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
const seatsShown = () => seatsDraft || reviewSeats;
const seatsChanged = () => !sameJson(seatsShown(), reviewSeats);

// Every seat in use with the model its CLI's pool left out. Orchestration seats wait unused
// while no boss runs orchestration.
function seatConflicts() {
  const review = REVIEW_ROLES.map(([role, name]) => ({ section: "review", name: AN.L(`${name}评审`, `${name} review`), seat: seatsShown()[role] }));
  const doc = orchestrationShown();
  const live = Boolean(doc) && (doc.preset !== "off" || Object.values(doc.apps || {}).some((mode) => mode !== "off"));
  const roles = !live ? [] : ORCHESTRATION_ROLES
    .map(([role, difficulty, name]) => ({ section: "orchestration", name, seat: orchestrationSeat(doc, role, difficulty) }));
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

const seatModels = (provider) => listedPool(provider) ? [...listedPool(provider)] : getAllModelIds(provider);

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
  const rows = REVIEW_ROLES.map(([role, name, hint, unset]) => {
    const seat = shown[role];
    return el("div", { className: "review-row" },
      el("div", { className: "review-role" }, el("strong", {}, name), el("small", {}, hint)),
      ...seatSelects(role, AN.L(`${name}评审`, `${name} review`), seat, (next) => setReviewSeat(role, next), unset));
  });
  box.replaceChildren(...rows);
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
  orchestrationDraft = null;
  customTouched = false;
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
