"use strict";
// Settings page logic: Members & Models, Versions & Updates.
// Adheres strictly to CSP: Pure DOM construction, no HTML string parsing.

const $ = (id) => document.getElementById(id);

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

// Ensure manage.css loads properly when previewed via file:// protocol
if (location.protocol === "file:") {
  const link = el("link", { rel: "stylesheet", href: "./manage.css" });
  document.head.appendChild(link);
}

// 7 supported CLI providers
const PROVIDERS = ["codex", "claude", "codebuddy", "grok", "antigravity", "pi", "deepseek"];
const PROVIDER_NAMES = {
  codex: "Codex CLI",
  claude: "Claude Code",
  codebuddy: "CodeBuddy Code",
  grok: "Grok CLI",
  antigravity: "Antigravity CLI",
  pi: "Pi Coding Agent",
  deepseek: "DeepSeek Harness",
};
const SOURCE_LABELS = {
  "codex-cache": "Codex 缓存",
  command: "命令输出",
  help: "帮助信息",
  aliases: "别名配置",
  catalog: "内置目录",
  none: "未检测到",
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
let currentMode = "task"; // "task" | "discussion"
let currentTab = "members"; // "members" | "avatars" | "hosts"
let dirty = false;
let savedSnapshot = ""; // The document as last loaded or saved; dirty compares against it.
let savedVersion = 0;
let leaveTarget = null; // A navigation held back while there are unsaved changes.
let busy = false;
let documentError = null;
let docVersion = 0; // Monotonic counter for save race detection
let expandedModel = null; // { provider: string, model: string } | null
const openCards = new Set(); // CLI cards are folded to one summary line until opened
let appStatus = null;
let installedService = null;
let statusPollTimer = null;
let lastMigrationData = { migrated: false, migration_skipped: [] };
// Keep unfinished option edits across drawer/mode changes without writing invalid values.
const optionDrafts = new WeakMap();
let operatedControl = null;
let pendingDisable = null;
let pendingRestrictOff = false; // Turning a mode's selection off waits for confirmation.

// Determine mock mode
const isMock =
  new URLSearchParams(location.search).has("mock") ||
  location.protocol === "file:";

// Mock data covering:
// 1. Task mode restricted, discussion mode unrestricted (omitted key).
// 2. Codex in task mode has models: [] (unlimited models) with default set.
// 3. Override tri-state example.
// 4. migration_skipped with migrated=false.
const mockMembersData = {
  document: {
    version: 1,
    clis: {
      codex: {
        models: {
          "gpt-6.1-sol": {
            efforts: ["medium", "high", "xhigh"],
            effort: "high",
            options: {
              skip_git_repo_check: true,
              dangerously_bypass_approvals_and_sandbox: false,
            },
            notes: "多文件实现、需要跑测试的任务",
          },
          "gpt-5.4-turbo": {},
        },
      },
      claude: {
        models: {
          "claude-3-7-sonnet": {
            effort: "high",
            options: { custom_timeout: 60 },
            notes: "通用编码与复杂重构",
          },
        },
      },
    },
    modes: {
      task: {
        codex: {
          models: [], // Unlimited models
          default: "gpt-6.1-sol",
          overrides: {},
        },
        claude: {
          models: ["claude-3-7-sonnet"],
          default: "claude-3-7-sonnet",
          overrides: {
            // Tri-state override example: effort customized, notes customized, efforts & options inherit
            "claude-3-7-sonnet": {
              effort: "medium",
              notes: "本模式专用说明",
            },
          },
        },
      },
      // discussion is omitted => unrestricted mode
    },
  },
  error: null,
  migrated: false,
  migration_skipped: [
    "档案“旧脚本测试”未指定模型，已跳过迁移",
    "档案“危险跳过项”包含权限类参数 sandbox_bypass，已跳过迁移",
  ],
  detected: {
    codex: {
      installed: true,
      path: "/opt/homebrew/bin/codex",
      source: "codex-cache",
      error: null,
      models: [
        {
          id: "gpt-6.1-sol",
          label: "gpt-6.1-sol",
          default: false,
          efforts: ["low", "medium", "high", "xhigh"],
        },
        { id: "gpt-5.4-turbo", label: "gpt-5.4-turbo", default: false },
      ],
      efforts: ["minimal", "low", "medium", "high", "xhigh"],
    },
    claude: {
      installed: true,
      path: "/usr/local/bin/claude",
      source: "command",
      error: null,
      models: [
        { id: "claude-3-7-sonnet", label: "claude-3-7-sonnet", default: true },
        { id: "claude-3-5-haiku", label: "claude-3-5-haiku", default: false },
      ],
      efforts: ["low", "medium", "high"],
    },
    codebuddy: {
      installed: true,
      path: "/opt/homebrew/bin/codebuddy",
      source: "help",
      error: null,
      models: [
        { id: "codebuddy-pro", label: "codebuddy-pro", default: true },
      ],
      efforts: [],
    },
    grok: {
      installed: false,
      path: null,
      source: "none",
      error: "未在 PATH 中找到 grok 可执行文件",
      models: [],
      efforts: [],
    },
    antigravity: {
      installed: true,
      path: "/opt/homebrew/bin/agy",
      source: "command",
      error: null,
      models: [
        { id: "gemini-3.8-flash", label: "gemini-3.8-flash", default: true },
      ],
      efforts: ["low", "high"],
    },
    pi: {
      installed: false,
      path: null,
      source: "none",
      error: "未安装",
      models: [],
      efforts: [],
    },
  },
};

// API Helpers
async function apiGetMembers(refresh = false) {
  if (isMock) {
    return JSON.parse(JSON.stringify(mockMembersData));
  }
  const url = refresh ? "/settings/members?refresh=1" : "/settings/members";
  const res = await fetch(url, { cache: "no-store" });
  if (!res.ok) throw new Error(`获取成员配置失败（${res.status}）`);
  return res.json();
}

async function apiPostMembers(doc) {
  if (isMock) {
    mockMembersData.document = JSON.parse(JSON.stringify(doc));
    return { saved: true };
  }
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
    throw new Error(data.error || `保存失败（${res.status}）`);
  }
  return data;
}

async function apiGetStatus() {
  if (isMock) {
    return window.AgentNaveMock.fetch("/app/status");
  }
  const res = await fetch("/app/status", { cache: "no-store" });
  if (!res.ok) throw new Error(`获取状态失败（${res.status}）`);
  return res.json();
}

async function apiCheckUpdate() {
  if (isMock) {
    return window.AgentNaveMock.post("/app/update/check");
  }
  const res = await fetch("/app/update/check", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-AgentNave-Action": "1",
    },
  });
  if (!res.ok) throw new Error(`检查更新失败（${res.status}）`);
  return res.json();
}

async function apiApplyUpdate() {
  if (isMock) {
    return window.AgentNaveMock.post("/app/update/apply");
  }
  const res = await fetch("/app/update/apply", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-AgentNave-Action": "1",
    },
  });
  if (!res.ok) throw new Error(`应用更新失败（${res.status}）`);
  return res.json();
}

// Notice display
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
  dirty = docVersion !== savedVersion &&
    (JSON.stringify(currentDoc) !== savedSnapshot || configurationErrors().length > 0);
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

// Check if any restricted mode has 0 enabled CLIs
function hasEmptyRestrictedMode() {
  if (!currentDoc.modes) return false;
  for (const [mode, clisObj] of Object.entries(currentDoc.modes)) {
    if (clisObj && typeof clisObj === "object" && Object.keys(clisObj).length === 0) {
      return true;
    }
  }
  return false;
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
    const label = `参数第 ${index + 1} 行`;
    if (!row.key.trim()) errors.push(`${label}：请填写参数键`);
    if (row.key.trim() && keys.has(row.key)) errors.push(`${label}：参数键 ${row.key} 重复`);
    if (["model", "effort"].includes(row.key)) errors.push(`${label}：不能使用 ${row.key} 键`);
    keys.add(row.key);
    let value = row.raw;
    if (row.type === "number") {
      value = Number(row.raw);
      if (!row.raw.trim() || !Number.isFinite(value)) errors.push(`${label}：必须为有效有限数字`);
    } else if (row.type === "boolean") {
      value = row.raw === "true";
    }
    entries.push([row.key, value]);
  }
  return { options: Object.fromEntries(entries), errors };
}

function effortError(config) {
  if (Object.hasOwn(config, "effort") && !config.effort) {
    return "默认思考档位不可为空，请选择具体档位或清除默认设置（覆盖时选择继承）";
  }
  if (Object.hasOwn(config, "effort") && config.efforts?.length && !config.efforts.includes(config.effort)) {
    return `默认思考档位“${config.effort}”不在允许档位中，请重新选择默认档位或调整允许档位（覆盖时也可改为继承）`;
  }
  return null;
}

// Validate every mode, including collapsed drawers and inherited defaults in other modes.
function configurationErrors() {
  const errors = [];
  const add = (provider, model, scope, message) => {
    if (message) errors.push({ provider, model, scope, message: `${scope} / ${provider} / ${model}：${message}` });
  };
  const check = (config, provider, model, scope) => {
    add(provider, model, scope, effortError(config));
    for (const error of parseOptionRows(optionRows(config)).errors) add(provider, model, scope, error);
  };
  for (const [provider, cli] of Object.entries(currentDoc.clis || {})) {
    for (const [model, shared] of Object.entries(cli.models)) check(shared, provider, model, "共享");
  }
  for (const [mode, providers] of Object.entries(currentDoc.modes || {})) {
    const scope = mode === "task" ? "私聊" : "会议";
    for (const [provider, selection] of Object.entries(providers)) {
      const models = currentDoc.clis?.[provider]?.models || {};
      for (const model of selection.models) {
        if (!Object.hasOwn(models, model)) add(provider, model, scope, "模型尚未配置");
      }
      if (Object.hasOwn(selection, "default") &&
          (!Object.hasOwn(models, selection.default) || (selection.models.length && !selection.models.includes(selection.default)))) {
        add(provider, selection.default, scope, "默认模型不在允许范围内");
      }
      for (const [model, override] of Object.entries(selection.overrides || {})) {
        if (!Object.hasOwn(models, model)) add(provider, model, scope, "覆盖的模型尚未配置");
        add(provider, model, scope, effortError({ ...models[model], ...override }));
        for (const error of parseOptionRows(optionRows(override)).errors) add(provider, model, scope, error);
      }
    }
  }
  for (const [mode, providers] of Object.entries(currentDoc.defaults || {})) {
    const scope = mode === "task" ? "私聊" : "会议";
    for (const [provider, model] of Object.entries(providers)) {
      if (!Object.hasOwn(currentDoc.clis?.[provider]?.models || {}, model)) add(provider, model, scope, "默认模型尚未配置");
    }
  }
  return errors;
}

function updateControls() {
  $("dirty-text").textContent = dirty ? "有未保存的修改" : "没有未保存的修改";
  $("members-badge").hidden = !dirty;
  if (!dirty && !busy) leaveTarget = null;
  renderLeaveConfirm();

  const emptyRestricted = hasEmptyRestrictedMode();
  const errors = configurationErrors();
  const canSave = !busy && dirty && !documentError && !emptyRestricted && errors.length === 0;

  $("save-btn").disabled = !canSave;
  $("refresh-detection").disabled = busy;

  // Feedback hints
  const feedback = $("save-feedback");
  if (emptyRestricted) {
    feedback.textContent = "开启自选的场景至少启用一个 CLI，或关闭自选";
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
      permSummary.textContent = `包含 ${permCount} 个权限类默认参数`;
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

// Switch tabs
function setTab(tab) {
  if (tab === "updates") tab = "hosts"; // Preserve old update links.
  // Status lines belong to the tab that produced them.
  if (currentTab !== tab) showNotice("");
  currentTab = tab;
  for (const id of ["members", "avatars", "hosts"]) {
    $("tab-" + id).classList.toggle("active", tab === id);
    $("tab-" + id).setAttribute("aria-selected", String(tab === id));
    $("tab-" + id).setAttribute("aria-controls", "pane-" + id);
    $("pane-" + id).hidden = tab !== id;
  }
  if (tab === "hosts") loadStatus();
}

// Switch mode
function setMode(mode) {
  if (currentMode === mode) return;
  currentMode = mode;
  pendingDisable = null;
  pendingRestrictOff = false;
  $("mode-task").classList.toggle("active", mode === "task");
  $("mode-task").setAttribute("aria-checked", mode === "task" ? "true" : "false");
  $("mode-discussion").classList.toggle("active", mode === "discussion");
  $("mode-discussion").setAttribute("aria-checked", mode === "discussion" ? "true" : "false");
  renderModeToolbar();
  renderCards();
}

// Check if mode is restricted
function isModeRestricted(mode = currentMode) {
  return Boolean(currentDoc.modes && (mode in currentDoc.modes));
}

function renderModeToolbar() {
  const restricted = isModeRestricted();
  const toggle = $("mode-restrict-toggle");
  const hint = $("mode-restrict-hint");
  if (toggle) toggle.checked = restricted;
  if (hint) {
    hint.textContent = restricted
      ? "已开启：只允许使用下方启用的工具与模型，并可为本场景设置默认模型。关闭会保留各 CLI 的默认模型，清除本场景的名单和专用设置。"
      : "已关闭：不额外限制可用工具和模型，仍可为本场景设置各 CLI 的默认模型。开启后可逐项选择可用名单。";
  }
  const confirm = $("restrict-confirm");
  if (confirm) {
    confirm.hidden = !pendingRestrictOff;
    const scene = currentMode === "task" ? "私聊" : "会议";
    confirm.replaceChildren(...(pendingRestrictOff ? [
      el("p", {}, `关闭后${scene}不再限制工具和模型：保留各 CLI 的默认模型和共享设置，清除可用名单和${scene}专用设置。重新开启不会恢复名单和专用设置，默认模型沿用保留值，没有的按检测结果补主力模型；点击“保存设置”后才生效。`),
      el("div", { className: "btn-group" },
        el("button", { type: "button", onClick: () => { pendingRestrictOff = false; renderModeToolbar(); $("mode-restrict-toggle").focus({ preventScroll: true }); } }, "保留开启"),
        el("button", { type: "button", className: "danger", onClick: () => toggleModeRestricted(false, true) }, "确认关闭")),
    ] : []));
  }
  $("mode-description").textContent = currentMode === "task"
    ? "私聊：boss 把一项工作交给一个 AI 助手执行。两种场景的可用名单分别保存。"
    : "会议：多个 AI 助手围绕同一问题分别回答或轮流交流。两种场景的可用名单分别保存。";
}

function toggleModeRestricted(restricted, confirmed = false) {
  pendingDisable = null;
  if (!restricted && !confirmed) {
    pendingRestrictOff = true;
    renderModeToolbar();
    return;
  }
  pendingRestrictOff = false;
  if (!currentDoc.modes) currentDoc.modes = {};
  // Default models survive the switch both ways; the allow-list and mode overrides do not (ADR-0021).
  if (restricted) {
    const kept = currentDoc.defaults?.[currentMode] || {};
    currentDoc.modes[currentMode] = {};
    // Pre-enable all installed CLIs with models: [] (unlimited models) and their default model
    for (const p of PROVIDERS) {
      if (detected[p]?.installed) {
        const conf = { models: [], overrides: {} };
        if (kept[p]) conf.default = kept[p];
        currentDoc.modes[currentMode][p] = kept[p] ? conf : withPrimary(p, conf);
      }
    }
    setKeptDefaults({});
  } else {
    const kept = {};
    for (const [p, conf] of Object.entries(currentDoc.modes[currentMode] || {})) {
      if (conf.default) kept[p] = conf.default;
    }
    delete currentDoc.modes[currentMode];
    setKeptDefaults(kept);
  }
  incDocVersion();
  renderModeToolbar();
  renderCards();
}

// Default models of an unrestricted mode; an empty map leaves no `defaults` key behind.
function setKeptDefaults(kept, mode = currentMode) {
  if (Object.keys(kept).length) {
    currentDoc.defaults = { ...currentDoc.defaults, [mode]: kept };
  } else if (currentDoc.defaults) {
    delete currentDoc.defaults[mode];
    if (!Object.keys(currentDoc.defaults).length) delete currentDoc.defaults;
  }
}

function defaultModel(provider) {
  return isModeRestricted()
    ? currentDoc.modes[currentMode]?.[provider]?.default
    : currentDoc.defaults?.[currentMode]?.[provider];
}

function isProviderEnabledInMode(provider) {
  if (!isModeRestricted()) return true; // Unrestricted mode => all available
  return Boolean(currentDoc.modes?.[currentMode]?.[provider]);
}

function toggleProviderInMode(provider, enabled, confirmed = false) {
  if (!isModeRestricted()) return;
  if (enabled) {
    if (!currentDoc.modes[currentMode][provider]) {
      currentDoc.modes[currentMode][provider] = withPrimary(provider, {
        models: [],
        overrides: {},
      });
    }
  } else {
    const previous = currentDoc.modes[currentMode][provider];
    const hasSettings = previous && (previous.models?.length || previous.default || Object.keys(previous.overrides || {}).length);
    if (hasSettings && !confirmed) {
      pendingDisable = provider;
      renderCards();
      return;
    }
    delete currentDoc.modes[currentMode][provider];
  }
  pendingDisable = null;
  incDocVersion();
  renderCards();
}

// A newly enabled CLI defaults to its main model when detection names one (ADR-0020).
function withPrimary(provider, provConf) {
  const primary = detected[provider]?.primary;
  if (primary) {
    ensureCliModel(provider, primary);
    provConf.default = primary;
  }
  return provConf;
}

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

// Model allowed in current mode
function isModelAllowed(provider, modelId) {
  if (!isModeRestricted()) return true;
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return false;
  if (!Array.isArray(provConf.models) || provConf.models.length === 0) return true; // Empty models => unlimited
  return provConf.models.includes(modelId);
}

function getAllModelIds(provider) {
  const info = detected[provider] || {};
  const detectedModels = info.models || [];
  const configuredModels = Object.keys(currentDoc.clis?.[provider]?.models || {});
  const modeModels = currentDoc.modes?.[currentMode]?.[provider]?.models || [];

  const modelIdSet = new Set();
  for (const m of detectedModels) modelIdSet.add(m.id);
  for (const id of configuredModels) modelIdSet.add(id);
  for (const id of modeModels) modelIdSet.add(id);
  return Array.from(modelIdSet).filter((id) => typeof id === "string" && id.trim());
}

function toggleModelAllowed(provider, modelId, allowed) {
  if (!isModeRestricted()) return;
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return;
  ensureCliModel(provider, modelId);
  if (!Array.isArray(provConf.models)) provConf.models = [];

  if (allowed) {
    if (!provConf.models.includes(modelId)) {
      provConf.models.push(modelId);
    }
  } else {
    // If only 1 model left, do not allow unchecking
    const uniqueModels = Array.from(new Set(provConf.models));
    if (uniqueModels.length <= 1 && uniqueModels.includes(modelId)) {
      showNotice("至少保留一个模型；要放开请打开“允许使用这个工具支持的任意模型”，要停用请关闭“在此场景启用”", true);
      renderCards();
      return;
    }
    provConf.models = provConf.models.filter((m) => m !== modelId);
    // If unchecked, delete override in this mode and clear default if it was default
    if (provConf.overrides) {
      delete provConf.overrides[modelId];
    }
    if (provConf.default === modelId) {
      delete provConf.default;
    }
  }
  incDocVersion();
  renderCards();
}

function toggleUnlimitedModels(provider, unlimited) {
  if (!isModeRestricted()) return;
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return;

  if (unlimited) {
    provConf.models = [];
  } else {
    const allModelIds = getAllModelIds(provider);
    if (allModelIds.length === 0) {
      showNotice("列表中没有可用模型，请先添加模型", true);
      renderCards();
      return;
    }
    const currentChecked = Array.isArray(provConf.models)
      ? provConf.models.filter((m) => allModelIds.includes(m))
      : [];
    if (currentChecked.length > 0) {
      provConf.models = currentChecked;
    } else {
      // Pick default model, or first model in candidate list
      const info = detected[provider] || {};
      let targetModel = null;
      if (provConf.default && allModelIds.includes(provConf.default)) {
        targetModel = provConf.default;
      } else {
        const detectedDefault = info.models?.find((m) => m.default && allModelIds.includes(m.id))?.id;
        targetModel = detectedDefault || allModelIds[0];
      }
      provConf.models = [targetModel];
    }
    for (const model of provConf.models) ensureCliModel(provider, model);
  }
  incDocVersion();
  renderCards();
}

function setModelDefault(provider, modelId) {
  if (!isModeRestricted()) {
    const kept = { ...currentDoc.defaults?.[currentMode] };
    ensureCliModel(provider, modelId);
    if (kept[provider] === modelId) delete kept[provider];
    else kept[provider] = modelId;
    setKeptDefaults(kept);
    incDocVersion();
    renderCards();
    return;
  }
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return;
  ensureCliModel(provider, modelId);
  if (provConf.default === modelId) {
    delete provConf.default;
  } else {
    provConf.default = modelId;
    // An empty list must stay empty: it means unlimited models.
    if (provConf.models.length && !provConf.models.includes(modelId)) {
      provConf.models.push(modelId);
    }
  }
  incDocVersion();
  renderCards();
}

function isModelOverridden(provider, modelId) {
  if (!isModeRestricted()) return false;
  return Object.hasOwn(currentDoc.modes[currentMode]?.[provider]?.overrides || {}, modelId);
}

function toggleModelOverride(provider, modelId, override) {
  if (!isModeRestricted()) return;
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return;
  ensureCliModel(provider, modelId);
  if (!provConf.overrides) provConf.overrides = {};

  if (override) {
    if (!Object.hasOwn(provConf.overrides, modelId)) {
      // Create empty override (all fields default to inherit)
      Object.defineProperty(provConf.overrides, modelId, {
        value: {}, enumerable: true, configurable: true, writable: true,
      });
    }
  } else {
    delete provConf.overrides[modelId];
  }
  incDocVersion();
  renderCards();
}

function addCustomModel(provider, modelId) {
  const trimmed = modelId.trim();
  if (!trimmed) return;
  ensureCliModel(provider, trimmed);

  if (isModeRestricted()) {
    const provConf = currentDoc.modes[currentMode]?.[provider];
    if (provConf && Array.isArray(provConf.models) && provConf.models.length > 0) {
      if (!provConf.models.includes(trimmed)) {
        provConf.models.push(trimmed);
      }
    }
  }
  expandedModel = { provider, model: trimmed };
  incDocVersion();
  renderCards();
}

function renderOptionsField(targetConfig, sharedConfig, overridden, refreshModelErrors) {
  const inherited = overridden && !Object.hasOwn(targetConfig, "options");
  const rows = optionRows(inherited ? sharedConfig : targetConfig);
  if (!inherited) optionDrafts.set(targetConfig, rows);
  const list = el("div", { className: `options-list${inherited ? " field-inherited-area" : ""}` });
  const sync = () => {
    const result = parseOptionRows(rows);
    if (!result.errors.length) targetConfig.options = result.options;
    incDocVersion();
    refreshModelErrors();
  };
  const renderRow = (draft) => {
    const key = el("input", { type: "text", value: draft.key, placeholder: "参数键", disabled: inherited });
    const type = el("select", { disabled: inherited },
      ...[["string", "字符串"], ["number", "数字"], ["boolean", "布尔"]].map(([value, label]) => el("option", { value }, label)));
    type.value = draft.type;
    const valueBox = el("span", { className: "option-value" });
    const warning = el("div", { className: "option-permission-warning" },
      "权限类参数：保存后会对该组员在此范围内的每次调用生效，boss 不会再逐次征求你的同意。");
    const markPermission = () => {
      warning.hidden = !grantsPermission(draft.key, draft.type === "boolean" ? draft.raw === "true" : draft.raw);
    };
    markPermission();
    const renderValue = () => {
      const value = draft.type === "boolean"
        ? el("select", { disabled: inherited }, el("option", { value: "true" }, "true"), el("option", { value: "false" }, "false"))
        : el("input", { type: "text", value: draft.raw, placeholder: "参数值", disabled: inherited });
      value.value = draft.raw;
      value.dataset.optionVal = "";
      const markInvalid = () => value.classList.toggle("input-error",
        draft.type === "number" && (!draft.raw.trim() || !Number.isFinite(Number(draft.raw))));
      markInvalid();
      const updateValue = () => {
        draft.raw = value.value; // Strings retain intentional whitespace.
        markInvalid();
        markPermission();
        sync();
      };
      value.addEventListener("input", updateValue);
      value.addEventListener("change", updateValue);
      valueBox.replaceChildren(value);
    };
    renderValue();
    type.addEventListener("change", () => {
      draft.type = type.value;
      if (draft.type === "boolean") draft.raw = "true";
      renderValue();
      markPermission();
      sync();
    });
    key.addEventListener("input", () => {
      draft.key = key.value;
      markPermission();
      sync();
    });
    const row = el("div", { className: "option-row" }, key, type, valueBox,
      el("button", { type: "button", className: "btn-remove-option", disabled: inherited, onClick: () => {
        rows.splice(rows.indexOf(draft), 1);
        row.remove();
        sync();
      } }, "移除"), warning);
    return row;
  };
  for (const row of rows) list.appendChild(renderRow(row));
  const heading = el("div", { className: "options-heading" }, el("h4", {}, "高级参数（可选）"));
  if (overridden) {
    heading.appendChild(el("label", { className: "field-inherit-label" }, el("input", {
      type: "checkbox", checked: inherited, onChange: (e) => {
        optionDrafts.delete(targetConfig);
        if (e.target.checked) delete targetConfig.options;
        else targetConfig.options = { ...sharedConfig.options };
        incDocVersion();
        renderCards();
      },
    }), textNode(" 继承共享设置")));
  }
  heading.appendChild(el("button", { type: "button", disabled: inherited, onClick: () => {
    const draft = { key: "", type: "string", raw: "" };
    rows.push(draft);
    list.appendChild(renderRow(draft));
    sync();
  } }, "＋ 添加参数"));
  return el("div", { className: "detail-field" }, heading,
    el("p", { className: "field-hint" }, "高级设置，不确定时留空。参数名和取值须符合该工具的要求；字符串是文字，数字用于数值，布尔表示 true（是）或 false（否）。参数名不能重复，模型和思考档位请在对应选项中设置。"), list);
}

// Render model detail form with tri-state inheritance
function renderModelDetail(provider, modelId) {
  ensureCliModel(provider, modelId);
  const overridden = isModelOverridden(provider, modelId);
  const provConf = currentDoc.modes?.[currentMode]?.[provider];
  const sharedConfig = currentDoc.clis[provider]?.models?.[modelId] || {};
  const overrideConfig = overridden ? provConf.overrides[modelId] : null;

  const detailBox = el("div", { className: "model-detail" });
  const validationMessage = el("p", { className: "field-error", role: "status", hidden: true });
  const refreshModelErrors = () => {
    const errors = configurationErrors().filter((error) => error.provider === provider && error.model === modelId);
    validationMessage.textContent = errors.map((error) => error.message).join("；");
    validationMessage.hidden = errors.length === 0;
  };
  refreshModelErrors();
  detailBox.appendChild(validationMessage);

  // Mode override switch (only active in restricted mode)
  if (isModeRestricted()) {
    const overrideCheckbox = el("input", {
      type: "checkbox",
      role: "switch",
      className: "switch-input",
      checked: overridden,
      onChange: (e) => toggleModelOverride(provider, modelId, e.target.checked),
    });
    const overrideToggle = el(
      "div",
      { className: "detail-override-toggle" },
      el("label", {}, overrideCheckbox, textNode(" 仅在当前场景使用不同设置")),
      el(
        "p",
        { className: "field-hint" },
        overridden
          ? "下方修改只用于当前场景。勾选“继承共享设置”的项目仍跟随共享值；关闭本开关会清除此模型的场景专用设置。"
          : "当前编辑的是共享设置，私聊和会议都会使用。开启后可只为当前场景设置不同的值。"
      )
    );
    detailBox.appendChild(overrideToggle);
  }
  if (!isModeRestricted()) detailBox.appendChild(el("p", { className: "field-hint" },
    "当前编辑的是共享设置，私聊和会议都会使用。需要分别设置时，请先开启“自选可用工具和模型”。"));

  // Target object to edit
  const targetConfig = overridden ? overrideConfig : sharedConfig;

  // 1. Efforts Field (checklist)
  const detectedModelObj = (detected[provider]?.models || []).find((m) => m.id === modelId);
  const modelEfforts = detectedModelObj?.efforts;
  const detectedEfforts =
    Array.isArray(modelEfforts) && modelEfforts.length > 0
      ? modelEfforts
      : detected[provider]?.efforts || [];
  // Preserve configured values even when a probe no longer lists them.
  const candidateEfforts = [...new Set([
    ...detectedEfforts, ...(sharedConfig.efforts || []), ...(overrideConfig?.efforts || []),
    ...[sharedConfig.effort, overrideConfig?.effort].filter((effort) => typeof effort === "string" && effort),
  ])];

  const effortsInherited = overridden && !("efforts" in overrideConfig);
  {
    const activeEfforts = effortsInherited
      ? (Array.isArray(sharedConfig.efforts) ? sharedConfig.efforts : [])
      : (Array.isArray(targetConfig.efforts) ? targetConfig.efforts : []);

    const checklist = el("div", {
      className: `efforts-checklist${effortsInherited ? " field-inherited-area" : ""}`,
    });

    for (const eff of candidateEfforts) {
      const effBox = el("input", {
        type: "checkbox",
        checked: activeEfforts.includes(eff),
        disabled: effortsInherited,
        onChange: (e) => {
          let list = Array.isArray(targetConfig.efforts) ? [...targetConfig.efforts] : [];
          if (e.target.checked) {
            if (!list.includes(eff)) list.push(eff);
          } else {
            list = list.filter((item) => item !== eff);
          }
          // An empty list means unlimited, so a default effort stays valid; other conflicts
          // stay visible until the user resolves them, including defaults inherited by a mode.
          targetConfig.efforts = list;
          incDocVersion();
          renderCards();
        },
      });
      checklist.appendChild(el("label", { className: "effort-item" }, effBox, textNode(eff)));
    }

    const headingKids = [el("h4", {}, "允许使用的思考档位")];
    if (overridden) {
      const inheritEffortsCheckbox = el("input", {
        type: "checkbox",
        checked: effortsInherited,
        onChange: (e) => {
          if (e.target.checked) {
            delete overrideConfig.efforts;
          } else {
            overrideConfig.efforts = Array.isArray(sharedConfig.efforts) ? [...sharedConfig.efforts] : [];
          }
          incDocVersion();
          renderCards();
        },
      });
      headingKids.push(
        el("label", { className: "field-inherit-label" }, inheritEffortsCheckbox, textNode(" 继承共享设置"))
      );
    }

    const effortsField = el(
      "div",
      { className: "detail-field" },
      el("div", { className: "field-heading-with-inherit" }, ...headingKids),
      checklist,
      el("p", { className: "field-hint" }, "全部不勾选表示不限制；勾选后只允许 boss 使用这些档位。low 为较低、medium 为中等、high 为较高，xhigh / max / ultra 是更高档位；通常思考更多也更慢，实际支持情况由工具决定。")
    );
    detailBox.appendChild(effortsField);
  }

  // 2. Default Effort Field
  const effortInherited = overridden && !("effort" in overrideConfig);
  const activeEffortVal = effortInherited ? sharedConfig.effort : targetConfig.effort;

  const currentConfiguredEfforts = effortsInherited
    ? sharedConfig.efforts
    : targetConfig.efforts;

  // Option candidate list for default effort
  const effortDropdownChoices =
    Array.isArray(currentConfiguredEfforts) && currentConfiguredEfforts.length > 0
      ? currentConfiguredEfforts
      : (candidateEfforts.length > 0 ? candidateEfforts : ["minimal", "low", "medium", "high", "xhigh"]);

  const effortSelect = el("select", {
    "aria-label": "默认思考档位",
    disabled: effortInherited,
    onChange: (e) => {
      const val = e.target.value;
      if (!val) {
        delete targetConfig.effort;
      } else {
        targetConfig.effort = val;
      }
      incDocVersion();
      renderCards();
    },
  });

  // If not in override, or if in override setting specific value, populate options
  if (!overridden || (effortInherited && activeEffortVal === undefined)) {
    effortSelect.appendChild(el("option", { value: "" }, "不设置（继承原生默认）"));
  }

  const unavailableEffort = activeEffortVal !== undefined && !effortDropdownChoices.includes(activeEffortVal);
  if (unavailableEffort) {
    effortSelect.appendChild(el("option", { value: "__unavailable_effort__", disabled: true, selected: true },
      `当前值：${activeEffortVal || "空值"}（不在允许档位中）`));
  }

  for (const eff of effortDropdownChoices) {
    const opt = el("option", { value: eff }, eff);
    if (activeEffortVal === eff) opt.selected = true;
    effortSelect.appendChild(opt);
  }
  effortSelect.value = unavailableEffort ? "__unavailable_effort__" : (activeEffortVal ?? "");

  const effortHeadingKids = [el("h4", {}, "默认思考档位")];
  if (overridden) {
    const inheritEffortCheckbox = el("input", {
      type: "checkbox",
      checked: effortInherited,
      onChange: (e) => {
        if (e.target.checked) {
          delete overrideConfig.effort;
        } else {
          // In override, effort must be a concrete choice, not empty
          overrideConfig.effort = effortDropdownChoices.includes(sharedConfig.effort)
            ? sharedConfig.effort : effortDropdownChoices[0];
        }
        incDocVersion();
        renderCards();
      },
    });
    effortHeadingKids.push(
      el("label", { className: "field-inherit-label" }, inheritEffortCheckbox, textNode(" 继承共享设置"))
    );
  }

  const defaultEffortField = el(
    "div",
    { className: "detail-field" },
    el("div", { className: "field-heading-with-inherit" }, ...effortHeadingKids),
    el("div", { className: effortInherited ? "field-inherited-area" : "" }, effortSelect),
    el("p", { className: "field-hint" }, "boss 没有指定思考档位时使用此值；“不设置”表示交给工具自行决定。默认值须在上方允许的档位内。")
  );
  detailBox.appendChild(defaultEffortField);

  // 3. Other Options Field
  detailBox.appendChild(renderOptionsField(targetConfig, sharedConfig, overridden, refreshModelErrors));

  // 4. Notes Field
  const notesInherited = overridden && !("notes" in overrideConfig);
  const activeNotes = notesInherited ? (sharedConfig.notes || "") : (targetConfig.notes || "");

  const notesText = el("textarea", {
    placeholder: "填写适用任务或选择理由…",
    disabled: notesInherited,
    onInput: (e) => {
      targetConfig.notes = e.target.value;
      incDocVersion();
    },
  });
  notesText.value = activeNotes;

  const notesHeadingKids = [el("h4", {}, "适合做什么")];
  if (overridden) {
    const inheritNotesCheckbox = el("input", {
      type: "checkbox",
      checked: notesInherited,
      onChange: (e) => {
        if (e.target.checked) {
          delete overrideConfig.notes;
        } else {
          overrideConfig.notes = sharedConfig.notes || "";
        }
        incDocVersion();
        renderCards();
      },
    });
    notesHeadingKids.push(
      el("label", { className: "field-inherit-label" }, inheritNotesCheckbox, textNode(" 继承共享设置"))
    );
  }

  const notesField = el(
    "div",
    { className: "detail-field" },
    el("div", { className: "field-heading-with-inherit" }, ...notesHeadingKids),
    el("div", { className: notesInherited ? "field-inherited-area" : "" }, notesText),
    el("p", { className: "field-hint" }, "写给 boss 的选择参考，例如“适合快速问答”或“适合多文件修改”。可以留空，它不会直接作为模型的任务指令。")
  );
  detailBox.appendChild(notesField);

  return detailBox;
}

// Render 6 CLI cards
// One line that says what a folded card allows, so the list can stay closed.
function cardSummary(provider, isEnabled, modeRestricted, info) {
  if (!modeRestricted) {
    const detectedCount = info.models?.length ? `检测到 ${info.models.length} 个模型` : "";
    const kept = defaultModel(provider);
    return kept ? [detectedCount, `默认 ${kept}`].filter(Boolean).join(" · ") : detectedCount;
  }
  if (!isEnabled) return "当前场景未启用";
  const conf = currentDoc.modes[currentMode]?.[provider] || {};
  const models = Array.isArray(conf.models) ? conf.models : [];
  const scope = models.length === 0 ? "不限模型" : `${models.length} 个模型`;
  return conf.default ? `${scope} · 默认 ${conf.default}` : scope;
}

function renderCards() {
  const container = $("cli-cards");
  // Build off-screen: emptying the live list first clamps the document scroll position.
  // Restore the operated control after replacement without native focus auto-scrolling.
  // WebKit does not focus every checkbox/button on a pointer click.
  const active = operatedControl?.isConnected ? operatedControl : document.activeElement;
  const focusKey = active?.dataset.focusKey;
  const anchorTop = focusKey ? active.getBoundingClientRect().top : null;
  const cards = document.createDocumentFragment();

  const modeRestricted = isModeRestricted();

  for (const provider of PROVIDERS) {
    const info = detected[provider] || {
      installed: false,
      source: "none",
      path: null,
      error: null,
      models: [],
      efforts: [],
    };
    const isEnabled = isProviderEnabledInMode(provider);
    const card = el("div", {
      className: `cli-card${info.installed ? "" : " uninstalled"}`,
      dataset: { provider },
    });

    // Header
    const statusBadge = info.installed
      ? el("span", { className: "badge badge-run" }, "已安装")
      : el("span", { className: "badge badge-bad" }, "未安装");

    const sourceLabel = SOURCE_LABELS[info.source] || info.source;
    const sourceBadge = el("span", { className: "badge badge-muted" }, `来源: ${sourceLabel}`);

    let enableControl;
    if (!modeRestricted) {
      enableControl = info.installed
        ? el("span", { className: "badge badge-run" }, "未限制：默认可用")
        : el("span", { className: "badge badge-muted" }, "未安装，暂不可用");
    } else {
      const enableCheckbox = el("input", {
        type: "checkbox",
        role: "switch",
        className: "switch-input",
        dataset: { focusKey: `${provider}/enabled` },
        "aria-label": `在${currentMode === "task" ? "私聊" : "会议"}中使用 ${PROVIDER_NAMES[provider]}`,
        checked: isEnabled,
        onChange: (e) => toggleProviderInMode(provider, e.target.checked),
      });
      enableControl = el("label", {}, enableCheckbox, textNode(" 在此场景启用"));
    }

    const isOpen = openCards.has(provider);
    const toggle = el(
      "button",
      {
        type: "button",
        className: "cli-toggle",
        dataset: { focusKey: `${provider}/expand` },
        "aria-expanded": String(isOpen),
        onClick: () => {
          if (isOpen) openCards.delete(provider);
          else openCards.add(provider);
          renderCards();
        },
      },
      el("span", { className: "cli-chevron", "aria-hidden": "true" }),
      el("span", { className: "cli-name" }, PROVIDER_NAMES[provider] || provider),
      el("span", { className: "cli-summary" }, cardSummary(provider, isEnabled, modeRestricted, info))
    );
    const header = el(
      "div",
      { className: "cli-header" },
      el("div", { className: "cli-title-area" }, toggle, statusBadge, ...(isOpen ? [sourceBadge] : [])),
      el("div", { className: "cli-enable-area" }, enableControl)
    );
    card.appendChild(header);
    if (pendingDisable === provider) {
      card.appendChild(el("div", { className: "disable-confirm", role: "group", "aria-label": `确认停用 ${PROVIDER_NAMES[provider]}` },
        el("p", {}, `停用会清除 ${PROVIDER_NAMES[provider]} 在此场景的模型名单、默认模型和专用设置，共享设置保留。重新启用不会恢复；点击“保存设置”后才会生效。`),
        el("div", { className: "btn-group" },
          el("button", { type: "button", dataset: { focusKey: `${provider}/cancel-disable` }, onClick: () => { pendingDisable = null; renderCards(); } }, "保留启用"),
          el("button", { type: "button", className: "danger", dataset: { focusKey: `${provider}/confirm-disable` }, onClick: () => toggleProviderInMode(provider, false, true) }, "确认停用"))));
    }
    if (!isOpen) {
      cards.appendChild(card);
      continue;
    }

    // Metadata
    if (info.path || info.error) {
      const metaRow = el("div", { className: "cli-meta-row" });
      if (info.path) {
        metaRow.appendChild(el("span", {}, `路径: ${info.path}`));
      }
      if (info.error) {
        metaRow.appendChild(el("span", { className: "cli-error" }, `探测状态: ${info.error}`));
      }
      card.appendChild(metaRow);
    }

    // Body
    const body = el("div", {
      className: `cli-body${isEnabled ? "" : " disabled"}`,
    });

    if (!isEnabled) {
      body.appendChild(
        el("p", { className: "cli-disabled-hint" }, "当前场景未启用该 CLI；启用后可配置允许调用的模型。")
      );
    } else {
      const provConf = modeRestricted ? currentDoc.modes[currentMode]?.[provider] : null;
      const isUnlimited = modeRestricted && Array.isArray(provConf?.models) && provConf.models.length === 0;

      // Unlimited models toggle (only in restricted mode)
      if (modeRestricted) {
        const unlimitedBox = el("input", {
          type: "checkbox",
          role: "switch",
          className: "switch-input",
          dataset: { focusKey: `${provider}/unlimited` },
          checked: isUnlimited,
          onChange: (e) => toggleUnlimitedModels(provider, e.target.checked),
        });
        const unlimitedWrap = el(
          "div",
          { className: "cli-unlimited-wrap" },
          el("label", { className: "cli-unlimited-label" }, unlimitedBox, textNode(" 允许使用这个工具支持的任意模型"))
        );
        body.appendChild(unlimitedWrap);
      }

      // Collect all models
      const detectedModels = info.models || [];
      const allModelIds = getAllModelIds(provider);

      const modelList = el("div", { className: "model-list" });
      const checkedCount = new Set(provConf?.models || []).size;

      for (const modelId of allModelIds) {
        const isDetected = detectedModels.some((m) => m.id === modelId);
        // "已不在列表" only if CLI installed and source in command, codex-cache, help
        const isStale =
          !isDetected &&
          info.installed &&
          ["command", "codex-cache", "help"].includes(info.source);

        const isAllowed = isModelAllowed(provider, modelId);
        const isDefault = defaultModel(provider) === modelId;
        const hasOverride = isModelOverridden(provider, modelId);
        const isExpanded =
          expandedModel &&
          expandedModel.provider === provider &&
          expandedModel.model === modelId;

        const isLastChecked =
          modeRestricted &&
          !isUnlimited &&
          checkedCount === 1 &&
          Array.isArray(provConf?.models) &&
          provConf.models.includes(modelId);

        const modelItem = el("div", {
          className: `model-item${hasOverride ? " has-override" : ""}`,
          dataset: { model: modelId },
        });

        // Model Checkbox
        let checkInput;
        if (modeRestricted) {
          checkInput = el("input", {
            type: "checkbox",
            dataset: { focusKey: `${provider}/${modelId}/allowed` },
            checked: isAllowed,
            disabled: isUnlimited || isLastChecked,
            onChange: (e) => toggleModelAllowed(provider, modelId, e.target.checked),
          });
        }

        // Default Radio
        const radioInput = el("input", {
          type: "radio",
          dataset: { focusKey: `${provider}/${modelId}/default` },
          name: `default-${provider}-${currentMode}`,
          checked: isDefault,
          onClick: () => setModelDefault(provider, modelId),
        });

        const defaultLabel = el(
          "label",
          { className: `default-radio-wrap${isDefault ? " active" : ""}`, title: "boss 未指定模型时使用；再次点击可取消默认" },
          radioInput,
          textNode("默认")
        );

        // Tags
        const tags = [];
        if (isStale) {
          tags.push(el("span", { className: "badge badge-warning" }, "已不在列表"));
        }
        if (hasOverride) {
          tags.push(el("span", { className: "badge badge-accent" }, "本场景已覆盖"));
        }

        // Detail toggle button
        const detailBtn = el("button", {
          type: "button",
          className: `btn-detail${isExpanded ? " active" : ""}`,
          dataset: { focusKey: `${provider}/${modelId}/detail` },
          "aria-expanded": String(Boolean(isExpanded)),
          onClick: () => {
            expandedModel = isExpanded ? null : { provider, model: modelId };
            renderCards();
          },
        }, isExpanded ? "收起" : "配置");

        const controlsKids = [];
        if (modeRestricted) {
          const checkLabelAttrs = { className: "model-check-label" };
          if (isLastChecked) {
            checkLabelAttrs.title = "至少保留一个模型；要放开请打开“允许使用这个工具支持的任意模型”，要停用请关闭“在此场景启用”";
          } else if (isUnlimited) {
            checkLabelAttrs.title = "已允许任意模型。要逐个选择，请先关闭上方“允许使用这个工具支持的任意模型”";
          }
          controlsKids.push(
            el("label", checkLabelAttrs, checkInput, el("span", { className: "model-id" }, modelId))
          );
        } else {
          controlsKids.push(el("span", { className: "model-id" }, modelId));
        }
        controlsKids.push(defaultLabel, ...tags);

        if (isLastChecked) {
          controlsKids.push(
            el(
              "span",
              {
                className: "model-single-hint",
                title: "至少保留一个模型；要放开请打开“允许使用这个工具支持的任意模型”，要停用请关闭“在此场景启用”",
              },
              "至少保留一个模型；要放开请打开“允许使用这个工具支持的任意模型”，要停用请关闭“在此场景启用”"
            )
          );
        }

        const row = el(
          "div",
          { className: "model-row" },
          el("div", { className: "model-controls" }, ...controlsKids),
          el("div", { className: "model-actions" }, detailBtn)
        );
        modelItem.appendChild(row);

        if (isExpanded) {
          modelItem.appendChild(renderModelDetail(provider, modelId));
        }

        modelList.appendChild(modelItem);
      }

      body.appendChild(modelList);

      // Add model input
      const addInput = el("input", {
        type: "text",
        className: "add-model-input",
        dataset: { focusKey: `${provider}/add-input` },
        "aria-label": `${PROVIDER_NAMES[provider]} 的模型 ID`,
        placeholder: detectedModels.length
          ? `手动添加模型 ID，如 ${info.primary || detectedModels[0].id}…`
          : "手动添加模型 ID…",
      });
      const submitModel = () => {
        if (!addInput.value.trim()) return;
        addCustomModel(provider, addInput.value);
      };
      const addBtn = el("button", {
        type: "button",
        className: "add-model-btn",
        dataset: { focusKey: `${provider}/add-button` },
        onClick: submitModel,
      }, "添加模型");
      addInput.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          submitModel();
        }
      });

      const addRow = el("div", { className: "add-model-row" }, addInput, addBtn);
      body.appendChild(addRow);
    }

    card.appendChild(body);
    cards.appendChild(card);
  }

  container.replaceChildren(cards);
  // Field headings give controls stable identities even when inheritance controls appear.
  for (const item of container.querySelectorAll(".model-item")) {
    const prefix = `${item.closest(".cli-card").dataset.provider}/${item.dataset.model}`;
    for (const field of item.querySelectorAll(".detail-field, .detail-override-toggle")) {
      const name = field.querySelector("h4")?.textContent || "override";
      field.querySelectorAll("input, select, textarea, button").forEach((control, index) => {
        control.dataset.focusKey = `${prefix}/${name}/${index}`;
      });
    }
  }
  updateControls();
  if (focusKey) {
    const replacement = [...container.querySelectorAll("[data-focus-key]")]
      .find((control) => control.dataset.focusKey === focusKey)
      || (focusKey.endsWith("-disable") ? [...container.querySelectorAll("[data-focus-key]")]
        .find((control) => control.dataset.focusKey === `${focusKey.split("/")[0]}/enabled`) : null);
    if (replacement && !replacement.disabled) {
      replacement.focus({ preventScroll: true });
      // A confirmation button returns focus to the switch, not its screen position.
      if (replacement.dataset.focusKey === focusKey) {
        window.scrollBy(0, replacement.getBoundingClientRect().top - anchorTop);
      }
    }
  }
}

// Save members.json with race version checking
async function saveMembers() {
  if (busy || !dirty || documentError || hasEmptyRestrictedMode()) return;
  const errors = configurationErrors();
  if (errors.length) {
    showNotice(errors[0].message, true);
    updateControls();
    return;
  }
  busy = true;
  updateControls();
  const feedback = $("save-feedback");
  feedback.textContent = "正在保存…";
  feedback.className = "save-feedback";

  const saving = JSON.stringify(currentDoc);

  try {
    await apiPostMembers(JSON.parse(saving));
    savedSnapshot = saving;
    refreshDirty();
    feedback.textContent = "设置已保存 · 参数是否有效以 boss 读取时的校验为准";
    feedback.className = "save-feedback success";
    showNotice("设置已保存");
  } catch (err) {
    feedback.textContent = `保存失败: ${err.message}`;
    feedback.className = "save-feedback error";
    showNotice(err.message, true);
  } finally {
    busy = false;
    updateControls();
  }
}

// Render migration banner & skipped reasons
function renderMigrationBanner(data) {
  const banner = $("migration-banner");
  const title = $("migration-title");
  const container = $("migration-skipped-container");
  if (!banner || !container) return;

  container.replaceChildren();

  const hasSkipped = Array.isArray(data.migration_skipped) && data.migration_skipped.length > 0;

  if (data.migrated) {
    banner.hidden = false;
    banner.className = "banner migration";
    if (title) {
      title.textContent = "已将原编排档案的模型与说明迁移；两种场景当前未限制，如需限制再开启";
    }
  } else if (hasSkipped) {
    banner.hidden = false;
    banner.className = "banner error";
    if (title) {
      title.textContent = "旧档案未迁移，保存后将不再自动迁移";
    }
  } else {
    banner.hidden = true;
    return;
  }

  if (hasSkipped) {
    const details = el(
      "details",
      { className: "migration-skipped-details", open: !data.migrated },
      el("summary", {}, `未能迁移的档案（${data.migration_skipped.length} 条）`),
      el(
        "ul",
        { className: "migration-skipped-list" },
        ...data.migration_skipped.map((reason) => el("li", {}, reason))
      )
    );
    container.appendChild(details);
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

// Refresh detection
async function refreshDetection() {
  if (busy) return;
  busy = true;
  updateControls();
  showNotice("正在重新检测 CLI 与模型…");
  try {
    const data = await apiGetMembers(true);
    detected = data.detected || {};
    // If no unsaved modifications, reload entire data including doc, error & migration
    if (!dirty) {
      currentDoc = data.document || { version: 1, clis: {}, modes: {} };
      markSaved();
      documentError = data.error || null;
      lastMigrationData = data;
      renderMigrationBanner(data);
      renderErrorBanner();
      renderModeToolbar();
    }
    showNotice("检测结果已刷新");
    renderCards();
  } catch (err) {
    showNotice(`刷新检测失败: ${err.message}`, true);
  } finally {
    busy = false;
    updateControls();
  }
}

// Load initial members data
async function loadMembers() {
  if (currentTab === "members") showNotice("正在读取成员与模型配置…");
  try {
    const data = await apiGetMembers(false);
    detected = data.detected || {};
    currentDoc = data.document || { version: 1, clis: {}, modes: {} };
    markSaved();
    documentError = data.error || null;
    lastMigrationData = data;

    renderMigrationBanner(data);
    renderErrorBanner();
    renderModeToolbar();

    if (documentError) {
      showNotice(`配置文件损坏: ${documentError}。当前只读，禁止保存。`, true);
    } else if (currentTab === "members") {
      showNotice("已载入执行者与模型配置");
    }

    renderCards();
  } catch (err) {
    documentError = err.message;
    renderErrorBanner();
    showNotice(`读取失败: ${err.message}`, true);
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
  if (!iso) return "尚未检查";
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString("zh-CN", {
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
  const installWarning = updateInfo?.state === "installing" && Boolean(updateInfo.error);

  if (updateInfo?.state === "available") {
    appBadge.hidden = false;
    appBadge.className = "tag available";
    appBadge.textContent = `发现新版 v${updateInfo.version}`;
    $("update-badge").hidden = false;
  } else if (updateInfo?.state === "installing") {
    appBadge.hidden = false;
    appBadge.className = "tag installing";
    appBadge.textContent = "正在下载更新…";
    $("update-badge").hidden = false;
  } else if (updateInfo?.state === "failed") {
    appBadge.hidden = false;
    appBadge.className = "tag failed";
    appBadge.textContent = "更新失败";
  } else if (updateInfo?.state === "none") {
    appBadge.hidden = false;
    appBadge.className = "tag done";
    appBadge.textContent = "已是最新版本";
    $("update-badge").hidden = true;
  } else {
    appBadge.hidden = true;
    $("update-badge").hidden = true;
  }

  // Service status
  const serviceInfo = appStatus.service;
  const serviceBadge = $("service-badge");
  const serviceMsg = $("service-msg");

  if (!serviceInfo) {
    $("service-version").textContent = installedService?.version ? `v${installedService.version}` : "—";
    serviceBadge.hidden = true;
    serviceMsg.hidden = false;
    serviceMsg.textContent = installedService?.installed
      ? "本机服务已安装。各应用的实际连接状态，请查看下方宿主卡片。"
      : "请在下方接入区安装本机服务。";
  } else {
    $("service-version").textContent = serviceInfo.version ? `v${serviceInfo.version}` : "—";
    const isServiceOutdated =
      serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version));

    if (["upgrading", "installing"].includes(serviceInfo.state)) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag upgrading";
      serviceBadge.textContent = serviceInfo.state === "installing" ? "正在安装…" : "正在升级…";
      serviceMsg.hidden = false;
      serviceMsg.textContent = "正在从 GitHub 安装或升级本机服务，约需一分钟…";
    } else if (serviceInfo.state === "failed") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag failed";
      serviceBadge.textContent = "升级失败";
      serviceMsg.hidden = false;
      serviceMsg.textContent = serviceInfo.error || "升级服务失败，请重试";
    } else if (isServiceOutdated) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag outdated";
      serviceBadge.textContent = "比托盘旧（建议升级）";
      serviceMsg.hidden = false;
      serviceMsg.textContent = installWarning
        ? "重启后请手动升级本机服务"
        : "升级后将自动检测各宿主连接，仅在接口未同步或配置有变化时提示重新加载。";
    } else if (serviceInfo.state === "done") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag done";
      serviceBadge.textContent = "安装完成";
      serviceMsg.hidden = false;
      serviceMsg.textContent = "安装完成。下方会自动检测各宿主连接，无需仅因版本变化逐个重启。";
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

  btnCheck.disabled = isUpdating;
  btnApply.hidden = !hasUpdate;
  btnApply.disabled = isUpdating;
  updateHint.hidden = !hasUpdate;

  if (hasUpdate) {
    if (installWarning) {
      updateHint.textContent = "重启后请手动升级本机服务";
    } else if (hasAppUpdate) {
      updateHint.textContent = "会先更新 App 并自动重启，再自动升级本机服务；各宿主是否需要重新加载，由下方连接检测结果决定。";
    } else {
      updateHint.textContent = "将升级本机服务，空闲后自动切换；若工具接口仍未同步，下方会提示重新连接。";
    }
  }

  if (isUpdating) {
    progressBox.hidden = false;
    if (updateInfo?.state === "checking") {
      progressText.textContent = "正在检查更新…";
    } else if (updateInfo?.state === "installing") {
      progressText.textContent = "正在下载 App 更新并准备重启…";
    } else if (["upgrading", "installing"].includes(serviceInfo?.state)) {
      progressText.textContent = "正在安装或升级本机服务，约需一分钟…";
    }
  } else {
    progressBox.hidden = true;
  }

  const hasError = updateInfo?.state === "failed" || serviceInfo?.state === "failed" || Boolean(updateInfo?.error);
  errorBox.classList.toggle("warning", installWarning);
  $("btn-retry-update").hidden = isUpdating;
  if (hasError && (!isUpdating || installWarning)) {
    errorBox.hidden = false;
    errorText.textContent = updateInfo?.error || serviceInfo?.error || "操作失败，请重试。";
  } else {
    errorBox.hidden = true;
  }

  // Manage polling
  if (isUpdating && !statusPollTimer) {
    statusPollTimer = setInterval(loadStatus, 2000);
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

async function loadStatus() {
  try {
    appStatus = await apiGetStatus();
    renderUpdates();
    window.dispatchEvent(new CustomEvent("agentnave:status", { detail: appStatus }));
  } catch (err) {
    showNotice(`获取版本状态失败: ${err.message}`, true);
  }
}

async function triggerCheckUpdate() {
  const btnCheck = $("btn-check-update");
  btnCheck.disabled = true;
  try {
    showNotice("正在检查更新…");
    appStatus = await apiCheckUpdate();
    const state = appStatus?.update?.state;
    showNotice(
      state === "failed" ? "检查更新失败" : state === "available" ? `发现新版 v${appStatus.update.version}` : state === "none" ? "已是最新版本" : "检查更新完成",
      state === "failed",
    );
  } catch (err) {
    showNotice(`检查更新失败: ${err.message}`, true);
  } finally {
    renderUpdates();
  }
}

async function triggerApplyUpdate() {
  const btnApply = $("btn-apply-update");
  btnApply.disabled = true;
  try {
    showNotice("已触发更新操作…");
    appStatus = await apiApplyUpdate();
    renderUpdates();
  } catch (err) {
    showNotice(`触发更新失败: ${err.message}`, true);
    renderUpdates();
  }
}

// Bind top-level event listeners
for (const event of ["click", "change"]) $("cli-cards").addEventListener(event, (e) => {
  operatedControl = e.target.closest("[data-focus-key]");
  queueMicrotask(() => { operatedControl = null; });
}, true);
for (const tab of ["members", "avatars", "hosts"]) $("tab-" + tab).addEventListener("click", () => { setTab(tab); history.replaceState(null, "", "#" + tab); });
const tabFromHash = () => ["hosts", "host-cards", "updates"].includes(location.hash.slice(1)) ? "hosts" : location.hash === "#avatars" ? "avatars" : "members";
window.addEventListener("hashchange", () => setTab(tabFromHash()));
setTab(tabFromHash());

for (const mode of ["task", "discussion"]) $("mode-" + mode).addEventListener("click", (e) => {
  e.currentTarget.focus({ preventScroll: true });
  setMode(mode);
});
$("mode-restrict-toggle").addEventListener("change", (e) => {
  e.target.focus({ preventScroll: true });
  toggleModeRestricted(e.target.checked);
});
$("refresh-detection").addEventListener("click", refreshDetection);
$("save-btn").addEventListener("click", saveMembers);

$("btn-check-update").addEventListener("click", triggerCheckUpdate);
$("btn-apply-update").addEventListener("click", triggerApplyUpdate);
$("btn-retry-update").addEventListener("click", () => {
  if (appStatus?.service?.state === "failed") {
    triggerApplyUpdate();
  } else {
    triggerCheckUpdate();
  }
});

$("dismiss-migration").addEventListener("click", () => {
  $("migration-banner").hidden = true;
});

// The app's web view does not show beforeunload prompts, so hold back in-app navigation.
function renderLeaveConfirm() {
  const box = $("leave-confirm");
  box.hidden = !leaveTarget;
  if (!leaveTarget) return box.replaceChildren();
  if (box.childElementCount) return;
  const stay = el("button", { type: "button", onClick: () => { leaveTarget = null; renderLeaveConfirm(); } }, "继续编辑");
  box.replaceChildren(
    el("p", {}, "有未保存的修改，离开设置页会丢失这些修改。"),
    el("div", { className: "btn-group" }, stay,
      el("button", { type: "button", className: "danger", onClick: () => {
        const target = leaveTarget;
        dirty = false;
        location.href = target;
      } }, "放弃修改并离开")));
  stay.focus({ preventScroll: true });
}
document.querySelector("header.bar").addEventListener("click", (e) => {
  const link = e.target.closest("a[href]");
  if (!link || !(dirty || busy) || link.getAttribute("href") === "/settings/") return;
  e.preventDefault();
  leaveTarget = link.href;
  if (currentTab !== "members") {
    setTab("members");
    history.replaceState(null, "", "#members");
  }
  renderLeaveConfirm();
}, true);

window.addEventListener("beforeunload", (e) => {
  if (dirty || busy) {
    e.preventDefault();
    e.returnValue = "";
  }
});

// Initialize
loadMembers();
loadStatus();
