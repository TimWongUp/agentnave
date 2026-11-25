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
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "effort", label: "Reasoning", options: [{ value: "low", label: "Low" }, { value: "high", label: "High" }, { value: "max", label: "Max" }] },
        },
        { slug: "claude-old", name: "Claude Old", legacy: true, hidden: false, default: false, reasoning: null },
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
          legacy: false,
          hidden: false,
          default: true,
          reasoning: { id: "reasoningEffort", label: "Reasoning", options: [{ value: "low", label: "Low" }, { value: "xhigh", label: "Extra High" }] },
        },
        { slug: "gpt-hidden", name: "GPT Hidden", legacy: false, hidden: true, default: false, reasoning: null },
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
      models: [{ slug: "glm-5", name: "GLM 5", legacy: false, hidden: false, default: false, reasoning: null }],
    },
    {
      instance: "acpRegistry_other",
      driver: "acpRegistry",
      name: "Other ACP",
      available: true,
      stale: false,
      models: [{ slug: "glm-5", name: "GLM 5", legacy: false, hidden: false, default: false, reasoning: null }],
    },
    {
      instance: "codex_stale",
      driver: "codex",
      name: "Stale Codex",
      available: true,
      stale: true,
      models: [{ slug: "gpt-old", name: "GPT Old", legacy: false, hidden: false, default: true, reasoning: null }],
    },
  ],
};

const SAVED_WORKER = {
  version: 1, enabled: false, random_candidates: false, review: {},
  seats: [{ id: "t3-w", kind: "worker", role: "通用执行", description: "改代码", access: "write", difficulty: "easy", candidates: [{ instance: "codex", model: "gpt-6.2-astra", options: [] }] }],
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
  for (const file of ["provider-health.js", "settings-state.js", "member-models.js", "seat-rows.js", "advanced.js", "decision-seats.js", "t3-seats-image.js", "t3-seats.js"]) {
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
      presets: {
        meetings: "meetings-rule",
        split: "split-rule",
        decision: "decision-rule",
        t3: {
          split: "\\n- T3-BOSS",
          decision: "\\n- T3-DECISION",
          custom: "\\n- T3-BOSS",
        },
        t3_other: { template: "\\n- OTHERS {names}", names: { grok: "Grok CLI", pi: "Pi" }, separator: "、" },
      },
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

  function findEnding(node, suffix) {
    if (!node || typeof node !== "object") return null;
    if (typeof node.dataset?.focusKey === "string" && node.dataset.focusKey.endsWith(suffix)) return node;
    for (const child of [node.children].flat(Infinity)) {
      const found = findEnding(child, suffix);
      if (found) return found;
    }
    return null;
  }

  return {
    run,
    calls,
    t3: (fn) => run(`(${fn})(window.AgentNaveT3Seats)`),
    find: (key) => findKey(run('$("t3-seats-section")'), key)
      || findKey(run('$("t3-review-seats")'), key)
      || findKey(run('$("orchestration-scope")'), key)
      || findKey(run('$("review-scope")'), key),
    findEnding: (suffix) => findEnding(run('$("t3-seats-section")'), suffix),
    section: () => run('$("t3-seats-section")'),
    seatRows: () => {
      const group = run('$("t3-seats-section")').children.find((c) => c.className?.includes("t3-seat-list"));
      return group ? group.children[0].children : [];
    },
    // The user's own cards (custom and image); the fixed ones have no delete button.
    ownRows() { return this.seatRows().filter((row) => this.find(`t3/${row.dataset.cid}/remove`)); },
    rowNamed(name) { return this.seatRows().find((row) => row.children[0].children[0].children[0].children[1].children[0] === name); },
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

test("opens blank the first time: the general and image seats with nothing set, nothing carried over from other seats", async () => {
  const h = makeHarness({
    t3State: { document: null, error: null, t3: SAMPLE_T3 },
    decisionState: {
      document: { version: 1, seats: [{ id: "ds-1", kind: "custom", role: "专项脚本", description: "写构建脚本", access: "write", difficulty: "hard", provider: "codex", model: "gpt-6.2-astra" }] },
      error: null, key_set: false, key_error: null,
    },
    orchestrationDoc: {
      version: 1, preset: "split", explorer: { easy: { provider: "codex", model: "gpt-6.2-astra" } },
      image_generation: { provider: "codex", model: "gpt-6.2-astra" },
    },
    reviewSeats: { "correctness-reviewer": { provider: "codex", model: "gpt-6.2-astra" } },
  });
  h.run("bindAdvanced()");
  await h.run("window.AgentNaveDecisionSeats.load()");
  await h.t3((t) => t.load());
  assert.equal(h.t3((t) => t.changed()), false);
  assert.deepEqual(h.doc(), { version: 1, enabled: false, random_candidates: false, seats: [], review: {} });
  assert.equal(h.seatRows().length, 3);
  assert.ok(["通用探索", "通用执行", "生图工位"].every((name) => h.rowNamed(name)));
  assert.equal(h.ownRows().length, 0, "fixed seats cannot be deleted");
  assert.ok(h.seatRows().every((row) => row.dataset.expanded === "false" && row.dataset.invalid === "false"));
  assert.equal(h.find("t3/enabled").disabled, true);

  // Only what was set is saved, and the blank fixed seats stay listed.
  const cid = h.rowNamed("通用执行").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  h.find(`t3/${cid}/easy/c0/instance`).listeners.change({ target: { value: "codex" } });
  assert.equal(h.t3((t) => t.problem()), null);
  await h.run('$("advanced-save").listeners.click()');
  const post = h.calls.find((c) => c.url === "/app/t3-seats" && c.method === "POST");
  assert.deepEqual(post.body.seats.map((seat) => [seat.kind, seat.difficulty]), [["worker", "easy"]]);
  assert.equal(h.seatRows().length, 3);

  // A specialist is the user's to add and delete, and needs a level before saving.
  h.t3((t) => t.addCard("worker_frontend"));
  assert.equal(h.ownRows().length, 1);
  assert.match(h.t3((t) => t.problem()), /至少要绑定一档/);
  h.find(`t3/${h.ownRows()[0].dataset.cid}/remove`).listeners.click();
  assert.equal(h.t3((t) => t.changed()), false);
});

test("an added fallback starts unset and blocks saving until the user picks it or removes it", async () => {
  const h = makeHarness({ t3State: { document: null, error: null, t3: SAMPLE_T3 } });
  await h.t3((t) => t.load());
  const cid = h.rowNamed("通用执行").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  h.find(`t3/${cid}/easy/c0/instance`).listeners.change({ target: { value: "codex" } });
  h.find(`t3/${cid}/easy/add`).listeners.click();
  assert.deepEqual(h.doc().seats[0].candidates[1], { instance: "", model: "", options: [] });
  assert.match(h.t3((t) => t.problem()), /备选还没选模型/);
  assert.equal(h.find(`t3/${cid}/easy/add`).disabled, true);
  h.find(`t3/${cid}/easy/c1/instance`).listeners.change({ target: { value: "grok" } });
  assert.equal(h.t3((t) => t.problem()), null);
  assert.deepEqual(h.doc().seats[0].candidates[1], { instance: "grok", model: "grok-4", options: [] });
  h.find(`t3/${cid}/easy/add`).listeners.click();
  h.find(`t3/${cid}/easy/c2/remove`).listeners.click();
  assert.equal(h.t3((t) => t.problem()), null);

  // A review role's blank fallback blocks saving too.
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "codex" } });
  h.find("t3/review/security-reviewer/add").listeners.click();
  assert.match(h.t3((t) => t.problem()), /备选还没选模型/);
});

test("round-trips image seats without marking the page dirty, treating difficulty null as absent", async () => {
  const savedDoc = {
    version: 1,
    enabled: false,
    random_candidates: false,
    seats: [
      {
        id: "t3-img-1",
        kind: "image",
        role: "插画",
        description: "手绘插画",
        access: "write",
        difficulty: null,
        candidates: [{ instance: "codex", model: "gpt-6.2-astra", options: [] }],
      },
      {
        id: "t3-img-2",
        kind: "image",
        role: "写实",
        description: "写实照片",
        access: "write",
        candidates: [],
      },
    ],
    review: {},
  };
  const h = makeHarness({ t3State: { document: savedDoc, error: null, t3: SAMPLE_T3 } });
  await h.t3((t) => t.load());
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal(h.seatRows().length, 3, "image seats share the one image card");

  const doc = h.doc();
  assert.equal(doc.seats.length, 2);
  assert.deepEqual(doc.seats[0], {
    id: "t3-img-1", kind: "image", role: "插画", description: "手绘插画", access: "write",
    candidates: [{ instance: "codex", model: "gpt-6.2-astra", options: [] }],
  });
  assert.deepEqual(doc.seats[1], {
    id: "t3-img-2", kind: "image", role: "写实", description: "写实照片", access: "write", candidates: [],
  });

  // Editing a row's specialty marks the page; the description follows the new specialty.
  const cid = h.rowNamed("生图工位").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  h.find(`t3/${cid}/t3r1/role`).listeners.input({ target: { value: "漫画" } });
  assert.equal(h.t3((t) => t.changed()), true);
  const edited = h.doc().seats.find((seat) => seat.id === "t3-img-1");
  assert.equal(edited.role, "漫画");
  assert.equal(edited.description, "漫画");
});

test("adds and removes image rows, rejects empty or repeated specialties, and saves rows still waiting for a model", async () => {
  const h = makeHarness({
    t3State: { document: { version: 1, enabled: false, random_candidates: false, seats: [], review: {} }, error: null, t3: SAMPLE_T3 },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());

  const cid = h.rowNamed("生图工位").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  h.find(`t3/${cid}/add-row`).listeners.click();
  assert.match(h.t3((t) => t.problem()), /缺少专业/);
  h.find(`t3/${cid}/t3r1/role`).listeners.input({ target: { value: "插画" } });
  // A row without a model may be saved and shows as unset.
  assert.equal(h.t3((t) => t.problem()), null);
  h.t3((t) => t.render());
  assert.equal(h.rowNamed("生图工位").children[0].children[1].children[1].children[0], "插画");
  const waiting = h.doc().seats[0];
  assert.equal(waiting.kind, "image");
  assert.deepEqual(waiting.candidates, []);

  // Pick a model for the first row.
  h.find(`t3/${cid}/t3r1/c0/instance`).listeners.change({ target: { value: "codex" } });
  assert.deepEqual(h.doc().seats[0].candidates, [
    { instance: "codex", model: "gpt-6.2-astra", options: [] },
  ]);

  // A second row with no specialty blocks the card; a repeated specialty does too.
  h.find(`t3/${cid}/add-row`).listeners.click();
  assert.match(h.t3((t) => t.problem()), /第 2 行缺少专业/);
  h.find(`t3/${cid}/t3r2/role`).listeners.input({ target: { value: "插画" } });
  assert.match(h.t3((t) => t.problem()), /重复/);
  h.find(`t3/${cid}/t3r2/role`).listeners.input({ target: { value: "写实" } });
  assert.equal(h.t3((t) => t.problem()), null);
  assert.deepEqual(h.doc().seats.map((seat) => seat.role), ["插画", "写实"]);
  assert.equal(new Set(h.doc().seats.map((seat) => seat.id)).size, 2, "each row keeps a unique id");

  // Deleting every row leaves the blank fixed card, with nothing to save.
  h.find(`t3/${cid}/t3r2/remove`).listeners.click();
  h.find(`t3/${cid}/t3r1/remove`).listeners.click();
  assert.equal(h.t3((t) => t.problem()), null);
  assert.equal(h.t3((t) => t.changed()), false);
});

test("counts image rows' instances as saved references, keeping their stale instances listed", async () => {
  const savedDoc = {
    version: 1,
    enabled: false,
    random_candidates: false,
    seats: [
      {
        id: "t3-img-1",
        kind: "image",
        role: "插画",
        description: "手绘插画",
        access: "write",
        candidates: [{ instance: "codex_stale", model: "gpt-old", options: [] }],
      },
      {
        id: "t3-img-2",
        kind: "image",
        role: "写实",
        description: "写实照片",
        access: "write",
        candidates: [{ instance: "claudeAgent", model: "claude-opus-4-6", options: [] }],
      },
    ],
    review: {},
  };
  const h = makeHarness({ t3State: { document: savedDoc, error: null, t3: SAMPLE_T3 } });
  await h.t3((t) => t.load());
  const cid = h.rowNamed("生图工位").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  // The second row does not use the stale instance itself, so it is listed only through the saved reference.
  const instOpts = h.find(`t3/${cid}/t3r2/c0/instance`).children.flat().map((o) => o.attrs.value);
  assert.ok(instOpts.includes("codex_stale"));
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
  // Valid loaded card starts collapsed; expand it to inspect candidate controls.
  assert.equal(h.ownRows()[0].dataset.expanded, "false");
  h.find("t3/t3c1/toggle").listeners.click();
  assert.equal(h.ownRows()[0].dataset.expanded, "true");

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

test("saves and discards via the shared orchestration save bar, updates rule files, and unifies rules preview by mode", async () => {
  const h = makeHarness({
    t3State: { document: SAVED_WORKER, error: null, t3: SAMPLE_T3 },
    orchestrationDoc: { version: 1, preset: "split" },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());
  // No separate rule fold inside T3 section; unified orchestration-text-fold shows mode full text.
  const hasT3RuleFold = () => h.section().children.some((c) => c.children?.[1]?.className === "s-card is-pre");
  assert.equal(hasT3RuleFold(), false);

  h.run('viewMode("split")');
  assert.equal(h.run('$("orchestration-text-fold").hidden'), false);
  assert.equal(h.run('$("orchestration-text").textContent'), "split-rule");

  // Toggle enabled on -> dirty, save bar enabled, and unified rules fold appends split T3 text.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  assert.equal(h.t3((t) => t.changed()), true);
  assert.equal(h.run('$("advanced-badge").hidden'), false);
  assert.equal(h.run('$("advanced-save").disabled'), false);
  assert.equal(hasT3RuleFold(), false);
  assert.equal(h.run('$("orchestration-text").textContent'), "split-rule\n- T3-BOSS");

  // Decision mode uses decision T3 suffix; custom seeds editor without T3 suffix and shows T3 suffix in fold.
  h.run('viewMode("decision")');
  assert.equal(h.run('$("orchestration-text").textContent'), "decision-rule\n- T3-DECISION");
  h.run('viewMode("custom")');
  assert.equal(h.run('$("custom-text").value'), "decision-rule");
  assert.equal(h.run('$("orchestration-text-fold").hidden'), false);
  assert.equal(h.run('$("orchestration-text").textContent'), "- T3-BOSS");

  // Meetings and off never append T3 text.
  h.run('viewMode("meetings")');
  assert.equal(h.run('$("orchestration-text").textContent'), "meetings-rule");
  h.run('viewMode("off")');
  assert.equal(h.run('$("orchestration-text-fold").hidden'), true);

  // Discard restores cleanly and hides custom fold when T3 is off.
  h.run('$("advanced-discard").listeners.click()');
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal(h.find("t3/enabled").checked, false);
  h.run('viewMode("custom")');
  assert.equal(h.run('$("orchestration-text-fold").hidden'), true);
  h.run('viewMode("split")');
  assert.equal(h.run('$("orchestration-text").textContent'), "split-rule");

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
    t3State: { document: SAVED_WORKER, error: null, t3: SAMPLE_T3 },
    postError: "磁盘已满",
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  await h.run('$("advanced-save").listeners.click()');
  assert.match(h.run('$("advanced-feedback").textContent'), /T3 Code：磁盘已满/);
  assert.equal(h.t3((t) => t.changed()), true);
});

test("supports adding/removing up to 4 candidates, reasoning selection to default, custom card validation, and 9 review roles on the review page", async () => {
  const h = makeHarness({
    t3State: { document: { version: 1, enabled: false, random_candidates: false, seats: [], review: {} }, error: null, t3: SAMPLE_T3 },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());

  // Add custom card -> auto-expands; missing specialty blocks save; fill specialty -> missing tier blocks save.
  h.t3((t) => t.addCard("custom"));
  assert.match(h.t3((t) => t.problem()), /缺少专业/);
  const cid = h.ownRows()[0].dataset.cid;
  assert.equal(h.ownRows()[0].dataset.expanded, "true");
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
  for (const [index, instance] of ["claudeAgent_work", "codex", "grok"].entries()) {
    h.find(`t3/${cid}/medium/add`).listeners.click();
    h.find(`t3/${cid}/medium/c${index + 1}/instance`).listeners.change({ target: { value: instance } });
  }
  assert.equal(h.doc().seats[0].candidates.length, 4);
  assert.equal(h.find(`t3/${cid}/medium/add`), null);
  h.find(`t3/${cid}/medium/c1/remove`).listeners.click();
  assert.equal(h.doc().seats[0].candidates.length, 3);

  // The 9 review roles sit on the review page under the shared "评审工位" heading; an edit there marks only that page.
  assert.equal(h.run('$("t3-review-seats")').children[0].children[0], "评审工位");
  assert.equal(h.run('$("t3-review-seats")').children[1].children.length, 9);
  assert.equal(h.t3((t) => t.reviewChanged()), false);
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "codex" } });
  assert.equal(h.doc().review["security-reviewer"].length, 1);
  assert.equal(h.t3((t) => t.reviewChanged()), true);
  h.t3((t) => t.discard());
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "codex" } });
  assert.equal(h.t3((t) => t.seatsChanged()), false);
  h.t3((t) => t.addCard("custom"));
  assert.equal(h.t3((t) => t.seatsChanged()), true);
  const readded = h.ownRows()[0].dataset.cid;
  h.find(`t3/${readded}/role`).listeners.input({ target: { value: "前端" } });
  h.find(`t3/${readded}/medium/c0/instance`).listeners.change({ target: { value: "claudeAgent" } });

  // Turning enabled on then clearing all candidates blocks save with Chinese reason.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  h.find(`t3/${readded}/medium/c0/instance`).listeners.change({ target: { value: "" } });
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "" } });
  h.find(`t3/${readded}/remove`).listeners.click();
  assert.equal(h.doc().enabled, true);
  assert.equal(h.t3((t) => t.hasCandidates()), false);
  assert.match(h.t3((t) => t.problem()), /开启 T3 编排前/);
});

test("shared General / T3 Code segment follows orchestration mode (hidden on off/meetings), keeps review unaffected, and shows badge-dots", async () => {
  // Hidden when T3 is not visible.
  const hidden = makeHarness({
    t3State: { document: null, error: null, t3: { detected: false, instances: [] } },
  });
  hidden.run('viewMode("split")');
  await hidden.t3((t) => t.load());
  assert.equal(hidden.run('$("orchestration-scope").hidden'), true);
  assert.equal(hidden.run('$("review-scope").hidden'), true);
  assert.equal(hidden.run('$("orchestration-general").hidden'), false);
  assert.equal(hidden.run('$("review-seats-group").hidden'), false);
  assert.equal(hidden.run('$("t3-seats-section").hidden'), true);
  assert.equal(hidden.run('$("t3-review-seats").hidden'), true);

  // Visible when T3 is detected: orchestration scope hides on off/meetings while review scope stays visible.
  const h = makeHarness({
    t3State: { document: SAVED_WORKER, error: null, t3: SAMPLE_T3 },
  });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());
  for (const noT3Mode of ["off", "meetings"]) {
    h.run(`viewMode("${noT3Mode}")`);
    assert.equal(h.run('$("orchestration-scope").hidden'), true, noT3Mode);
    assert.equal(h.run('$("orchestration-general").hidden'), false, noT3Mode);
    assert.equal(h.run('$("t3-seats-section").hidden'), true, noT3Mode);
    assert.equal(h.run('$("review-scope").hidden'), false, noT3Mode);
  }

  // On split/decision/custom, orchestration scope appears and defaults to General.
  h.run('viewMode("split")');
  assert.equal(h.run('$("orchestration-scope").hidden'), false);
  assert.equal(h.run('$("review-scope").hidden'), false);
  assert.equal(h.run('$("orchestration-general").hidden'), false);
  assert.equal(h.run('$("t3-seats-section").hidden'), true);
  assert.equal(h.run('$("review-seats-group").hidden'), false);
  assert.equal(h.run('$("t3-review-seats").hidden'), true);

  const dotHidden = (key) => h.find(key).children[1].hidden;
  assert.equal(dotHidden("scope/orchestration/general"), true);
  assert.equal(dotHidden("scope/orchestration/t3"), true);
  assert.equal(h.find("scope/orchestration/t3").children[0], "T3 Code");
  assert.equal(h.find("scope/orchestration/t3").dataset.reserve, "T3 Code · 已开");
  assert.equal(h.find("scope/review/t3").children[0], "T3 Code");

  // Edit T3 task settings while viewing General -> badge-dot appears only on T3 segment in Orchestration,
  // and toggling T3 enabled immediately updates the T3 button label on both pages while keeping reserve width.
  h.find("t3/enabled").listeners.change({ target: { checked: true } });
  assert.equal(dotHidden("scope/orchestration/t3"), false);
  assert.equal(dotHidden("scope/orchestration/general"), true);
  assert.equal(dotHidden("scope/review/t3"), true);
  assert.equal(h.find("scope/orchestration/t3").children[0], "T3 Code · 已开");
  assert.equal(h.find("scope/orchestration/t3").dataset.reserve, "T3 Code · 已开");
  assert.equal(h.find("scope/review/t3").children[0], "T3 Code · 已开");

  // Switch to T3 Code -> switches BOTH Orchestration and Review Settings; T3 dot hides while active.
  h.find("scope/orchestration/t3").listeners.click();
  assert.equal(h.run('$("orchestration-general").hidden'), true);
  assert.equal(h.run('$("t3-seats-section").hidden'), false);
  assert.equal(h.run('$("review-seats-group").hidden'), true);
  assert.equal(h.run('$("t3-review-seats").hidden'), false);
  assert.equal(dotHidden("scope/orchestration/t3"), true);

  // Switching to meetings hides orchestration scope & T3 seats, while Review stays on T3;
  // switching to decision or custom brings orchestration scope & T3 seats right back.
  h.run('viewMode("meetings")');
  assert.equal(h.run('$("orchestration-scope").hidden'), true);
  assert.equal(h.run('$("orchestration-general").hidden'), false);
  assert.equal(h.run('$("t3-seats-section").hidden'), true);
  assert.equal(h.run('$("review-scope").hidden'), false);
  assert.equal(h.run('$("t3-review-seats").hidden'), false);
  for (const t3Mode of ["decision", "custom", "split"]) {
    h.run(`viewMode("${t3Mode}")`);
    assert.equal(h.run('$("orchestration-scope").hidden'), false, t3Mode);
    assert.equal(h.run('$("orchestration-general").hidden'), true, t3Mode);
    assert.equal(h.run('$("t3-seats-section").hidden'), false, t3Mode);
  }

  // Edit General orchestration and General review seats while viewing T3 -> General dots appear on both pages.
  h.run('setBossMode(orchestration.bosses[0], "split")');
  h.run('setReviewSeat("security-reviewer", { provider: "codex", model: "gpt-6.2-astra" })');
  assert.equal(dotHidden("scope/orchestration/general"), false);
  assert.equal(dotHidden("scope/review/general"), false);

  // Switch back to General from Review scope button -> T3 orchestration dot returns; edit T3 review shows T3 review dot.
  h.find("scope/review/general").listeners.click();
  assert.equal(h.run('$("orchestration-general").hidden'), false);
  assert.equal(h.run('$("review-seats-group").hidden'), false);
  assert.equal(dotHidden("scope/orchestration/t3"), false);
  assert.equal(dotHidden("scope/orchestration/general"), true);
  h.find("t3/review/security-reviewer/c0/instance").listeners.change({ target: { value: "codex" } });
  assert.equal(dotHidden("scope/review/t3"), false);

  // Discard clears every badge-dot and restores the T3 Code button label.
  h.run('$("advanced-discard").listeners.click()');
  h.run('viewMode("split")');
  assert.equal(dotHidden("scope/orchestration/t3"), true);
  assert.equal(dotHidden("scope/review/t3"), true);
  assert.equal(h.find("scope/orchestration/t3").children[0], "T3 Code");
  assert.equal(h.find("scope/review/t3").children[0], "T3 Code");
});

test("T3 seat rows summarize primary model display names, preserve expand state, force-open invalid cards, and save collapsed edits", async () => {
  const savedDoc = {
    version: 1,
    enabled: false,
    random_candidates: false,
    seats: [
      {
        id: "t3-1",
        kind: "worker",
        role: "通用执行",
        description: "改代码",
        access: "write",
        difficulty: "easy",
        candidates: [{ instance: "claudeAgent", model: "claude-opus-4-6", options: [] }],
      },
      {
        id: "t3-2",
        kind: "worker",
        role: "通用执行",
        description: "改代码",
        access: "write",
        difficulty: "hard",
        candidates: [
          { instance: "antigravity", model: "gemini-4-neon", options: [] },
          { instance: "codex", model: "gpt-6.2-astra", options: [] },
        ],
      },
    ],
    review: {},
  };
  const h = makeHarness({ t3State: { document: savedDoc, error: null, t3: SAMPLE_T3 } });
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());

  // No outer t3-task-fold; one seat row starting collapsed with model display names (primary only).
  assert.equal(h.find("t3/task-fold"), null);
  assert.equal(h.ownRows().length, 0);
  assert.equal(h.rowNamed("通用执行").dataset.expanded, "false");
  const summaryOf = (row) => row.children[0].children[1].children[1].children[0];
  assert.equal(summaryOf(h.rowNamed("通用执行")), "简单 Claude Opus 4.6 · 困难 Gemini 4 Neon");

  // Expand, change medium tier, collapse again -> edits stay intact on save.
  const cid = h.rowNamed("通用执行").dataset.cid;
  h.find(`t3/${cid}/toggle`).listeners.click();
  assert.equal(h.rowNamed("通用执行").dataset.expanded, "true");
  h.find(`t3/${cid}/medium/c0/instance`).listeners.change({ target: { value: "codex" } });
  assert.equal(h.rowNamed("通用执行").dataset.expanded, "true", "re-render keeps row expanded");
  assert.equal(summaryOf(h.rowNamed("通用执行")), "简单 Claude Opus 4.6 · 中等 GPT 6.2 Astra · 困难 Gemini 4 Neon");
  h.find(`t3/${cid}/toggle`).listeners.click();
  assert.equal(h.rowNamed("通用执行").dataset.expanded, "false");

  await h.run('$("advanced-save").listeners.click()');
  const postCall = h.calls.find((c) => c.url === "/app/t3-seats" && c.method === "POST");
  assert.equal(postCall.body.seats.length, 3, "collapsed seat row saves all 3 tiers");

  // A custom card with no tiers is forced open and shows 未设置 until configured.
  h.t3((t) => t.addCard("custom"));
  const added = h.ownRows()[0];
  assert.equal(added.dataset.expanded, "true");
  assert.equal(added.dataset.invalid, "true");
  assert.equal(summaryOf(added), "未设置");
  h.find(`t3/${added.dataset.cid}/toggle`).listeners.click();
  assert.equal(h.ownRows()[0].dataset.expanded, "true", "invalid card cannot be collapsed");
});

test("the T3 page picks its own bosses: only CLIs both support, following the orchestration page until picked", async () => {
  const h = makeHarness({
    t3State: { document: { ...SAVED_WORKER, enabled: true }, error: null, t3: SAMPLE_T3 },
    orchestrationDoc: { version: 1, preset: "split", apps: { grok: "off" } },
  });
  const group = (id, label) => `{ keys: ["${id}"], ids: ["${id}"], labels: ["${label}"], mark: "${id}" }`;
  h.run(`orchestration.bosses = [${group("claude", "Claude Code")}, ${group("codex", "Codex")}, ${group("grok", "Grok")}, ${group("workbuddy", "WorkBuddy")}];`);
  h.run("bindAdvanced()");
  await h.t3((t) => t.load());
  const chip = (id) => h.find(`t3/boss/${id}`);
  const on = (id) => chip(id).attrs["aria-pressed"] === "true";
  const chosen = () => [...h.t3((t) => t.bosses())];

  // Default: the supported bosses that are on; WorkBuddy, which T3 does not support, has no chip.
  assert.ok(chip("claude") && chip("codex") && chip("grok"));
  assert.equal(chip("workbuddy"), null);
  assert.deepEqual([on("claude"), on("codex"), on("grok")], [true, true, false]);
  assert.deepEqual(chosen(), ["claude", "codex"]);
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal("bosses" in h.doc(), false);
  // Until picked it follows the orchestration page.
  h.run('AN.settings.setBossMode(orchestration.bosses[2], "split")');
  assert.equal(on("grok"), true);
  h.run('AN.settings.setBossMode(orchestration.bosses[2], "off")');

  // Picking saves an explicit list in the supported order, and the Rules preview follows it.
  chip("grok").listeners.click();
  assert.equal(chip("grok").dataset.pending, "true");
  chip("claude").listeners.click();
  assert.deepEqual(h.doc().bosses, ["codex", "grok"]);
  assert.equal(h.t3((t) => t.changed()), true);
  h.run('viewMode("split")');
  assert.equal(h.run('$("orchestration-text").textContent'), "split-rule\n- T3-BOSS\n- OTHERS Grok CLI");
  h.run('AN.settings.setBossMode(orchestration.bosses[0], "off")');
  assert.deepEqual(chosen(), ["codex", "grok"]);
  await h.run('$("advanced-save").listeners.click()');
  const post = h.calls.find((c) => c.url === "/app/t3-seats" && c.method === "POST");
  assert.deepEqual(post.body.bosses, ["codex", "grok"]);
  assert.equal(h.t3((t) => t.changed()), false);
  assert.equal(chip("claude").dataset.pending, "false");

  // Picked once, the orchestration page no longer moves it.
  h.run('AN.settings.setBossMode(orchestration.bosses[0], "split")');
  assert.deepEqual(chosen(), ["codex", "grok"]);
});
