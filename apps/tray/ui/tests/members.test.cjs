const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function editor(fetch = async () => { throw new Error("unexpected request"); }, os = "macos") {
  const listeners = {}, controls = new Map();
  const control = (id) => {
    if (!controls.has(id)) controls.set(id, { options: [], value: "8", classList: { toggle() {} },
      append(node) { this.options.push(node); } });
    return controls.get(id);
  };
  const context = vm.createContext({
    AN: { shellOS: () => os }, structuredClone, fetch, setTimeout: () => 0,
    document: { getElementById: control },
    window: { addEventListener: (name, fn) => { listeners[name] = fn; } },
  });
  const run = (code) => vm.runInContext(code, context);
  for (const file of ["settings-state.js", "member-models.js"]) {
    run(readFileSync(`${__dirname}/../${file}`, "utf8"));
  }
  run(`
    updateControls = () => {};
    function renderCards() {}
    function renderErrorBanner() {}
    detected = { codex: { installed: true } };
    currentDoc = { version: 1, clis: {}, modes: {
      task: { codex: { models: [], overrides: {} } },
      discussion: { codex: { models: [], overrides: {} } }
    } };
    markSaved();
  `);
  return { run, listeners, doc: () => JSON.parse(run("JSON.stringify(currentDoc)")) };
}

test("Devin Fusion models stay out of choices even when saved previously", () => {
  const app = editor();
  app.run(`
    detected.devin = { models: [{ id: "swe-2-medium" }, { id: "fusion-gpt-sidekick-swe" }] };
    currentDoc.clis.devin = { models: { "fusion-gpt-sidekick-swe": {}, "swe-2-medium": {} },
      allowed_models: ["fusion-gpt-sidekick-swe", "swe-2-medium"] };
    currentDoc.modes.task.devin = { models: ["fusion-gpt-sidekick-swe"] };
  `);
  assert.deepEqual(Array.from(app.run('getAllModelIds("devin")')), ["swe-2-medium"]);
  assert.equal(app.doc().clis.devin.allowed_models[0], "fusion-gpt-sidekick-swe");
  app.run("AN.L = (zh) => zh");
  app.run(readFileSync(`${__dirname}/../member-cards.js`, "utf8"));
  assert.equal(app.run('poolSummary("devin", getAllModelIds("devin"), shownPool("devin"))'), "1 个：swe-2-medium");
  // A hidden model alone is no choice at all.
  app.run('currentDoc.clis.devin.allowed_models = ["fusion-gpt-sidekick-swe"]');
  assert.ok(app.run("configurationErrors().some((e) => /至少选一个模型/.test(e.message))"));
});

// The settings page's orchestration half, with a DOM just deep enough to read rendered rows.
function advancedEditor(fetch) {
  const app = editor(fetch);
  app.run(`
    AN.L = (zh) => zh;
    refreshDirty = () => {};
    const nodes = new Map();
    const node = (tag) => ({
      tagName: tag, attrs: {}, children: [], dataset: {}, listeners: {}, classList: { toggle() {} },
      setAttribute(key, value) { this.attrs[key] = value; }, removeAttribute(key) { delete this.attrs[key]; },
      addEventListener(type, fn) { this.listeners[type] = fn; }, appendChild(child) { this.children.push(child); },
      replaceChildren(...kids) { this.children = kids; }, querySelectorAll: () => [], querySelector: () => null,
    });
    document.createElement = node;
    document.createTextNode = (text) => text;
    document.getElementById = (id) => nodes.get(id) || nodes.set(id, node("div")).get(id);
    currentDoc.modes.task.codex = { models: ["chat-model"], overrides: {}, default: "chat-model" };
  `);
  app.run(readFileSync(`${__dirname}/../advanced.js`, "utf8"));
  return app;
}

test("the optional image seat picks, clears, discards, saves and reloads as one flat seat", async () => {
  let saved = { version: 1, preset: "split" }, members;
  const app = advancedEditor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: saved, error: null, bosses: [], presets: { split: "rule", image_generation: "\nimage-rule" }, files: [] }) };
    }
    if (url === "/app/orchestration") saved = body;
    else members = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  });
  await app.run("loadOrchestration()");
  const row = () => app.run('$("orchestration-roles").children.at(-1)');
  const pick = (value) => row().children[1].listeners.change({ target: { value } });
  const seat = { provider: "codex", model: "chat-model" };

  assert.equal(row().children[0].children[0].children[0], "生图（可选）");
  assert.equal(row().children[0].children.length, 1);
  assert.equal(row().children[1].children[0].children[0], "未配置");
  assert.equal(row().children[2].attrs["aria-label"], "生图的对话模型");
  app.run('viewMode("off")');
  assert.equal(app.run('$("orchestration-roles").hidden'), false);
  assert.equal(app.run('$("orchestration-roles").children.length'), 1);
  assert.equal(row().children[0].children[0].children[0], "生图（可选）");
  app.run('viewMode("split")');

  assert.equal(app.run('orchestrationPresetText("split")'), "rule");
  pick("codex");
  assert.equal(app.run('orchestrationPresetText("split")'), "rule\nimage-rule");
  assert.equal(JSON.stringify(app.run("orchestrationWanted().image_generation")), JSON.stringify(seat));
  pick("");
  assert.equal(app.run('"image_generation" in orchestrationWanted()'), false);
  assert.equal(app.run('orchestrationPresetText("split")'), "rule");
  assert.equal(app.run("orchestrationChanged()"), false);
  pick("codex");
  app.run("discardAdvanced()");
  assert.equal(app.run("orchestrationChanged()"), false);

  pick("codex");
  await app.run("saveOrchestration()");
  assert.equal(JSON.stringify(saved.image_generation), JSON.stringify(seat));
  assert.ok(members.clis.codex.models["chat-model"]);
  await app.run("loadOrchestration()");
  assert.equal(row().children[2].children.find((option) => option.attrs.value === "chat-model").attrs.selected, "");

  pick("");
  await app.run("saveOrchestration()");
  assert.equal(Object.hasOwn(saved, "image_generation"), false);
  app.run('delete currentDoc.modes.task.codex; renderOrchestration()');
  assert.ok(row().children[1].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled),
    "orchestration can use a confirmed CLI even when solo tasks are off");
  app.run('delete currentDoc.modes.discussion.codex; renderOrchestration()');
  assert.equal(row().children[1].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled), false,
    "removing both scenes disables the CLI despite its historical model settings");
});

function rolesEditor(document, roles = JSON.parse(readFileSync(`${__dirname}/../../../../src/agentnave/orchestration_roles.json`, "utf8"))) {
  const state = { saved: document, members: null };
  const presets = {
    split: "whole", parallel: "whole+", image_generation: "\nimage",
    role_base: { parallel: "base+" },
    split_parts: { base: "base", meetings_asked: "\nasked", meetings_own: "\nown" },
    role_sections: Object.fromEntries((roles || []).map((role) => [role.group, `\n${role.group}`])),
  };
  const app = advancedEditor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: state.saved, error: null, bosses: [], presets, roles, files: [] }) };
    }
    if (url === "/app/orchestration") state.saved = body;
    else state.members = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  });
  const sections = () => app.run('$("orchestration-roles").children').filter((box) => box.className === "orchestration-section");
  const groups = () => sections().flatMap((box) => box.children.slice(1));
  const group = (name) => groups().find((box) => box.children[0].children[0].children[1].children[0] === name);
  const toggle = (name, checked) => group(name).children[0].children[0].children[0].listeners.change({ target: { checked } });
  const split = (name, checked) => group(name).children[0].children[1].children[0].listeners.change({ target: { checked } });
  return { app, state, sections, groups, group, toggle, split };
}

test("custom editor tracks generated role and image lines without changing user text", async () => {
  const { app, state, toggle } = rolesEditor({ version: 1, preset: "custom", enabled_roles: [], custom: "worker_writing\n用户正文" });
  await app.run("loadOrchestration()");
  assert.equal(app.run('$("custom-text").value'), "用户正文");
  assert.equal(app.run("orchestrationChanged()"), false);
  toggle("写作", true);
  assert.equal(app.run('$("custom-text").value'), "用户正文\nworker_writing");
  toggle("写作", false);
  assert.equal(app.run('$("custom-text").value'), "用户正文");
  assert.equal(app.run("orchestrationChanged()"), false);
  app.run('setOrchestrationSeat("image_generation", null, {provider:"codex",model:"chat-model"})');
  assert.equal(app.run('$("custom-text").value'), "用户正文\nimage");
  await app.run("saveOrchestration()");
  assert.equal(state.saved.custom, "用户正文\nimage");
  await app.run("loadOrchestration()");
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("role switches default like old files, round-trip through save, and keep bindings when off", async () => {
  const binding = { provider: "codex", model: "chat-model" };
  const { app, state, groups, group, toggle } = rolesEditor({ version: 1, preset: "split", worker_writing: { easy: binding } });
  await app.run("loadOrchestration()");

  assert.equal(groups().length, 7);
  assert.equal(group("通用探索").children.length, 3);
  assert.equal(group("写作").children.length, 1, "a new role starts off with its seats out of sight");
  assert.equal(group("写作").children[0].children.length, 1, "and without its difficulty switch");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked\nexplorer\nworker\nworker_frontend");

  toggle("写作", true);
  assert.equal(group("写作").children.length, 3, "turning a role on shows its seats, split while only one is set");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked\nexplorer\nworker\nworker_frontend\nworker_writing");
  assert.equal(app.run("orchestrationChanged()"), true);
  toggle("写作", false);
  assert.equal(app.run("orchestrationChanged()"), false, "back to what is saved leaves nothing unsaved");
  assert.equal(app.run('"enabled_roles" in orchestrationWanted()'), false);

  for (const name of ["通用探索", "通用执行", "界面制作"]) toggle(name, false);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked");
  await app.run("saveOrchestration()");
  assert.deepEqual(state.saved.enabled_roles, []);
  assert.deepEqual(state.saved.worker_writing, { easy: binding }, "turning off keeps the binding");
  assert.equal(state.members, null, "a binding behind a switch stays out of the member file");

  await app.run("loadOrchestration()");
  assert.equal(group("通用探索").children.length, 1);
  toggle("写作", true);
  await app.run("saveOrchestration()");
  assert.ok(state.members.clis.codex.models["chat-model"], "turning a role on lists the binding it brings back");
  assert.deepEqual(state.saved.enabled_roles, ["worker_writing"]);
});

test("a global off shows only the image seat, and no catalog means no role is guessed", async () => {
  const off = rolesEditor({ version: 1, preset: "off" });
  await off.app.run("loadOrchestration()");
  assert.equal(off.app.run('$("orchestration-roles").children.length'), 1);
  off.app.run('viewMode("split")');
  assert.equal(off.groups().length, 7);

  const older = rolesEditor({ version: 1, preset: "split", enabled_roles: ["worker"] }, null);
  await older.app.run("loadOrchestration()");
  assert.equal(older.app.run('$("orchestration-roles").children.length'), 1);
  assert.equal(older.app.run("orchestrationChanged()"), false);
});

test("general and specialist roles are grouped, and no role borrows another role's seat", async () => {
  const outside = { provider: "codex", model: "gone" };
  const worker = { provider: "codex", model: "chat-model" };
  const { app, sections, group, toggle } = rolesEditor({
    version: 1, preset: "split", enabled_roles: ["worker", "worker_frontend", "worker_writing"],
    worker: { easy: worker }, explorer_web: { hard: outside },
  });
  await app.run("loadOrchestration()");
  app.run('currentDoc.clis.codex = { allowed_models: ["chat-model"], models: {} }');

  const title = (box) => box.children[0].children[0];
  const name = (box) => box.children[0].children[0].children[1].children[0];
  assert.deepEqual(Array.from(sections(), title), ["通用", "专项"]);
  assert.deepEqual(Array.from(sections()[0].children.slice(1), name), ["通用探索", "通用执行"]);
  assert.equal(sections()[1].children.slice(1).map(name).includes("界面制作"), true);
  const unset = (row) => row.children[1].children[0].children[0];
  assert.equal(group("通用执行").children[0].children.length, 1, "general roles have no difficulty switch");
  assert.equal(group("界面制作").children.length, 2, "an unset specialist shows one shared row");
  assert.equal(unset(group("界面制作").children[1]), "未配置", "the interface role no longer borrows");

  assert.equal(app.run("seatConflicts().length"), 0, "a role turned off keeps a stale binding quietly");
  toggle("网页调研", true);
  assert.equal(app.run("seatConflicts().length"), 1);
  assert.match(app.run("seatConflictText(seatConflicts()[0])"), /网页调研 · 困难 用的 gone/);
  assert.ok(app.run("blockedSeat()"), "and blocks saving until rebound, without repairing it");
  assert.deepEqual(JSON.parse(app.run("JSON.stringify(orchestrationWanted().explorer_web)")), { hard: outside });
});

test("a specialist picks one seat for both difficulties until it is split", async () => {
  const { app, group, split } = rolesEditor({ version: 1, preset: "split", enabled_roles: ["worker_writing"] });
  await app.run("loadOrchestration()");
  const seats = () => JSON.parse(app.run("JSON.stringify(orchestrationWanted().worker_writing ?? null)"));
  const pick = (row, value) => group("写作").children[row].children[1].listeners.change({ target: { value } });
  const shared = { provider: "codex", model: "chat-model" };

  pick(1, "codex");
  assert.deepEqual(seats(), { easy: shared, hard: shared });
  assert.equal(group("写作").children.length, 2, "the same seat on both stays one row");

  split("写作", true);
  assert.equal(group("写作").children.length, 3);
  assert.equal(app.run("orchestrationChanged()"), true);
  pick(1, "");
  assert.deepEqual(seats(), { hard: shared });
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "joining keeps the hard seat");
  assert.equal(group("写作").children.length, 2);

  split("写作", true);
  pick(2, "");
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "or the easy one when hard is unset");
  pick(1, "");
  assert.equal(seats(), null);
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("one review model fills only the review roles left unset, and follows until saved", async () => {
  const app = advancedEditor();
  app.run('reviewSeats = { "security-reviewer": { provider: "codex", model: "own" } }; renderReviewSeats()');
  const shown = () => JSON.parse(app.run("JSON.stringify(seatsShown())"));
  const pick = (value) => app.run('$("review-seats").children[0]').children[1].listeners.change({ target: { value } });
  const uniform = { provider: "codex", model: "chat-model" };

  assert.equal(app.run('$("review-seats").children[0].children[0].children[0].children[0]'), "统一评审模型");
  pick("codex");
  assert.deepEqual(shown()["security-reviewer"], { provider: "codex", model: "own" }, "a role set on its own is kept");
  assert.deepEqual(shown()["correctness-reviewer"], uniform);
  assert.equal(Object.hasOwn(shown(), "correctness-reviewer-easy"), false, "unset, it already follows correctness");
  app.run('setReviewSeat("architecture-reviewer", { provider: "codex", model: "picked" })');
  pick("");
  assert.deepEqual(Object.keys(shown()).sort(), ["architecture-reviewer", "security-reviewer"]);
  assert.equal(app.run("seatsChanged()"), true);
  app.run("discardAdvanced()");
  assert.equal(app.run("seatsChanged()"), false);
});

test("editing a full list keeps both scenes explicit and newly detected CLIs off", () => {
  const app = editor();
  app.run('setProviderEnabled("codex", false); setProviderEnabled("codex", true)');
  app.run('detected.claude = { installed: true }');
  assert.equal(app.run('slot("task", "claude").enabled'), false);
  assert.equal(app.run('slot("discussion", "claude").enabled'), false);
  app.run('setProviderEnabled("claude", true)');
  for (const scene of ["task", "discussion"]) {
    assert.deepEqual(Object.keys(app.doc().modes[scene]).sort(), ["claude", "codex"]);
  }
  assert.equal(Object.hasOwn(app.doc(), "defaults"), false);
});

test("saving handwritten open scenes moves defaults into the explicit CLI lists", async () => {
  let sent;
  const app = editor(async (_url, options) => {
    sent = JSON.parse(options.body);
    return { ok: true, json: async () => ({ saved: true }) };
  });
  app.run(`currentDoc = {
    version: 1, clis: { grok: { models: { saved: {} } } }, modes: {},
    defaults: { task: { grok: "saved" } }
  }`);
  assert.equal(app.run('slot("task", "grok").model'), "saved");
  await app.run("apiPostMembers(currentDoc)");
  assert.deepEqual(sent.modes.task, {
    codex: { models: [], overrides: {} },
    grok: { models: [], overrides: {}, default: "saved" },
  });
  assert.deepEqual(sent.modes.discussion, { codex: { models: [], overrides: {} } });
  assert.equal(Object.hasOwn(sent, "defaults"), false);
});

test("host events reload clean members and preserve edits made before or during the read", async () => {
  let answer, requests = 0;
  const app = editor(() => {
    requests++;
    return new Promise((resolve) => { answer = (document) => resolve({ ok: true, json: async () => ({ document }) }); });
  });
  const hosts = (at) => ({ detail: { hosts: [{ id: "codex", status: "connected", last_action: { at } }] } });
  const reloaded = app.listeners["agentnave:hosts"](hosts("1"));
  const document = app.doc();
  document.modes.task.claude = { models: [], overrides: {} };
  answer(document);
  await reloaded;
  assert.equal(app.run('slot("task", "claude").enabled'), true);

  const racing = app.listeners["agentnave:hosts"](hosts("2"));
  app.run('setProviderEnabled("codex", false)');
  answer(document);
  await racing;
  assert.equal(app.run('slot("task", "codex").enabled'), false);
  // With edits unsaved a later event reads again, yet only merges CLIs the file gained.
  const dirtyRead = app.listeners["agentnave:hosts"](hosts("3"));
  answer(document);
  await dirtyRead;
  assert.equal(requests, 3);
  assert.equal(app.run('slot("task", "codex").enabled'), false);
});

test("a connection made while employee edits are unsaved joins them instead of being lost", async () => {
  // The second start is a handwritten file whose scenes are still open.
  for (const open of [false, true]) {
    let file;
    const app = editor(async () => ({ ok: true, json: async () => ({ document: file }) }));
    if (open) app.run("currentDoc = { version: 1, clis: {}, modes: {} }; markSaved();");
    app.run('setProviderEnabled("codex", false)');
    file = { version: 1, clis: {}, modes: {} };
    for (const scene of ["task", "discussion"]) {
      file.modes[scene] = { codex: { models: [], overrides: {} }, grok: { models: [], overrides: {} } };
    }
    await app.listeners["agentnave:hosts"]({ detail: { hosts: [{ id: "grok", status: "connected", last_action: { at: "1" } }] } });
    assert.equal(app.run('slot("task", "grok").enabled'), true);
    assert.equal(app.run('slot("task", "codex").enabled'), false);
    assert.equal(app.run("dirty"), true);
  }
});

test("turning a CLI off and on again leaves nothing unsaved", () => {
  const app = editor();
  app.run(`
    detected.claude = { installed: true };
    for (const scene of SCENE_IDS) currentDoc.modes[scene] = { codex: { models: [], overrides: {} }, claude: { models: [], overrides: {} } };
    markSaved();
    setProviderEnabled("codex", false, true);
    setProviderEnabled("codex", true);
  `);
  assert.equal(app.run("dirty"), false);
});

test("a connection published while the members file is being read is read again later", async () => {
  const answers = [];
  const app = editor(() => new Promise((resolve) => answers.push((document) => resolve({ ok: true, json: async () => ({ document }) }))));
  const hosts = (...ids) => ({ detail: { hosts: ids.map((id) => ({ id, status: "connected", last_action: { at: "1" } })) } });
  const file = app.doc();
  const first = app.listeners["agentnave:hosts"](hosts("codex"));
  await app.listeners["agentnave:hosts"](hosts("codex", "grok"));
  answers.shift()(file);
  // No further publication: the first read itself reads again for the hosts it missed.
  await new Promise(setImmediate);
  for (const scene of ["task", "discussion"]) file.modes[scene].grok = { models: [], overrides: {} };
  answers.shift()(file);
  await first;
  await new Promise(setImmediate);
  assert.equal(app.run('slot("task", "grok").enabled'), true);
});

test("a saved employee without an installed CLI stays known until its removal is saved", () => {
  const app = editor();
  app.run(`
    currentDoc.modes.task.claude = { models: [], overrides: {} };
    markSaved();
    setProviderEnabled('claude', false, true);
    let lastSaved = confirmScenes(JSON.parse(savedSnapshot));
  `);
  assert.equal(app.run("known('claude', lastSaved)"), true);
  assert.equal(app.run("slot('task', 'claude').enabled"), false);
  app.run("markSaved(); lastSaved = confirmScenes(JSON.parse(savedSnapshot))");
  assert.equal(app.run("known('claude', lastSaved)"), false);
});

function runtimeEditor(fetch, os) {
  const app = editor(fetch, os);
  app.run(readFileSync(`${__dirname}/../member-cards.js`, "utf8"));
  app.run(`
    renderCards = () => {};
    el = (tag, attrs, ...children) => ({ tag, attrs, children });
    textNode = (s) => s;
  `);
  return app;
}

test("Claude runtime switch is labelled, defaults off, and is hidden on Windows and other cards", async () => {
  const app = runtimeEditor(async () => ({ ok: true, json: async () => ({ settings: { version: 1, max_running: 8 } }) }));
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.disabled"), true);
  await app.run("loadLimit()");
  assert.equal(app.run("renderClaudeInteractive('codex')"), null);
  const row = app.run("renderClaudeInteractive('claude')");
  assert.equal(row.attrs.className, "member-row member-runtime");
  const label = row.children[0], input = label.children[0];
  assert.equal(label.tag, "label");
  assert.equal(label.children[1], "交互模式运行");
  assert.equal(input.attrs.checked, false);
  assert.equal(input.attrs.disabled, false);
  const windows = runtimeEditor(undefined, "windows");
  assert.equal(windows.run("renderClaudeInteractive('claude')"), null);
});

test("runtime saves immediately without changing other settings or the unsaved limit", async () => {
  const calls = [];
  const app = runtimeEditor(async (url, options) => {
    assert.equal(url, "/app/settings");
    if (options.method === "POST") {
      calls.push(JSON.parse(options.body));
      assert.equal(options.headers["X-AgentNave-Action"], "1");
      return { ok: true, json: async () => ({ saved: true }) };
    }
    return { ok: true, json: async () => ({ settings: { version: 1, max_running: 8, history_retention_days: 90, claude_interactive: true } }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.checked"), true);
  app.run("$('max-running').value = '3'; refreshDirty()");
  await app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.onChange({ target: { checked: false } })");
  assert.deepEqual(calls, [{ version: 1, claude_interactive: false }]);
  assert.equal(app.run("limitChanged()"), true);
  assert.equal(app.run("dirty"), true);
  assert.equal(app.run("limitDoc.history_retention_days"), 90);
  await app.run("saveLimit()");
  assert.deepEqual(calls[1], { version: 1, max_running: 3 });
  assert.equal(app.run("limitDoc.claude_interactive"), false);
});

test("failed writes restore the runtime switch and report preview refusals", async () => {
  let finish;
  const app = runtimeEditor(() => new Promise(resolve => { finish = resolve; }));
  app.run("limitDoc = { version: 1, max_running: 8, claude_interactive: false }");
  const saving = app.run("setClaudeInteractive(true)");
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.disabled"), true);
  finish({ ok: false, json: async () => ({ error: "预览版不修改正式数据" }) });
  await saving;
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.checked"), false);
  assert.equal(app.run("busy"), false);
  assert.match(app.run("$('notice').textContent"), /预览版不修改正式数据/);
});

test("broken settings keep the switch disabled until the existing settings save repairs them", async () => {
  let posts = 0;
  const app = runtimeEditor(async (_url, options) => {
    if (options.method === "POST") {
      posts++;
      const payload = JSON.parse(options.body);
      assert.equal(payload.claude_interactive, true);
      return { ok: true, json: async () => ({ saved: true }) };
    }
    return { ok: true, json: async () => ({ settings: { version: 1, max_running: 8, claude_interactive: true }, error: "claude_interactive 必须为布尔值" }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.disabled"), true);
  await app.run("setClaudeInteractive(true)");
  assert.equal(posts, 0);
  assert.match(app.run("$('notice').textContent"), /claude_interactive/);
  app.run("$('max-running').value = '3'");
  await app.run("saveLimit()");
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.disabled"), false);
});

test("a settings read started before a runtime save cannot restore its old value", async () => {
  let finish;
  const app = runtimeEditor((_url, options) => options.method === "POST"
    ? Promise.resolve({ ok: true, json: async () => ({ saved: true }) })
    : new Promise(resolve => { finish = resolve; }));
  app.run("limitDoc = { version: 1, max_running: 8, claude_interactive: false }");
  const loading = app.run("loadLimit()");
  await app.run("setClaudeInteractive(true)");
  finish({ ok: true, json: async () => ({ settings: { version: 1, max_running: 8, claude_interactive: false } }) });
  await loading;
  assert.equal(app.run("limitDoc.claude_interactive"), true);
});


test("a failed settings read cannot make a limit save reset other runtime settings", async () => {
  let sent;
  const app = runtimeEditor(async (_url, options) => {
    if (options.method !== "POST") throw new Error("offline");
    sent = JSON.parse(options.body);
    return { ok: true, json: async () => ({ saved: true }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderClaudeInteractive('claude').children[0].children[0].attrs.disabled"), true);
  app.run("$('max-running').value = '3'");
  await app.run("saveLimit()");
  assert.deepEqual(sent, { version: 1, max_running: 3 });
});

function poolEditor(fetch) {
  const app = runtimeEditor(fetch);
  app.run("AN.L = (zh) => zh");
  app.run(readFileSync(`${__dirname}/../advanced.js`, "utf8"));
  app.run(`
    function findKey(nodes, key) {
      for (const node of nodes || []) {
        if (!node || typeof node !== "object") continue;
        if (node.attrs?.dataset?.focusKey === key) return node;
        const found = findKey(node.children, key);
        if (found) return found;
      }
      return null;
    }
    const offered = (rows, key) => findKey(rows, key).children.flat()
      .filter((option) => !option.attrs.disabled).map((option) => option.attrs.value);
    const seatOptions = (seat) => seatSelects("seat", "工位", seat, () => {})[1].children.flat();
    const poolPick = (key, checked) => {
      pickerOpen = "codex/pool";
      const action = findKey(poolRows("codex"), key).attrs;
      if (action.onClick) action.onClick();
      else action.onChange({ target: { checked } });
    };
    detected.codex = { installed: true, native: { model: "a" }, models: [{ id: "a" }, { id: "b" }] };
    currentDoc.clis.codex = { models: { old: {} } };
    markSaved();
  `);
  return app;
}

test("one CLI pool bounds solo tasks, meetings and seats and lets no left-out model through", async () => {
  const posts = [];
  const app = poolEditor(async (url, options) => {
    posts.push(url);
    return { ok: true, json: async () => ({ saved: true, document: JSON.parse(options.body) }) };
  });
  app.run(`
    currentDoc.clis.codex.models.b = {};
    currentDoc.modes.discussion.codex.default = "b";
    reviewSeats = { "security-reviewer": { provider: "codex", model: "b" } };
    markSaved();
    poolPick("codex/pool/all");
    poolPick("codex/pool/allow/old", false);
    poolPick("codex/pool/allow/b", false);
  `);
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a"]);
  assert.deepEqual(app.doc().modes.task.codex.models, []);
  assert.equal(app.doc().modes.task.codex.default, "a");

  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "task", true), "codex/task/model"))')).slice(1), ["a"]);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "discussion", true), "codex/discussion/model"))')).slice(1), ["a"]);

  // The default and the seat outside the pool stay as saved and are shown as conflicts.
  assert.equal(app.doc().modes.discussion.codex.default, "b");
  assert.equal(app.run('findKey(slotRows("codex", "discussion", true), "codex/discussion/model").children.flat().find((o) => o.attrs.value === "b").attrs.disabled'), true);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "b" }).find((o) => o.attrs.value === "b").attrs.disabled'), true);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(seatOptions({ provider: "codex", model: "a" }).map((o) => o.attrs.value))')), ["a"]);
  assert.equal(app.run('poolConflicts("codex").length'), 2);
  await app.run("saveMembers()");
  assert.deepEqual(posts, []);
  app.run('changeSlot("codex", "discussion", { model: "a" })');
  assert.match(app.run("seatConflictText(seatConflicts()[0])"), /安全评审 用的 b/);
  assert.equal(app.run("memberSaveErrors().length"), 0);
  await app.run("saveMembers()");
  assert.deepEqual(posts, ["/app/members"], "the pool can be saved before rebinding a conflicting seat");

  // A role cannot widen the pool: the saved list refuses the seat before anything is written.
  app.run('reviewSeats = {}; markSaved()');
  assert.equal(app.run("memberSaveErrors().length"), 0);
  await assert.rejects(app.run('listSeatModel({ provider: "codex", model: "b" })'), /不在 Codex CLI 的可用模型里/);
  assert.deepEqual(posts, ["/app/members"]);

  // Select all is an explicit list; future CLI models are not silently allowed.
  app.run('poolPick("codex/pool/all"); detected.codex.models.push({ id: "next" })');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a", "b", "old"]);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "a" }).some((o) => o.attrs.value === "next")'), false);
  app.run(`
    currentDoc.clis.codex.allowed_models = ["a"];
    orchestration = { document: { version: 1, preset: "off", image_generation: { provider: "codex", model: "b" } } };
  `);
  assert.equal(app.run("seatConflicts().length"), 0, "inactive image bindings do not block model-pool edits");
  app.run('orchestration.document.preset = "split"');
  assert.equal(app.run("seatConflicts().length"), 1, "enabling orchestration restores the binding conflict");
});

test("model choices start unchecked, clear all stays unsaved, and selection stops at 20", () => {
  const app = poolEditor();
  app.run(`
    detected.codex.models = Array.from({ length: 21 }, (_, i) => ({ id: "model-" + i }));
    currentDoc.clis.codex.models = {};
    markSaved();
  `);
  assert.equal(app.run('poolSummary("codex", getAllModelIds("codex"), shownPool("codex"))'), "不限模型");
  app.run('pickerOpen = "codex/pool"');
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/allow/model-0").attrs.checked'), false);
  app.run('poolPick("codex/pool/all")');
  assert.equal(Object.hasOwn(app.doc().clis.codex, "allowed_models"), false);
  assert.match(app.run("$('notice').textContent"), /最多选 20 个/);
  app.run('writePool("codex", Array.from({ length: 20 }, (_, i) => `model-${i}`))');
  app.run('poolPick("codex/pool/allow/model-20", true)');
  assert.equal(app.doc().clis.codex.allowed_models.length, 20);
  app.run('poolPick("codex/pool/none")');
  assert.deepEqual(app.doc().clis.codex.allowed_models, []);
  assert.match(app.run('configurationErrors()[0].message'), /至少选一个模型/);
  app.run('poolPick("codex/pool/allow/model-0", true)');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["model-0"]);

  // An older list above the limit shrinks one model at a time.
  app.run('currentDoc.clis.codex.allowed_models = Array.from({ length: 22 }, (_, i) => `model-${i}`); markSaved()');
  app.run('poolPick("codex/pool/allow/model-20", false)');
  assert.equal(app.doc().clis.codex.allowed_models.length, 21);
});

test("older scene lists keep working until the pool is edited, and then stop narrowing it", () => {
  const app = poolEditor();
  app.run(`
    currentDoc.clis.codex.models = { a: {}, b: {} };
    currentDoc.modes.task.codex = { models: ["a"], overrides: {}, default: "a" };
    currentDoc.modes.discussion.codex = { models: ["a", "b"], overrides: {}, default: "b" };
    reviewSeats = { "security-reviewer": { provider: "codex", model: "b" } };
    markSaved();
  `);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "codex").allowed)')), ["a"]);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(shownPool("codex"))')), ["a", "b"]);
  assert.equal(app.run('poolSummary("codex", ["a", "b"], shownPool("codex"))'), "单独干活 1 个、开会 2 个");
  assert.equal(app.run("memberSaveErrors().length"), 0);

  app.run('detected.codex.models.push({ id: "c" }); poolPick("codex/pool/allow/c", true)');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a", "b", "c"]);
  assert.deepEqual(app.doc().modes.task.codex, { models: [], overrides: {}, default: "a" });
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "task", true), "codex/task/model"))')).slice(1), ["a", "b", "c"]);
  assert.ok(app.doc().clis.codex.models.c);
});

test("each boss is named by its own saved orchestration mode", () => {
  const app = poolEditor();
  app.run(`orchestration = { error: null, files: [], document: { version: 1, preset: "off", apps: { codex: "split" } }, bosses: [
    { keys: ["codex"], labels: ["Codex CLI"] }, { keys: ["grok"], labels: ["Grok CLI"] },
  ] }; renderTaskScope();`);
  assert.equal(app.run('$("task-scope").hidden'), false);
  assert.match(app.run('$("task-scope").textContent'), /^Codex CLI 已启用编排.*单独干活只提供给未启用编排的 boss/);
  app.run('orchestration.document.apps = {}; renderTaskScope()');
  assert.equal(app.run('$("task-scope").hidden'), true);
});

test("split work lets the boss call meetings only when its switch is on", async () => {
  const { app } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked");
  const meetings = () => app.run('$("orchestration-split").children[0].children[0]');
  meetings().listeners.change({ target: { checked: true } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().split_options)"), '{"meetings":true}');
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nown");
  meetings().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationWanted().split_options"), undefined);
  app.run('viewMode("parallel")');
  assert.equal(app.run('$("orchestration-split").hidden'), true);
});

// The settings scripts share one global scope: a later top-level function of the same name
// silently replaces an earlier one, as member-models.js's setSplit once did to the role switch.
test("settings page scripts do not redeclare each other's top-level functions", () => {
  const html = readFileSync(`${__dirname}/../settings.html`, "utf8");
  const seen = new Map();
  for (const [, file] of html.matchAll(/<script src="\.\/([^"]+)"/g)) {
    const source = readFileSync(`${__dirname}/../${file}`, "utf8");
    if (/^\s*(?:"use strict";\s*)?[{(]/.test(source)) continue; // Block-scoped files keep theirs.
    for (const [, name] of source.matchAll(/^(?:async )?function (\w+)/gm)) {
      assert.ok(!seen.has(name), `${name} is declared in both ${seen.get(name)} and ${file}`);
      seen.set(name, file);
    }
  }
});
