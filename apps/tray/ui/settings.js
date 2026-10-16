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

// Application state
let currentDoc = {
  version: 1,
  clis: {},
  modes: { task: {}, discussion: {} },
};
let detected = {};
let currentMode = "task"; // "task" | "discussion"
let currentTab = "members"; // "members" | "updates"
let dirty = false;
let busy = false;
let documentError = null;
let expandedModel = null; // { provider: string, model: string } | null
let appStatus = null;
let statusPollTimer = null;

// Determine mock mode
const isMock =
  new URLSearchParams(location.search).has("mock") ||
  location.protocol === "file:";

// Mock data compliant with contract.md
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
        },
      },
      claude: {
        models: {
          "claude-3-7-sonnet": {
            effort: "high",
            options: {},
            notes: "通用编码与复杂重构",
          },
        },
      },
    },
    modes: {
      task: {
        codex: {
          models: ["gpt-6.1-sol"],
          default: "gpt-6.1-sol",
          overrides: {},
        },
        claude: {
          models: ["claude-3-7-sonnet"],
          default: "claude-3-7-sonnet",
          overrides: {},
        },
      },
      discussion: {
        codex: {
          models: ["gpt-6.1-sol"],
          default: "gpt-6.1-sol",
        },
      },
    },
  },
  error: null,
  migrated: true,
  migration_skipped: ["档案“旧脚本测试”未指定模型，已跳过迁移"],
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

function markDirty() {
  dirty = true;
  updateControls();
}

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

function updateControls() {
  $("dirty-text").textContent = dirty ? "有未保存的修改" : "";
  $("save-btn").disabled = busy || !dirty || Boolean(documentError);
  $("refresh-detection").disabled = busy;

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
  renderCards();
}

// Member data helpers
function ensureModeContainer(mode) {
  if (!currentDoc.modes) currentDoc.modes = {};
  if (!currentDoc.modes[mode]) currentDoc.modes[mode] = {};
}

function isProviderEnabled(provider) {
  return Boolean(currentDoc.modes?.[currentMode]?.[provider]);
}

function toggleProvider(provider, enabled) {
  ensureModeContainer(currentMode);
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
  markDirty();
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

function toggleModelAllowed(provider, modelId, allowed) {
  ensureCliModel(provider, modelId);
  ensureModeContainer(currentMode);
  const provConf = currentDoc.modes[currentMode][provider];
  if (!provConf) return;
  if (!Array.isArray(provConf.models)) provConf.models = [];

  if (allowed) {
    if (!provConf.models.includes(modelId)) {
      provConf.models.push(modelId);
    }
  } else {
    provConf.models = provConf.models.filter((m) => m !== modelId);
    if (provConf.default === modelId) {
      delete provConf.default;
    }
  }
  markDirty();
  renderCards();
}

function setModelDefault(provider, modelId) {
  ensureModeContainer(currentMode);
  const provConf = currentDoc.modes[currentMode][provider];
  if (!provConf) return;

  if (provConf.default === modelId) {
    delete provConf.default;
  } else {
    ensureCliModel(provider, modelId);
    if (!Array.isArray(provConf.models)) provConf.models = [];
    if (!provConf.models.includes(modelId)) {
      provConf.models.push(modelId);
    }
    provConf.default = modelId;
  }
  markDirty();
  renderCards();
}

function isModelOverridden(provider, modelId) {
  return Boolean(currentDoc.modes?.[currentMode]?.[provider]?.overrides?.[modelId]);
}

function toggleModelOverride(provider, modelId, override) {
  ensureModeContainer(currentMode);
  const provConf = currentDoc.modes[currentMode][provider];
  if (!provConf) return;
  if (!provConf.overrides) provConf.overrides = {};

  if (override) {
    if (!provConf.overrides[modelId]) {
      const shared = currentDoc.clis?.[provider]?.models?.[modelId] || {};
      provConf.overrides[modelId] = JSON.parse(JSON.stringify(shared));
    }
  } else {
    delete provConf.overrides[modelId];
  }
  markDirty();
  renderCards();
}

function addCustomModel(provider, modelId) {
  const trimmed = modelId.trim();
  if (!trimmed) return;
  ensureCliModel(provider, trimmed);
  ensureModeContainer(currentMode);
  const provConf = currentDoc.modes[currentMode][provider];
  if (provConf) {
    if (!Array.isArray(provConf.models)) provConf.models = [];
    if (!provConf.models.includes(trimmed)) {
      provConf.models.push(trimmed);
    }
  }
  expandedModel = { provider, model: trimmed };
  markDirty();
  renderCards();
}

// Render model detail form
function renderModelDetail(provider, modelId) {
  ensureCliModel(provider, modelId);
  const overridden = isModelOverridden(provider, modelId);
  const provConf = currentDoc.modes?.[currentMode]?.[provider];
  const targetConfig = overridden
    ? provConf.overrides[modelId]
    : currentDoc.clis[provider].models[modelId] || {};

  const detailBox = el("div", { className: "model-detail" });

  // Override switch
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
        ? "当前处于模式专属设置（覆盖共享设置）；关闭开关将清除本模式覆盖并恢复共享设置。"
        : "开启后可为当前模式指定单独的努力程度档位、默认档位与参数；关闭时使用共享设置。"
    )
  );
  detailBox.appendChild(overrideToggle);

  // Efforts checklist (prioritize model-level efforts, fall back to provider-level efforts)
  const detectedModelObj = (detected[provider]?.models || []).find((m) => m.id === modelId);
  const modelEfforts = detectedModelObj?.efforts;
  const detectedEfforts =
    Array.isArray(modelEfforts) && modelEfforts.length > 0
      ? modelEfforts
      : detected[provider]?.efforts || [];
  if (detectedEfforts.length > 0) {
    const activeEfforts = Array.isArray(targetConfig.efforts) ? targetConfig.efforts : [];
    const checklist = el("div", { className: "efforts-checklist" });
    for (const eff of detectedEfforts) {
      const effBox = el("input", {
        type: "checkbox",
        checked: activeEfforts.includes(eff),
        onChange: (e) => {
          let list = Array.isArray(targetConfig.efforts) ? [...targetConfig.efforts] : [];
          if (e.target.checked) {
            if (!list.includes(eff)) list.push(eff);
          } else {
            list = list.filter((item) => item !== eff);
          }
          if (list.length === 0) {
            delete targetConfig.efforts;
          } else {
            targetConfig.efforts = list;
          }
          markDirty();
        },
      });
      checklist.appendChild(
        el("label", { className: "effort-item" }, effBox, textNode(eff))
      );
    }

    const effortsField = el(
      "div",
      { className: "detail-field" },
      el("label", {}, "允许的努力档位（efforts）"),
      checklist,
      el("p", { className: "field-hint" }, "不勾选表示不限制努力档位；若勾选，主控调用时仅允许选中的档位。")
    );
    detailBox.appendChild(effortsField);
  }

  // Default effort select
  const effortSelect = el("select", {
    onChange: (e) => {
      const val = e.target.value;
      if (!val) {
        delete targetConfig.effort;
      } else {
        targetConfig.effort = val;
      }
      markDirty();
    },
  });
  effortSelect.appendChild(el("option", { value: "" }, "不设置（继承原生默认）"));
  const effortChoices = detectedEfforts.length > 0 ? detectedEfforts : ["minimal", "low", "medium", "high", "xhigh"];
  for (const eff of effortChoices) {
    const opt = el("option", { value: eff }, eff);
    if (targetConfig.effort === eff) opt.selected = true;
    effortSelect.appendChild(opt);
  }
  // If targetConfig.effort is not in choices, append it
  if (targetConfig.effort && !effortChoices.includes(targetConfig.effort)) {
    const opt = el("option", { value: targetConfig.effort }, targetConfig.effort);
    opt.selected = true;
    effortSelect.appendChild(opt);
  }

  const defaultEffortField = el(
    "div",
    { className: "detail-field" },
    el("label", {}, "默认努力档位（effort）"),
    effortSelect,
    el("p", { className: "field-hint" }, "主控未指定 effort 且模式未覆盖时套用此默认值。")
  );
  detailBox.appendChild(defaultEffortField);

  // Other options
  const optionsContainer = el("div", { className: "options-list" });
  const renderOptionRow = (key, val) => {
    let currentVal = val;
    let currentType = typeof val === "boolean" ? "boolean" : typeof val === "number" ? "number" : "string";

    const keyInput = el("input", {
      type: "text",
      value: key,
      placeholder: "参数键",
      dataset: { optionKey: "" },
    });

    const typeSelect = el(
      "select",
      {},
      el("option", { value: "string" }, "字符串"),
      el("option", { value: "number" }, "数字"),
      el("option", { value: "boolean" }, "布尔")
    );
    typeSelect.value = currentType;

    const valInput = el("input", {
      type: "text",
      value: String(currentVal),
      placeholder: "参数值",
      dataset: { optionVal: "" },
    });

    const removeBtn = el("button", {
      type: "button",
      className: "btn-remove-option",
      onClick: () => {
        row.remove();
        syncOptions();
      },
    }, "移除");

    const syncOptions = () => {
      const rows = optionsContainer.querySelectorAll(".option-row");
      const nextOptions = {};
      for (const r of rows) {
        const k = r.querySelector("[data-option-key]").value.trim();
        const t = r.querySelector("select").value;
        const raw = r.querySelector("[data-option-val]").value.trim();
        if (!k) continue;
        let finalVal = raw;
        if (t === "number") {
          const num = Number(raw);
          finalVal = Number.isFinite(num) ? num : raw;
        } else if (t === "boolean") {
          finalVal = raw.toLowerCase() === "true";
        }
        nextOptions[k] = finalVal;
      }
      if (Object.keys(nextOptions).length === 0) {
        delete targetConfig.options;
      } else {
        targetConfig.options = nextOptions;
      }
      markDirty();
    };

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

    keyInput.addEventListener("input", updateWarning);
    keyInput.addEventListener("input", syncOptions);
    typeSelect.addEventListener("change", syncOptions);
    valInput.addEventListener("input", syncOptions);

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

  const existingOptions = targetConfig.options || {};
  for (const [k, v] of Object.entries(existingOptions)) {
    if (k !== "model" && k !== "effort") {
      optionsContainer.appendChild(renderOptionRow(k, v));
    }
  }

  const addOptionBtn = el("button", {
    type: "button",
    onClick: () => {
      optionsContainer.appendChild(renderOptionRow("", ""));
      markDirty();
    },
  }, "＋ 添加参数");

  const optionsField = el(
    "div",
    { className: "detail-field" },
    el(
      "div",
      { className: "options-heading" },
      el("h4", {}, "其他参数 (options)"),
      addOptionBtn
    ),
    el("p", { className: "field-hint" }, "用于为该模型传递专属参数（不可包含 model 或 effort 键）。"),
    optionsContainer
  );
  detailBox.appendChild(optionsField);

  // Notes field
  const notesText = el("textarea", {
    placeholder: "填写适用任务或选择理由…",
    onInput: (e) => {
      const text = e.target.value.trim();
      if (!text) {
        delete targetConfig.notes;
      } else {
        targetConfig.notes = e.target.value;
      }
      markDirty();
    },
  });
  notesText.value = targetConfig.notes || "";

  const notesField = el(
    "div",
    { className: "detail-field" },
    el("label", {}, "说明与适用场景 (notes)"),
    notesText
  );
  detailBox.appendChild(notesField);

  return detailBox;
}

// Render 6 CLI cards
function renderCards() {
  const container = $("cli-cards");
  container.replaceChildren();

  for (const provider of PROVIDERS) {
    const info = detected[provider] || {
      installed: false,
      source: "none",
      path: null,
      error: null,
      models: [],
      efforts: [],
    };
    const isEnabled = isProviderEnabled(provider);
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

    const enableCheckbox = el("input", {
      type: "checkbox",
      checked: isEnabled,
      onChange: (e) => toggleProvider(provider, e.target.checked),
    });

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
      el(
        "div",
        { className: "cli-enable-area" },
        el("label", {}, enableCheckbox, textNode(" 在当前模式启用"))
      )
    );
    card.appendChild(header);

    // Metadata (path or detection error)
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

    // Body (models list)
    const body = el("div", {
      className: `cli-body${isEnabled ? "" : " disabled"}`,
    });

    if (!isEnabled) {
      body.appendChild(
        el("p", { className: "cli-disabled-hint" }, "当前模式未启用该 CLI；启用后可配置允许调用的模型。")
      );
    } else {
      // Collect all models
      const detectedModels = info.models || [];
      const configuredModels = Object.keys(currentDoc.clis?.[provider]?.models || {});
      const modeModels = currentDoc.modes?.[currentMode]?.[provider]?.models || [];

      const modelIdSet = new Set();
      for (const m of detectedModels) modelIdSet.add(m.id);
      for (const id of configuredModels) modelIdSet.add(id);
      for (const id of modeModels) modelIdSet.add(id);
      const allModelIds = Array.from(modelIdSet);

      const modelList = el("div", { className: "model-list" });

      for (const modelId of allModelIds) {
        const isDetected = detectedModels.some((m) => m.id === modelId);
        const isAllowed = modeModels.includes(modelId);
        const isDefault = currentDoc.modes?.[currentMode]?.[provider]?.default === modelId;
        const hasOverride = isModelOverridden(provider, modelId);
        const isExpanded =
          expandedModel &&
          expandedModel.provider === provider &&
          expandedModel.model === modelId;

        const modelItem = el("div", {
          className: `model-item${hasOverride ? " has-override" : ""}`,
        });

        // Model Checkbox
        const checkInput = el("input", {
          type: "checkbox",
          checked: isAllowed,
          onChange: (e) => toggleModelAllowed(provider, modelId, e.target.checked),
        });

        // Default Radio
        const radioInput = el("input", {
          type: "radio",
          name: `default-${provider}-${currentMode}`,
          checked: isDefault,
          onClick: () => setModelDefault(provider, modelId),
        });

        const defaultLabel = el(
          "label",
          { className: `default-radio-wrap${isDefault ? " active" : ""}`, title: "设为当前模式默认模型" },
          radioInput,
          textNode("默认")
        );

        // Tags
        const tags = [];
        if (!isDetected) {
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

        const row = el(
          "div",
          { className: "model-row" },
          el(
            "div",
            { className: "model-controls" },
            el("label", { className: "model-check-label" }, checkInput, el("span", { className: "model-id" }, modelId)),
            defaultLabel,
            ...tags
          ),
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

// Save members.json
async function saveMembers() {
  if (busy || !dirty || documentError) return;
  busy = true;
  updateControls();
  const feedback = $("save-feedback");
  feedback.textContent = "正在保存…";
  feedback.className = "save-feedback";

  try {
    // Validate options
    for (const [prov, cliConf] of Object.entries(currentDoc.clis || {})) {
      for (const [mId, mConf] of Object.entries(cliConf.models || {})) {
        if (mConf.options) {
          if ("model" in mConf.options || "effort" in mConf.options) {
            throw new Error(`${prov} 模型 ${mId} 的 options 不能包含 model 或 effort 键`);
          }
        }
      }
    }

    await apiPostMembers(currentDoc);
    dirty = false;
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

// Refresh detection
async function refreshDetection() {
  if (busy) return;
  busy = true;
  updateControls();
  showNotice("正在重新检测 CLI 与模型…");
  try {
    const data = await apiGetMembers(true);
    detected = data.detected || {};
    // If user has not modified document, update it
    if (!dirty && data.document) {
      currentDoc = data.document;
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

    if (data.migrated) {
      $("migration-banner").hidden = false;
      const skippedContainer = $("migration-skipped-container");
      if (skippedContainer) {
        skippedContainer.replaceChildren();
        if (Array.isArray(data.migration_skipped) && data.migration_skipped.length > 0) {
          const details = el(
            "details",
            { className: "migration-skipped-details" },
            el("summary", {}, `部分档案未能迁移（${data.migration_skipped.length} 条）`),
            el(
              "ul",
              { className: "migration-skipped-list" },
              ...data.migration_skipped.map((reason) => el("li", {}, reason))
            )
          );
          skippedContainer.appendChild(details);
        }
      }
    }

    if (documentError) {
      $("error-banner").hidden = false;
      $("error-desc").textContent = documentError;
      showNotice(`配置文件损坏: ${documentError}。当前禁止保存。`, true);
    } else {
      showNotice("已载入执行者与模型配置");
    }

    renderCards();
  } catch (err) {
    documentError = err.message;
    $("error-banner").hidden = false;
    $("error-desc").textContent = err.message;
    showNotice(`读取失败: ${err.message}`, true);
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
  const serviceVer = serviceInfo?.version || appStatus.version;
  $("service-version").textContent = serviceVer ? `v${serviceVer}` : "—";

  const isServiceOutdated =
    serviceInfo?.state === "outdated" ||
    (serviceInfo?.version && isVersionOlder(serviceInfo.version, appStatus.version));

  if (serviceInfo?.state === "upgrading") {
    serviceBadge.hidden = false;
    serviceBadge.className = "tag upgrading";
    serviceBadge.textContent = "正在升级…";
    serviceMsg.hidden = false;
    serviceMsg.textContent = "正在从 GitHub 升级本机服务，约需一分钟…";
  } else if (serviceInfo?.state === "failed") {
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
  } else if (serviceInfo?.state === "done") {
    serviceBadge.hidden = false;
    serviceBadge.className = "tag done";
    serviceBadge.textContent = "已是最新";
    serviceMsg.hidden = true;
  } else {
    serviceBadge.hidden = true;
    serviceMsg.hidden = true;
  }

  $("last-checked").textContent = formatTime(appStatus.last_checked);

  // Update action buttons and progress
  const hasUpdate = updateInfo?.state === "available" || isServiceOutdated;
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

  const hasError = updateInfo?.state === "failed" || serviceInfo?.state === "failed";
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
    renderUpdates();
    showNotice("检查更新完成");
  } catch (err) {
    showNotice(`检查更新失败: ${err.message}`, true);
  } finally {
    btnCheck.disabled = false;
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
    btnApply.disabled = false;
  }
}

// Bind top-level event listeners
$("tab-members").addEventListener("click", () => setTab("members"));
$("tab-updates").addEventListener("click", () => setTab("updates"));
$("mode-task").addEventListener("click", () => setMode("task"));
$("mode-discussion").addEventListener("click", () => setMode("discussion"));
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
