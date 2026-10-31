"use strict";
// Settings -> 高级: Skills, orchestration and review seats. Viewing and picking
// only change the drafts here; nothing is written until 保存设置. Sections save independently, so
// one that fails stays unsaved for another try while the others land.

const canonical = (value) => Array.isArray(value) ? value.map(canonical)
  : value && typeof value === "object"
    ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]))
    : value;
const sameJson = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));

// Orchestration lives in orchestration.json: the rules text each boss gets, by mode (`apps`, with
// `preset` for rows without one, and bosses connected later), the user's own text, and the Explorer
// and Worker seats boss reads from list_members. The App writes the modes into the connected boss
// apps' global rules. The rules files are listed one row per file with every app that reads it.
const ORCHESTRATION_EFFECTS = {
  off: "不写入全局规则，boss 按自己的习惯派发。",
  split: "写入所选 boss 的全局规则：boss 未被点名也会把探索、规划、评审交给 Explorer，实现交给 Worker，会用到它们的额度。",
  parallel: "在分工协作之外，互不依赖的步骤会同时派给多个 Worker，实现后再换一个 Explorer 复核，额度用得更多。",
  custom: "写入你自己的规则，起始内容是选自定义前的档位。",
};
const ORCHESTRATION_ROLES = [
  ["explorer", "easy", "Explorer · 简单", "只读：探索、规划、评审"],
  ["explorer", "hard", "Explorer · 困难", "只读：探索、规划、评审"],
  ["worker", "easy", "Worker · 简单", "可写：实现"],
  ["worker", "hard", "Worker · 困难", "可写：实现"],
];
const ORCHESTRATION_NAMES = { off: "关闭", split: "分工协作", parallel: "并行复核", custom: "自定义" };
let orchestration = null; // { document, error, presets, files, bosses } from GET /app/orchestration: what is saved.
let orchestrationDraft = null; // The document as picked, without the custom text, which the editor holds.
let orchestrationView = "off"; // The mode being looked at; looking changes nothing.
let ruleOpenError = null; // { path, message }: why a rules file did not open, shown on its row.
let customTouched = false; // Typed into the editor, so its text is saved even with no boss on it yet.
const mockOrchestration = {
  document: { version: 1, preset: "split", apps: { codex: "off" } },
  error: null,
  bosses: [
    { keys: ["claude", "grok"], labels: ["Claude Code", "Grok"], mark: "claude" },
    { keys: ["codex"], labels: ["Codex"], mark: "codex" },
  ],
  presets: { split: "## AgentNave 编排偏好\n\n- （示例）分工协作的规则", parallel: "## AgentNave 编排偏好\n\n- （示例）并行复核的规则" },
  files: [
    { path: "/Users/me/.claude/CLAUDE.md", readers: ["Claude Code", "Grok"], exists: true, target: true, current: true, blocked: false },
    { path: "/Users/me/.codex/AGENTS.md", readers: ["Codex"], exists: false, target: false, current: false, blocked: false },
  ],
};

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
    let loaded;
    if (isMock) loaded = JSON.parse(JSON.stringify(mockOrchestration));
    else {
      const response = await fetch("/app/orchestration", { cache: "no-store" });
      if (!response.ok) throw new Error(`读取失败（${response.status}）`);
      loaded = await response.json();
    }
    // Asked once the answer is in: an edit made while it was on its way stays.
    const keep = orchestrationChanged();
    orchestration = loaded;
    if (orchestration.error) showNotice(`${orchestration.error}；请先修复或删除这个文件`, true);
    if (!keep) {
      orchestrationDraft = null;
      customTouched = false;
      $("custom-text").value = orchestration.document.custom ?? "";
      orchestrationView = savedView();
    }
  } catch (error) {
    orchestration = null;
    showNotice(`无法读取编排设置：${error.message}`, true);
  }
  renderOrchestration();
  renderAdvancedBar();
}

async function saveOrchestration() {
  const wanted = orchestrationWanted();
  for (const [role, difficulty] of ORCHESTRATION_ROLES) {
    const seat = wanted[role]?.[difficulty];
    if (seat && !sameJson(seat, orchestration.document[role]?.[difficulty])) await listSeatModel(seat);
  }
  let data = { files: orchestration.files, bosses: orchestration.bosses, failures: [] };
  if (!isMock) {
    const response = await fetch("/app/orchestration", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(wanted),
    });
    data = await response.json().catch(() => ({}));
    if (!response.ok || data.error) throw new Error(data.error || `保存失败（${response.status}）`);
  }
  orchestration.document = wanted;
  orchestration.files = data.files;
  orchestration.bosses = data.bosses;
  orchestrationDraft = null;
  customTouched = false;
  // The choice is saved even when a rules file could not take it; the list below shows which one.
  const failed = data.failures || [];
  if (failed.length) throw new Error(`已保存，但 ${failed.map((f) => `${f.path}（${f.error}）`).join("、")} 未能写入`);
}

function viewMode(mode) {
  const editor = $("custom-text");
  // The editor starts from the mode the user leaves; what it shows is what gets written.
  if (mode === "custom" && !editor.value.trim()) {
    editor.value = orchestration.presets[orchestrationView === "off" ? "split" : orchestrationView];
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
    mode !== savedMode ? el("small", {}, "待保存") : !on && mode !== "off" ? el("small", {}, `现用${ORCHESTRATION_NAMES[mode]}`) : null);
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
  box.replaceChildren(...orchestration.bosses.map((boss) => modeChip(`boss/${boss.keys[0]}`, boss.labels.join("、"),
    AN.hostTile ? AN.hostTile(boss.mark, boss.labels[0]) : null,
    bossMode(shown, boss), bossMode(saved, boss), (mode) => setBossMode(boss, mode))),
  modeChip("boss/later", "之后新接入的 boss", null, shown.preset, saved.preset, setDefaultMode));
  if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
}

function setOrchestrationSeat(role, difficulty, seat) {
  const shown = orchestrationShown();
  const seats = { ...(shown[role] || {}) };
  if (seat) seats[difficulty] = seat;
  else delete seats[difficulty];
  orchestrationDraft = { ...shown, [role]: seats };
  if (!Object.keys(seats).length) delete orchestrationDraft[role];
  renderOrchestration();
  advancedEdited();
}

function renderOrchestration() {
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
  const text = orchestration?.presets[preset];
  $("orchestration-text").hidden = !text;
  $("orchestration-text").textContent = text || "";
  // A broken file cannot take edits: nothing here could be saved or discarded.
  $("orchestration-custom").hidden = preset !== "custom" || Boolean(orchestration?.error);
  const roles = $("orchestration-roles");
  roles.hidden = preset === "off" || Boolean(orchestration?.error);
  if (!roles.hidden) {
    const focusKey = document.activeElement?.dataset?.focusKey;
    roles.replaceChildren(...ORCHESTRATION_ROLES.map(([role, difficulty, name, hint]) => {
      const seat = shown[role]?.[difficulty];
      return el("div", { className: "review-row" },
        el("div", { className: "review-role" }, el("strong", {}, name), el("small", {}, hint)),
        ...seatSelects(`${role}/${difficulty}`, name, seat, (next) => setOrchestrationSeat(role, difficulty, next)));
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
      : file.current ? ["written", "已写入"]
        : file.blocked ? ["failed", "经过符号链接，无法写入"] : ["failed", "未写入"];
    return el("li", {},
      el("span", { className: "rule-label" }, file.readers.join("、")),
      el("code", {}, file.path),
      state && el("span", { className: `rule-state ${state[0]}` }, state[1]),
      ruleOpenError?.path === file.path && el("span", { className: "rule-state failed" }, ruleOpenError.message),
      file.exists
        ? el("button", { type: "button", onClick: () => openRuleFile(file) }, "打开")
        : el("span", { className: "rule-missing" }, "未找到"));
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
    if (!response.ok || data.error) throw new Error(data.error || `打开失败（${response.status}）`);
    ruleOpenError = null;
  } catch (error) {
    ruleOpenError = { path: file.path, message: error.message };
  }
  renderRuleFiles();
}

// agentnave-review roles, in the order the Skill grades them; saved in review.json.
const REVIEW_ROLES = [
  ["correctness-reviewer", "正确性", "每次评审必派：行为、回归、边界"],
  ["architecture-reviewer", "架构", "职责、耦合、公开接口"],
  ["security-reviewer", "安全", "信任边界、授权、数据流"],
  ["performance-reviewer", "性能", "热路径、规模、资源"],
  ["test-reliability-reviewer", "测试可靠性", "测试能否护住行为"],
  ["simplicity-reviewer", "简化", "多余的复杂度"],
  ["specialist-reviewer", "专项", "其他明确的专业领域"],
  ["adversarial-verifier", "反证", "高风险改动修完后尝试推翻结论"],
];
let seatsDraft = null; // Review seats as picked; `reviewSeats` holds what is saved.
const seatsShown = () => seatsDraft || reviewSeats;
const seatsChanged = () => !sameJson(seatsShown(), reviewSeats);

// A picked model missing from that CLI's configured models is added to the last saved
// members.json, where the server looks for it; unsaved 员工 edits stay unsaved.
async function listSeatModel(seat) {
  const add = (doc) => {
    doc.clis ||= {};
    doc.clis[seat.provider] ||= { models: {} };
    doc.clis[seat.provider].models ||= {};
    doc.clis[seat.provider].models[seat.model] ||= {};
    return doc;
  };
  // Until 员工 is read there is nothing to add to: an empty document would replace the file.
  if (!savedSnapshot && !documentError) throw new Error("员工配置还在读取，请稍后再保存");
  const saved = JSON.parse(savedSnapshot || JSON.stringify({ version: 1, clis: {}, modes: {} }));
  if (Object.hasOwn(saved.clis?.[seat.provider]?.models || {}, seat.model)) return;
  if (documentError) throw new Error("配置文件有错误，请先在“员工”里修复");
  await apiPostMembers(add(saved));
  savedSnapshot = JSON.stringify(saved);
  add(currentDoc);
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
    showNotice(`${reviewError}；请先修复或删除这个文件`, true);
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

// A seat's CLI, then one of that CLI's models, then an effort or the member's default.
function seatSelects(key, name, seat, onSet) {
  const provider = seat?.provider || "";
  const clis = PROVIDERS.filter((p) => detected[p]?.installed || currentDoc.clis?.[p] || p === provider)
    .map((p) => [p, PROVIDER_NAMES[p] + (getAllModelIds(p).length ? "" : "（没有可选模型）"), !getAllModelIds(p).length && p !== provider]);
  const models = provider ? getAllModelIds(provider) : [];
  if (seat?.model && !models.includes(seat.model)) models.unshift(seat.model);
  const levels = seat?.model ? effortLevels(provider, seat.model) : [];
  if (seat?.effort && !levels.includes(seat.effort)) levels.push(seat.effort);
  const pickCli = (next) => {
    if (!next) return onSet(null);
    const ids = getAllModelIds(next);
    const preferred = slot("task", next).model || nativeDefault(next).model;
    const model = ids.includes(preferred) ? preferred : ids[0];
    onSet({ provider: next, model });
  };
  const pickModel = (model) => onSet({ provider, model });
  const pickEffort = (effort) => onSet(effort ? { ...seat, effort } : { provider, model: seat.model });
  return [
    reviewSelect(`${key}/cli`, `${name}的 CLI`, provider, [["", "自动"], ...clis], pickCli),
    reviewSelect(`${key}/model`, `${name}的模型`, seat?.model || "",
      seat ? models.map((m) => [m, m]) : [["", "—"]], pickModel, !seat),
    reviewSelect(`${key}/effort`, `${name}的思考档位`, seat?.effort || "",
      seat ? [["", "员工默认档位"], ...levels.map((l) => [l, l])] : [["", "—"]], pickEffort, !seat || !levels.length),
  ];
}

// One row per role.
function renderReviewSeats() {
  const box = $("review-seats");
  if (!box) return;
  const focusKey = document.activeElement?.dataset?.focusKey;
  const shown = seatsShown();
  const rows = REVIEW_ROLES.map(([role, name, hint]) => {
    const seat = shown[role];
    const provider = seat?.provider || "";
    // A restricted private-task list that leaves this seat out makes the server report it unavailable.
    const task = currentDoc.modes?.task;
    const listed = !seat || !task || (task[provider] && (!task[provider].models?.length || task[provider].models.includes(seat.model)));
    return el("div", { className: "review-row" },
      el("div", { className: "review-role" }, el("strong", {}, name), el("small", {}, hint),
        !listed && el("small", { className: "review-warn" }, "单独干活的名单没有选中这个模型，评审时会改找其他 CLI")),
      ...seatSelects(role, `${name}评审`, seat, (next) => setReviewSeat(role, next)));
  });
  box.replaceChildren(...rows);
  if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
}

// What 保存设置 would write, by section.
function advancedChanges() {
  return [
    window.AgentNaveSkills?.changed() && "技能安装",
    seatsChanged() && "评审工位",
    orchestrationChanged() && "编排",
  ].filter(Boolean);
}

const customEmpty = () => usesCustom(orchestrationShown()) && !$("custom-text").value.trim();

function renderAdvancedBar() {
  const changes = advancedChanges();
  const invalid = changes.includes("编排") && customEmpty();
  $("advanced-dirty").textContent = changes.length ? `未保存：${changes.join("、")}` : "";
  $("advanced-badge").hidden = !changes.length;
  $("advanced-save").disabled = busy || !changes.length || invalid;
  $("advanced-discard").disabled = busy || !changes.length;
  const feedback = $("advanced-feedback");
  if (invalid) {
    feedback.textContent = "自定义规则不能为空；不想写入时选「关闭」";
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
  if (busy || !advancedChanges().length || (orchestrationChanged() && customEmpty())) return;
  busy = true;
  $("pane-advanced").inert = true;
  updateControls();
  const feedback = $("advanced-feedback");
  feedback.textContent = "正在保存…";
  feedback.className = "save-feedback";
  const failures = [];
  const attempt = async (changed, name, save) => {
    if (!changed()) return;
    try {
      await save();
    } catch (error) {
      failures.push(`${name}：${error.message}`);
    }
  };
  await attempt(seatsChanged, "评审工位", saveSeats);
  await attempt(orchestrationChanged, "编排", saveOrchestration);
  failures.push(...await window.AgentNaveSkills.save());
  busy = false;
  $("pane-advanced").inert = false;
  feedback.textContent = failures.length ? failures.join("；") : "设置已保存";
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
