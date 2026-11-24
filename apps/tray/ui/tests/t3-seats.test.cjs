const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { mirrorSettings } = require("./helpers/settings.cjs");

const ROLES = [
  { group: "explorer", name_zh: "通用探索", name_en: "General exploration", scope_zh: "读代码", access: "read", fallback_group: null, legacy_enabled: true },
  { group: "worker", name_zh: "通用执行", name_en: "General execution", scope_zh: "改代码", access: "write", fallback_group: null, legacy_enabled: true },
  { group: "worker_frontend", name_zh: "界面制作", name_en: "Interface work", scope_zh: "做界面", access: "write", fallback_group: "worker", legacy_enabled: true },
  { group: "worker_writing", name_zh: "写作", name_en: "Writing", scope_zh: "写文章", access: "write", fallback_group: "worker", legacy_enabled: false },
];

const SAMPLE_T3 = {
  detected: true,
  instances: [
    {
      instance: "claudeAgent",
      driver: "claudeAgent",
      name: "Claude Code",
      available: true,
      stale: false,
      models: [
        {
          slug: "claude-opus-4-6",
          name: "Claude Opus 4.6",
          aliases: ["opus", "claude-opus"],
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "effort", label: "Reasoning", options: [{ value: "low", label: "Low" }, { value: "high", label: "High" }, { value: "max", label: "Max" }] },
        },
        { slug: "claude-old", name: "Claude Old", aliases: [], legacy: true, hidden: false, default: false, reasoning: null },
      ],
    },
    {
      instance: "claudeAgent_work",
      driver: "claudeAgent",
      name: "Claude (Work)",
      available: true,
      stale: false,
      models: [
        {
          slug: "claude-opus-4-6",
          name: "Claude Opus 4.6",
          aliases: ["opus", "claude-opus"],
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "effort", label: "Reasoning", options: [{ value: "high", label: "High" }] },
        },
      ],
    },
    {
      instance: "codex",
      driver: "codex",
      name: "Codex",
      available: true,
      stale: false,
      models: [
        {
          slug: "gpt-6.2-astra",
          name: "GPT 6.2 Astra",
          aliases: [],
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "reasoningEffort", label: "Reasoning", options: [{ value: "low", label: "Low" }, { value: "xhigh", label: "Extra High" }] },
        },
        { slug: "gpt-hidden", name: "GPT Hidden", aliases: [], legacy: false, hidden: true, default: false, reasoning: null },
      ],
    },
    {
      instance: "antigravity",
      driver: "antigravity",
      name: "Antigravity",
      available: true,
      stale: false,
      models: [
        {
          slug: "gemini-4-neon",
          name: "Gemini 4 Neon",
          aliases: [],
          legacy: false,
          hidden: false,
          default: false,
          reasoning: { id: "thought_level", label: "Thinking Level", options: [{ value: "low", label: "Low" }, { value: "high", label: "High" }, { value: "xhigh", label: "Extra" }] },
        },
      ],
    },
    {
      instance: "grok",
      driver: "grok",
      name: "Grok",
      available: true,
      stale: false,
      models: [
        {
          slug: "grok-4",
          name: "Grok 4",
          aliases: [],
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "reasoningEffort", label: "Reasoning", options: [{ value: "high", label: "High" }] },
        },
      ],
    },
    {
      instance: "acpRegistry_codebuddy_code",
      driver: "acpRegistry",
      name: "CodeBuddy",
      available: true,
      stale: false,
      models: [{ slug: "glm-5", name: "GLM 5", aliases: [], legacy: false, hidden: false, default: false, reasoning: null }],
    },
    {
      instance: "acpRegistry_other",
      driver: "acpRegistry",
      name: "Other ACP",
      available: true,
      stale: false,
      models: [{ slug: "glm-5", name: "GLM 5", aliases: [], legacy: false, hidden: false, default: false, reasoning: null }],
    },
    {
      instance: "codex_stale",
      driver: "codex",
      name: "Stale Codex",
      available: true,
      stale: true,
      models: [{ slug: "gpt-old", name: "GPT Old", aliases: [], legacy: false, hidden: false, default: true, reasoning: null }],
    },
  ],
};

function makeHarness({ t3State, decisionState = null, orchestrationDoc = null, reviewSeats = {}, postError = null, postFailures = [] } = {}) {
  const calls = [];
  const fetch = async (url, options = {}) => {
    const body = options.body ? JSON.parse(options.body) : null;
    calls.push({ url, method: options.method || "GET", body });
    if (url === "/app/t3-seats" && !options.method) {
      return { ok: true, json: async () => structuredClone(t3State) };
    }
    if (url === "/app/t3-seats" && options.method === "POST") {
      if (postError) return { ok: false, status: 400, json: async () => ({ error: postError }) };
      return {
        ok: true,
        json: async () => ({
          saved: true,
          failures: postFailures,
          files: [{ path: "/home/.claude/CLAUDE.md", readers: ["Claude Code"], exists: true, target: Boolean(body.enabled), current: Boolean(body.enabled), blocked: false }],
          bosses: [{ keys: ["claude"], ids: ["claude"], labels: ["Claude Code"], mark: "claude" }],
        }),
      };
    }
    if (url === "/app/decision-seats" && !options.method) {
      return { ok: true, json: async () => structuredClone(decisionState || { document: null, error: null, key_set: false, key_error: null }) };
    }
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  };

  const nodes = new Map();
  const makeNode = (tag) => ({
    tagName: tag,
    attrs: {},
    children: [],
    dataset: {},
    listeners: {},
    options: [],
    value: "",
    checked: false,
    disabled: false,
    hidden: false,
    classList: { toggle() {} },
    setAttribute(k, v) { this.attrs[k] = v; },
    removeAttribute(k) { delete this.attrs[k]; },
    addEventListener(type, fn) { this.listeners[type] = fn; },
    appendChild(child) { this.children.push(child); },
    replaceChildren(...kids) { this.children = kids; },
    querySelectorAll: () => [],
    querySelector: () => null,
  });

  const context = vm.createContext({
    AN: { L: (zh) => zh, shellOS: () => "macos", art: { url: (n) => `/art/${n}` } },
    CSS: { escape: (s) => s },
    structuredClone,
    fetch,
    setTimeout: () => 0,
    document: {
      getElementById: (id) => nodes.get(id) || nodes.set(id, makeNode("div")).get(id),
      createElement: makeNode,
      createTextNode: (text) => text,
    },
    window: { addEventListener() {} },
  });
  const run = (code) => vm.runInContext(code, context);
  const load = (file) => {
    run(readFileSync(`${__dirname}/../${file}`, "utf8"));
    if (context.AN.settings) mirrorSettings(context);
  };
  for (const file of ["provider-health.js", "settings-state.js", "member-models.js", "advanced.js", "decision-seats.js", "t3-seats.js"]) {
    load(file);
  }
  run(`
    updateControls = () => { renderAdvancedBar(); };
    AN.settings.renderCards = () => {};
    AN.settings.renderErrorBanner = () => {};
    refreshDirty = () => {};
    window.AgentNaveSkills = { save: async () => [], discard() {}, changed: () => false };
    detected = { codex: { installed: true, models: [{ id: "gpt-6.2-astra" }] } };
    orchestration = {
      error: null,
      files: [],
      bosses: [{ keys: ["claude"], ids: ["claude"], labels: ["Claude Code"], mark: "claude" }],
      roles: ${JSON.stringify(ROLES)},
      presets: { split: "rule" },
      document: ${JSON.stringify(orchestrationDoc || { version: 1, preset: "off" })},
    };
    reviewSeats = ${JSON.stringify(reviewSeats)};
  `);

  function findKey(node, key) {
    if (!node || typeof node !== "object") return null;
    if (node.dataset?.focusKey === key) return node;
    for (const child of [node.children].flat(Infinity)) {
      const found = findKey(child, key);
      if (found) return found;
    }
    return null;
  }

  return {
    run,
    calls,
    t3: (fn) => run(`(${fn})(window.AgentNaveT3Seats)`),
    find: (key) => findKey(run('$("t3-seats-section")'), key),
    section: () => run('$("t3-seats-section")'),
    doc: () => JSON.parse(run("JSON.stringify(window.AgentNaveT3Seats.document())")),
  };
}

test("hides entire T3 Code block when detected=false and no saved document", async () => {
  const h = makeHarness({
    t3State: { document: null, error: null, t3: { detected: false, instances: [] } },
  });
  await h.t3((t) => t.load());
  assert.equal(h.t3((t) => t.visible()), false);
  assert.equal(h.section().hidden, true);
  assert.equal(h.section().children.length, 0);
  assert.equal(h.t3((t) => t.changed()), false);
});

test("prefills from decision-seats (skipping image seats) and maps providers, aliases, antigravity thought_level, codebuddy, and review", async () => {
  const decisionSeats = [
    {
      id: "ds-1",
      kind: "explorer",
      role: "通用探索",
      description: "读代码",
      access: "read",
      difficulty: "easy",
      provider: "claude",
      model: "claude-opus", // alias match; unbound account expands to ALL available Claude instances
      effort: "high",
      fallbacks: [
        { provider: "claude", account: "a1", model: "claude-opus-4-6", effort: "high" }, // named account skipped
        { provider: "codex", model: "gpt-6.2-astra", effort: "xhigh" },
        { provider: "antigravity", model: "gemini-4-neon-xhigh" }, // suffix split -> thought_level
        { provider: "grok", model: "grok-4", effort: "high" }, // capped at 4 candidates total
      ],
    },
    {
      id: "ds-2",
      kind: "custom",
      role: "专项脚本",
      description: "写构建脚本",
      access: "write",
      difficulty: "hard",
      provider: "codebuddy",
      model: "glm-5",
      effort: "high", // codebuddy has no reasoning options -> options: []
      fallbacks: [{ provider: "deepseek", model: "v4" }], // unsupported CLI skipped
    },
    {
      id: "ds-3",
      kind: "custom",
      role: "未匹配工位",
      description: "只用 DeepSeek",
      access: "write",
      difficulty: "medium",
      provider: "deepseek",
      model: "v4",
    },
    {
      id: "ds-img",
      kind: "image",
      role: "插画",
      description: "插画",
      access: "write",
      provider: "codex",
      model: "gpt-6.2-astra",
    },
  ];
  const reviewSeats = {
    "correctness-reviewer": {
      provider: "antigravity",
      model: "gemini-4-neon-max", // max not in options -> matched model without option
      fallbacks: [{ provider: "grok", model: "grok-4", effort: "high" }],
    },
  };
  const h = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
    decisionState: { document: { version: 1, seats: decisionSeats }, error: null, key_set: false, key_error: null },
    reviewSeats,
  });
  await h.run("window.AgentNaveDecisionSeats.load()");
  await h.t3((t) => t.load());

  assert.equal(h.t3((t) => t.visible()), true);
  // First-time prefill does not auto-save, does not enable, and does not falsely dirty the page.
  assert.equal(h.calls.some((c) => c.method === "POST"), false);
  assert.equal(h.t3((t) => t.changed()), false);
  const doc = h.doc();
  assert.equal(doc.enabled, false);
  assert.equal(doc.random_candidates, false);
  assert.equal(doc.seats.length, 3, "image seat excluded, unmatched task seat kept empty");
  assert.ok(doc.seats.every((s) => !["ds-1", "ds-2", "ds-3"].includes(s.id)), "fresh ids generated");

  // Seat 1: claude expanded to 2 instances + codex + antigravity (capped at 4; grok cut off).
  assert.deepEqual(doc.seats[0].candidates, [
    { instance: "claudeAgent", model: "claude-opus-4-6", options: [{ id: "effort", value: "high" }] },
    { instance: "claudeAgent_work", model: "claude-opus-4-6", options: [{ id: "effort", value: "high" }] },
    { instance: "codex", model: "gpt-6.2-astra", options: [{ id: "reasoningEffort", value: "xhigh" }] },
    { instance: "antigravity", model: "gemini-4-neon", options: [{ id: "thought_level", value: "xhigh" }] },
  ]);
  // Seat 2: only acpRegistry_codebuddy* matched (acpRegistry_other skipped), no effort carried.
  assert.equal(doc.seats[1].description, "写构建脚本");
  assert.deepEqual(doc.seats[1].candidates, [
    { instance: "acpRegistry_codebuddy_code", model: "glm-5", options: [] },
  ]);
  // Seat 3: unmatched seat stays empty.
  assert.deepEqual(doc.seats[2].candidates, []);
  // Review: antigravity -max dropped unsupported option, grok fallback carried reasoningEffort.
  assert.deepEqual(doc.review["correctness-reviewer"], [
    { instance: "antigravity", model: "gemini-4-neon", options: [] },
    { instance: "grok", model: "grok-4", options: [{ id: "reasoningEffort", value: "high" }] },
  ]);
});

test("prefills from fixed seats when decision-seats is absent, and gives two starter medium seats when neither exists", async () => {
  // Fixed seats fallback.
  const fixed = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
    orchestrationDoc: {
      version: 1,
      preset: "split",
      enabled_roles: ["explorer"],
      explorer: { easy: { provider: "codex", model: "gpt-6.2-astra", effort: "low" } },
    },
  });
  await fixed.t3((t) => t.load());
  const fixedDoc = fixed.doc();
  assert.equal(fixedDoc.seats.length, 1);
  assert.equal(fixedDoc.seats[0].kind, "explorer");
  assert.equal(fixedDoc.seats[0].difficulty, "easy");
  assert.deepEqual(fixedDoc.seats[0].candidates, [
    { instance: "codex", model: "gpt-6.2-astra", options: [{ id: "reasoningEffort", value: "low" }] },
  ]);

  // Empty everything -> two starter medium seats using each available non-stale instance's default model (max 4).
  const starter = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
  });
  await starter.t3((t) => t.load());
  const starterDoc = starter.doc();
  assert.deepEqual(
    starterDoc.seats.map((s) => [s.kind, s.role, s.access, s.difficulty]),
    [
      ["explorer", "通用探索", "read", "medium"],
      ["worker", "通用执行", "write", "medium"],
    ],
  );
  assert.equal(starterDoc.seats[0].candidates.length, 4);
  assert.deepEqual(
    starterDoc.seats[0].candidates.map((c) => [c.instance, c.model]),
    [
      ["claudeAgent", "claude-opus-4-6"],
      ["claudeAgent_work", "claude-opus-4-6"],
      ["codex", "gpt-6.2-astra"],
      ["antigravity", "gemini-4-neon"],
    ],
  );
});

test("switch cannot open without candidates, dropdown hides legacy/hidden/stale, and unlisted saved entries stay marked", async () => {
  const savedDoc = {
    version: 1,
    enabled: false,
    random_candidates: false,
    seats: [
      {
        id: "t3-saved",
        kind: "custom",
        role: "架构",
        description: "架构",
        access: "read",
        difficulty: "hard",
        candidates: [
          { instance: "removed_inst", model: "missing-model", options: [{ id: "effort", value: "turbo" }] },
          { instance: "codex", model: "gpt-hidden", options: [] },
        ],
      },
    ],
    review: {},
  };
  const h = makeHarness({
    t3State: { document: savedDoc, error: null, t3: SAMPLE_T3 },
  });
  await h.t3((t) => t.load());

  const instSel = h.find("t3/t3c1/hard/c0/instance");
  const instOpts = instSel.children.flat().map((o) => [o.attrs.value, o.children[0]]);
  // Stale instance not referenced by saved doc is excluded; unlisted saved instance is kept and tagged.
  assert.ok(!instOpts.some(([v]) => v === "codex_stale"));
  assert.ok(instOpts.some(([v, text]) => v === "removed_inst" && text.includes("T3 未列出")));
  // Unlisted saved model and reasoning option stay preserved and tagged.
  const modelSel = h.find("t3/t3c1/hard/c0/model");
  assert.ok(modelSel.children.flat().some((o) => o.attrs.value === "missing-model" && o.children[0].includes("T3 未列出")));
  const reasonSel = h.find("t3/t3c1/hard/c0/reasoning");
  assert.equal(reasonSel.children.flat()[0].children[0], "默认");
  assert.ok(reasonSel.children.flat().some((o) => o.attrs.value === "turbo" && o.children[0].includes("T3 未列出")));

  // Hidden model is shown on fallback row because it is currently selected, while legacy model on claudeAgent is hidden unless selected.
  const fbModel = h.find("t3/t3c1/hard/c1/model");
  assert.ok(fbModel.children.flat().some((o) => o.attrs.value === "gpt-hidden"));
  // Models without reasoning options omit the reasoning select altogether.
  assert.equal(h.find("t3/t3c1/hard/c1/reasoning"), null);

  // Empty catalog disables the enable switch.
  const empty = makeHarness({
    t3State: { document: { version: 1, enabled: false, random_candidates: false, seats: [], review: {} }, error: null, t3: SAMPLE_T3 },
  });
  await empty.t3((t) => t.load());
  assert.equal(empty.find("t3/enabled").disabled, true);
});

test("saves and discards via the shared orchestration save bar and updates rule files", async () => {
  const h = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());

  // Toggle enabled on -> dirty, save bar enabled -> save posts /app/t3-seats and refreshes rule files.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  assert.equal(h.t3((t) => t.changed()), true);
  assert.equal(h.run('$("advanced-badge").hidden'), false);
  assert.equal(h.run('$("advanced-save").disabled'), false);

  // Discard restores cleanly.
  h.run('$("advanced-discard").listeners.click()');
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal(h.find("t3/enabled").checked, false);

  // Toggle enabled + random on and save.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  h.find("t3/random").listeners.change({ target: { checked: true } });
  await h.run('$("advanced-save").listeners.click()');

  const postCall = h.calls.find((c) => c.url === "/app/t3-seats" && c.method === "POST");
  assert.ok(postCall);
  assert.equal(postCall.body.enabled, true);
  assert.equal(postCall.body.random_candidates, true);
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal(h.run('$("advanced-feedback").textContent'), "设置已保存");
  assert.equal(h.run("orchestration.files[0].current"), true);
});

test("surfaces save errors without losing draft edits", async () => {
  const h = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
    postError: "磁盘已满",
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  await h.run('$("advanced-save").listeners.click()');
  assert.match(h.run('$("advanced-feedback").textContent'), /T3 Code：磁盘已满/);
  assert.equal(h.t3((t) => t.changed()), true);
});

test("supports adding/removing up to 4 candidates, reasoning selection to default, custom card validation, and 9 review roles in fold", async () => {
  const h = makeHarness({
    t3State: { document: { version: 1, enabled: false, random_candidates: false, seats: [], review: {} }, error: null, t3: SAMPLE_T3 },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());

  // Add custom card -> missing specialty blocks save; fill specialty -> missing tier blocks save.
  h.t3((t) => t.addCard("custom"));
  assert.match(h.t3((t) => t.problem()), /缺少专业/);
  const cid = h.section().children[1].children[1].children[0].children[0].children[0].children[0].dataset.focusKey.split("/")[1];
  h.find(`t3/${cid}/role`).listeners.input({ target: { value: "前端" } });
  assert.match(h.t3((t) => t.problem()), /至少要绑定一档/);

  // Pick claudeAgent on medium -> reasoning dropdown appears with 默认 first; select high then back to 默认.
  h.find(`t3/${cid}/medium/c0/instance`).listeners.change({ target: { value: "claudeAgent" } });
  assert.equal(h.t3((t) => t.problem()), null);
  const reason = () => h.find(`t3/${cid}/medium/c0/reasoning`);
  assert.equal(reason().children.flat()[0].children[0], "默认");
  reason().listeners.change({ target: { value: "high" } });
  assert.deepEqual(h.doc().seats[0].candidates[0].options, [{ id: "effort", value: "high" }]);
  reason().listeners.change({ target: { value: "" } });
  assert.deepEqual(h.doc().seats[0].candidates[0].options, []);

  // Add 3 fallbacks -> capped at 4, then remove one.
  h.find(`t3/${cid}/medium/add`).listeners.click();
  h.find(`t3/${cid}/medium/add`).listeners.click();
  h.find(`t3/${cid}/medium/add`).listeners.click();
  assert.equal(h.doc().seats[0].candidates.length, 4);
  assert.equal(h.find(`t3/${cid}/medium/add`), null);
  h.find(`t3/${cid}/medium/c1/remove`).listeners.click();
  assert.equal(h.doc().seats[0].candidates.length, 3);

  // Review fold contains all 9 review roles and preserves open state on edit.
  const reviewFold = h.section().children.at(-1);
  assert.equal(reviewFold.tagName, "details");
  assert.equal(reviewFold.children[1].children.length, 9);
  reviewFold.listeners.toggle({ target: { open: true } });
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "codex" } });
  assert.equal(h.doc().review["security-reviewer"].length, 1);
  assert.equal(h.section().children.at(-1).attrs.open, "");

  // Turning enabled on then clearing all candidates blocks save with Chinese reason.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  h.find(`t3/${cid}/medium/c0/instance`).listeners.change({ target: { value: "" } });
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "" } });
  h.t3((t) => t.addCard("explorer")); // ensure problem checks candidate gate after cards
  // Remove empty cards so only enabled-without-candidates triggers.
  const remaining = h.doc();
  assert.equal(remaining.enabled, true);
  assert.equal(h.t3((t) => t.hasCandidates()), false);
});

