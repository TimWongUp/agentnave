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

// 6 supported CLI providers
const PROVIDERS = ["codex", "claude", "codebuddy", "grok", "antigravity", "pi"];
const PROVIDER_NAMES = {
  codex: "Codex CLI",
  claude: "Claude Code",
  codebuddy: "CodeBuddy Code",
  grok: "Grok CLI",
  antigravity: "Antigravity CLI",
  pi: "Pi Coding Agent",
};
const SOURCE_LABELS = {
  "codex-cache": "Codex 缓存",
  command: "命令输出",
  help: "帮助信息",
  aliases: "别名配置",
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

// Application state
let currentDoc = {
  version: 1,
  clis: {},
  modes: {},
};
let detected = {};
let currentMode = "task"; // "task" | "discussion"
let currentTab = "members"; // "members" | "updates"
let dirty = false;
let busy = false;
let documentError = null;
let docVersion = 0; // Monotonic counter for save race detection
let expandedModel = null; // { provider: string, model: string } | null
let appStatus = null;
let statusPollTimer = null;
let lastMigrationData = { migrated: false, migration_skipped: [] };

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

const mockStatusData = {
  version: "0.20.0",
  update: {
    state: "available",
    version: "0.20.1",
    error: null,
  },
  service: {
    state: "outdated",
    version: "0.19.0",
    error: null,
  },
  last_checked: new Date().toISOString(),
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
    return JSON.parse(JSON.stringify(mockStatusData));
  }
  const res = await fetch("/app/status", { cache: "no-store" });
  if (!res.ok) throw new Error(`获取状态失败（${res.status}）`);
  return res.json();
}

async function apiCheckUpdate() {
  if (isMock) {
    mockStatusData.last_checked = new Date().toISOString();
    return JSON.parse(JSON.stringify(mockStatusData));
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
    mockStatusData.update.state = "installing";
    setTimeout(() => {
      mockStatusData.update.state = "none";
      mockStatusData.service.state = "done";
      mockStatusData.service.version = mockStatusData.update.version;
      mockStatusData.version = mockStatusData.update.version;
      renderUpdates();
    }, 4000);
    return JSON.parse(JSON.stringify(mockStatusData));
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

function incDocVersion() {
  docVersion++;
  dirty = true;
  updateControls();
}

// Count permission-related options across entire document
function countPermissionOptions() {
  let count = 0;
  for (const cli of Object.values(currentDoc.clis || {})) {
    for (const m of Object.values(cli.models || {})) {
      if (m.options && typeof m.options === "object") {
        for (const k of Object.keys(m.options)) {
          if (isPermissionOptionKey(k)) count++;
        }
      }
    }
  }
  for (const modeObj of Object.values(currentDoc.modes || {})) {
    for (const provObj of Object.values(modeObj || {})) {
      for (const override of Object.values(provObj.overrides || {})) {
        if (override.options && typeof override.options === "object") {
          for (const k of Object.keys(override.options)) {
            if (isPermissionOptionKey(k)) count++;
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

// Check whether current options contain invalid numbers
function hasInvalidOptionInputs() {
  const badInputs = document.querySelectorAll(".input-error");
  return badInputs.length > 0;
}

function updateControls() {
  $("dirty-text").textContent = dirty ? "有未保存的修改" : "";

  const emptyRestricted = hasEmptyRestrictedMode();
  const invalidInputs = hasInvalidOptionInputs();
  const canSave = !busy && dirty && !documentError && !emptyRestricted && !invalidInputs;

  $("save-btn").disabled = !canSave;
  $("refresh-detection").disabled = busy;

  // Feedback hints
  const feedback = $("save-feedback");
  if (emptyRestricted) {
    feedback.textContent = "受限模式至少启用一个 CLI，或关闭限制";
    feedback.className = "save-feedback error";
  } else if (invalidInputs) {
    feedback.textContent = "存在非法的数字参数，请检查标红输入框";
    feedback.className = "save-feedback error";
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
  currentTab = tab;
  $("tab-members").classList.toggle("active", tab === "members");
  $("tab-members").setAttribute("aria-selected", tab === "members" ? "true" : "false");
  $("tab-updates").classList.toggle("active", tab === "updates");
  $("tab-updates").setAttribute("aria-selected", tab === "updates" ? "true" : "false");
  $("pane-members").hidden = tab !== "members";
  $("pane-updates").hidden = tab !== "updates";

  if (tab === "updates") {
    loadStatus();
  }
}

// Switch mode
function setMode(mode) {
  if (currentMode === mode) return;
  currentMode = mode;
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
      ? "已限制：仅允许下方启用的 CLI 与模型"
      : "未限制：所有 CLI 都可用";
  }
}

function toggleModeRestricted(restricted) {
  if (!currentDoc.modes) currentDoc.modes = {};
  if (restricted) {
    currentDoc.modes[currentMode] = {};
    // Pre-enable all installed CLIs with models: [] (unlimited models)
    for (const p of PROVIDERS) {
      if (detected[p]?.installed) {
        currentDoc.modes[currentMode][p] = { models: [], overrides: {} };
      }
    }
  } else {
    delete currentDoc.modes[currentMode];
  }
  incDocVersion();
  renderModeToolbar();
  renderCards();
}

function isProviderEnabledInMode(provider) {
  if (!isModeRestricted()) return true; // Unrestricted mode => all available
  return Boolean(currentDoc.modes?.[currentMode]?.[provider]);
}

function toggleProviderInMode(provider, enabled) {
  if (!isModeRestricted()) return;
  if (enabled) {
    if (!currentDoc.modes[currentMode][provider]) {
      currentDoc.modes[currentMode][provider] = {
        models: [],
        overrides: {},
      };
    }
  } else {
    delete currentDoc.modes[currentMode][provider];
  }
  incDocVersion();
  renderCards();
}

function ensureCliModel(provider, modelId) {
  if (!currentDoc.clis) currentDoc.clis = {};
  if (!currentDoc.clis[provider]) currentDoc.clis[provider] = { models: {} };
  if (!currentDoc.clis[provider].models) currentDoc.clis[provider].models = {};
  if (!currentDoc.clis[provider].models[modelId]) {
    currentDoc.clis[provider].models[modelId] = {};
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
  return Array.from(modelIdSet);
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
      showNotice("至少保留一个模型；要放开请打开“不限模型”，要停用请关闭“在当前模式启用”", true);
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
  }
  incDocVersion();
  renderCards();
}

function setModelDefault(provider, modelId) {
  ensureCliModel(provider, modelId);

  // If in restricted mode
  if (isModeRestricted()) {
    const provConf = currentDoc.modes[currentMode]?.[provider];
    if (!provConf) return;

    if (provConf.default === modelId) {
      delete provConf.default;
    } else {
      provConf.default = modelId;
      // Do NOT force modelId into provConf.models if models is empty (unlimited)
      if (Array.isArray(provConf.models) && provConf.models.length > 0) {
        if (!provConf.models.includes(modelId)) {
          provConf.models.push(modelId);
        }
      }
    }
  } else {
    // In unrestricted mode, default can be stored if provider config exists, or kept in clis
    ensureModeContainer(currentMode);
    if (!currentDoc.modes[currentMode][provider]) {
      currentDoc.modes[currentMode][provider] = { models: [] };
    }
    const provConf = currentDoc.modes[currentMode][provider];
    if (provConf.default === modelId) {
      delete provConf.default;
    } else {
      provConf.default = modelId;
    }
  }
  incDocVersion();
  renderCards();
}

function isModelOverridden(provider, modelId) {
  if (!isModeRestricted()) return false;
  return Boolean(currentDoc.modes[currentMode]?.[provider]?.overrides?.[modelId]);
}

function toggleModelOverride(provider, modelId, override) {
  if (!isModeRestricted()) return;
  const provConf = currentDoc.modes[currentMode]?.[provider];
  if (!provConf) return;
  if (!provConf.overrides) provConf.overrides = {};

  if (override) {
    if (!provConf.overrides[modelId]) {
      // Create empty override (all fields default to inherit)
      provConf.overrides[modelId] = {};
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

function ensureModeContainer(mode) {
  if (!currentDoc.modes) currentDoc.modes = {};
  if (!currentDoc.modes[mode]) currentDoc.modes[mode] = {};
}

// Render model detail form with tri-state inheritance
function renderModelDetail(provider, modelId) {
  ensureCliModel(provider, modelId);
  const overridden = isModelOverridden(provider, modelId);
  const provConf = currentDoc.modes?.[currentMode]?.[provider];
  const sharedConfig = currentDoc.clis[provider]?.models?.[modelId] || {};
  const overrideConfig = overridden ? provConf.overrides[modelId] : null;

  const detailBox = el("div", { className: "model-detail" });

  // Mode override switch (only active in restricted mode)
  if (isModeRestricted()) {
    const overrideCheckbox = el("input", {
      type: "checkbox",
      checked: overridden,
      onChange: (e) => toggleModelOverride(provider, modelId, e.target.checked),
    });
    const overrideToggle = el(
      "div",
      { className: "detail-override-toggle" },
      el("label", {}, overrideCheckbox, textNode(" 仅在当前模式使用不同设置")),
      el(
        "p",
        { className: "field-hint" },
        overridden
          ? "模式覆盖已开启：下方各字段可独立选择“继承共享设置”或设定本模式值。"
          : "开启后可在此模式下覆盖该模型的档位、默认档位、参数或说明；关闭时完全使用共享设置。"
      )
    );
    detailBox.appendChild(overrideToggle);
  }

  // Target object to edit
  const targetConfig = overridden ? overrideConfig : sharedConfig;

  // 1. Efforts Field (checklist)
  const detectedModelObj = (detected[provider]?.models || []).find((m) => m.id === modelId);
  const modelEfforts = detectedModelObj?.efforts;
  const candidateEfforts =
    Array.isArray(modelEfforts) && modelEfforts.length > 0
      ? modelEfforts
      : detected[provider]?.efforts || [];

  const effortsInherited = overridden && !("efforts" in overrideConfig);
  if (candidateEfforts.length > 0) {
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
          targetConfig.efforts = list; // In override, keep empty array [] if cleared

          // Clear default effort if selected default is no longer in active efforts
          if (targetConfig.effort && list.length > 0 && !list.includes(targetConfig.effort)) {
            delete targetConfig.effort;
          }
          incDocVersion();
          renderCards();
        },
      });
      checklist.appendChild(el("label", { className: "effort-item" }, effBox, textNode(eff)));
    }

    const headingKids = [el("h4", {}, "允许的努力档位 (efforts)")];
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
      el("p", { className: "field-hint" }, "不勾选表示不限制努力档位；勾选时主控调用仅允许选中的档位。")
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
    disabled: effortInherited,
    onChange: (e) => {
      const val = e.target.value;
      if (!val) {
        delete targetConfig.effort;
      } else {
        targetConfig.effort = val;
      }
      incDocVersion();
    },
  });

  // If not in override, or if in override setting specific value, populate options
  if (!overridden) {
    effortSelect.appendChild(el("option", { value: "" }, "不设置（继承原生默认）"));
  }

  for (const eff of effortDropdownChoices) {
    const opt = el("option", { value: eff }, eff);
    if (activeEffortVal === eff) opt.selected = true;
    effortSelect.appendChild(opt);
  }

  const effortHeadingKids = [el("h4", {}, "默认努力档位 (effort)")];
  if (overridden) {
    const inheritEffortCheckbox = el("input", {
      type: "checkbox",
      checked: effortInherited,
      onChange: (e) => {
        if (e.target.checked) {
          delete overrideConfig.effort;
        } else {
          // In override, effort must be a concrete choice, not empty
          overrideConfig.effort = effortDropdownChoices[0] || "medium";
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
    el("p", { className: "field-hint" }, "主控未指定 effort 时套用此默认值。")
  );
  detailBox.appendChild(defaultEffortField);

  // 3. Other Options Field
  const optionsInherited = overridden && !("options" in overrideConfig);
  const optionsHeadingKids = [el("h4", {}, "其他参数 (options)")];

  const addOptionBtn = el("button", {
    type: "button",
    disabled: optionsInherited,
    onClick: () => {
      if (overridden && optionsInherited) return;
      if (!targetConfig.options) targetConfig.options = {};
      optionsContainer.appendChild(renderOptionRow("", ""));
      incDocVersion();
    },
  }, "＋ 添加参数");

  if (overridden) {
    const inheritOptionsCheckbox = el("input", {
      type: "checkbox",
      checked: optionsInherited,
      onChange: (e) => {
        if (e.target.checked) {
          delete overrideConfig.options;
        } else {
          overrideConfig.options = JSON.parse(JSON.stringify(sharedConfig.options || {}));
        }
        incDocVersion();
        renderCards();
      },
    });
    optionsHeadingKids.push(
      el("label", { className: "field-inherit-label" }, inheritOptionsCheckbox, textNode(" 继承共享设置"))
    );
  }
  optionsHeadingKids.push(addOptionBtn);

  const optionsContainer = el("div", {
    className: `options-list${optionsInherited ? " field-inherited-area" : ""}`,
  });

  const renderOptionRow = (key, val) => {
    let currentVal = val;
    let currentType = typeof val === "boolean" ? "boolean" : typeof val === "number" ? "number" : "string";

    const keyInput = el("input", {
      type: "text",
      value: key,
      placeholder: "参数键",
      disabled: optionsInherited,
      dataset: { optionKey: "" },
    });

    const typeSelect = el(
      "select",
      { disabled: optionsInherited },
      el("option", { value: "string" }, "字符串"),
      el("option", { value: "number" }, "数字"),
      el("option", { value: "boolean" }, "布尔")
    );
    typeSelect.value = currentType;

    let valInput;
    if (currentType === "boolean") {
      valInput = el(
        "select",
        { disabled: optionsInherited, dataset: { optionVal: "" } },
        el("option", { value: "true" }, "true"),
        el("option", { value: "false" }, "false")
      );
      valInput.value = String(currentVal) === "true" ? "true" : "false";
    } else {
      valInput = el("input", {
        type: "text",
        value: currentVal != null ? String(currentVal) : "",
        placeholder: "参数值",
        disabled: optionsInherited,
        dataset: { optionVal: "" },
      });
      if (currentType === "number") {
        const num = Number(currentVal);
        if (currentVal === "" || !Number.isFinite(num)) {
          valInput.classList.add("input-error");
        }
      }
    }

    const removeBtn = el("button", {
      type: "button",
      className: "btn-remove-option",
      disabled: optionsInherited,
      onClick: () => {
        row.remove();
        syncOptions();
      },
    }, "移除");

    const isWarning = isPermissionOptionKey(key);
    const warningBox = el(
      "div",
      { className: "option-permission-warning", hidden: !isWarning },
      "权限类参数：保存后会对该组员在此范围内的每次调用生效，主控不会再逐次征求你的同意。"
    );

    const updateWarning = () => {
      const k = keyInput.value.trim();
      warningBox.hidden = !isPermissionOptionKey(k);
    };

    const syncOptions = () => {
      const rows = optionsContainer.querySelectorAll(".option-row");
      const nextOptions = {};
      for (const r of rows) {
        const k = r.querySelector("[data-option-key]").value.trim();
        const t = r.querySelector("select").value;
        const inputElem = r.querySelector("[data-option-val]");
        const raw = inputElem ? inputElem.value.trim() : "";
        if (!k) continue;

        let finalVal = raw;
        if (t === "number") {
          const num = Number(raw);
          if (raw === "" || !Number.isFinite(num)) {
            inputElem?.classList.add("input-error");
          } else {
            inputElem?.classList.remove("input-error");
            finalVal = num;
          }
        } else if (t === "boolean") {
          finalVal = raw === "true";
        }
        nextOptions[k] = finalVal;
      }
      targetConfig.options = nextOptions;
      incDocVersion();
    };

    typeSelect.addEventListener("change", () => {
      const nextType = typeSelect.value;
      let newValElem;
      if (nextType === "boolean") {
        newValElem = el(
          "select",
          { dataset: { optionVal: "" } },
          el("option", { value: "true" }, "true"),
          el("option", { value: "false" }, "false")
        );
      } else {
        newValElem = el("input", {
          type: "text",
          placeholder: "参数值",
          dataset: { optionVal: "" },
        });
        if (nextType === "number") {
          newValElem.classList.add("input-error");
        }
      }
      valInput.replaceWith(newValElem);
      valInput = newValElem;
      valInput.addEventListener("input", syncOptions);
      valInput.addEventListener("change", syncOptions);
      syncOptions();
    });

    keyInput.addEventListener("input", updateWarning);
    keyInput.addEventListener("input", syncOptions);
    valInput.addEventListener("input", syncOptions);
    valInput.addEventListener("change", syncOptions);

    const row = el(
      "div",
      { className: "option-row" },
      keyInput,
      typeSelect,
      valInput,
      removeBtn,
      warningBox
    );
    return row;
  };

  const activeOptions = optionsInherited ? (sharedConfig.options || {}) : (targetConfig.options || {});
  for (const [k, v] of Object.entries(activeOptions)) {
    if (k !== "model" && k !== "effort") {
      optionsContainer.appendChild(renderOptionRow(k, v));
    }
  }

  const optionsField = el(
    "div",
    { className: "detail-field" },
    el("div", { className: "options-heading" }, ...optionsHeadingKids),
    el("p", { className: "field-hint" }, "用于为该模型传递专属参数（不可包含 model 或 effort 键）。"),
    optionsContainer
  );
  detailBox.appendChild(optionsField);

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

  const notesHeadingKids = [el("h4", {}, "说明与适用场景 (notes)")];
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
    el("div", { className: notesInherited ? "field-inherited-area" : "" }, notesText)
  );
  detailBox.appendChild(notesField);

  return detailBox;
}

// Render 6 CLI cards
function renderCards() {
  const container = $("cli-cards");
  container.replaceChildren();

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
      enableControl = el(
        "span",
        { className: "badge badge-run" },
        "未限制：默认可用"
      );
    } else {
      const enableCheckbox = el("input", {
        type: "checkbox",
        checked: isEnabled,
        onChange: (e) => toggleProviderInMode(provider, e.target.checked),
      });
      enableControl = el("label", {}, enableCheckbox, textNode(" 在当前模式启用"));
    }

    const header = el(
      "div",
      { className: "cli-header" },
      el(
        "div",
        { className: "cli-title-area" },
        el("span", { className: "cli-name" }, PROVIDER_NAMES[provider] || provider),
        statusBadge,
        sourceBadge
      ),
      el("div", { className: "cli-enable-area" }, enableControl)
    );
    card.appendChild(header);

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
        el("p", { className: "cli-disabled-hint" }, "当前模式未启用该 CLI；启用后可配置允许调用的模型。")
      );
    } else {
      const provConf = modeRestricted ? currentDoc.modes[currentMode]?.[provider] : null;
      const isUnlimited = modeRestricted && Array.isArray(provConf?.models) && provConf.models.length === 0;

      // Unlimited models toggle (only in restricted mode)
      if (modeRestricted) {
        const unlimitedBox = el("input", {
          type: "checkbox",
          checked: isUnlimited,
          onChange: (e) => toggleUnlimitedModels(provider, e.target.checked),
        });
        const unlimitedWrap = el(
          "div",
          { className: "cli-unlimited-wrap" },
          el("label", { className: "cli-unlimited-label" }, unlimitedBox, textNode(" 不限模型（允许调用该 CLI 支持的任意模型）"))
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
        const isDefault = provConf ? provConf.default === modelId : false;
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
        });

        // Model Checkbox
        let checkInput;
        if (modeRestricted) {
          checkInput = el("input", {
            type: "checkbox",
            checked: isAllowed,
            disabled: isUnlimited || isLastChecked,
            onChange: (e) => toggleModelAllowed(provider, modelId, e.target.checked),
          });
        }

        // Default Radio
        const radioInput = el("input", {
          type: "radio",
          name: `default-${provider}-${currentMode}`,
          checked: isDefault,
          onClick: () => setModelDefault(provider, modelId),
        });

        const defaultLabel = el(
          "label",
          { className: `default-radio-wrap${isDefault ? " active" : ""}`, title: "设为当前模式默认模型（再次点击可清除）" },
          radioInput,
          textNode("默认")
        );

        // Tags
        const tags = [];
        if (isStale) {
          tags.push(el("span", { className: "badge badge-warning" }, "已不在列表"));
        }
        if (hasOverride) {
          tags.push(el("span", { className: "badge badge-accent" }, "本模式已覆盖"));
        }

        // Detail toggle button
        const detailBtn = el("button", {
          type: "button",
          className: `btn-detail${isExpanded ? " active" : ""}`,
          onClick: () => {
            expandedModel = isExpanded ? null : { provider, model: modelId };
            renderCards();
          },
        }, isExpanded ? "收起" : "配置");

        const controlsKids = [];
        if (modeRestricted) {
          const checkLabelAttrs = { className: "model-check-label" };
          if (isLastChecked) {
            checkLabelAttrs.title = "至少保留一个模型；要放开请打开“不限模型”，要停用请关闭“在当前模式启用”";
          } else if (isUnlimited) {
            checkLabelAttrs.title = "当前为不限模型模式";
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
                title: "至少保留一个模型；要放开请打开“不限模型”，要停用请关闭“在当前模式启用”",
              },
              "至少保留一个模型；要放开请打开“不限模型”，要停用请关闭“在当前模式启用”"
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
        placeholder: "手动添加模型 ID，如 claude-3-7-sonnet…",
      });
      const addBtn = el("button", {
        type: "button",
        className: "add-model-btn",
        onClick: () => {
          if (!addInput.value.trim()) return;
          addCustomModel(provider, addInput.value);
          addInput.value = "";
        },
      }, "添加模型");
      addInput.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          addBtn.click();
        }
      });

      const addRow = el("div", { className: "add-model-row" }, addInput, addBtn);
      body.appendChild(addRow);
    }

    card.appendChild(body);
    container.appendChild(card);
  }

  updateControls();
}

// Save members.json with race version checking
async function saveMembers() {
  if (busy || !dirty || documentError || hasEmptyRestrictedMode() || hasInvalidOptionInputs()) return;
  busy = true;
  updateControls();
  const feedback = $("save-feedback");
  feedback.textContent = "正在保存…";
  feedback.className = "save-feedback";

  const savingVersion = docVersion;

  try {
    // Validate options
    for (const [prov, cliConf] of Object.entries(currentDoc.clis || {})) {
      for (const [mId, mConf] of Object.entries(cliConf.models || {})) {
        if (mConf.options) {
          if ("model" in mConf.options || "effort" in mConf.options) {
            throw new Error(`${prov} 模型 ${mId} 的 options 不能包含 model 或 effort 键`);
          }
          for (const [k, v] of Object.entries(mConf.options)) {
            if (typeof v === "number" && !Number.isFinite(v)) {
              throw new Error(`${prov} 模型 ${mId} 的参数 ${k} 必须为有效有限数字`);
            }
          }
        }
      }
    }

    await apiPostMembers(currentDoc);
    if (savingVersion === docVersion) {
      dirty = false;
    }
    feedback.textContent = "设置已保存 · 参数是否有效以主控读取时的校验为准";
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
      title.textContent = "已将原编排档案的模型与说明迁移；两种模式当前未限制，如需限制再开启";
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
  showNotice("正在读取成员与模型配置…");
  try {
    const data = await apiGetMembers(false);
    detected = data.detected || {};
    currentDoc = data.document || { version: 1, clis: {}, modes: {} };
    documentError = data.error || null;
    lastMigrationData = data;

    renderMigrationBanner(data);
    renderErrorBanner();
    renderModeToolbar();

    if (documentError) {
      showNotice(`配置文件损坏: ${documentError}。当前只读，禁止保存。`, true);
    } else {
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
  } else {
    appBadge.hidden = true;
    $("update-badge").hidden = true;
  }

  // Service status
  const serviceInfo = appStatus.service;
  const serviceBadge = $("service-badge");
  const serviceMsg = $("service-msg");

  if (!serviceInfo) {
    $("service-version").textContent = "—";
    serviceBadge.hidden = true;
    serviceMsg.hidden = false;
    serviceMsg.textContent = "本机服务与托盘一致或未通过 uv 安装";
  } else {
    $("service-version").textContent = serviceInfo.version ? `v${serviceInfo.version}` : "—";
    const isServiceOutdated =
      serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version));

    if (serviceInfo.state === "upgrading") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag upgrading";
      serviceBadge.textContent = "正在升级…";
      serviceMsg.hidden = false;
      serviceMsg.textContent = "正在从 GitHub 升级本机服务，约需一分钟…";
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
      serviceMsg.textContent = "升级后各宿主会在空闲时自动切换到新版本。";
    } else if (serviceInfo.state === "done") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag done";
      serviceBadge.textContent = "已是最新";
      serviceMsg.hidden = true;
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
    serviceInfo?.state === "upgrading";

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
    if (hasAppUpdate) {
      updateHint.textContent = "会先更新 App 并自动重启，重启后自动升级本机服务。";
    } else {
      updateHint.textContent = "将升级本机服务。";
    }
  }

  if (isUpdating) {
    progressBox.hidden = false;
    if (updateInfo?.state === "checking") {
      progressText.textContent = "正在检查更新…";
    } else if (updateInfo?.state === "installing") {
      progressText.textContent = "正在下载 App 更新并准备重启…";
    } else if (serviceInfo?.state === "upgrading") {
      progressText.textContent = "正在升级本机服务，约需一分钟…";
    }
  } else {
    progressBox.hidden = true;
  }

  const hasError = updateInfo?.state === "failed" || serviceInfo?.state === "failed" || Boolean(updateInfo?.error);
  if (hasError && !isUpdating) {
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

async function loadStatus() {
  try {
    appStatus = await apiGetStatus();
    renderUpdates();
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
    showNotice("检查更新完成");
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
$("tab-members").addEventListener("click", () => setTab("members"));
$("tab-updates").addEventListener("click", () => setTab("updates"));
$("mode-task").addEventListener("click", () => setMode("task"));
$("mode-discussion").addEventListener("click", () => setMode("discussion"));
$("mode-restrict-toggle").addEventListener("change", (e) => toggleModeRestricted(e.target.checked));
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

window.addEventListener("beforeunload", (e) => {
  if (dirty || busy) {
    e.preventDefault();
    e.returnValue = "";
  }
});

// Initialize
loadMembers();
loadStatus();
