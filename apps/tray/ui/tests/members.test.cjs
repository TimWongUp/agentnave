const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function editor(fetch = async () => { throw new Error("unexpected request"); }) {
  const listeners = {};
  const context = vm.createContext({
    AN: {}, structuredClone, fetch,
    document: { getElementById: () => ({ options: [] }) },
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
  await first;
  for (const scene of ["task", "discussion"]) file.modes[scene].grok = { models: [], overrides: {} };
  const retry = app.listeners["agentnave:hosts"](hosts("codex", "grok"));
  answers.shift()(file);
  await retry;
  assert.equal(app.run('slot("task", "grok").enabled'), true);
});
