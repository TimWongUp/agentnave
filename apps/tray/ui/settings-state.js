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
  confirmScenes(doc);
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

// Host cards publish after a connection. Reload a clean employee editor, or add the CLIs the
// connection confirmed to unsaved edits, so a later save cannot drop them.
let membersHostsKey = "";
window.addEventListener("agentnave:hosts", async ({ detail }) => {
  const key = JSON.stringify((detail.hosts || [])
    .filter((host) => host.status === "connected")
    .map((host) => [host.id, host.last_action?.at]));
  if (key === membersHostsKey) return;
  membersHostsKey = key;
  if (!savedSnapshot || busy) return;
  const before = docVersion;
  const snapshot = savedSnapshot;
  try {
    const data = await apiGetMembers();
    if (busy || savedSnapshot !== snapshot) return;
    if (!dirty && docVersion === before) {
      currentDoc = data.document || { version: 1, clis: {}, modes: {} };
      documentError = data.error || null;
      markSaved();
      renderErrorBanner();
      renderCards();
      return;
    }
    const saved = JSON.parse(savedSnapshot);
    let added = false;
    for (const scene of SCENE_IDS) {
      for (const [provider, conf] of Object.entries(data.document?.modes?.[scene] || {})) {
        if (saved.modes?.[scene]?.[provider]) continue;
        ((saved.modes ||= {})[scene] ||= {})[provider] = structuredClone(conf);
        restrict(scene)[provider] ||= structuredClone(conf);
        added = true;
      }
    }
    if (!added) return;
    savedSnapshot = JSON.stringify(saved);
    refreshDirty();
    renderCards();
  } catch {
    // Keep the editor on a transient read failure; refresh detection can read it again.
  }
});

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
