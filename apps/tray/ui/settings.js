"use strict";
// Settings page logic: Members & Models, Versions & Updates.
// Adheres strictly to CSP: Pure DOM construction, no HTML string parsing.

const $ = (id) => document.getElementById(id);
const L = AN.L || ((zh) => zh);

// Safe DOM element creation
function el(tag, attrs, ...children) {
  const element = document.createElement(tag);
  if (attrs) {
    for (const [key, val] of Object.entries(attrs)) {
      if (val == null) continue;
      if (key === "className" || key === "class") {
        element.className = val;
      } else if (key === "dataset") {
        for (const [dKey, dVal] of Object.entries(val)) {
          element.dataset[dKey] = dVal;
        }
      } else if (key.startsWith("on") && typeof val === "function") {
        element.addEventListener(key.slice(2).toLowerCase(), val);
      } else if (typeof val === "boolean") {
        if (val) {
          element.setAttribute(key, "");
        } else {
          element.removeAttribute(key);
        }
        if (key === "checked") element.checked = val;
        if (key === "disabled") element.disabled = val;
        if (key === "hidden") element.hidden = val;
      } else {
        element.setAttribute(key, val);
      }
    }
  }
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    if (typeof child === "string" || typeof child === "number") {
      element.appendChild(document.createTextNode(String(child)));
    } else if (child && (typeof Node !== "undefined" ? child instanceof Node : (child.nodeType || child.tagName))) {
      element.appendChild(child);
    }
  }
  return element;
}

function textNode(content) {
  return document.createTextNode(String(content == null ? "" : content));
}

// Supported CLI providers
const PROVIDERS = ["codex", "claude", "codebuddy", "grok", "antigravity", "pi", "deepseek", "zcode"];
const PROVIDER_NAMES = {
  codex: "Codex CLI",
  claude: "Claude Code",
  codebuddy: "CodeBuddy Code",
  grok: "Grok CLI",
  antigravity: "Antigravity CLI",
  pi: "Pi Coding Agent",
  deepseek: "DeepSeek Harness",
  zcode: "ZCode",
};

// Permission-related keywords for sensitive options
const PERMISSION_KEYWORDS = [
  "dangerously",
  "permission",
  "sandbox",
  "yolo",
  "skip_permissions",
  "bypass",
];

function isPermissionOptionKey(key) {
  if (!key) return false;
  const lower = String(key).toLowerCase();
  return PERMISSION_KEYWORDS.some((kw) => lower.includes(kw));
}

// `false` switches a permission flag off, so it grants nothing.
function grantsPermission(key, value) {
  return isPermissionOptionKey(key) && value !== false;
}

// Application state
let currentDoc = {
  version: 1,
  clis: {},
  modes: {},
};
let detected = {};
let currentTab = "members"; // "general" | "members" | "avatars" | "voice" | "advanced" | "hosts" | "storage" | "about"
let dirty = false;
let savedSnapshot = ""; // The document as last loaded or saved; dirty compares against it.
let savedVersion = 0;
let leaveTarget = null; // A navigation held back while there are unsaved changes.
let busy = false;
let probing = false; // A CLI re-probe is running; it never blocks saving.
let probedAt = 0;
let documentError = null;
let reviewSeats = {}; // review.json seats; saved on their own, apart from members.json.
let reviewError = null;
let permissionTiers = null; // permissions.json tiers by CLI; a CLI not in it runs "auto".
let permissionsError = null;
let reviewSaves = 0; // Seats or tiers saved so far; a probe answered before a save carries old ones.
let docVersion = 0; // Monotonic counter for save race detection
let missingOpen = false; // The folded list of CLIs not installed here
let appStatus = null;
let installedService = null;
let statusPollTimer = null;
// Keep unfinished option edits across drawer/mode changes without writing invalid values.
const optionDrafts = new WeakMap();
let operatedControl = null;
let pendingDisable = null;

// API Helpers
async function apiGetMembers(refresh = false) {
  const url = refresh ? "/settings/members?refresh=1" : "/settings/members";
  const res = await fetch(url, { cache: "no-store" });
  if (!res.ok) throw new Error(L(`获取成员配置失败（${res.status}）`, `Failed to load employee settings (${res.status})`));
  return res.json();
}

async function apiPostReview(doc) {
  const res = await fetch("/app/review", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
    body: JSON.stringify(doc),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.error) throw new Error(data.error || L(`保存失败（${res.status}）`, `Save failed (${res.status})`));
}

function takeReview(data) {
  reviewSeats = data.review?.seats || {};
  reviewError = data.review_error || null;
  permissionTiers = data.permissions?.clis || null;
  permissionsError = data.permissions_error || null;
}

async function apiPostPermissions(doc) {
  const res = await fetch("/app/permissions", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
    body: JSON.stringify(doc),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.error) throw new Error(data.error || L(`保存失败（${res.status}）`, `Save failed (${res.status})`));
}

// What each tier means for each CLI when a task runs unattended; the server maps the tier to
// that CLI's own options, "auto" unless the user picked "full". Pi has no approval step.
const TIERS = [["auto", L("自动", "Auto")], ["full", L("完全放开", "Full access")]];
// CLIs without a usable automatic reviewer: both tiers grant full access, so there is no choice.
const FULL_ONLY = {
  codebuddy: L("CodeBuddy 的自动模式在派发时连查看目录都会拒绝，所以直接执行任何命令和改动。", "CodeBuddy’s auto mode rejects even directory listing when dispatched, so it runs with full access."),
  deepseek: L("DeepSeek 没有自动审核，所以直接执行任何命令和改动。", "DeepSeek has no automatic review, so it runs with full access."),
  zcode: L("ZCode 的 auto 模式尚未实现，所以直接执行任何命令和改动。", "ZCode’s auto mode is not implemented yet, so it runs with full access."),
  antigravity: L("Antigravity 派发时直接执行任何命令和改动。", "Antigravity runs with full access when dispatched."),
};
const TIER_TEXT = {
  auto: {
    any: L("CLI 自带的自动审核放行常规命令和改动，明显危险的操作仍会被拦下。", "The CLI’s built-in auto-review allows routine commands and edits while blocking obviously dangerous operations."),
    codex: L("Codex 自动审核放行命令，读写限在工作目录内。", "Codex auto-review allows commands while keeping reads and writes within the working directory."),
  },
  full: {
    any: L("不再审核，直接执行任何命令和改动。只在你信任派给它的任务时选。", "Runs any command or edit without review. Choose only when you trust the dispatched tasks."),
    codex: L("不审核也不进沙箱，直接执行任何命令和改动。只在你信任派给它的任务时选。", "Runs any command or edit without review or sandbox. Choose only when you trust the dispatched tasks."),
  },
};

function permissionTier(provider) {
  return permissionTiers?.[provider] || "auto";
}

async function setPermissionTier(provider, tier) {
  if (permissionsError) return showNotice(L(`${permissionsError}；请先修复或删除这个文件`, `${permissionsError}; fix or delete this file first`), true);
  if (busy) return showNotice(L("正在保存，请稍后再选", "Saving in progress; please try again shortly"), true);
  const clis = { ...(permissionTiers || {}), [provider]: tier };
  busy = true;
  updateControls();
  try {
    await apiPostPermissions({ version: 1, clis });
    permissionTiers = clis;
    reviewSaves += 1;
    showNotice(L(`已保存：${PROVIDER_NAMES[provider]} 的权限改为「${TIERS.find(([t]) => t === tier)[1]}」，之后单独干活和新开的会都生效`, `Saved: ${PROVIDER_NAMES[provider]} permissions set to “${TIERS.find(([t]) => t === tier)[1]}” for future solo tasks and new meetings`));
  } catch (err) {
    showNotice(L(`权限保存失败: ${err.message}`, `Failed to save permissions: ${err.message}`), true);
  } finally {
    busy = false;
    updateControls();
  }
  renderCards();
}

function renderPermission(provider) {
  if (provider === "pi") return null;
  if (FULL_ONLY[provider]) {
    return el("span", { className: "member-tier", title: FULL_ONLY[provider] }, textNode(L("权限", "Permissions")), el("span", {}, L("完全放开", "Full access")));
  }
  const tier = permissionTier(provider);
  return el("label", { className: "member-tier", title: permissionsError || L(`${TIER_TEXT[tier][provider] || TIER_TEXT[tier].any} 选完立即保存。`, `${TIER_TEXT[tier][provider] || TIER_TEXT[tier].any} Saved immediately when selected.`) },
    textNode(L("权限", "Permissions")),
    el("select", {
      dataset: { focusKey: `${provider}/tier` },
      disabled: Boolean(permissionsError),
      onChange: (e) => setPermissionTier(provider, e.target.value),
    }, TIERS.map(([value, label]) => el("option", { value, selected: value === tier }, label))));
}

async function apiPostMembers(doc) {
  const res = await fetch("/app/members", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-AgentNave-Action": "1",
    },
    body: JSON.stringify(doc),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.error) {
    throw new Error(data.error || L(`保存失败（${res.status}）`, `Save failed (${res.status})`));
  }
  return data;
}

async function apiGetStatus() {
  const res = await fetch("/app/status", { cache: "no-store" });
  if (!res.ok) throw new Error(L(`获取状态失败（${res.status}）`, `Failed to load status (${res.status})`));
  return res.json();
}

async function apiCheckUpdate(quiet = false) {
  // quiet: a failed check keeps the previous state instead of reporting a failure.
  const res = await fetch(quiet ? "/app/update/check?quiet=1" : "/app/update/check", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-AgentNave-Action": "1",
    },
  });
  if (!res.ok) throw new Error(L(`检查更新失败（${res.status}）`, `Update check failed (${res.status})`));
  return res.json();
}

// Notice display
// Settings name the two modes by what the employees do in them.
const SCENES = { task: L("单独干活", "Solo tasks"), discussion: L("开会", "Meetings") };
function showNotice(msg, isError = false) {
  const notice = $("notice");
  notice.textContent = msg;
  notice.classList.toggle("error", isError);
}

function markSaved(snapshot = JSON.stringify(currentDoc)) {
  savedSnapshot = snapshot;
  savedVersion = docVersion;
  refreshDirty();
}

// Unsaved means the document differs from the saved one, or an edit left an option draft
// that cannot be written yet.
function refreshDirty() {
  dirty = limitChanged() || (docVersion !== savedVersion &&
    (JSON.stringify(currentDoc) !== savedSnapshot || configurationErrors().length > 0));
}

function incDocVersion() {
  docVersion++;
  refreshDirty();
  updateControls();
}

// Count permission-related options across entire document
function countPermissionOptions() {
  let count = 0;
  for (const cli of Object.values(currentDoc.clis || {})) {
    for (const m of Object.values(cli.models || {})) {
      if (m.options && typeof m.options === "object") {
        for (const [k, v] of Object.entries(m.options)) {
          if (grantsPermission(k, v)) count++;
        }
      }
    }
  }
  for (const modeObj of Object.values(currentDoc.modes || {})) {
    for (const provObj of Object.values(modeObj || {})) {
      for (const override of Object.values(provObj.overrides || {})) {
        if (override.options && typeof override.options === "object") {
          for (const [k, v] of Object.entries(override.options)) {
            if (grantsPermission(k, v)) count++;
          }
        }
      }
    }
  }
  return count;
}

// The first scene that uses no CLI at all, if any.
function emptyScene() {
  return Object.entries(currentDoc.modes || {})
    .find(([, selection]) => selection && typeof selection === "object" && !Object.keys(selection).length)?.[0];
}

function optionRows(config) {
  return optionDrafts.get(config) || Object.entries(config.options || {}).map(([key, value]) => ({
    key, type: typeof value, raw: String(value),
  }));
}

function parseOptionRows(rows) {
  const entries = [];
  const errors = [];
  const keys = new Set();
  for (const [index, row] of rows.entries()) {
    const label = L(`参数第 ${index + 1} 行`, `Option row ${index + 1}`);
    if (!row.key.trim()) errors.push(L(`${label}：请填写参数键`, `${label}: Enter an option key`));
    if (row.key.trim() && keys.has(row.key)) errors.push(L(`${label}：参数键 ${row.key} 重复`, `${label}: Duplicate option key ${row.key}`));
    if (["model", "effort"].includes(row.key)) errors.push(L(`${label}：不能使用 ${row.key} 键`, `${label}: Cannot use ${row.key} as an option key`));
    keys.add(row.key);
    let value = row.raw;
    if (row.type === "number") {
      value = Number(row.raw);
      if (!row.raw.trim() || !Number.isFinite(value)) errors.push(L(`${label}：必须为有效有限数字`, `${label}: Must be a valid finite number`));
    } else if (row.type === "boolean") {
      value = row.raw === "true";
    }
    entries.push([row.key, value]);
  }
  return { options: Object.fromEntries(entries), errors };
}

function effortError(config) {
  if (Object.hasOwn(config, "effort") && !config.effort) {
    return L("默认思考档位不可为空，请选择具体档位或清除默认设置（覆盖时选择继承）", "Default effort cannot be empty; choose a level or clear the default (or inherit when overriding)");
  }
  if (Object.hasOwn(config, "effort") && config.efforts?.length && !config.efforts.includes(config.effort)) {
    return L(`默认思考档位“${config.effort}”不在允许档位中，请重新选择默认档位或调整允许档位（覆盖时也可改为继承）`, `Default effort “${config.effort}” is not in allowed levels; choose another default or adjust allowed levels (or inherit when overriding)`);
  }
  return null;
}

// Validate every mode, including collapsed drawers and inherited defaults in other modes.
function configurationErrors() {
  const errors = [];
  const add = (provider, model, scope, message) => {
    if (message) errors.push({ provider, model, scope, message: L(`${scope} / ${provider} / ${model}：${message}`, `${scope} / ${provider} / ${model}: ${message}`) });
  };
  const check = (config, provider, model, scope) => {
    add(provider, model, scope, effortError(config));
    for (const error of parseOptionRows(optionRows(config)).errors) add(provider, model, scope, error);
  };
  for (const [provider, cli] of Object.entries(currentDoc.clis || {})) {
    for (const [model, shared] of Object.entries(cli.models)) check(shared, provider, model, L("共享", "Shared"));
  }
  for (const [mode, providers] of Object.entries(currentDoc.modes || {})) {
    const scope = SCENES[mode];
    for (const [provider, selection] of Object.entries(providers)) {
      const models = currentDoc.clis?.[provider]?.models || {};
      for (const model of selection.models) {
        if (!Object.hasOwn(models, model)) add(provider, model, scope, L("模型尚未配置", "Model is not configured"));
      }
      if (Object.hasOwn(selection, "default") &&
          (!Object.hasOwn(models, selection.default) || (selection.models.length && !selection.models.includes(selection.default)))) {
        add(provider, selection.default, scope, L("默认模型不在允许范围内", "Default model is not in the allowed list"));
      }
      for (const [model, override] of Object.entries(selection.overrides || {})) {
        if (!Object.hasOwn(models, model)) add(provider, model, scope, L("覆盖的模型尚未配置", "Overridden model is not configured"));
        add(provider, model, scope, effortError({ ...models[model], ...override }));
        for (const error of parseOptionRows(optionRows(override)).errors) add(provider, model, scope, error);
      }
    }
  }
  for (const [mode, providers] of Object.entries(currentDoc.defaults || {})) {
    const scope = SCENES[mode];
    for (const [provider, model] of Object.entries(providers)) {
      if (!Object.hasOwn(currentDoc.clis?.[provider]?.models || {}, model)) add(provider, model, scope, L("默认模型尚未配置", "Default model is not configured"));
    }
  }
  // A chosen default model runs at a chosen level; only one left to the CLI may go without.
  for (const scene of SCENE_IDS) {
    for (const provider of PROVIDERS) {
      const current = slot(scene, provider);
      if (current.model && !current.effort && effortLevels(provider, current.model).length > 1) {
        add(provider, current.model, SCENES[scene], L("要选一个思考档位", "Choose an effort level"));
      }
    }
  }
  return errors;
}

function updateControls() {
  $("dirty-text").textContent = dirty ? L("有未保存的修改", "Unsaved changes") : "";
  $("members-badge").hidden = !dirty;
  if (!unsaved() && !busy) leaveTarget = null;
  renderLeaveConfirm();
  renderAdvancedBar();

  const emptyRestricted = emptyScene();
  const errors = configurationErrors();
  const canSave = !busy && dirty && !documentError && !emptyRestricted && errors.length === 0;

  $("save-btn").disabled = !canSave;
  $("refresh-detection").disabled = busy || probing;
  $("refresh-detection").textContent = probing ? L("正在检测…", "Detecting…") : L("刷新模型列表", "Refresh models");

  // Feedback hints
  const feedback = $("save-feedback");
  if (emptyRestricted) {
    feedback.textContent = L(`${SCENES[emptyRestricted] || emptyRestricted}至少要用一个员工`, `${SCENES[emptyRestricted] || emptyRestricted} must enable at least one employee`);
    feedback.className = "save-feedback error";
    feedback.dataset.validation = "true";
  } else if (errors.length) {
    feedback.textContent = errors[0].message;
    feedback.className = "save-feedback error";
    feedback.dataset.validation = "true";
  } else if (feedback.dataset.validation === "true") {
    delete feedback.dataset.validation;
    feedback.textContent = "";
    feedback.className = "save-feedback";
  } else if (!dirty && !feedback.classList.contains("success")) {
    feedback.textContent = "";
    feedback.className = "save-feedback";
  }

  // Permission summary tag
  const permCount = countPermissionOptions();
  const permSummary = $("permission-summary");
  if (permSummary) {
    if (permCount > 0) {
      permSummary.hidden = false;
      permSummary.textContent = L(`包含 ${permCount} 个权限类默认参数`, `Includes ${permCount} permission-related default ${permCount === 1 ? "option" : "options"}`);
    } else {
      permSummary.hidden = true;
      permSummary.textContent = "";
    }
  }

  // Readonly state if document error
  const paneMembers = $("pane-members");
  if (paneMembers) {
    paneMembers.classList.toggle("pane-readonly", Boolean(documentError));
  }
}

// The running limit: settings.json, read by the service before each start. Saved with 员工; its
// draft is the menu itself.
let limitDoc = null;
const limitSaved = () => limitDoc?.max_running ?? 8;
const limitChanged = () => Boolean($("max-running").options.length) && Number($("max-running").value) !== limitSaved();

async function loadLimit() {
  if (!$("max-running").options.length) {
    for (let n = 1; n <= 32; n += 1) $("max-running").append(el("option", { value: String(n) }, n === 8 ? L("8（默认）", "8 (default)") : String(n)));
    $("max-running").value = "8";
  }
  try {
    const response = await fetch("/app/settings", { cache: "no-store" });
    if (!response.ok) throw new Error(L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
    const data = await response.json();
    const keep = limitChanged();
    limitDoc = data.settings;
    if (!keep) $("max-running").value = String(data.settings.max_running);
    if (data.error) showNotice(L(`${data.error}；保存会用当前值覆盖`, `${data.error}; saving will overwrite with the current value`), true);
  } catch (error) {
    showNotice(L(`无法读取同时工作的员工上限：${error.message}`, `Failed to load max concurrent employees: ${error.message}`), true);
  }
  refreshDirty();
  updateControls();
}

async function saveLimit() {
  const value = Number($("max-running").value);
  const response = await fetch("/app/settings", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
    body: JSON.stringify({ ...(limitDoc || { version: 1 }), version: 1, max_running: value }),
  });
  const data = await response.json();
  if (!response.ok || data.error) throw new Error(data.error || L(`同时工作的员工上限保存失败（${response.status}）`, `Failed to save max concurrent employees (${response.status})`));
  limitDoc = { version: 1, max_running: value };
}

const unsaved = () => dirty || advancedChanges().length > 0;

// Switch tabs
function setTab(tab) {
  if (tab === "updates") tab = "about"; // Preserve old update links.
  // Status lines belong to the tab that produced them.
  if (currentTab !== tab) showNotice("");
  currentTab = tab;
  for (const id of ["general", "hosts", "members", "avatars", "voice", "advanced", "storage", "about"]) {
    if (tab === id) $("nav-" + id).setAttribute("aria-current", "page");
    else $("nav-" + id).removeAttribute("aria-current");
    $("pane-" + id).hidden = tab !== id;
  }
  if (tab === "general") {
    window.AgentNaveLanguageSettings?.load();
    window.AgentNaveGeneralSettings?.load();
  }
  if (tab === "voice") window.AgentNaveVoiceSettings?.load();
  if (tab === "about") loadStatus().then(checkIfStale);
  if (tab === "members") {
    loadLimit();
    window.AgentNaveAlertsSettings?.load();
  }
  if (tab === "advanced") {
    window.AgentNaveSkills?.load();
    loadOrchestration();
    loadLimit();
  }
  if (tab === "storage") window.AgentNaveStorageSettings?.load();
}

const SCENE_IDS = ["task", "discussion"];
const OFF = "\u0000off"; // The model menu entry that stops a scene from using a CLI.

const isRestricted = (scene) => Object.hasOwn(currentDoc.modes || {}, scene);

// Default models of an unrestricted mode; an empty map leaves no `defaults` key behind.
function setKeptDefaults(kept, mode) {
  if (Object.keys(kept).length) {
    currentDoc.defaults = { ...currentDoc.defaults, [mode]: kept };
  } else if (currentDoc.defaults) {
    delete currentDoc.defaults[mode];
    if (!Object.keys(currentDoc.defaults).length) delete currentDoc.defaults;
  }
}

// A model the App starts configuring keeps the CLI's own default effort until one is chosen (ADR-0028).
function ensureCliModel(provider, modelId) {
  if (!currentDoc.clis) currentDoc.clis = {};
  if (!currentDoc.clis[provider]) currentDoc.clis[provider] = { models: {} };
  if (!currentDoc.clis[provider].models) currentDoc.clis[provider].models = {};
  if (!Object.hasOwn(currentDoc.clis[provider].models, modelId)) {
    Object.defineProperty(currentDoc.clis[provider].models, modelId, {
      value: {}, enumerable: true, configurable: true, writable: true,
    });
  }
}

// What the CLI itself uses when the App sets nothing: its own default model and effort, if known.
function nativeDefault(provider) {
  const native = detected[provider]?.native || {};
  return { model: native.model || null, effort: native.effort || null };
}

// The level the CLI runs one model at when a call names none, if known.
function cliEffort(provider, modelId) {
  const listed = (detected[provider]?.models || []).find((m) => m.id === modelId);
  if (listed?.effort) return listed.effort;
  return modelId === nativeDefault(provider).model ? nativeDefault(provider).effort : null;
}

const EFFORT_ORDER = ["off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

// A model's effort levels from low to high: its own detected levels, else its CLI's, plus any
// configured value a probe no longer lists.
function effortLevels(provider, modelId) {
  // A model's own list, even an empty one, is what its CLI offers; the CLI's list is the fallback.
  const own = (detected[provider]?.models || []).find((m) => m.id === modelId)?.efforts;
  const detectedEfforts = Array.isArray(own) ? own : detected[provider]?.efforts || [];
  const shared = currentDoc.clis?.[provider]?.models?.[modelId] || {};
  const overrides = Object.values(currentDoc.modes || {})
    .map((providers) => providers?.[provider]?.overrides?.[modelId])
    .filter(Boolean);
  const levels = new Set(detectedEfforts);
  for (const config of [shared, ...overrides]) {
    for (const effort of [...(config.efforts || []), config.effort]) {
      if (typeof effort === "string" && effort) levels.add(effort);
    }
  }
  const rank = (effort) => {
    const index = EFFORT_ORDER.indexOf(effort);
    return index < 0 ? EFFORT_ORDER.length : index;
  };
  return [...levels].sort((a, b) => rank(a) - rank(b));
}

// The level a chosen model is saved with: the one wanted when it has it, else the CLI's current
// one, else its only one; "" when it has no levels or none can be told, so the user picks.
function savedEffort(provider, modelId, wanted) {
  const levels = modelId ? effortLevels(provider, modelId) : [];
  if (levels.includes(wanted)) return wanted;
  const cli = cliEffort(provider, modelId);
  if (levels.includes(cli)) return cli;
  return levels.length === 1 ? levels[0] : "";
}

function getAllModelIds(provider) {
  const ids = new Set((detected[provider]?.models || []).map((m) => m.id));
  for (const id of Object.keys(currentDoc.clis?.[provider]?.models || {})) ids.add(id);
  for (const selection of Object.values(currentDoc.modes || {})) {
    for (const id of selection?.[provider]?.models || []) ids.add(id);
  }
  return [...ids].filter((id) => typeof id === "string" && id.trim());
}

// A scene's effort for one model: its override, else the shared level; "" follows the CLI.
function sceneEffort(scene, provider, modelId) {
  if (!modelId) return "";
  const override = currentDoc.modes?.[scene]?.[provider]?.overrides?.[modelId];
  if (override && Object.hasOwn(override, "effort")) return override.effort;
  return currentDoc.clis?.[provider]?.models?.[modelId]?.effort || "";
}

// What one scene runs for one CLI: whether it is used, the default model ("" follows the CLI),
// the models boss may pick (null: any), and the default model's effort ("" follows the CLI).
function slot(scene, provider) {
  const selection = isRestricted(scene) ? currentDoc.modes[scene][provider] : null;
  const enabled = !isRestricted(scene) || Boolean(selection);
  const model = (selection ? selection.default : currentDoc.defaults?.[scene]?.[provider]) || "";
  const allowed = selection?.models?.length ? [...selection.models] : null;
  const effort = enabled ? sceneEffort(scene, provider, model || nativeDefault(provider).model) : "";
  return { enabled, model: enabled ? model : "", allowed, effort };
}

// Scenes that show the same model and effort, followed or chosen, need only one row.
function shownSlot(scene, provider) {
  const current = slot(scene, provider);
  // A model list without a default runs nothing until one is chosen.
  const model = current.model || (current.allowed ? "" : nativeDefault(provider).model) || "";
  return { ...current, model, effort: current.effort || (model && cliEffort(provider, model)) || "" };
}
const splitCards = new Set(); // Cards split by hand while both scenes still match.
const sceneOverrides = (scene, provider) => JSON.stringify(currentDoc.modes?.[scene]?.[provider]?.overrides || {});
const isSplit = (provider) => splitCards.has(provider) ||
  JSON.stringify(shownSlot("task", provider)) !== JSON.stringify(shownSlot("discussion", provider)) ||
  sceneOverrides("task", provider) !== sceneOverrides("discussion", provider);

// Installed CLIs, and ones a scene still names after they were removed.
const known = (provider) => detected[provider]?.installed ||
  SCENE_IDS.some((s) => currentDoc.modes?.[s]?.[provider] || currentDoc.defaults?.[s]?.[provider]);

// A scene names its CLIs only when it must (one is off, a model list or a scene-only effort):
// every known CLI is listed with any model and keeps its default model (ADR-0021).
function restrict(scene) {
  if (isRestricted(scene)) return currentDoc.modes[scene];
  const kept = currentDoc.defaults?.[scene] || {};
  const selection = {};
  for (const p of PROVIDERS) {
    if (!known(p) && !kept[p]) continue;
    selection[p] = { models: [], overrides: {} };
    if (kept[p]) selection[p].default = kept[p];
  }
  currentDoc.modes = { ...currentDoc.modes, [scene]: selection };
  setKeptDefaults({}, scene);
  return selection;
}

// A list that names every known CLI with any model and nothing scene-only says no more than
// an open scene, which also takes CLIs installed later.
function relax(scene, kept) {
  const selection = currentDoc.modes?.[scene];
  if (!selection) return;
  const entries = Object.entries(selection);
  const open = entries.every(([, c]) => !c.models?.length && !Object.keys(c.overrides || {}).length);
  const all = PROVIDERS.every((p) => !kept.has(p) || selection[p]);
  if (!entries.length || !open || !all) return;
  delete currentDoc.modes[scene];
  setKeptDefaults(Object.fromEntries(entries.filter(([, c]) => c.default).map(([p, c]) => [p, c.default])), scene);
}

// A model list needs a default model, so following the CLI pins the CLI's own model.
function writeSlot(scene, provider, next) {
  if (!next.enabled) {
    delete restrict(scene)[provider];
    return;
  }
  const native = nativeDefault(provider).model;
  const model = next.model || (next.allowed ? (next.allowed.includes(native) ? native : next.allowed[0]) : "");
  if (next.allowed || isRestricted(scene)) {
    const selection = restrict(scene);
    const conf = (selection[provider] ||= { models: [], overrides: {} });
    conf.models = next.allowed ? [...new Set([...next.allowed, model])] : [];
    if (model) conf.default = model;
    else delete conf.default;
  } else {
    const kept = { ...currentDoc.defaults?.[scene] };
    if (model) kept[provider] = model;
    else delete kept[provider];
    setKeptDefaults(kept, scene);
  }
  for (const m of [model, ...(next.allowed || [])]) if (m) ensureCliModel(provider, m);
}

// One model's effort in both scenes: shared when they agree, else a scene override. An override
// cannot unset the shared level, so a scene that follows the CLI holds the shared one.
function writeEfforts(provider, modelId, wants) {
  if (!modelId) return;
  ensureCliModel(provider, modelId);
  const shared = currentDoc.clis[provider].models[modelId];
  let base = wants.task === wants.discussion || !wants.task ? "task"
    : !wants.discussion || isRestricted("task") ? "discussion" : "task";
  const fits = (level) => !level || !shared.efforts?.length || shared.efforts.includes(level);
  const other = base === "task" ? "discussion" : "task";
  if (!fits(wants[base]) && fits(wants[other])) base = other;
  // A level only a scene's own range has stays with the scenes.
  const sharable = fits(wants[base]);
  if (sharable && wants[base]) shared.effort = wants[base];
  else if (sharable) delete shared.effort;
  for (const scene of SCENE_IDS) {
    if (wants[scene] !== wants[base] || !sharable) {
      const conf = restrict(scene)[provider];
      if (!conf) continue;
      conf.overrides ||= {};
      conf.overrides[modelId] = { ...conf.overrides[modelId], effort: wants[scene] };
      continue;
    }
    const overrides = currentDoc.modes?.[scene]?.[provider]?.overrides;
    if (!overrides?.[modelId]) continue;
    delete overrides[modelId].effort;
    if (!Object.keys(overrides[modelId]).length) delete overrides[modelId];
  }
}

function edited() {
  // Known before relaxing: opening one scene must not forget a CLI the other turned off.
  const kept = new Set(PROVIDERS.filter(known));
  for (const scene of SCENE_IDS) relax(scene, kept);
  incDocVersion();
  renderCards();
}

// Change one scene, or both while the card is not split.
function changeSlot(provider, scene, change) {
  const scenes = scene ? [scene] : SCENE_IDS;
  // A joined card shows the private-task row, so both scenes take it.
  const shown = slot(scene || "task", provider);
  for (const s of scenes) {
    const next = { ...shown, ...change };
    writeSlot(s, provider, next);
    if (Object.hasOwn(change, "effort")) {
      const modelId = next.model || nativeDefault(provider).model;
      const other = SCENE_IDS.find((o) => o !== s);
      const follows = !scene || !slot(other, provider).enabled;
      writeEfforts(provider, modelId, {
        [s]: change.effort,
        [other]: follows ? change.effort : sceneEffort(other, provider, modelId),
      });
    }
  }
  edited();
}

function setProviderEnabled(provider, enabled, confirmed = false) {
  if (!enabled) {
    const settings = SCENE_IDS.some((s) => {
      const conf = currentDoc.modes?.[s]?.[provider];
      return conf && (conf.models?.length || conf.default || Object.keys(conf.overrides || {}).length);
    }) || SCENE_IDS.some((s) => currentDoc.defaults?.[s]?.[provider]);
    if (settings && !confirmed) {
      pendingDisable = provider;
      return renderCards();
    }
  }
  pendingDisable = null;
  for (const scene of SCENE_IDS) {
    if (!enabled) delete restrict(scene)[provider];
    else if (isRestricted(scene)) currentDoc.modes[scene][provider] ||= { models: [], overrides: {} };
  }
  edited();
}

// Joining the scenes again gives meetings what private tasks have.
function setSplit(provider, split) {
  if (split) {
    splitCards.add(provider);
    return renderCards();
  }
  splitCards.delete(provider);
  writeSlot("task", provider, slot("task", provider));
  const task = slot("task", provider);
  writeSlot("discussion", provider, task);
  const overrides = currentDoc.modes?.task?.[provider]?.overrides || {};
  const joined = task.enabled && Object.keys(overrides).length
    ? restrict("discussion")[provider]
    : currentDoc.modes?.discussion?.[provider];
  if (joined) joined.overrides = structuredClone(overrides);
  const modelId = task.model || nativeDefault(provider).model;
  if (task.enabled) writeEfforts(provider, modelId, { task: task.effort, discussion: task.effort });
  pickerOpen = null;
  edited();
}

let pickerOpen = null; // "provider/scene" whose model list is open

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

  for (const provider of PROVIDERS) {
    const info = detected[provider] || { installed: false, source: "none", path: null, error: null, models: [], efforts: [] };
    const enabled = SCENE_IDS.some((s) => slot(s, provider).enabled);
    const listed = known(provider);
    if (!info.installed && !listed) {
      missing.push(provider);
      continue;
    }
    const name = PROVIDER_NAMES[provider] || provider;
    const split = enabled && isSplit(provider);
    const head = el("div", { className: "member-head" },
      el("div", { className: "member-title" },
        el("span", { className: "cli-icon", "aria-hidden": "true" }, AN.hostTile(provider, name)),
        el("span", { className: "cli-name" }, name),
        info.installed && info.version ? el("span", { className: "cli-version" }, info.version) : null,
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
        renderPermission(provider) || el("span")));
    const card = el("div", { className: `cli-card member-card${enabled ? "" : " off"}`, dataset: { provider } }, head);
    if (pendingDisable === provider) {
      card.appendChild(el("div", { className: "disable-confirm", role: "group", "aria-label": L(`确认停用 ${name}`, `Confirm disabling ${name}`) },
        el("p", {}, L(`停用会清除 ${name} 的默认模型和可选模型。重新启用不会恢复；点击“保存设置”后才会生效。`, `Disabling clears default and allowed models for ${name}. Re-enabling will not restore them; takes effect after clicking “Save settings”.`)),
        el("div", { className: "btn-group" },
          el("button", { type: "button", dataset: { focusKey: `${provider}/cancel-disable` }, onClick: () => { pendingDisable = null; renderCards(); } }, L("保留启用", "Keep enabled")),
          el("button", { type: "button", className: "danger", dataset: { focusKey: `${provider}/confirm-disable` }, onClick: () => setProviderEnabled(provider, false, true) }, L("确认停用", "Disable")))));
    }
    const rows = split ? SCENE_IDS.flatMap((s) => slotRows(provider, s, true)) : slotRows(provider, null, false);
    for (const row of rows) card.appendChild(row);
    if (info.error) card.appendChild(el("p", { className: "member-error" }, L(`读取模型列表失败：${info.error}`, `Failed to load model list: ${info.error}`)));
    cards.appendChild(card);
  }

  if (missing.length) {
    const names = missing.map((p) => PROVIDER_NAMES[p] || p);
    const box = el("div", { className: "cli-missing" },
      el("button", {
        type: "button",
        className: "cli-missing-toggle",
        dataset: { focusKey: "missing/toggle" },
        "aria-expanded": String(missingOpen),
        onClick: () => { missingOpen = !missingOpen; renderCards(); },
      }, el("span", { className: "cli-chevron", "aria-hidden": "true" }), textNode(L(`未安装的 CLI（${missing.length}）`, `Uninstalled CLIs (${missing.length})`))));
    if (missingOpen) box.appendChild(el("p", { className: "cli-missing-list" },
      textNode(L(`${names.join("、")}。装好后点「刷新模型列表」，就会出现在上面，可以开始使用。`, `${names.join(", ")}. After installing, click “Refresh models” and they will appear above ready to use.`))));
    cards.appendChild(box);
  }

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

  const saving = JSON.stringify(currentDoc);

  try {
    if (saving !== savedSnapshot) {
      await apiPostMembers(JSON.parse(saving));
      savedSnapshot = saving;
    }
    if (limitChanged()) await saveLimit();
    refreshDirty();
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
  if (busy || probing) return;
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
  } catch (err) {
    if (!quiet) showNotice(L(`刷新检测失败: ${err.message}`, `Refresh failed: ${err.message}`), true);
  } finally {
    probing = false;
    updateControls();
  }
}

function probeWhenStale() {
  if (document.visibilityState === "visible" && Date.now() - probedAt > 30_000) refreshDetection(true);
}

// Load initial members data
async function loadMembers() {
  if (currentTab === "members") showNotice(L("正在读取成员与模型配置…", "Loading employee and model settings…"));
  try {
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
  } catch (err) {
    documentError = err.message;
    renderErrorBanner();
    showNotice(L(`读取失败: ${err.message}`, `Load failed: ${err.message}`), true);
    updateControls();
  }
}

// Helpers for version check
function isVersionOlder(a, b) {
  if (!a || !b) return false;
  const partsA = String(a).replace(/^v/, "").split(".").map(Number);
  const partsB = String(b).replace(/^v/, "").split(".").map(Number);
  for (let i = 0; i < Math.max(partsA.length, partsB.length); i++) {
    const numA = partsA[i] || 0;
    const numB = partsB[i] || 0;
    if (numA !== numB) return numA < numB;
  }
  return false;
}

function formatTime(iso) {
  if (!iso) return L("尚未检查", "Never");
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString(L("zh-CN", "en-US"), {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

// Render Updates panel
function renderUpdates() {
  if (!appStatus) return;

  $("app-version").textContent = appStatus.version ? `v${appStatus.version}` : "—";
  const appBadge = $("app-badge");
  const updateInfo = appStatus.update;

  if (updateInfo?.state === "available") {
    appBadge.hidden = false;
    appBadge.className = "tag available";
    appBadge.textContent = L(`发现新版 v${updateInfo.version}`, `New version v${updateInfo.version}`);
  } else if (updateInfo?.state === "installing") {
    appBadge.hidden = false;
    appBadge.className = "tag installing";
    appBadge.textContent = L("正在下载更新…", "Downloading update…");
  } else if (updateInfo?.state === "failed") {
    appBadge.hidden = false;
    appBadge.className = "tag failed";
    appBadge.textContent = L("更新失败", "Update failed");
  } else if (updateInfo?.state === "none") {
    appBadge.hidden = false;
    appBadge.className = "tag done";
    appBadge.textContent = L("已是最新版本", "Up to date");
  } else {
    appBadge.hidden = true;
  }

  // Service status
  const serviceInfo = appStatus.service;
  const serviceBadge = $("service-badge");
  const serviceMsg = $("service-msg");

  if (!serviceInfo) {
    $("service-version").textContent = installedService?.version ? `v${installedService.version}` : "—";
    serviceBadge.hidden = true;
    serviceMsg.hidden = Boolean(installedService?.installed);
    serviceMsg.textContent = L("请在 boss 分区安装本机服务。", "Install the local service in the Boss section.");
  } else {
    $("service-version").textContent = serviceInfo.version ? `v${serviceInfo.version}` : "—";
    const isServiceOutdated =
      serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version));

    if (["upgrading", "installing"].includes(serviceInfo.state)) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag upgrading";
      serviceBadge.textContent = serviceInfo.state === "installing" ? L("正在安装…", "Installing…") : L("正在升级…", "Upgrading…");
      serviceMsg.hidden = false;
      serviceMsg.textContent = L("正在从 GitHub 安装或升级本机服务，约需一分钟…", "Installing or upgrading local service from GitHub (~1 min)…");
    } else if (serviceInfo.state === "failed") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag failed";
      serviceBadge.textContent = L("升级失败", "Upgrade failed");
      serviceMsg.hidden = false;
      serviceMsg.textContent = serviceInfo.error || L("升级服务失败，请重试", "Failed to upgrade service; please try again");
    } else if (isServiceOutdated) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag outdated";
      serviceBadge.textContent = L("比 App 旧，需要升级", "Older than App; upgrade needed");
      serviceMsg.hidden = true;
    } else if (serviceInfo.state === "done") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag done";
      serviceBadge.textContent = L("安装完成", "Installed");
      serviceMsg.hidden = false;
      serviceMsg.textContent = L("服务已安装，各 boss 重连后使用新版本。", "Service installed. Each boss will use the new version after reconnecting.");
    } else {
      serviceBadge.hidden = true;
      serviceMsg.hidden = true;
    }
  }

  $("last-checked").textContent = formatTime(appStatus.last_checked);

  // Update action buttons and progress
  const hasAppUpdate = updateInfo?.state === "available";
  const isServiceOutdated =
    serviceInfo &&
    (serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version)));
  const hasUpdate = hasAppUpdate || Boolean(isServiceOutdated);

  const isUpdating =
    updateInfo?.state === "checking" ||
    updateInfo?.state === "installing" ||
    ["upgrading", "installing"].includes(serviceInfo?.state);

  const btnCheck = $("btn-check-update");
  const btnApply = $("btn-apply-update");
  const updateHint = $("update-hint");
  const progressBox = $("update-progress-box");
  const progressText = $("update-progress-text");
  const errorBox = $("update-error-box");
  const errorText = $("update-error-text");

  // One action at a time: with an update found, "更新" leads and checking again stays secondary.
  $("update-badge").hidden = !hasUpdate && updateInfo?.state !== "installing";
  btnCheck.disabled = isUpdating;
  btnCheck.textContent = hasUpdate ? L("重新检查", "Check again") : L("立即检查", "Check now");
  // While anything runs the progress line says what; a disabled action would only contradict it.
  btnApply.hidden = !hasUpdate || isUpdating;
  btnApply.textContent = hasAppUpdate ? L(`更新到 v${updateInfo.version}…`, `Update to v${updateInfo.version}…`) : L("升级本机服务…", "Upgrade local service…");
  updateHint.hidden = !hasUpdate || isUpdating;

  if (hasUpdate) {
    if (hasAppUpdate) {
      updateHint.textContent = L("先看更新内容，确认后一次完成：升级本机服务，再安装 App 并自动重启。", "Review release notes first; once confirmed, it upgrades the local service, installs the App, and restarts automatically.");
    } else {
      updateHint.textContent = L("本机服务比 App 旧。升级约需一分钟，宿主重连后生效。", "Local service is older than the App. Upgrading takes ~1 min and takes effect after reconnecting the host.");
    }
  }

  if (isUpdating) {
    progressBox.hidden = false;
    if (updateInfo?.state === "checking") {
      progressText.textContent = L("正在检查更新…", "Checking for updates…");
    } else if (updateInfo?.state === "installing") {
      const percent = updateInfo.total > 0 ? ` ${Math.min(100, Math.floor(updateInfo.downloaded / updateInfo.total * 100))}%` : "";
      progressText.textContent = {
        service: L("正在更新本机服务，约需一分钟…", "Updating local service (~1 min)…"),
        app: L("正在安装 App…", "Installing App…"),
        restart: L("正在重启并更新 Skill…", "Restarting and updating Skill…"),
      }[updateInfo.step] || L(`正在下载 App 更新包${percent}…`, `Downloading App update${percent}…`);
    } else if (["upgrading", "installing"].includes(serviceInfo?.state)) {
      progressText.textContent = L("正在安装或升级本机服务，约需一分钟…", "Installing or upgrading local service (~1 min)…");
    }
  } else {
    progressBox.hidden = true;
  }

  const hasError = updateInfo?.state === "failed" || serviceInfo?.state === "failed" || Boolean(updateInfo?.error);
  $("btn-retry-update").hidden = isUpdating;
  if (hasError && !isUpdating) {
    errorBox.hidden = false;
    errorText.textContent = updateInfo?.error || serviceInfo?.error || L("操作失败，请重试。", "Operation failed. Please try again.");
  } else {
    errorBox.hidden = true;
  }

  // Manage polling
  if (isUpdating && !statusPollTimer) {
    statusPollTimer = setInterval(() => document.hidden || loadStatus(), 2000);
  } else if (!isUpdating && statusPollTimer) {
    clearInterval(statusPollTimer);
    statusPollTimer = null;
  }
}

window.addEventListener("agentnave:hosts", ({ detail }) => {
  installedService = detail.service;
  renderUpdates();
});
window.addEventListener("agentnave:service-installed", () => loadStatus());
// The update dialog reports each status it reads; skip the ones this page sent itself.
window.addEventListener("agentnave:status", ({ detail }) => {
  if (detail === appStatus) return;
  appStatus = detail;
  renderUpdates();
});
// A hidden window skips the update poll; catch up as soon as it is shown again.
document.addEventListener("visibilitychange", () => document.hidden || !statusPollTimer || loadStatus());

async function loadStatus() {
  try {
    appStatus = await apiGetStatus();
    renderUpdates();
    window.dispatchEvent(new CustomEvent("agentnave:status", { detail: appStatus }));
  } catch (err) {
    showNotice(L(`获取版本状态失败: ${err.message}`, `Failed to load version status: ${err.message}`), true);
  }
}

// Opening this tab checks again when the automatic daily check is over an hour old. A found
// update or a failure the user has not retried is left as it is.
const AUTO_CHECK_AFTER = 60 * 60 * 1000;
async function checkIfStale() {
  if ((await window.AgentNaveAboutSettings?.automatic()) === false) return;
  const update = appStatus?.update?.state;
  const service = appStatus?.service?.state;
  if (!appStatus || ["checking", "installing", "available", "failed"].includes(update) || ["upgrading", "installing"].includes(service)) return;
  const checked = Date.parse(appStatus.last_checked || "");
  if (Number.isNaN(checked) || Date.now() - checked > AUTO_CHECK_AFTER) triggerCheckUpdate(true);
}

async function triggerCheckUpdate(quiet = false) {
  const btnCheck = $("btn-check-update");
  btnCheck.disabled = true;
  const previous = appStatus;
  try {
    // The server refuses updates while it checks; show that instead of a live button.
    appStatus = { ...appStatus, update: { state: "checking", version: null, error: null } };
    renderUpdates();
    if (!quiet) showNotice(L("正在检查更新…", "Checking for updates…"));
    appStatus = await apiCheckUpdate(quiet);
    const state = appStatus?.update?.state;
    if (!quiet || state === "available") showNotice(
      state === "failed" ? L("检查更新失败", "Update check failed") : state === "available" ? L(`发现新版 v${appStatus.update.version}`, `New version v${appStatus.update.version}`) : state === "none" ? L("已是最新版本", "Up to date") : L("检查更新完成", "Update check complete"),
      state === "failed",
    );
  } catch (err) {
    appStatus = previous;
    if (quiet) await loadStatus();
    else showNotice(L(`检查更新失败: ${err.message}`, `Update check failed: ${err.message}`), true);
  } finally {
    renderUpdates();
  }
}

// Bind top-level event listeners
for (const event of ["click", "change"]) $("cli-cards").addEventListener(event, (e) => {
  operatedControl = e.target.closest("[data-focus-key]");
  queueMicrotask(() => { operatedControl = null; });
}, true);
bindAdvanced();
const OLD_TABS = { "#updates": "about", "#skills": "advanced", "#orchestration": "advanced" }; // Preserve old links.
const tabFromHash = () => OLD_TABS[location.hash] || (["general", "members", "avatars", "voice", "advanced", "storage", "about"].includes(location.hash.slice(1)) ? location.hash.slice(1) : "hosts");
window.addEventListener("hashchange", () => setTab(tabFromHash()));
setTab(tabFromHash());

$("refresh-detection").addEventListener("click", () => refreshDetection());
window.addEventListener("focus", probeWhenStale);
document.addEventListener("visibilitychange", probeWhenStale);
$("save-btn").addEventListener("click", saveMembers);
$("max-running").addEventListener("change", () => {
  const feedback = $("save-feedback");
  feedback.textContent = "";
  feedback.className = "save-feedback";
  refreshDirty();
  updateControls();
});

$("btn-check-update").addEventListener("click", () => triggerCheckUpdate());
// Updating goes through the shared dialog: it shows what changed first and then the progress.
$("btn-apply-update").addEventListener("click", () => AN.updateFlow());
$("btn-retry-update").addEventListener("click", () => AN.updateFlow());

// The app's web view does not show beforeunload prompts, so hold back in-app navigation.
function renderLeaveConfirm() {
  const box = $(currentTab === "advanced" ? "advanced-leave-confirm" : "leave-confirm");
  const other = $(currentTab === "advanced" ? "leave-confirm" : "advanced-leave-confirm");
  other.hidden = true;
  other.replaceChildren();
  box.hidden = !leaveTarget;
  if (!leaveTarget) return box.replaceChildren();
  if (box.childElementCount) return;
  const stay = el("button", { type: "button", onClick: () => { leaveTarget = null; renderLeaveConfirm(); } }, L("继续编辑", "Keep editing"));
  box.replaceChildren(
    el("p", {}, L("有未保存的修改，离开设置页会丢失这些修改。", "You have unsaved changes. Leaving Settings will discard them.")),
    el("div", { className: "btn-group" }, stay,
      el("button", { type: "button", className: "danger", onClick: () => {
        const target = leaveTarget;
        leaveTarget = null;
        dirty = false;
        discardAdvanced();
        if (typeof target === "function") target();
        else location.href = target;
      } }, L("放弃修改并离开", "Discard and leave"))));
  stay.focus({ preventScroll: true });
}
function confirmLeave(target) {
  if (!(unsaved() || busy)) return false;
  leaveTarget = target;
  // The confirmation sits in the save bar of the page holding the edits.
  const tab = dirty || !advancedChanges().length ? "members" : "advanced";
  if (currentTab !== tab) {
    setTab(tab);
    history.replaceState(null, "", `#${tab}`);
  }
  renderLeaveConfirm();
  return true;
}
document.querySelector("header.bar").addEventListener("click", (e) => {
  const link = e.target.closest("a[href]");
  // The settings link points at this page; following it would reload and drop the edits.
  if (link?.getAttribute("href") === "/settings/") return e.preventDefault();
  if (link && confirmLeave(link.href)) e.preventDefault();
}, true);

window.addEventListener("beforeunload", (e) => {
  if (unsaved() || busy) {
    e.preventDefault();
    e.returnValue = "";
  }
});

// Initialize: show the cached detection at once, then re-probe for newly added models.
loadMembers().then(() => refreshDetection(true));
loadStatus();

// Model names: short (without the routing prefix) or full; shared with every window.
(function () {
  const buttons = { short: document.getElementById("model-names-short"), full: document.getElementById("model-names-full") };
  function show() {
    const full = AN.fullModelNames();
    for (const [key, button] of Object.entries(buttons)) {
      const on = (key === "full") === full;
      button.classList.toggle("active", on);
      button.setAttribute("aria-checked", on ? "true" : "false");
    }
    document.getElementById("model-names-example").textContent = full
      ? L("显示为 opencode-go/deepseek-v4.1-flash，带上前面的路由", "Shown as opencode-go/deepseek-v4.1-flash, with the routing prefix")
      : L("显示为 deepseek-v4.1-flash，省去前面的路由 opencode-go/", "Shown as deepseek-v4.1-flash, without the opencode-go/ prefix");
  }
  for (const [key, button] of Object.entries(buttons))
    button.addEventListener("click", () => { AN.setFullModelNames(key === "full"); show(); });
  show();
})();
