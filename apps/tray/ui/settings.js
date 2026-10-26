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
  config: "dsh 配置",
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
let currentTab = "members"; // "members" | "skills" | "avatars" | "orchestration" | "hosts"
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
let reviewSaves = 0; // Seats saved so far; a probe answered before a save carries old seats.
let docVersion = 0; // Monotonic counter for save race detection
const openCards = new Set(); // CLI cards are folded to one summary line until opened
let missingOpen = false; // The folded list of CLIs not installed here
let appStatus = null;
let installedService = null;
let statusPollTimer = null;
// Keep unfinished option edits across drawer/mode changes without writing invalid values.
const optionDrafts = new WeakMap();
let operatedControl = null;
let pendingDisable = null;
let pendingRestrictOff = false; // Turning a mode's selection off waits for confirmation.

// Determine mock mode
const isMock = new URLSearchParams(location.search).has("mock");

// Mock data covering:
// 1. Task mode restricted, discussion mode unrestricted (omitted key).
// 2. Codex in task mode has models: [] (unlimited models) with default set.
// 3. Override tri-state example.
const mockMembersData = {
  review: { version: 1, seats: {} },
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
  detected: {
    codex: {
      installed: true,
      path: "/opt/homebrew/bin/codex",
      version: "0.161.0",
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
      version: "2.1.308",
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

async function apiPostReview(doc) {
  if (isMock) {
    mockMembersData.review = JSON.parse(JSON.stringify(doc));
    return;
  }
  const res = await fetch("/app/review", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
    body: JSON.stringify(doc),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.error) throw new Error(data.error || `保存失败（${res.status}）`);
}

function takeReview(data) {
  reviewSeats = data.review?.seats || {};
  reviewError = data.review_error || null;
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

async function apiCheckUpdate(quiet = false) {
  if (isMock) {
    return window.AgentNaveMock.post("/app/update/check");
  }
  // quiet: a failed check keeps the previous state instead of reporting a failure.
  const res = await fetch(quiet ? "/app/update/check?quiet=1" : "/app/update/check", {
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
  $("refresh-detection").disabled = busy || probing;
  $("refresh-detection").textContent = probing ? "正在检测…" : "刷新模型列表";

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
  for (const id of ["members", "skills", "avatars", "orchestration", "hosts"]) {
    if (tab === id) $("nav-" + id).setAttribute("aria-current", "page");
    else $("nav-" + id).removeAttribute("aria-current");
    $("pane-" + id).hidden = tab !== id;
  }
  if (tab === "hosts") loadStatus().then(checkIfStale);
  if (tab === "skills") window.AgentNaveSkills?.load();
  if (tab === "orchestration") {
    loadRuleFiles();
    loadLimit();
  }
}

// The running limit lives in settings.json, read by the service before each start.
let limitDoc = null;
async function loadLimit() {
  if (!$("max-running").options.length) {
    for (let n = 1; n <= 32; n += 1) $("max-running").append(el("option", { value: String(n) }, String(n)));
    $("max-running").value = "8";
  }
  try {
    const response = await fetch("/app/settings", { cache: "no-store" });
    if (!response.ok) throw new Error(`读取失败（${response.status}）`);
    const data = await response.json();
    limitDoc = data.settings;
    $("max-running").value = String(data.settings.max_running);
    if (data.error) showNotice(`${data.error}；保存会用当前值覆盖`, true);
  } catch (error) {
    showNotice(`无法读取同时运行上限：${error.message}`, true);
  }
}

async function saveLimit() {
  const value = Number($("max-running").value);
  if (!Number.isInteger(value) || value < 1 || value > 32) {
    showNotice("同时运行上限必须为 1–32 的整数", true);
    return;
  }
  const button = $("save-limit");
  button.disabled = true;
  try {
    const response = await fetch("/app/settings", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ ...(limitDoc || { version: 1 }), version: 1, max_running: value }),
    });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `保存失败（${response.status}）`);
    limitDoc = { version: 1, max_running: value };
    showNotice(`已保存：同时最多运行 ${value} 个`);
  } catch (error) {
    showNotice(`未能保存：${error.message}`, true);
  } finally {
    button.disabled = false;
  }
}

// Orchestration preferences live in each host's own global rules; the App only opens them.
async function loadRuleFiles() {
  const list = $("rule-files");
  try {
    const response = await fetch("/app/rules", { cache: "no-store" });
    if (!response.ok) throw new Error(`读取失败（${response.status}）`);
    const data = await response.json();
    list.replaceChildren(...data.files.map((file) => el("li", {},
      el("span", { className: "rule-label" }, file.label),
      el("code", {}, file.path),
      file.exists
        ? el("button", { type: "button", onClick: () => openRuleFile(file) }, "打开")
        : el("span", { className: "rule-missing" }, "未找到"))));
  } catch (error) {
    list.replaceChildren(el("li", {}, `无法读取规则文件位置：${error.message}`));
  }
}

async function openRuleFile(file) {
  try {
    const response = await fetch("/app/rules/open", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ id: file.id }),
    });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `打开失败（${response.status}）`);
    showNotice(data.message);
  } catch (error) {
    showNotice(error.message, true);
  }
}

async function copyOrchestration() {
  const text = $("orchestration-template").textContent;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Some webviews deny the async clipboard; fall back to copying the selected text.
    const range = document.createRange();
    range.selectNodeContents($("orchestration-template"));
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    const copied = document.execCommand("copy");
    selection.removeAllRanges();
    if (!copied) { showNotice("无法写入剪贴板，请手动选择文字复制", true); return; }
  }
  showNotice("已复制推荐写法");
  // The page notice sits at the top, out of view below the template; confirm on the button too.
  const button = $("copy-orchestration");
  button.textContent = "已复制";
  setTimeout(() => { button.textContent = "复制推荐写法"; }, 2000);
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
      ? "已开启：只允许使用下方启用的工具与模型，并可为本场景设置默认模型。关闭会保留已设置的默认模型，清除本场景的名单和专用设置。"
      : "已关闭：不额外限制可用工具和模型，仍可为本场景设置各 CLI 的默认模型。开启后可逐项选择可用名单。";
  }
  const confirm = $("restrict-confirm");
  if (confirm) {
    confirm.hidden = !pendingRestrictOff;
    const scene = currentMode === "task" ? "私聊" : "会议";
    confirm.replaceChildren(...(pendingRestrictOff ? [
      el("p", {}, `关闭后${scene}不再限制工具和模型：保留已设置的默认模型和共享设置，清除可用名单和${scene}专用设置。重新开启不会恢复名单和专用设置，默认模型沿用保留值，没设置的继续跟随 CLI 默认；点击“保存设置”后才生效。`),
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
        currentDoc.modes[currentMode][p] = conf;
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
      currentDoc.modes[currentMode][provider] = { models: [], overrides: {} };
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
// What a call that names no model or effort runs, and whose choice each part is: the default
// set here wins, else the CLI's own default, else the CLI decides without telling us.
function fallback(provider) {
  const native = nativeDefault(provider);
  const chosen = defaultModel(provider);
  const model = chosen || native.model;
  const shared = currentDoc.clis?.[provider]?.models?.[model] || {};
  const override = currentDoc.modes?.[currentMode]?.[provider]?.overrides?.[model];
  const effort = override && Object.hasOwn(override, "effort") ? override.effort : shared.effort;
  return {
    model,
    modelBy: chosen ? "app" : native.model ? "cli" : null,
    effort: effort || native.effort,
    effortBy: effort ? "app" : native.effort ? "cli" : null,
  };
}
const CHOICE_BY = { app: "你设的默认", cli: "CLI 默认" };
function needsExplicitModel(provider) {
  return isModeRestricted() && currentDoc.modes[currentMode]?.[provider]?.models?.length > 0 && !defaultModel(provider);
}
function fallbackText(provider) {
  if (needsExplicitModel(provider)) return "boss 必须指定模型，或在这里设为默认";
  const { model, modelBy, effort, effortBy } = fallback(provider);
  const modelPart = model ? `${model}（${CHOICE_BY[modelBy]}）` : "CLI 自己选的模型";
  const effortPart = effort ? `档位 ${effort}（${CHOICE_BY[effortBy]}）` : "档位由 CLI 决定";
  return `boss 不指定时用 ${modelPart} · ${effortPart}`;
}
const FALLBACK_HINT = "boss 指定的模型和档位优先；没指定时用你在这里设的默认；没设且允许任意模型时，才用 CLI 自己的默认（标着“CLI 默认”）。限制了模型名单却没设默认时，boss 必须指定模型。读不到 CLI 默认时，由 CLI 自己决定。";

const EFFORT_ORDER = ["off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

// A model's effort levels from low to high: its own detected levels, else its CLI's, plus any
// configured value a probe no longer lists.
function effortLevels(provider, modelId) {
  const own = (detected[provider]?.models || []).find((m) => m.id === modelId)?.efforts;
  const detectedEfforts = Array.isArray(own) && own.length ? own : detected[provider]?.efforts || [];
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

function middleEffort(range) {
  return range.length ? range[Math.floor((range.length - 1) / 2)] : undefined;
}

// The allowed efforts as a range bar after a model's default switch: two ends bound the
// allowed levels (all of them when nothing is chosen), the round knob is the default effort.
// Without a default the knob sits hollow on the middle level until the bar is touched.
function renderEffortBar(provider, modelId) {
  const levels = effortLevels(provider, modelId);
  if (!levels.length) return null;
  const overridden = isModelOverridden(provider, modelId);
  const override = overridden ? currentDoc.modes[currentMode][provider].overrides[modelId] : null;
  const shared = currentDoc.clis?.[provider]?.models?.[modelId] || {};
  const pick = (key) => (override && Object.hasOwn(override, key) ? override[key] : shared[key]);
  const efforts = Array.isArray(pick("efforts")) ? pick("efforts") : [];
  const allowed = efforts.length ? levels.filter((level) => efforts.includes(level)) : levels;
  const last = levels.length - 1;
  const initial = {
    lo: allowed.length ? levels.indexOf(allowed[0]) : 0,
    hi: allowed.length ? levels.indexOf(allowed[allowed.length - 1]) : last,
    def: levels.includes(pick("effort")) ? levels.indexOf(pick("effort")) : null,
  };
  const state = { ...initial };
  const position = (index) => `${last ? (index / last) * 100 : 50}%`;
  // Levels the default may take: the allowed ones, gaps included, until an end is moved.
  const choices = () => {
    const moved = state.lo !== initial.lo || state.hi !== initial.hi;
    return levels.map((_, index) => index)
      .filter((index) => index >= state.lo && index <= state.hi && (moved || allowed.includes(levels[index])));
  };
  const nearest = (index) => choices().reduce((best, choice) =>
    (Math.abs(choice - index) < Math.abs(best - index) ? choice : best));
  // Without a default the hollow knob shows the CLI's own effort, else the middle level.
  const nativeEffort = nativeDefault(provider).effort;
  const preset = () => state.def ?? (choices().includes(levels.indexOf(nativeEffort))
    ? levels.indexOf(nativeEffort)
    : middleEffort(choices()));

  const fill = el("span", { className: "effort-fill" });
  const ticks = levels.map((level) => el("span", { className: "effort-tick", title: level }));
  const handle = (kind, label) => el("button", {
    type: "button",
    role: "slider",
    className: `effort-handle effort-${kind}`,
    dataset: { focusKey: `${provider}/${modelId}/effort-${kind}`, handle: kind },
    "aria-label": `${modelId} ${label}`,
    "aria-valuemin": "0",
    "aria-valuemax": String(last),
  });
  const handles = { lo: handle("lo", "最低允许档位"), hi: handle("hi", "最高允许档位"), def: handle("def", "默认思考档位") };
  const text = el("span", { className: "effort-text" });
  const track = el("div", { className: "effort-track" }, fill, ...ticks, handles.lo, handles.hi, handles.def);
  // Remove the field that actually supplies the effort. A mode override can fall back
  // to a shared default; clearing a shared default also affects other inheriting modes.
  const effortOverride = override && Object.hasOwn(override, "effort");
  const sharedFallback = effortOverride && Object.hasOwn(shared, "effort");
  const resetAllowed = !sharedFallback || !efforts.length || efforts.includes(shared.effort);
  const reset = initial.def === null ? null : el("button", {
    type: "button",
    className: "effort-reset",
    dataset: { focusKey: `${provider}/${modelId}/effort-reset` },
    disabled: !resetAllowed,
    title: sharedFallback
      ? resetAllowed ? `取消本场景的默认档位，改用共享档位 ${shared.effort}` : `共享档位 ${shared.effort} 不在当前允许范围内，请先调整范围`
      : effortOverride ? "清除本场景的默认档位，改用 CLI 默认" : "清除共享默认档位；所有继承共享设置的场景都会改用 CLI 默认",
    onClick: () => {
      delete (effortOverride ? override : currentDoc.clis[provider].models[modelId]).effort;
      incDocVersion();
      renderCards();
    },
  }, sharedFallback ? `改用共享档位 ${shared.effort}` : effortOverride ? "改回 CLI 默认" : "改回 CLI 默认（共享设置）");
  const bar = el("div", { className: "effort-bar" }, track, text, ...(reset ? [reset] : []));

  const paint = () => {
    const def = preset();
    fill.style.left = position(state.lo);
    fill.style.width = `calc(${position(state.hi)} - ${position(state.lo)})`;
    // A saved list with gaps keeps them until the range is moved.
    const moved = state.lo !== initial.lo || state.hi !== initial.hi;
    levels.forEach((level, index) => {
      ticks[index].style.left = position(index);
      ticks[index].classList.toggle("on", moved ? index >= state.lo && index <= state.hi : allowed.includes(level));
    });
    for (const [kind, index] of [["lo", state.lo], ["hi", state.hi], ["def", def]]) {
      handles[kind].style.left = position(index);
      handles[kind].setAttribute("aria-valuenow", String(index));
      handles[kind].setAttribute("aria-valuetext", levels[index]);
    }
    handles.def.classList.toggle("preset", state.def === null);
    const range = state.lo === 0 && state.hi === last
      ? "全部档位"
      : state.lo === state.hi ? levels[state.lo] : `${levels[state.lo]}–${levels[state.hi]}`;
    text.textContent = `${range} · ${state.def === null ? (nativeEffort ? `CLI 默认 ${nativeEffort}` : "档位由 CLI 决定") : `默认 ${levels[def]}`}`;
    bar.title = state.def === null
      ? `你没有设默认档位：boss 不指定时${nativeEffort ? `用 CLI 默认 ${nativeEffort}` : "由 CLI 自己决定"}。点选或拖动圆点即设为你的默认。两端是允许 boss 使用的档位范围。`
      : "两端是允许 boss 使用的档位范围，实心圆点是你设的默认档位：boss 不指定时用它。";
  };
  const move = (kind, index) => {
    index = Math.max(0, Math.min(last, index));
    if (kind === "lo") state.lo = Math.min(index, state.hi);
    if (kind === "hi") state.hi = Math.max(index, state.lo);
    if (kind === "def") state.def = nearest(index);
    // Moving an end back to where it was restores the gaps and the saved default.
    else if (state.def !== null) {
      const back = state.lo === initial.lo && state.hi === initial.hi && initial.def !== null;
      state.def = back ? initial.def : nearest(state.def);
    }
    paint();
  };
  // Touching the bar writes the shown default too, so a preset becomes the saved default.
  const commit = () => {
    ensureCliModel(provider, modelId);
    const target = override || currentDoc.clis[provider].models[modelId];
    if (state.lo !== initial.lo || state.hi !== initial.hi) {
      target.efforts = state.lo === 0 && state.hi === last ? [] : levels.slice(state.lo, state.hi + 1);
    }
    target.effort = levels[preset()];
    incDocVersion();
    renderCards();
  };
  const indexAt = (x) => {
    const box = track.getBoundingClientRect();
    return last ? Math.round(((x - box.left) / box.width) * last) : 0;
  };
  track.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const index = indexAt(e.clientX);
    const kind = e.target.dataset?.handle || (index < state.lo ? "lo" : index > state.hi ? "hi" : "def");
    handles[kind].focus({ preventScroll: true });
    track.setPointerCapture(e.pointerId);
    move(kind, index);
    const drag = (event) => move(kind, indexAt(event.clientX));
    const drop = () => {
      track.removeEventListener("pointermove", drag);
      track.removeEventListener("pointerup", drop);
      track.removeEventListener("pointercancel", drop);
      commit();
    };
    track.addEventListener("pointermove", drag);
    track.addEventListener("pointerup", drop);
    track.addEventListener("pointercancel", drop);
  });
  for (const [kind, button] of Object.entries(handles)) {
    button.addEventListener("keydown", (e) => {
      const current = kind === "def" ? preset() : state[kind];
      const steps = kind === "def" ? choices() : levels.map((_, index) => index);
      const lower = steps.filter((index) => index < current).pop() ?? current;
      const higher = steps.find((index) => index > current) ?? current;
      const next = { ArrowLeft: lower, ArrowDown: lower, ArrowRight: higher, ArrowUp: higher, Home: steps[0], End: steps[steps.length - 1] }[e.key];
      if (next === undefined) return;
      e.preventDefault();
      move(kind, next);
      commit();
    });
  }
  paint();
  return bar;
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




// Render 6 CLI cards
// One line that says what a folded card allows, so the list can stay closed.
function cardSummary(provider, isEnabled, modeRestricted, info) {
  if (!modeRestricted) {
    const detectedCount = info.models?.length ? `检测到 ${info.models.length} 个模型` : "";
    const kept = defaultModel(provider);
    return [detectedCount, fallbackText(provider)].filter(Boolean).join(" · ");
  }
  if (!isEnabled) return "当前场景未启用";
  const conf = currentDoc.modes[currentMode]?.[provider] || {};
  const models = Array.isArray(conf.models) ? conf.models : [];
  const scope = models.length === 0 ? "不限模型" : `${models.length} 个模型`;
  return `${scope} · ${fallbackText(provider)}`;
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
  // Not installed and not on this scene's list: only named in the folded list at the end.
  const missing = [];

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
    if (!info.installed && !(modeRestricted && isEnabled)) {
      missing.push(provider);
      continue;
    }
    const card = el("div", {
      className: `cli-card${info.installed ? "" : " uninstalled"}`,
      dataset: { provider },
    });

    // Header
    // An installed CLI shows its version beside the name instead of a badge.
    const statusBadge = !info.installed
      ? el("span", { className: "badge badge-bad" }, "未安装")
      : info.version
        ? null
        : el("span", { className: "badge badge-run" }, "已安装");

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
      el("span", { className: "cli-icon", "aria-hidden": "true" }, AN.hostTile(provider, PROVIDER_NAMES[provider])),
      el("span", { className: "cli-name" }, PROVIDER_NAMES[provider] || provider),
      info.installed && info.version ? el("span", { className: "cli-version" }, info.version) : null,
      el("span", { className: "cli-summary", title: FALLBACK_HINT }, cardSummary(provider, isEnabled, modeRestricted, info))
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
          { className: `default-radio-wrap${isDefault ? " active" : ""}`, title: "设为你的默认：boss 不指定模型时用它，优先于 CLI 默认。再次点击取消；限制了模型名单时，取消后 boss 必须指定模型，否则跟随 CLI 默认" },
          radioInput,
          textNode("设为默认")
        );

        // Tags
        const tags = [];
        if (isStale) {
          tags.push(el("span", { className: "badge badge-warning" }, "已不在列表"));
        }
        if (nativeDefault(provider).model === modelId) {
          tags.push(el("span", { className: "badge", title: defaultModel(provider)
            ? `CLI 自己的默认模型。你已把 ${defaultModel(provider)} 设为默认，所以 boss 不指定时不会用它`
            : needsExplicitModel(provider) ? "CLI 自己的默认模型；当前限制了模型名单且没设默认，boss 仍必须指定模型"
            : "CLI 自己的默认模型：你没有设默认，boss 不指定时就用它，这一行的档位设置也会生效" }, "CLI 默认"));
        }


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
        const effortBar = renderEffortBar(provider, modelId);
        controlsKids.push(defaultLabel, ...(effortBar ? [effortBar] : []), ...tags);

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
          el("div", { className: "model-controls" }, ...controlsKids)
        );
        modelItem.appendChild(row);

        modelList.appendChild(modelItem);
      }

      body.appendChild(modelList);

    }

    card.appendChild(body);
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
      }, el("span", { className: "cli-chevron", "aria-hidden": "true" }), textNode(`未安装的 CLI（${missing.length}）`)));
    if (missingOpen) box.appendChild(el("p", { className: "cli-missing-list" },
      textNode(`${names.join("、")}。装好后点「刷新模型列表」，就会出现在上面，可以开始使用。`)));
    cards.appendChild(box);
  }

  container.replaceChildren(cards);
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
  renderReviewSeats();
}

// Save members.json with race version checking
// agentnave-review roles, in the order the Skill grades them; saved in members.json `review`.
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

// Seats save on their own to review.json, so unsaved 执行者 edits stay unsaved. A picked model
// missing from that CLI's configured models is also added to the last saved members.json,
// where the server looks for it.
async function setReviewSeat(role, seat) {
  if (reviewError) return showNotice(`${reviewError}；请先修复或删除这个文件`, true);
  // One write at a time: a write built on a stale saved document would undo the other one.
  if (busy) {
    showNotice("正在保存，请稍后再选", true);
    return renderReviewSeats();
  }
  const seats = { ...reviewSeats };
  if (seat) seats[role] = seat;
  else delete seats[role];
  const add = (doc) => {
    doc.clis ||= {};
    doc.clis[seat.provider] ||= { models: {} };
    doc.clis[seat.provider].models ||= {};
    doc.clis[seat.provider].models[seat.model] ||= {};
    return doc;
  };
  const saved = JSON.parse(savedSnapshot || JSON.stringify({ version: 1, clis: {}, modes: {} }));
  const missing = seat && !Object.hasOwn(saved.clis?.[seat.provider]?.models || {}, seat.model);
  busy = true;
  updateControls();
  try {
    if (missing) {
      if (documentError) throw new Error("配置文件有错误，请先在“执行者”里修复");
      await apiPostMembers(add(saved));
      savedSnapshot = JSON.stringify(saved);
      add(currentDoc);
      refreshDirty();
    }
    await apiPostReview({ version: 1, seats });
    reviewSeats = seats;
    reviewSaves += 1;
    showNotice("评审工位已保存");
  } catch (err) {
    showNotice(`评审工位保存失败: ${err.message}`, true);
  } finally {
    busy = false;
    updateControls();
  }
  renderReviewSeats();
}

function reviewSelect(key, label, value, options, onPick, disabled = false) {
  return el("select", {
    dataset: { focusKey: key }, "aria-label": label, disabled,
    onChange: (e) => onPick(e.target.value),
  }, options.map(([v, text, off]) => el("option", { value: v, selected: v === value, disabled: Boolean(off) }, text)));
}

// One row per role: CLI, then one of that CLI's models, then an effort or the member's default.
function renderReviewSeats() {
  const box = $("review-seats");
  if (!box) return;
  const focusKey = document.activeElement?.dataset?.focusKey;
  const rows = REVIEW_ROLES.map(([role, name, hint]) => {
    const seat = reviewSeats[role];
    const provider = seat?.provider || "";
    const clis = PROVIDERS.filter((p) => detected[p]?.installed || currentDoc.clis?.[p] || p === provider)
      .map((p) => [p, PROVIDER_NAMES[p] + (getAllModelIds(p).length ? "" : "（没有可选模型）"), !getAllModelIds(p).length && p !== provider]);
    const models = provider ? getAllModelIds(provider) : [];
    if (seat?.model && !models.includes(seat.model)) models.unshift(seat.model);
    const levels = seat?.model ? effortLevels(provider, seat.model) : [];
    if (seat?.effort && !levels.includes(seat.effort)) levels.push(seat.effort);
    const pickCli = (next) => {
      if (!next) return setReviewSeat(role, null);
      const ids = getAllModelIds(next);
      const model = ids.includes(fallback(next).model) ? fallback(next).model : ids[0];
      setReviewSeat(role, { provider: next, model });
    };
    const pickModel = (model) => setReviewSeat(role, { provider, model });
    const pickEffort = (effort) => setReviewSeat(role, effort ? { ...seat, effort } : { provider, model: seat.model });
    // A restricted private-task list that leaves this seat out makes the server report it unavailable.
    const task = currentDoc.modes?.task;
    const listed = !seat || !task || (task[provider] && (!task[provider].models?.length || task[provider].models.includes(seat.model)));
    return el("div", { className: "review-row" },
      el("div", { className: "review-role" }, el("strong", {}, name), el("small", {}, hint),
        !listed && el("small", { className: "review-warn" }, "私聊名单没有选中这个模型，评审时会改找其他 CLI")),
      reviewSelect(`${role}/cli`, `${name}评审的 CLI`, provider, [["", "自动"], ...clis], pickCli),
      reviewSelect(`${role}/model`, `${name}评审的模型`, seat?.model || "",
        seat ? models.map((m) => [m, m]) : [["", "—"]], pickModel, !seat),
      reviewSelect(`${role}/effort`, `${name}评审的思考档位`, seat?.effort || "",
        seat ? [["", "组员默认档位"], ...levels.map((l) => [l, l])] : [["", "—"]], pickEffort, !seat || !levels.length),
    );
  });
  box.replaceChildren(...rows);
  if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
}

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
  if (!quiet) showNotice("正在重新检测 CLI 与模型…");
  try {
    const data = await apiGetMembers(true);
    const changed = JSON.stringify(data.detected || {}) !== JSON.stringify(detected);
    detected = data.detected || {};
    if (!busy && reviewSaves === seatsBefore) takeReview(data);
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
      renderModeToolbar();
    }
    if (!quiet) showNotice("检测结果已刷新");
    else if (changed && currentTab === "members") showNotice("检测到工具或模型列表有变化，已自动刷新");
    // A quiet probe that found nothing new leaves the cards, and any input in them, alone.
    if (!quiet || changed || reloaded) renderCards();
  } catch (err) {
    if (!quiet) showNotice(`刷新检测失败: ${err.message}`, true);
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
  if (currentTab === "members") showNotice("正在读取成员与模型配置…");
  try {
    const data = await apiGetMembers(false);
    detected = data.detected || {};
    currentDoc = data.document || { version: 1, clis: {}, modes: {} };
    markSaved();
    documentError = data.error || null;
    takeReview(data);
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
  } else if (updateInfo?.state === "installing") {
    appBadge.hidden = false;
    appBadge.className = "tag installing";
    appBadge.textContent = "正在下载更新…";
  } else if (updateInfo?.state === "failed") {
    appBadge.hidden = false;
    appBadge.className = "tag failed";
    appBadge.textContent = "更新失败";
  } else if (updateInfo?.state === "none") {
    appBadge.hidden = false;
    appBadge.className = "tag done";
    appBadge.textContent = "已是最新版本";
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
      serviceBadge.textContent = "比 App 旧，需要升级";
      serviceMsg.hidden = true;
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

  // One action at a time: with an update found, "全部更新" leads and checking again stays secondary.
  $("update-badge").hidden = !hasUpdate && updateInfo?.state !== "installing";
  btnCheck.disabled = isUpdating;
  btnCheck.textContent = hasUpdate ? "重新检查" : "立即检查";
  // While anything runs the progress line says what; a disabled action would only contradict it.
  btnApply.hidden = !hasUpdate || isUpdating;
  btnApply.textContent = hasAppUpdate ? `全部更新到 v${updateInfo.version}` : "升级本机服务";
  updateHint.hidden = !hasUpdate || (isUpdating && !installWarning);

  if (hasUpdate) {
    if (installWarning) {
      updateHint.textContent = "重启后请手动升级本机服务";
    } else if (hasAppUpdate) {
      updateHint.textContent = isServiceOutdated
        ? "先更新 App 并自动重启，重启后自动升级本机服务，不用分两次操作。"
        : "更新 App 后自动重启，本机服务随后自动跟上。";
    } else {
      updateHint.textContent = "本机服务比 App 旧。升级约需一分钟，空闲后自动切换，正在运行的任务不受影响。";
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
// A hidden window skips the update poll; catch up as soon as it is shown again.
document.addEventListener("visibilitychange", () => document.hidden || !statusPollTimer || loadStatus());

async function loadStatus() {
  try {
    appStatus = await apiGetStatus();
    renderUpdates();
    window.dispatchEvent(new CustomEvent("agentnave:status", { detail: appStatus }));
  } catch (err) {
    showNotice(`获取版本状态失败: ${err.message}`, true);
  }
}

// Opening this tab checks again when the automatic daily check is over an hour old. A found
// update or a failure the user has not retried is left as it is.
const AUTO_CHECK_AFTER = 60 * 60 * 1000;
function checkIfStale() {
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
    if (!quiet) showNotice("正在检查更新…");
    appStatus = await apiCheckUpdate(quiet);
    const state = appStatus?.update?.state;
    if (!quiet || state === "available") showNotice(
      state === "failed" ? "检查更新失败" : state === "available" ? `发现新版 v${appStatus.update.version}` : state === "none" ? "已是最新版本" : "检查更新完成",
      state === "failed",
    );
  } catch (err) {
    appStatus = previous;
    if (quiet) await loadStatus();
    else showNotice(`检查更新失败: ${err.message}`, true);
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
$("copy-orchestration").addEventListener("click", copyOrchestration);
$("save-limit").addEventListener("click", saveLimit);
const tabFromHash = () => ["hosts", "host-cards", "updates"].includes(location.hash.slice(1)) ? "hosts" : ["skills", "avatars", "orchestration"].includes(location.hash.slice(1)) ? location.hash.slice(1) : "members";
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
$("refresh-detection").addEventListener("click", () => refreshDetection());
window.addEventListener("focus", probeWhenStale);
document.addEventListener("visibilitychange", probeWhenStale);
$("save-btn").addEventListener("click", saveMembers);

$("btn-check-update").addEventListener("click", () => triggerCheckUpdate());
$("btn-apply-update").addEventListener("click", triggerApplyUpdate);
$("btn-retry-update").addEventListener("click", () => {
  if (appStatus?.service?.state === "failed") {
    triggerApplyUpdate();
  } else {
    triggerCheckUpdate();
  }
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
  // Moving between this page's sections keeps the edits; only leaving the page loses them.
  if (!link || !(dirty || busy) || link.getAttribute("href").startsWith("/settings/")) return;
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
  }
  for (const [key, button] of Object.entries(buttons))
    button.addEventListener("click", () => { AN.setFullModelNames(key === "full"); show(); });
  show();
})();
