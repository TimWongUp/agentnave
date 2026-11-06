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

test("the optional image seat picks, clears, discards, saves and reloads as one flat seat", async () => {
  let saved = { version: 1, preset: "split" }, members;
  const app = editor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: saved, error: null, bosses: [], presets: { split: "rule", image_generation: "\nimage-rule" }, files: [] }) };
    }
    if (url === "/app/orchestration") saved = body;
    else members = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  });
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

test("unchecking all models keeps the full model list and default so items can be unchecked and default changed", () => {
  const app = runtimeEditor();
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
    detected.codex = {
      installed: true,
      native: { model: "gpt-5-mini" },
      models: [{ id: "gpt-5" }, { id: "gpt-5-mini" }],
    };
    currentDoc.clis.codex = { models: { "o4-mini": {} } };
    pickerOpen = "codex/task";
    findKey(slotRows("codex", "task", true), "codex/task/all").attrs.onChange({ target: { checked: false } });
  `);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "codex"))')), {
    enabled: true,
    model: "gpt-5-mini",
    allowed: ["gpt-5", "gpt-5-mini", "o4-mini"],
    effort: "",
  });
  assert.equal(app.run('slot("discussion", "codex").allowed'), null);

  app.run('findKey(slotRows("codex", "task", true), "codex/task/allow/o4-mini").attrs.onChange({ target: { checked: false } })');
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "codex").allowed)')), ["gpt-5", "gpt-5-mini"]);
  assert.equal(app.run('slot("task", "codex").model'), "gpt-5-mini");

  app.run('findKey(slotRows("codex", "task", true), "codex/task/model").attrs.onChange({ target: { value: "gpt-5" } })');
  assert.equal(app.run('slot("task", "codex").model'), "gpt-5");
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "codex").allowed)')), ["gpt-5", "gpt-5-mini"]);

  app.run(`
    detected.claude = { installed: true, models: [] };
    currentDoc.modes.task.claude = { models: [], overrides: {}, default: "claude-sonnet-4-5" };
    pickerOpen = "claude/task";
    findKey(slotRows("claude", "task", true), "claude/task/all").attrs.onChange({ target: { checked: false } });
  `);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "claude").allowed)')), ["claude-sonnet-4-5"]);
  assert.equal(app.run('slot("task", "claude").model'), "claude-sonnet-4-5");
});
